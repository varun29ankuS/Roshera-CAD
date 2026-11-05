//! After dispatch: place assembly instances and give every imported solid
//! and face its identity.
//!
//! ## Placement
//!
//! The tier-1/2 handlers build each part's solids once, at the part's own
//! origin. An assembly says where — and how many times — each part goes,
//! through its product tree (see `handlers::tier3::product`):
//!
//! ```text
//! NEXT_ASSEMBLY_USAGE_OCCURRENCE(parent PD, child PD)        ← one use
//!   ← PDS ← CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(rr, PDS)
//! rr = (REPRESENTATION_RELATIONSHIP(rep_1, rep_2)
//!       REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#idt) …)
//! ITEM_DEFINED_TRANSFORMATION(…, item_1, item_2)
//! ```
//!
//! `item_1` is a frame in `rep_1`'s space and `item_2` the frame it lands on
//! in `rep_2`'s; whichever of the two representations is the child's decides
//! the direction, so files written either way round place correctly. One
//! use maps the child into its parent by `M = frame(parent item) ·
//! frame(child item)⁻¹`, and a use nested under sub-assemblies composes the
//! chain from the top. Every use becomes its own independent copy of the
//! part's solids (a part used twice is two bodies, never one body moved
//! twice). Solids that belong to no product in the tree are kept as they
//! are.
//!
//! Honest gaps are reported, never guessed silently: a use with no
//! placement, a placement that is not an `ITEM_DEFINED_TRANSFORMATION`, or a
//! cycle in the tree each push a warning naming the entity. A file that also
//! places parts with `MAPPED_ITEM` keeps that path's placements and skips
//! this pass (applying both would move the same bodies twice).
//!
//! ## Identity
//!
//! Every solid and face gets a [`PersistentId`] derived from the FILE's
//! content and the STEP entity it came from — `#161` stays `#161` however
//! many times the file is read — plus, for an assembly use, the path of
//! occurrences that placed it. Same file → same PIDs on any machine; two
//! uses of one part → distinct PIDs. Every solid is tagged
//! `imported:step`.

use std::collections::{BTreeMap, HashMap, HashSet};

use geometry_engine::math::Matrix4;
use geometry_engine::operations::transform::{transform_solid, TransformOptions};
use geometry_engine::primitives::{
    face::FaceId,
    persistent_id::{PersistentId, Role},
    provenance::OperationKind,
    solid::SolidId,
    topology_builder::BRepModel,
};
use ruststep::ast::Parameter;

use crate::formats::step::{
    context::{ProductStructure, ResolutionCaches},
    diagnostics::{ImportReport, Severity, Warning},
    handlers::tier3::assembly::frame_matrix,
    merge::{copy_solid, solid_faces},
    registry::{EntityKind, EntityRegistry},
};

/// Operation tag for PIDs minted from a STEP entity.
const IMPORT_TAG: &str = "import:step";
/// Operation tag for PIDs of one assembly use of a part.
const INSTANCE_TAG: &str = "import:step:instance";
/// Product trees deeper than this are reported as cyclic or malformed.
const MAX_ASSEMBLY_DEPTH: usize = 64;

/// One placed use of a product's solids.
struct Instance {
    /// The `PRODUCT_DEFINITION` being placed.
    definition: u64,
    /// Its solids in the import model.
    solids: Vec<SolidId>,
    /// Part frame → world.
    transform: Matrix4,
    /// The occurrences from the top assembly down to this use (empty for a
    /// top-level product's own solids).
    path: Vec<u64>,
}

/// Lookups shared by placement and identity.
struct Ids<'a> {
    file_root: PersistentId,
    step_solid: HashMap<SolidId, u64>,
    step_face: HashMap<FaceId, u64>,
    product: &'a ProductStructure,
}

