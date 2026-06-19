//! `MAPPED_ITEM` — assembly instancing (ISO 10303-43 `mapped_item`).
//!
//! ```text
//! #IM = MAPPED_ITEM('', #MAP, #TARGET);
//! #MAP = REPRESENTATION_MAP(#SRC_PLACEMENT, #SUB_REPRESENTATION);
//! ```
//!
//! A `MAPPED_ITEM` places an instance of `mapped_representation` (a
//! `SHAPE_REPRESENTATION` holding one or more solids) into the owning
//! representation. The placement maps `REPRESENTATION_MAP.mapping_origin`
//! onto `MAPPED_ITEM.mapping_target`; both are `AXIS2_PLACEMENT_3D`
//! frames, so the rigid body transform is
//!
//! ```text
//!     M = frame(target) · frame(source)⁻¹
//! ```
//!
//! We resolve the sub-representation's solids (running the Root handler
//! on the referenced `SHAPE_REPRESENTATION` re-entrantly), transform
//! each in place by `M`, and record the placed solid ids at
//! `caches.mapped_solids[mapped_item]`. The owning root's items walk
//! then picks them up.
//!
//! ## Honest scope
//!
//! - A representation map referenced by a single mapped item is fully
//!   supported (the common "component appears once" case).
//! - If the same `mapped_representation` is instanced more than once,
//!   transforming in place would corrupt the shared topology. We place
//!   the first instance and log every subsequent one as an unsupported
//!   shared-instance limitation (deep solid-cloning is out of scope for
//!   this slice) rather than silently producing wrong geometry.
//! - Non-`AXIS2_PLACEMENT_3D` mapping operators (e.g.
//!   `CARTESIAN_TRANSFORMATION_OPERATOR_3D` with scaling) place the
//!   untransformed solids and log the dropped transform.

use ruststep::ast::Record;

use geometry_engine::math::{Matrix4, Vector3};
use geometry_engine::operations::transform::{transform_solid, TransformOptions};
use geometry_engine::primitives::solid::SolidId;

use crate::formats::step::{
    context::{Axis2Placement, ImportContext},
    diagnostics::{Severity, Warning},
    dispatch::{EntityDispatch, EntityHandler, HandlerOutcome, Phase},
    registry::{EntityKind, EntityRegistry},
};

use super::super::tier1::params;
use super::super::tier1::resolver::ensure_resolved;

const MAPPED_ITEM: &str = "MAPPED_ITEM";
const REPRESENTATION_MAP: &str = "REPRESENTATION_MAP";
const AXIS2_PLACEMENT_3D: &str = "AXIS2_PLACEMENT_3D";

/// `MAPPED_ITEM('label', #mapping_source, #mapping_target)`.
pub struct MappedItemHandler;
/// Static binding consumed by [`register`].
pub static MAPPED_ITEM_HANDLER: MappedItemHandler = MappedItemHandler;