/// Place assembly instances and stamp identity. Returns the final model.
pub(crate) fn finalize(
    mut src: BRepModel,
    caches: &ResolutionCaches,
    registry: &EntityRegistry,
    report: &mut ImportReport,
    file_root: PersistentId,
) -> BRepModel {
    let ids = Ids {
        file_root,
        step_solid: caches.solids.iter().map(|(n, s)| (*s, *n)).collect(),
        step_face: caches.faces.iter().map(|(n, f)| (*f, *n)).collect(),
        product: &caches.product,
    };

    let Some(instances) = plan(caches, registry, report) else {
        // No product tree to place: identity in place, geometry untouched.
        let solids: Vec<SolidId> = src.solids.iter().map(|(id, _)| id).collect();
        for sid in solids {
            let faces: Vec<(FaceId, FaceId)> =
                solid_faces(&src, sid).into_iter().map(|f| (f, f)).collect();
            stamp(&mut src, sid, sid, &faces, &[], &ids);
        }
        return src;
    };

    let mut uses_of: HashMap<u64, usize> = HashMap::new();
    for inst in &instances {
        *uses_of.entry(inst.definition).or_default() += 1;
    }

    let mut out = BRepModel::new();
    let mut placed: HashSet<SolidId> = HashSet::new();
    for inst in &instances {
        let base_name = product_name(ids.product, inst.definition);
        for (k, &part_solid) in inst.solids.iter().enumerate() {
            placed.insert(part_solid);
            let name = instance_name(
                base_name.as_deref(),
                src.solids.get(part_solid).and_then(|s| s.name.clone()),
                uses_of.get(&inst.definition).copied().unwrap_or(1) > 1,
                inst.path.last().copied(),
                (inst.solids.len() > 1).then_some(k + 1),
                ids.product,
            );
            let Some((sid, face_map)) = copy_solid(&mut out, &src, part_solid, name) else {
                continue;
            };
            if !inst.path.is_empty() {
                if let Err(e) =
                    transform_solid(&mut out, sid, inst.transform, TransformOptions::default())
                {
                    warn(
                        report,
                        "NEXT_ASSEMBLY_USAGE_OCCURRENCE",
                        inst.path.last().copied(),
                        &format!("placing solid {part_solid:?} failed: {e}"),
                    );
                }
            }
            let faces: Vec<(FaceId, FaceId)> = face_map.iter().map(|(s, d)| (*d, *s)).collect();
            stamp(&mut out, sid, part_solid, &faces, &inst.path, &ids);
        }
    }
    // Bodies no product claims (free solids, orphans) come across unchanged.
    let free: Vec<SolidId> = src
        .solids
        .iter()
        .map(|(id, _)| id)
        .filter(|id| !placed.contains(id))
        .collect();
    for part_solid in free {
        if let Some((sid, face_map)) = copy_solid(&mut out, &src, part_solid, None) {
            let faces: Vec<(FaceId, FaceId)> = face_map.iter().map(|(s, d)| (*d, *s)).collect();
            stamp(&mut out, sid, part_solid, &faces, &[], &ids);
        }
    }
    out
}

/// Assign the solid's and its faces' PIDs and its `imported:step`
/// provenance. `faces` pairs each face in `model` with the import-model face
/// it was copied from (identical ids when stamping in place).
fn stamp(
    model: &mut BRepModel,
    sid: SolidId,
    src_solid: SolidId,
    faces: &[(FaceId, FaceId)],
    path: &[u64],
    ids: &Ids<'_>,
) {
    let base_solid = match ids.step_solid.get(&src_solid) {
        Some(n) => entity_pid(ids.file_root, &format!("solid#{n}")),
        None => entity_pid(ids.file_root, &format!("solid-index:{src_solid}")),
    };
    let label = path_label(path);
    model.set_solid_pid(sid, placed_pid(base_solid, &label));

    // Faces in the order of the copy, so a face with no STEP entity (one a
    // healing pass split) still gets a stable positional name.
    let mut ordered: Vec<(FaceId, FaceId)> = faces.to_vec();
    ordered.sort_by_key(|(_, src_face)| *src_face);
    for (k, (face, src_face)) in ordered.iter().enumerate() {
        let base_face = match ids.step_face.get(src_face) {
            Some(n) => entity_pid(ids.file_root, &format!("face#{n}")),
            None => PersistentId::derive(
                &[base_solid],
                IMPORT_TAG,
                &Role::Generic {
                    source_pid: base_solid,
                    label: format!("face-index:{k}"),
                },
            ),
        };
        model.set_face_pid(*face, placed_pid(base_face, &label));
    }
    model.set_solid_provenance(sid, OperationKind::Imported("step".into()), Vec::new());
}

/// PID of a STEP entity of this file.
fn entity_pid(file_root: PersistentId, label: &str) -> PersistentId {
    PersistentId::derive(
        &[file_root],
        IMPORT_TAG,
        &Role::Generic {
            source_pid: file_root,
            label: label.to_string(),
        },
    )
}

/// PID of one assembly use (`path` label), or the base PID itself when the
/// solid is not placed by an assembly.
fn placed_pid(base: PersistentId, path_label: &str) -> PersistentId {
    if path_label.is_empty() {
        return base;
    }
    PersistentId::derive(
        &[base],
        INSTANCE_TAG,
        &Role::Generic {
            source_pid: base,
            label: path_label.to_string(),
        },
    )
}

fn path_label(path: &[u64]) -> String {
    path.iter()
        .map(|n| format!("#{n}"))
        .collect::<Vec<_>>()
        .join("/")
}

/// Build the placement plan, or `None` when the file has no product tree to
/// place (single parts, or assemblies placed by `MAPPED_ITEM`).
fn plan(
    caches: &ResolutionCaches,
    registry: &EntityRegistry,
    report: &mut ImportReport,
) -> Option<Vec<Instance>> {
    let ps = &caches.product;
    if ps.occurrences.is_empty() {
        return None;
    }
    if !caches.mapped_solids.is_empty() {
        report.push_warning(Warning {
            severity: Severity::Info,
            entity: "NEXT_ASSEMBLY_USAGE_OCCURRENCE".into(),
            instance: None,
            message: "assembly is placed by MAPPED_ITEM; product-tree placement \
                      skipped so no body is moved twice"
                .into(),
        });
        return None;
    }

    // What each definition (PD or occurrence) is represented by — every
    // representation reachable through plain same-shape links, so a part
    // whose product points at a placement-only rep still finds the B-Rep
    // rep holding its solids (the SolidWorks layout).
    let mut links: HashMap<u64, Vec<u64>> = HashMap::new();
    for (a, b) in &ps.rep_links {
        links.entry(*a).or_default().push(*b);
        links.entry(*b).or_default().push(*a);
    }
    let mut reps_of: HashMap<u64, Vec<u64>> = HashMap::new();
    for (pds, rep) in &ps.shape_representations {
        if let Some(def) = ps.definition_shapes.get(pds) {
            let reps = reps_of.entry(*def).or_default();
            let mut stack = vec![*rep];
            while let Some(r) = stack.pop() {
                if reps.contains(&r) {
                    continue;
                }
                reps.push(r);
                stack.extend(links.get(&r).into_iter().flatten().copied());
            }
        }
    }
    let solids_of = |pd: u64| -> Vec<SolidId> {
        let mut seen = HashSet::new();
        reps_of
            .get(&pd)
            .into_iter()
            .flatten()
            .filter_map(|rep| caches.roots.get(rep))
            .flatten()
            .copied()
            .filter(|s| seen.insert(*s))
            .collect()
    };

    // Each occurrence's local placement (child frame → parent frame).
    let mut local: HashMap<u64, Matrix4> = HashMap::new();
    for (relation, pds) in &ps.placements {
        let Some(nauo) = ps.definition_shapes.get(pds).copied() else {
            continue;
        };
        let Some(occ) = ps.occurrences.get(&nauo) else {
            continue;
        };
        let child_reps = reps_of.get(&occ.child).cloned().unwrap_or_default();
        match occurrence_transform(*relation, &child_reps, ps, caches, registry) {
            Ok(m) => {
                local.insert(nauo, m);
            }
            Err(message) => warn(
                report,
                "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION",
                Some(*relation),
                &message,
            ),
        }
    }

    let mut children: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for (nauo, occ) in &ps.occurrences {
        children.entry(occ.parent).or_default().push(*nauo);
    }
    for list in children.values_mut() {
        list.sort_unstable();
    }
    let used_as_child: HashSet<u64> = ps.occurrences.values().map(|o| o.child).collect();
    let tops: Vec<u64> = children
        .keys()
        .copied()
        .filter(|pd| !used_as_child.contains(pd))
        .collect();
    if tops.is_empty() {
        warn(
            report,
            "NEXT_ASSEMBLY_USAGE_OCCURRENCE",
            None,
            "product tree has no top-level assembly (every product is used by another): \
             cyclic structure, placement skipped",
        );
        return None;
    }

    let mut instances = Vec::new();
    for top in tops {
        walk(
            top,
            Matrix4::identity(),
            &mut Vec::new(),
            &children,
            &local,
            ps,
            &solids_of,
            &mut instances,
            report,
        );
    }
    Some(instances)
}