impl EntityHandler for MappedItemHandler {
    fn names(&self) -> &'static [&'static str] {
        &[MAPPED_ITEM]
    }
    fn phase(&self) -> Phase {
        // Runs after Topology so the sub-representation's solids exist.
        Phase::Root
    }
    fn handle(
        &self,
        instance: u64,
        record: &Record,
        registry: &EntityRegistry,
        dispatch: &EntityDispatch,
        ctx: &mut ImportContext<'_>,
    ) -> HandlerOutcome {
        let fields = match params::record_fields(&record.parameter, MAPPED_ITEM, instance) {
            Ok(f) => f,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                return HandlerOutcome::Failed {
                    message: "bad record shape".into(),
                };
            }
        };
        if fields.len() < 3 {
            warn(
                ctx,
                MAPPED_ITEM,
                instance,
                "expected (label, source, target)",
            );
            return HandlerOutcome::Failed {
                message: "too few fields".into(),
            };
        }
        let map_ref =
            match params::as_entity_ref(&fields[1], MAPPED_ITEM, instance, "mapping_source") {
                Ok(r) => r,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad mapping_source".into(),
                    };
                }
            };
        let target_ref =
            match params::as_entity_ref(&fields[2], MAPPED_ITEM, instance, "mapping_target") {
                Ok(r) => r,
                Err(e) => {
                    ctx.report.push_warning(e.into_warning());
                    return HandlerOutcome::Failed {
                        message: "bad mapping_target".into(),
                    };
                }
            };

        // REPRESENTATION_MAP(#mapping_origin, #mapped_representation).
        let (src_placement_ref, sub_rep_ref) =
            match representation_map_fields(map_ref, registry, ctx) {
                Some(t) => t,
                None => {
                    return HandlerOutcome::Failed {
                        message: "REPRESENTATION_MAP unresolved".into(),
                    };
                }
            };

        // Resolve the sub-representation's solids (re-entrant root walk).
        let solids = resolve_sub_representation_solids(sub_rep_ref, registry, dispatch, ctx);
        if solids.is_empty() {
            warn(
                ctx,
                MAPPED_ITEM,
                instance,
                "mapped representation produced no solids",
            );
            return HandlerOutcome::Failed {
                message: "empty mapped representation".into(),
            };
        }

        // Shared-instance guard: an in-place transform corrupts a
        // representation reused by another mapped item.
        if ctx
            .caches
            .mapped_solids
            .values()
            .any(|placed| placed.iter().any(|s| solids.contains(s)))
        {
            warn(
                ctx,
                MAPPED_ITEM,
                instance,
                "mapped_representation already instanced; deep solid-cloning for repeated \
                 assembly instances is not supported in this slice — instance dropped",
            );
            return HandlerOutcome::Failed {
                message: "repeated representation instance".into(),
            };
        }

        // Compute the placement transform M = frame(target)·frame(src)⁻¹.
        let xform =
            match placement_transform(src_placement_ref, target_ref, registry, dispatch, ctx) {
                Some(m) => m,
                None => {
                    warn(
                        ctx,
                        MAPPED_ITEM,
                        instance,
                        "non-AXIS2_PLACEMENT_3D mapping operator; solids placed untransformed",
                    );
                    Matrix4::identity()
                }
            };

        // Apply the transform in place to each placed solid.
        for &sid in &solids {
            if let Err(e) = transform_solid(ctx.model, sid, xform, TransformOptions::default()) {
                warn(
                    ctx,
                    MAPPED_ITEM,
                    instance,
                    &format!("transform of solid {sid:?} failed: {e}"),
                );
            }
        }

        ctx.caches.mapped_solids.insert(instance, solids);
        HandlerOutcome::Resolved
    }
}

/// Extract `(mapping_origin_ref, mapped_representation_ref)` from a
/// `REPRESENTATION_MAP` instance.
fn representation_map_fields(
    map_ref: u64,
    registry: &EntityRegistry,
    ctx: &mut ImportContext<'_>,
) -> Option<(u64, u64)> {
    let entity = registry.get(map_ref)?;
    let rec = match &entity.kind {
        EntityKind::Simple(r) => r,
        _ => return None,
    };
    if !rec.name.eq_ignore_ascii_case(REPRESENTATION_MAP) {
        warn(
            ctx,
            MAPPED_ITEM,
            map_ref,
            &format!("expected REPRESENTATION_MAP, found {}", rec.name),
        );
        return None;
    }
    let fields = params::record_fields(&rec.parameter, REPRESENTATION_MAP, map_ref).ok()?;
    if fields.len() < 2 {
        return None;
    }
    let origin =
        params::as_entity_ref(&fields[0], REPRESENTATION_MAP, map_ref, "mapping_origin").ok()?;
    let rep = params::as_entity_ref(
        &fields[1],
        REPRESENTATION_MAP,
        map_ref,
        "mapped_representation",
    )
    .ok()?;
    Some((origin, rep))
}