#[allow(clippy::too_many_arguments)]
fn walk(
    definition: u64,
    to_world: Matrix4,
    path: &mut Vec<u64>,
    children: &BTreeMap<u64, Vec<u64>>,
    local: &HashMap<u64, Matrix4>,
    ps: &ProductStructure,
    solids_of: &dyn Fn(u64) -> Vec<SolidId>,
    out: &mut Vec<Instance>,
    report: &mut ImportReport,
) {
    let solids = solids_of(definition);
    if !solids.is_empty() {
        out.push(Instance {
            definition,
            solids,
            transform: to_world,
            path: path.clone(),
        });
    }
    let Some(uses) = children.get(&definition) else {
        return;
    };
    for &nauo in uses {
        if path.contains(&nauo) || path.len() >= MAX_ASSEMBLY_DEPTH {
            warn(
                report,
                "NEXT_ASSEMBLY_USAGE_OCCURRENCE",
                Some(nauo),
                "assembly occurrence repeats on its own path (cycle) or the tree is deeper \
                 than 64 levels: this branch is not placed",
            );
            continue;
        }
        let Some(occ) = ps.occurrences.get(&nauo) else {
            continue;
        };
        let step = match local.get(&nauo) {
            Some(m) => *m,
            None => {
                warn(
                    report,
                    "NEXT_ASSEMBLY_USAGE_OCCURRENCE",
                    Some(nauo),
                    "occurrence has no usable placement (no CONTEXT_DEPENDENT_SHAPE_REPRESENTATION \
                     with an ITEM_DEFINED_TRANSFORMATION): component placed at its parent's origin",
                );
                Matrix4::identity()
            }
        };
        path.push(nauo);
        walk(
            occ.child,
            to_world * step,
            path,
            children,
            local,
            ps,
            solids_of,
            out,
            report,
        );
        path.pop();
    }
}

/// The child → parent transform carried by one placement relationship.
fn occurrence_transform(
    relation: u64,
    child_reps: &[u64],
    ps: &ProductStructure,
    caches: &ResolutionCaches,
    registry: &EntityRegistry,
) -> Result<Matrix4, String> {
    let entity = registry
        .get(relation)
        .ok_or_else(|| format!("representation relation #{relation} not in file"))?;
    let records = match &entity.kind {
        EntityKind::Complex(records) => records.as_slice(),
        EntityKind::Simple(r) => std::slice::from_ref(r),
    };
    let find = |name: &str| records.iter().find(|r| r.name.eq_ignore_ascii_case(name));
    let rel = find("REPRESENTATION_RELATIONSHIP")
        .ok_or_else(|| format!("#{relation} is not a REPRESENTATION_RELATIONSHIP"))?;
    let with_t = find("REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION").ok_or_else(|| {
        format!("#{relation} carries no transformation (REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION)")
    })?;
    let rel_fields = list(&rel.parameter);
    let rep_1 = reference(rel_fields.get(2))
        .ok_or_else(|| format!("#{relation}: rep_1 is not a reference"))?;
    let rep_2 = reference(rel_fields.get(3))
        .ok_or_else(|| format!("#{relation}: rep_2 is not a reference"))?;
    let operator = reference(list(&with_t.parameter).first())
        .ok_or_else(|| format!("#{relation}: transformation_operator is not a reference"))?;
    let (item_1, item_2) = ps.transformations.get(&operator).copied().ok_or_else(|| {
        format!(
            "#{relation}: transformation #{operator} is not an ITEM_DEFINED_TRANSFORMATION \
             (other operators are not supported)"
        )
    })?;
    // item_1 lives in rep_1's space, item_2 in rep_2's: the child's side is
    // the one being moved.
    let (child_item, parent_item) = if child_reps.contains(&rep_2) && !child_reps.contains(&rep_1) {
        (item_2, item_1)
    } else {
        (item_1, item_2)
    };
    let frame = |item: u64| {
        caches
            .placements
            .get(&item)
            .map(frame_matrix)
            .ok_or_else(|| format!("#{relation}: placement #{item} is not an AXIS2_PLACEMENT_3D"))
    };
    let child_frame = frame(child_item)?;
    let parent_frame = frame(parent_item)?;
    let inverse = child_frame
        .inverse()
        .map_err(|e| format!("#{relation}: placement #{child_item} is singular: {e:?}"))?;
    Ok(parent_frame * inverse)
}

fn list(p: &Parameter) -> &[Parameter] {
    match p {
        Parameter::List(items) => items.as_slice(),
        _ => &[],
    }
}

fn reference(p: Option<&Parameter>) -> Option<u64> {
    match p? {
        Parameter::Ref(ruststep::ast::Name::Entity(n)) => Some(*n),
        _ => None,
    }
}

/// The `PRODUCT` name behind a `PRODUCT_DEFINITION`.
fn product_name(ps: &ProductStructure, definition: u64) -> Option<String> {
    let formation = ps.definitions.get(&definition)?;
    let product = ps.formations.get(formation)?;
    ps.products
        .get(product)
        .filter(|n| !n.trim().is_empty())
        .cloned()
}

/// `cube`, or `cube [i2]` when the part is used more than once, with
/// ` (2)` appended when the part itself has several bodies.
fn instance_name(
    product: Option<&str>,
    solid_name: Option<String>,
    used_more_than_once: bool,
    occurrence: Option<u64>,
    body: Option<usize>,
    ps: &ProductStructure,
) -> Option<String> {
    let mut name = product.map(str::to_string).or(solid_name)?;
    if used_more_than_once {
        if let Some(nauo) = occurrence {
            let occ_label = ps
                .occurrences
                .get(&nauo)
                .map(|o| {
                    if !o.name.trim().is_empty() {
                        o.name.clone()
                    } else if !o.id.trim().is_empty() {
                        o.id.clone()
                    } else {
                        format!("#{nauo}")
                    }
                })
                .unwrap_or_else(|| format!("#{nauo}"));
            name = format!("{name} [{occ_label}]");
        }
    }
    if let Some(b) = body {
        name = format!("{name} ({b})");
    }
    Some(name)
}

fn warn(report: &mut ImportReport, entity: &str, instance: Option<u64>, message: &str) {
    report.push_warning(Warning {
        severity: Severity::Warn,
        entity: entity.into(),
        instance,
        message: message.to_string(),
    });
}