/// Resolve the solids contained in a (sub-)`SHAPE_REPRESENTATION`. The
/// representation handler already routes its `MANIFOLD_SOLID_BREP` /
/// `BREP_WITH_VOIDS` items into `caches.roots[sub_rep]`; we force that
/// to run and read the result.
fn resolve_sub_representation_solids(
    sub_rep_ref: u64,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> Vec<SolidId> {
    if let Some(solids) = ctx.caches.roots.get(&sub_rep_ref) {
        return solids.clone();
    }
    let _ = ensure_resolved(
        sub_rep_ref,
        &["SHAPE_REPRESENTATION", "ADVANCED_BREP_SHAPE_REPRESENTATION"],
        registry,
        dispatch,
        ctx,
    );
    ctx.caches
        .roots
        .get(&sub_rep_ref)
        .cloned()
        .unwrap_or_default()
}

/// Build the rigid transform mapping the source frame onto the target
/// frame: `M = frame(target) · frame(source)⁻¹`. Returns `None` when
/// either operator is not an `AXIS2_PLACEMENT_3D`.
fn placement_transform(
    src_ref: u64,
    target_ref: u64,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> Option<Matrix4> {
    let src = resolve_axis2_frame(src_ref, registry, dispatch, ctx)?;
    let target = resolve_axis2_frame(target_ref, registry, dispatch, ctx)?;
    let src_m = frame_matrix(&src);
    let target_m = frame_matrix(&target);
    let src_inv = src_m.inverse().ok()?;
    Some(target_m * src_inv)
}

/// Force an `AXIS2_PLACEMENT_3D` to resolve and return its frame.
fn resolve_axis2_frame(
    instance: u64,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> Option<Axis2Placement> {
    if let Some(p) = ctx.caches.placements.get(&instance) {
        return Some(*p);
    }
    let _ = ensure_resolved(instance, &[AXIS2_PLACEMENT_3D], registry, dispatch, ctx);
    ctx.caches.placements.get(&instance).copied()
}

/// Homogeneous frame matrix from an orthonormal placement: columns are
/// the x / y / z axes and the origin.
fn frame_matrix(p: &Axis2Placement) -> Matrix4 {
    Matrix4::from_cols(
        Vector3::new(p.x[0], p.x[1], p.x[2]),
        Vector3::new(p.y[0], p.y[1], p.y[2]),
        Vector3::new(p.z[0], p.z[1], p.z[2]),
        Vector3::new(p.origin[0], p.origin[1], p.origin[2]),
    )
}

fn warn(ctx: &mut ImportContext<'_>, entity: &str, instance: u64, message: &str) {
    ctx.report.push_warning(Warning {
        severity: Severity::Warn,
        entity: entity.into(),
        instance: Some(instance),
        message: message.to_string(),
    });
}

/// Register the assembly handler.
pub fn register(dispatch: &mut EntityDispatch) {
    dispatch.register(&MAPPED_ITEM_HANDLER);
}

#[cfg(test)]
mod tests {
    use crate::formats::step::{
        context::{ImportContext, ResolutionCaches, UnitScale},
        diagnostics::ImportReport,
        dispatch::EntityDispatch,
        parser::parse_step,
        registry::EntityRegistry,
    };
    use geometry_engine::primitives::topology_builder::BRepModel;

    fn wrap(body: &str) -> String {
        format!(
            "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('t'),'2;1');\n\
             FILE_NAME('t.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
             FILE_SCHEMA(('AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF'));\n\
             ENDSEC;\nDATA;\n{body}\nENDSEC;\nEND-ISO-10303-21;\n"
        )
    }

    fn run(body: &str) -> (BRepModel, ImportReport, ResolutionCaches) {
        let src = wrap(body);
        let ex = parse_step(&src, "test").expect("parse");
        let reg = EntityRegistry::build(&ex);
        let mut dispatch = EntityDispatch::new();
        super::super::super::register_all(&mut dispatch);
        let mut model = BRepModel::new();
        let mut report = ImportReport::new();
        let mut ctx = ImportContext::new(&mut model, &mut report);
        ctx.unit = UnitScale::default();
        let _ = dispatch.run_all(&reg, &mut ctx);
        let caches = std::mem::take(&mut ctx.caches);
        (model, report, caches)
    }

    /// Minimal unit-cube → CLOSED_SHELL #151 → MANIFOLD_SOLID_BREP #161.
    fn cube_solid() -> String {
        let mut s = String::new();
        s += "#1=CARTESIAN_POINT('',(0.,0.,0.));#2=CARTESIAN_POINT('',(1.,0.,0.));";
        s += "#3=CARTESIAN_POINT('',(1.,1.,0.));#4=CARTESIAN_POINT('',(0.,1.,0.));";
        s += "#5=CARTESIAN_POINT('',(0.,0.,1.));#6=CARTESIAN_POINT('',(1.,0.,1.));";
        s += "#7=CARTESIAN_POINT('',(1.,1.,1.));#8=CARTESIAN_POINT('',(0.,1.,1.));";
        s += "#11=VERTEX_POINT('',#1);#12=VERTEX_POINT('',#2);#13=VERTEX_POINT('',#3);";
        s += "#14=VERTEX_POINT('',#4);#15=VERTEX_POINT('',#5);#16=VERTEX_POINT('',#6);";
        s += "#17=VERTEX_POINT('',#7);#18=VERTEX_POINT('',#8);";
        s += "#21=DIRECTION('',(1.,0.,0.));#22=DIRECTION('',(0.,1.,0.));#23=DIRECTION('',(0.,0.,1.));";
        s += "#24=DIRECTION('',(-1.,0.,0.));#25=DIRECTION('',(0.,-1.,0.));#26=DIRECTION('',(0.,0.,-1.));";
        s += "#31=VECTOR('',#21,1.);#32=VECTOR('',#22,1.);#33=VECTOR('',#23,1.);";
        s += "#41=LINE('',#1,#31);#42=LINE('',#1,#32);#43=LINE('',#1,#33);";
        s += "#51=EDGE_CURVE('',#11,#12,#41,.T.);#52=EDGE_CURVE('',#12,#13,#42,.T.);";
        s += "#53=EDGE_CURVE('',#14,#13,#41,.T.);#54=EDGE_CURVE('',#11,#14,#42,.T.);";
        s += "#55=EDGE_CURVE('',#15,#16,#41,.T.);#56=EDGE_CURVE('',#16,#17,#42,.T.);";
        s += "#57=EDGE_CURVE('',#18,#17,#41,.T.);#58=EDGE_CURVE('',#15,#18,#42,.T.);";
        s += "#59=EDGE_CURVE('',#11,#15,#43,.T.);#60=EDGE_CURVE('',#12,#16,#43,.T.);";
        s += "#61=EDGE_CURVE('',#13,#17,#43,.T.);#62=EDGE_CURVE('',#14,#18,#43,.T.);";
        s += "#71=ORIENTED_EDGE('',*,*,#51,.T.);#72=ORIENTED_EDGE('',*,*,#52,.T.);";
        s += "#73=ORIENTED_EDGE('',*,*,#53,.F.);#74=ORIENTED_EDGE('',*,*,#54,.F.);";
        s += "#75=ORIENTED_EDGE('',*,*,#55,.T.);#76=ORIENTED_EDGE('',*,*,#56,.T.);";
        s += "#77=ORIENTED_EDGE('',*,*,#57,.F.);#78=ORIENTED_EDGE('',*,*,#58,.F.);";
        s += "#79=ORIENTED_EDGE('',*,*,#51,.T.);#80=ORIENTED_EDGE('',*,*,#60,.T.);";
        s += "#81=ORIENTED_EDGE('',*,*,#55,.F.);#82=ORIENTED_EDGE('',*,*,#59,.F.);";
        s += "#83=ORIENTED_EDGE('',*,*,#52,.T.);#84=ORIENTED_EDGE('',*,*,#61,.T.);";
        s += "#85=ORIENTED_EDGE('',*,*,#56,.F.);#86=ORIENTED_EDGE('',*,*,#60,.F.);";
        s += "#87=ORIENTED_EDGE('',*,*,#53,.T.);#88=ORIENTED_EDGE('',*,*,#61,.T.);";
        s += "#89=ORIENTED_EDGE('',*,*,#57,.F.);#90=ORIENTED_EDGE('',*,*,#62,.F.);";
        s += "#91=ORIENTED_EDGE('',*,*,#54,.T.);#92=ORIENTED_EDGE('',*,*,#62,.T.);";
        s += "#93=ORIENTED_EDGE('',*,*,#58,.F.);#94=ORIENTED_EDGE('',*,*,#59,.F.);";
        s += "#101=EDGE_LOOP('',(#71,#72,#73,#74));#102=EDGE_LOOP('',(#75,#76,#77,#78));";
        s += "#103=EDGE_LOOP('',(#79,#80,#81,#82));#104=EDGE_LOOP('',(#83,#84,#85,#86));";
        s += "#105=EDGE_LOOP('',(#87,#88,#89,#90));#106=EDGE_LOOP('',(#91,#92,#93,#94));";
        s += "#111=FACE_OUTER_BOUND('',#101,.T.);#112=FACE_OUTER_BOUND('',#102,.T.);";
        s += "#113=FACE_OUTER_BOUND('',#103,.T.);#114=FACE_OUTER_BOUND('',#104,.T.);";
        s += "#115=FACE_OUTER_BOUND('',#105,.T.);#116=FACE_OUTER_BOUND('',#106,.T.);";
        s += "#121=AXIS2_PLACEMENT_3D('',#1,#26,#21);#122=AXIS2_PLACEMENT_3D('',#5,#23,#21);";
        s += "#123=AXIS2_PLACEMENT_3D('',#1,#25,#21);#124=AXIS2_PLACEMENT_3D('',#2,#21,#22);";
        s += "#125=AXIS2_PLACEMENT_3D('',#4,#22,#21);#126=AXIS2_PLACEMENT_3D('',#1,#24,#22);";
        s += "#131=PLANE('',#121);#132=PLANE('',#122);#133=PLANE('',#123);";
        s += "#134=PLANE('',#124);#135=PLANE('',#125);#136=PLANE('',#126);";
        s += "#141=ADVANCED_FACE('',(#111),#131,.T.);#142=ADVANCED_FACE('',(#112),#132,.T.);";
        s += "#143=ADVANCED_FACE('',(#113),#133,.T.);#144=ADVANCED_FACE('',(#114),#134,.T.);";
        s += "#145=ADVANCED_FACE('',(#115),#135,.T.);#146=ADVANCED_FACE('',(#116),#136,.T.);";
        s += "#151=CLOSED_SHELL('',(#141,#142,#143,#144,#145,#146));";
        s += "#161=MANIFOLD_SOLID_BREP('cube',#151);";
        s
    }

    #[test]
    fn mapped_item_places_a_translated_instance() {
        let mut s = cube_solid();
        // World placements: src at origin, target translated to (10,0,0).
        s += "#200=CARTESIAN_POINT('',(0.,0.,0.));#201=DIRECTION('',(0.,0.,1.));#202=DIRECTION('',(1.,0.,0.));";
        s += "#203=AXIS2_PLACEMENT_3D('src',#200,#201,#202);";
        s += "#210=CARTESIAN_POINT('',(10.,0.,0.));";
        s += "#211=AXIS2_PLACEMENT_3D('tgt',#210,#201,#202);";
        // Sub-representation holding the cube + its world origin.
        s += "#205=GEOMETRIC_REPRESENTATION_CONTEXT(3);";
        s += "#220=SHAPE_REPRESENTATION('component',(#161,#203),#205);";
        // Representation map + mapped item in the assembly root.
        s += "#230=REPRESENTATION_MAP(#203,#220);";
        s += "#240=MAPPED_ITEM('inst',#230,#211);";
        s += "#250=SHAPE_REPRESENTATION('assembly',(#240),#205);";
        let (model, report, caches) = run(&s);
        // The mapped item recorded one placed solid.
        let placed = caches
            .mapped_solids
            .get(&240)
            .unwrap_or_else(|| panic!("MAPPED_ITEM must place a solid: {:?}", report.warnings));
        assert_eq!(placed.len(), 1);
        // The assembly root reaches that solid.
        let assembly = caches.roots.get(&250).expect("assembly root cached");
        assert_eq!(assembly.len(), 1);
        // The placed solid was translated to x≈10: every vertex moved by
        // the target offset, so the minimum X coordinate across all
        // vertices is ≥ 9 (the cube spans [0,1] before placement).
        let mut min_x = f64::INFINITY;
        for vid in model.vertices.iter().map(|(id, _)| id) {
            if let Some(p) = model.vertices.get_position(vid) {
                min_x = min_x.min(p[0]);
            }
        }
        assert!(
            min_x > 9.0,
            "instance should be translated to x≈10, got min_x={min_x}"
        );
    }
}
