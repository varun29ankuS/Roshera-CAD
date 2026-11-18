//! ISO 10303-21 STEP file format.
//!
//! ## Export
//!
//! The writer lives in [`writer`] and is re-exported at module-root
//! for backwards compatibility with callers that imported
//! `crate::formats::step::export_brep_to_step` directly.
//!
//! ## Import
//!
//! The import path is built around a **handler-dispatch architecture**:
//!
//! ```text
//!   bytes
//!     │
//!     ▼ parser::parse_step      (ruststep::parser, schema-agnostic)
//!   Exchange (AST)
//!     │
//!     ▼ EntityRegistry::build
//!   HashMap<#N, IndexedEntity>
//!     │
//!     ▼ EntityDispatch::run_all (Unit → Geometry → Topology → Root)
//!   (BRepModel + ImportReport)
//! ```
//!
//! Coverage is grown one handler at a time in
//! [`handlers`]. IMP1 ships with zero handlers — every entity in the
//! source file is logged as Unsupported and the resulting BRep is
//! empty. IMP2 registers tier-1 (planar + cylindrical solids), IMP3
//! registers tier-2 (NURBS), IMP4 registers tier-3 (assemblies and
//! voids).
//!
//! No silent fallback to a hand-rolled parser. Imports that produce
//! no geometry produce an honest [`ImportReport`] explaining why.

mod assembly_placement;
pub mod context;
pub mod diagnostics;
pub mod dispatch;
pub mod handlers;
pub mod merge;
pub mod parser;
pub mod pcurve;
pub mod registry;
pub mod writer;

pub use merge::merge_solids_into;

// Re-export the writer's full public surface so existing callers
// (`crate::engine::ExportEngine`, `roshera-backend/api-server`)
// continue to compile unchanged.
pub use writer::*;

pub use diagnostics::ImportReport;

use geometry_engine::primitives::topology_builder::BRepModel;
use std::path::Path;

use crate::ExportError;
use context::ImportContext;
use dispatch::EntityDispatch;
use registry::EntityRegistry;

/// Import a STEP file into a fresh [`BRepModel`].
///
/// Convenience wrapper around [`import_step_to_brep_with_report`] for
/// callers that don't need the diagnostics report.
///
/// **Note:** as of IMP1, no entity handlers are registered, so the
/// returned BRepModel is always empty. The full report (with every
/// entity logged as Unsupported) is dropped. Callers that want the
/// report should use [`import_step_to_brep_with_report`] directly.
pub async fn import_step_to_brep(path: &Path) -> Result<BRepModel, ExportError> {
    let (model, _report) = import_step_to_brep_with_report(path).await?;
    Ok(model)
}

/// Import a STEP file, returning both the populated [`BRepModel`] and
/// a structured [`ImportReport`].
///
/// `ok = false` in the report when no root representation produced
/// non-empty geometry (every root entity was unsupported or every
/// handler that ran failed). The BRep is still returned — it may be
/// empty — so callers can decide whether to surface the report to the
/// user, retry with a different file, or accept the partial import.
pub async fn import_step_to_brep_with_report(
    path: &Path,
) -> Result<(BRepModel, ImportReport), ExportError> {
    // Read the entire file. STEP files are typically a few MB at most;
    // the largest "real" parts we've seen are ~50 MB. Streaming is
    // not worth the complexity at this scale.
    let path_str = path.to_string_lossy().to_string();
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|_| ExportError::FileReadError {
            path: path_str.clone(),
        })?;
    let text = String::from_utf8(bytes).map_err(|e| ExportError::ExportFailed {
        reason: format!(
            "STEP file {path_str} is not valid UTF-8 \
             (ISO 10303-21 §6.4 mandates 7-bit ASCII): {e}"
        ),
    })?;

    import_step_text_with_report(&text, &path_str)
}

/// Import STEP from an in-memory exchange-structure string (no file I/O).
///
/// The agent/REST path receives STEP content inline; this is the shared
/// core that [`import_step_to_brep_with_report`] delegates to after
/// reading the file. `source_hint` is used only in diagnostics.
pub fn import_step_text_with_report(
    text: &str,
    source_hint: &str,
) -> Result<(BRepModel, ImportReport), ExportError> {
    let (mut model, mut report) = import_step_text_uncertified(text, source_hint)?;
    // Validation gate (IMP5): the FULL certificate on every solid, one batch.
    let t = std::time::Instant::now();
    let solid_ids: Vec<_> = model.solids.iter().map(|(sid, _)| sid).collect();
    let certs = model.certify_solids(&solid_ids);
    apply_certificates(&mut report, &certs);
    if import_profile_enabled() {
        eprintln!(
            "[import-profile] certify {} solids {:?}",
            solid_ids.len(),
            t.elapsed()
        );
    }
    Ok((model, report))
}

/// [`import_step_text_with_report`] without the certificate: everything up
/// to and including assembly placement, with `report.certified = false` and
/// `report.ok = false` (an unverified import is not a verified one).
///
/// For a caller that certifies the solids itself where they END UP: the
/// REST import route merges them into the document and certifies them there
/// (a certificate also judges the model around a solid, so the document is
/// the right place), then calls [`apply_certificates`]. Certifying here as
/// well ran every check twice and produced two verdicts for one part.
pub fn import_step_text_uncertified(
    text: &str,
    source_hint: &str,
) -> Result<(BRepModel, ImportReport), ExportError> {
    // Parse → index → dispatch.
    let prof = import_profile_enabled();
    let t_all = std::time::Instant::now();
    let exchange = parser::parse_step(text, source_hint)?;
    if prof {
        eprintln!("[import-profile] parse {:?}", t_all.elapsed());
    }
    let t = std::time::Instant::now();
    let registry = EntityRegistry::build(&exchange);
    if prof {
        eprintln!("[import-profile] index {:?}", t.elapsed());
    }
    let mut model = BRepModel::new();
    let mut report = ImportReport::new();

    // Extract FILE_SCHEMA from the header, if present, for the report.
    report.schema = extract_file_schema(&exchange);

    let mut dispatch = EntityDispatch::new();
    handlers::register_all(&mut dispatch);

    let t = std::time::Instant::now();
    let (resolved, roots_with_solids, total_root_solids, mut caches) = {
        let mut ctx = ImportContext::new(&mut model, &mut report);
        let resolved = dispatch.run_all(&registry, &mut ctx);
        let roots_with_solids = ctx.caches.roots.values().filter(|v| !v.is_empty()).count();
        let total_root_solids: usize = ctx.caches.roots.values().map(|v| v.len()).sum();
        let caches = std::mem::take(&mut ctx.caches);
        (resolved, roots_with_solids, total_root_solids, caches)
    };
    if prof {
        eprintln!("[import-profile] build geometry {:?}", t.elapsed());
    }
    report.roots_resolved = roots_with_solids;
    report.solids_in_roots = total_root_solids;
    // Each repair pass is timed under ROSHERA_IMPORT_PROFILE: together they
    // were most of an import (37.7 s of CADGenBench part 208's 38 s before
    // certification) and none was measured on its own.
    let lap = |label: &str, t: std::time::Instant| {
        if prof {
            eprintln!("[import-profile] repair: {label} {:?}", t.elapsed());
        }
    };
    let t = std::time::Instant::now();
    fit_linear_extrusions_to_faces(&mut model, &mut caches);
    lap("fit linear extrusions", t);
    let t = std::time::Instant::now();
    merge_vertices_a_loop_passes_between(&mut model, &mut caches, &mut report);
    lap("merge loop-crossed vertices", t);
    let t = std::time::Instant::now();
    tighten_vertices_onto_their_faces(&mut model, &mut caches, &mut report);
    lap("tighten vertices onto their faces", t);
    let t = std::time::Instant::now();
    collapse_edges_shorter_than_their_tolerance(&mut model, &mut caches, &mut report);
    lap("collapse short edges", t);
    let t = std::time::Instant::now();
    rebuild_edges_onto_face_intersections(&mut model, &mut caches, &mut report);
    lap("rebuild edges on face intersections", t);
    let t = std::time::Instant::now();
    raise_edge_tolerances_to_measured_gaps(&mut model, &mut caches, &mut report);
    lap("raise edge tolerances to gaps", t);
    let t = std::time::Instant::now();
    reverse_loops_walking_against_neighbours(&mut model, &mut caches, &mut report);
    lap("reverse loops against neighbours", t);

    // Place assembly instances from the product tree, then give every solid
    // and face its identity — PIDs keyed by this file's CONTENT and the STEP
    // entity each came from, and `imported:step` provenance.
    let t = std::time::Instant::now();
    let file_root = geometry_engine::primitives::persistent_id::PersistentId::root(text.as_bytes());
    let mut model = assembly_placement::finalize(
        model,
        &caches,
        &registry,
        &mut report,
        file_root,
        source_hint,
    );

    if prof {
        eprintln!("[import-profile] place assembly {:?}", t.elapsed());
    }
    // Structurally complete: at least one entity resolved, at least one
    // kernel solid materialised, and every part the assembly uses came in (a
    // use whose geometry built no solid is recorded as missing). `ok` also
    // needs every solid SOUND, which only the certificate can say.
    report.structurally_complete =
        resolved > 0 && !model.solids.is_empty() && report.missing_parts == 0;
    report.certified = false;
    report.ok = false;
    Ok((model, report))
}

/// Fold full certificates into an import report: one verdict per solid on
/// `report.validation`, `certified = true`, and `ok` = structurally complete
/// AND every solid sound.
///
/// This is the SAME ground-truth certificate the ambient perception verdict
/// reports -- deliberately NOT the weaker B-Rep-only `validate_solid_scoped`,
/// which once let the report claim `valid: true` on a solid whose display
/// mesh did not close (a re-imported blend rim). Each verdict carries the
/// component flags (`brep_valid`, `watertight`, `manifold`, `oriented`)
/// beside the `sound` headline, so the caller sees which solid failed and
/// why. One function for both import paths, so they cannot drift.
pub fn apply_certificates(
    report: &mut ImportReport,
    certs: &[(
        geometry_engine::primitives::solid::SolidId,
        geometry_engine::primitives::provenance::ValidityCertificate,
    )],
) {
    report.validation.clear();
    let mut all_sound = true;
    for (solid_id, cert) in certs {
        let sound = cert.is_sound();
        all_sound &= sound;
        // Cap the captured messages so a pathological shell can't bloat the
        // report; the count is always exact.
        let errors: Vec<String> = cert.errors.iter().take(8).cloned().collect();
        report.validation.push(diagnostics::SolidValidation {
            solid_id: *solid_id,
            valid: sound,
            brep_valid: cert.brep_valid,
            watertight: cert.watertight,
            manifold: cert.manifold,
            oriented: cert.oriented,
            sound,
            error_count: cert.errors.len(),
            errors,
        });
    }
    report.certified = true;
    report.ok = report.structurally_complete && !certs.is_empty() && all_sound;
}

/// `ROSHERA_IMPORT_PROFILE=1` prints how long each import stage took, to
/// stderr. Off by default; the same switch the drawing pipeline has
/// (`ROSHERA_DRAW_PROFILE`).
fn import_profile_enabled() -> bool {
    std::env::var_os("ROSHERA_IMPORT_PROFILE").is_some_and(|v| !v.is_empty())
}

/// Pull the AP / schema identifier out of the parsed `FILE_SCHEMA`
/// header. Returns `None` when the header is absent or malformed.
fn extract_file_schema(exchange: &ruststep::ast::Exchange) -> Option<String> {
    use ruststep::ast::Parameter;
    for record in &exchange.header {
        if record.name == "FILE_SCHEMA" {
            // FILE_SCHEMA is one parameter: a list-of-strings.
            if let Parameter::List(items) = &record.parameter {
                if let Some(Parameter::List(inner)) = items.first() {
                    if let Some(Parameter::String(s)) = inner.first() {
                        return Some(s.clone());
                    }
                }
            }
        }
    }
    None
}

/// An edge shorter than this (mm), or than its own tolerance if that is
/// larger, has no extent: it is a point.
const SMALL_EDGE_FLOOR: f64 = 1e-5;

/// Collapse every edge shorter than its own tolerance (or
/// [`SMALL_EDGE_FLOOR`]) to a point: merge its end vertex into its start
/// vertex everywhere and take it out of every loop. Such an edge has no
/// extent, but as an edge it still has two vertices a hair apart, and the
/// mesh weld joins or separates them by rounding -- the faces meeting there
/// then do not close (CADGenBench part 107: a 1.2 micrometre edge at a
/// four-face corner left four open mesh edges). Open CASCADE's
/// `ShapeFix_Wire::FixSmall` makes the same repair. Each collapse is a
/// [`diagnostics::HealingKind::SmallEdgeCollapsed`] (`deviation` the edge's
/// extent, `tolerance` the bound) and a Lens event on each face using it.
fn collapse_edges_shorter_than_their_tolerance(
    model: &mut geometry_engine::primitives::topology_builder::BRepModel,
    caches: &mut context::ResolutionCaches,
    report: &mut ImportReport,
) {
    use std::collections::HashMap;
    let mut edge_ids: Vec<u32> = model.edges.iter().map(|(e, _)| e).collect();
    edge_ids.sort_unstable();
    let face_instance: HashMap<u32, u64> = caches.faces.iter().map(|(n, &f)| (f, *n)).collect();
    let edge_instance: HashMap<u32, u64> = caches.edges.iter().map(|(n, &e)| (e, *n)).collect();
    for eid in edge_ids {
        let Some(edge) = model.edges.get(eid) else {
            continue;
        };
        let (keep, gone) = (edge.start_vertex, edge.end_vertex);
        if keep == gone {
            continue;
        }
        let (Some(a), Some(b)) = (model.vertices.get(keep), model.vertices.get(gone)) else {
            continue;
        };
        let a = a.point();
        let bound = edge.tolerance.max(SMALL_EDGE_FLOOR);
        // The edge's extent: its ends, and how far its curve strays from
        // the start between them.
        let mut extent = (b.point() - a).magnitude();
        for k in 1..8 {
            if let Ok(p) = edge.evaluate(k as f64 / 8.0, &model.curves) {
                extent = extent.max((p - a).magnitude());
            }
        }
        if !(extent < bound) {
            continue;
        }
        // Merge `gone` into `keep` on every edge.
        let others: Vec<u32> = model.edges.iter().map(|(e, _)| e).collect();
        for other in others {
            if let Some(e) = model.edges.get_mut(other) {
                if e.start_vertex == gone {
                    e.start_vertex = keep;
                }
                if e.end_vertex == gone {
                    e.end_vertex = keep;
                }
            }
        }
        for v in caches.vertices.values_mut() {
            if *v == gone {
                *v = keep;
            }
        }
        // Take the edge out of every loop, noting the faces it bounded.
        let mut faces = Vec::new();
        let loop_ids: Vec<u32> = model.loops.iter().map(|(l, _)| l).collect();
        for l in loop_ids {
            let Some(lp) = model.loops.get_mut(l) else {
                continue;
            };
            while let Some(i) = lp.edges.iter().position(|&e| e == eid) {
                lp.remove_edge(i);
                faces.push(l);
            }
        }
        let face_ids: Vec<u32> = model
            .faces
            .iter()
            .filter(|(_, f)| {
                faces.contains(&f.outer_loop) || f.inner_loops.iter().any(|l| faces.contains(l))
            })
            .map(|(f, _)| f)
            .collect();
        let instance = edge_instance.get(&eid).copied().unwrap_or(0);
        report.healings.push(diagnostics::Healing {
            kind: diagnostics::HealingKind::SmallEdgeCollapsed,
            entity: "EDGE_CURVE".into(),
            instance,
            deviation: extent,
            tolerance: bound,
        });
        for f in face_ids {
            if let Some(&n) = face_instance.get(&f) {
                caches.record_lens(
                    n,
                    geometry_engine::lens::Stage::Repair,
                    geometry_engine::lens::EventKind::SmallEdgeCollapsed,
                    Some(instance),
                    "an edge of this face was shorter than its tolerance; it was collapsed to a point",
                    &[("extent_mm", extent), ("bound_mm", bound)],
                );
            }
        }
    }
}

/// Make one vertex of any two that a loop passes from one to the other at the
/// same point: one edge of the loop ends at the first, the next edge starts at
/// the second, and the two lie within the larger of the edges' tolerances.
/// Some writers emit a `VERTEX_POINT` per edge end rather than per corner
/// (CADGenBench part 132 has two at exactly (-40, 2.36e-13, 128)); the loop
/// then does not chain and the B-Rep is invalid. Only vertices a loop joins
/// are merged: two corners that merely touch (bodies meeting at a point) stay
/// two vertices. Every edge is pointed at the kept vertex (the lower id, so
/// the outcome is deterministic); each merge is recorded as a
/// [`diagnostics::HealingKind::VerticesMergedToCloseLoop`] and, on each face
/// whose loop it closed, in the Lens journal.
fn merge_vertices_a_loop_passes_between(
    model: &mut geometry_engine::primitives::topology_builder::BRepModel,
    caches: &mut context::ResolutionCaches,
    report: &mut ImportReport,
) {
    use std::collections::{BTreeMap, HashMap};
    let floor = geometry_engine::math::Tolerance::default().distance();
    // Union-find over vertex ids; the root of a set is its lowest id.
    let mut parent: HashMap<u32, u32> = HashMap::new();
    fn root(parent: &HashMap<u32, u32>, mut v: u32) -> u32 {
        while let Some(&p) = parent.get(&v) {
            if p == v {
                break;
            }
            v = p;
        }
        v
    }
    // (kept, merged) -> (distance, bound, faces whose loop broke there).
    let mut merges: BTreeMap<(u32, u32), (f64, f64, Vec<u32>)> = BTreeMap::new();
    let mut faces: Vec<u32> = model.faces.iter().map(|(fid, _)| fid).collect();
    faces.sort_unstable();
    for fid in faces {
        let Some(face) = model.faces.get(fid) else {
            continue;
        };
        let loops: Vec<u32> = std::iter::once(face.outer_loop)
            .chain(face.inner_loops.iter().copied())
            .collect();
        for l in loops {
            let Some(lp) = model.loops.get(l) else {
                continue;
            };
            // Each edge as walked: (start, end, tolerance).
            let walked: Vec<(u32, u32, f64)> = lp
                .edges
                .iter()
                .enumerate()
                .filter_map(|(k, &e)| {
                    let edge = model.edges.get(e)?;
                    let fwd = lp.orientations.get(k).copied().unwrap_or(true);
                    Some(if fwd {
                        (edge.start_vertex, edge.end_vertex, edge.tolerance)
                    } else {
                        (edge.end_vertex, edge.start_vertex, edge.tolerance)
                    })
                })
                .collect();
            if walked.len() != lp.edges.len() {
                continue;
            }
            for k in 0..walked.len() {
                let (_, end, tol_a) = walked[k];
                let (start, _, tol_b) = walked[(k + 1) % walked.len()];
                let (ra, rb) = (root(&parent, end), root(&parent, start));
                if ra == rb {
                    continue;
                }
                let (Some(pa), Some(pb)) = (model.vertices.get(ra), model.vertices.get(rb)) else {
                    continue;
                };
                let d = ((pa.position[0] - pb.position[0]).powi(2)
                    + (pa.position[1] - pb.position[1]).powi(2)
                    + (pa.position[2] - pb.position[2]).powi(2))
                .sqrt();
                let bound = tol_a.max(tol_b).max(floor);
                if !(d <= bound) {
                    continue;
                }
                let (kept, merged) = (ra.min(rb), ra.max(rb));
                parent.insert(merged, kept);
                parent.entry(kept).or_insert(kept);
                merges
                    .entry((kept, merged))
                    .or_insert((d, bound, Vec::new()))
                    .2
                    .push(fid);
            }
        }
    }
    if merges.is_empty() {
        return;
    }
    let edge_ids: Vec<u32> = model.edges.iter().map(|(eid, _)| eid).collect();
    for eid in edge_ids {
        if let Some(edge) = model.edges.get_mut(eid) {
            edge.start_vertex = root(&parent, edge.start_vertex);
            edge.end_vertex = root(&parent, edge.end_vertex);
        }
    }
    let vertex_instance: HashMap<u32, u64> =
        caches.vertices.iter().map(|(n, &vid)| (vid, *n)).collect();
    for v in caches.vertices.values_mut() {
        *v = root(&parent, *v);
    }
    let face_instance: HashMap<u32, u64> = caches.faces.iter().map(|(n, &fid)| (fid, *n)).collect();
    for ((kept, merged), (d, bound, broke)) in merges {
        let merged_instance = vertex_instance.get(&merged).copied();
        report.healings.push(diagnostics::Healing {
            kind: diagnostics::HealingKind::VerticesMergedToCloseLoop,
            entity: "VERTEX_POINT".into(),
            instance: merged_instance.unwrap_or(0),
            deviation: d,
            tolerance: bound,
        });
        let kept_instance = vertex_instance.get(&kept).copied().unwrap_or(0) as f64;
        for fid in broke {
            if let Some(&n) = face_instance.get(&fid) {
                caches.record_lens(
                    n,
                    geometry_engine::lens::Stage::Repair,
                    geometry_engine::lens::EventKind::VerticesMerged,
                    merged_instance,
                    "its loop passed between two vertices at one point; they were made one",
                    &[
                        ("gap_mm", d),
                        ("bound_mm", bound),
                        ("kept_vertex", kept_instance),
                    ],
                );
            }
        }
    }
}

/// How far off its faces -- or off the end of an edge's curve -- a vertex
/// must be before it is tightened (model units): the importer's on-curve
/// tolerance; closer than this a vertex is ON them and is left alone.
const TIGHTEN_ABOVE: f64 = 1e-3;

/// The farthest a vertex is moved when tightened (model units, mm): the same
/// bound as a vertex's distance off its edge's curve, just above the largest
/// gap on the valid CADGenBench parts (4.7e-2). A point farther from where
/// the file put the vertex is not that vertex.
const TIGHTEN_MOVE_CAP: f64 = 0.05;

/// TIGHTEN every vertex that lies off the faces meeting at it, or off the
/// end of an edge's curve, by more than [`TIGHTEN_ABOVE`]: move it to the
/// point that lies on those faces, then re-fit the edges ending there (a
/// line is rebuilt between its vertices; any other curve is re-trimmed at its
/// nearest point to the vertex). Writers keep a tolerance on each vertex,
/// and on CADGenBench parts 211, 212, 240 and 246 none of 137 such vertices
/// lay on all its faces; accepting the gap as a tolerance (what OCCT does on
/// import) leaves the model inexact, so the gap is closed instead -- the
/// "tightening" of tolerant-modelling kernels.
///
/// The new point is the best of: the vertex, each curve end there, and their
/// mean, each refined by averaging its projections onto the faces while that
/// lowers the worst distance. A vertex is moved only when that strictly
/// lowers its worst distance to its faces and the move is within
/// [`TIGHTEN_MOVE_CAP`]; whatever distance is left becomes the vertex's
/// tolerance. Each move is a [`diagnostics::HealingKind::VertexTightenedOntoFaces`]
/// (`deviation` the distance off the faces before, `tolerance` after).
fn tighten_vertices_onto_their_faces(
    model: &mut geometry_engine::primitives::topology_builder::BRepModel,
    caches: &mut context::ResolutionCaches,
    report: &mut ImportReport,
) {
    use geometry_engine::math::{Point3, Tolerance};
    use std::collections::{BTreeMap, BTreeSet, HashMap};

    // The edges and faces at each vertex.
    let mut edges_at: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut edge_ids: Vec<u32> = model.edges.iter().map(|(e, _)| e).collect();
    edge_ids.sort_unstable();
    for &eid in &edge_ids {
        if let Some(e) = model.edges.get(eid) {
            edges_at.entry(e.start_vertex).or_default().push(eid);
            if e.end_vertex != e.start_vertex {
                edges_at.entry(e.end_vertex).or_default().push(eid);
            }
        }
    }
    let mut faces_at: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for (fid, face) in model.faces.iter() {
        for l in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
            let Some(lp) = model.loops.get(l) else {
                continue;
            };
            for &eid in &lp.edges {
                if let Some(e) = model.edges.get(eid) {
                    faces_at.entry(e.start_vertex).or_default().insert(fid);
                    faces_at.entry(e.end_vertex).or_default().insert(fid);
                }
            }
        }
    }

    // A point's projection onto each face's surface, with the surface's
    // unit normal there.
    let project = |faces: &BTreeSet<u32>, p: &Point3| -> Option<Vec<(Point3, [f64; 3])>> {
        faces
            .iter()
            .map(|&f| {
                let s = model.surfaces.get(model.faces.get(f)?.surface_id)?;
                let (u, v) = s.closest_point(p, Tolerance::default()).ok()?;
                let q = s.point_at(u, v).ok()?;
                let n = s.normal_at(u, v).ok()?;
                let len = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt();
                (len > 0.0).then(|| (q, [n.x / len, n.y / len, n.z / len]))
            })
            .collect()
    };
    let worst_off = |faces: &BTreeSet<u32>, p: &Point3| -> f64 {
        match project(faces, p) {
            Some(q) => q.iter().map(|(q, _)| p.distance(q)).fold(0.0_f64, f64::max),
            None => f64::INFINITY,
        }
    };
    // One Gauss-Newton step: the point nearest `p` on every face's tangent
    // plane at `p`'s projection (least squares, a small pull towards `p`
    // fixing the directions fewer than three faces constrain). Exact in one
    // step for planes; quadratic for curved faces.
    let newton = |q: &[(Point3, [f64; 3])], p: &Point3| -> Option<Point3> {
        const PULL: f64 = 1e-9;
        let mut a = [[0.0_f64; 3]; 3];
        let mut b = [PULL * p.x, PULL * p.y, PULL * p.z];
        for (i, row) in a.iter_mut().enumerate() {
            row[i] = PULL;
        }
        for (qp, n) in q {
            let d = n[0] * qp.x + n[1] * qp.y + n[2] * qp.z;
            for i in 0..3 {
                for j in 0..3 {
                    a[i][j] += n[i] * n[j];
                }
                b[i] += n[i] * d;
            }
        }
        let det = |m: &[[f64; 3]; 3]| {
            m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
                - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
                + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
        };
        let d = det(&a);
        if !(d.abs() > 0.0) {
            return None;
        }
        let mut x = [0.0_f64; 3];
        for (k, xk) in x.iter_mut().enumerate() {
            let mut m = a;
            for i in 0..3 {
                m[i][k] = b[i];
            }
            *xk = det(&m) / d;
        }
        x.iter()
            .all(|c| c.is_finite())
            .then(|| Point3::new(x[0], x[1], x[2]))
    };
    // The end of edge `eid`'s trimmed curve at vertex `vid`: whether it is
    // the range's start, and the point there.
    let end_at = |eid: u32, vid: u32| -> Vec<(bool, Point3)> {
        let Some(e) = model.edges.get(eid) else {
            return Vec::new();
        };
        let Some(c) = model.curves.get(e.curve_id) else {
            return Vec::new();
        };
        let r = e.param_range;
        let (Ok(a), Ok(b)) = (c.evaluate(r.start), c.evaluate(r.end)) else {
            return Vec::new();
        };
        if e.start_vertex == e.end_vertex {
            return vec![(true, a.position), (false, b.position)];
        }
        let Some(v) = model.vertices.get(vid) else {
            return Vec::new();
        };
        let p = v.point();
        vec![if p.distance(&a.position) <= p.distance(&b.position) {
            (true, a.position)
        } else {
            (false, b.position)
        }]
    };

    struct Move {
        vertex: u32,
        from: Point3,
        to: Point3,
        before: f64,
        after: f64,
        faces: Vec<u32>,
    }
    let mut moves: Vec<Move> = Vec::new();
    for (&vid, edges) in &edges_at {
        // A point is pinned only where two or more faces meet: a vertex of a
        // single face (a plate written off its plane) is not tightened --
        // moving it would rewrite the face, not close a gap -- and is left to
        // the edge-tolerance repair or its refusal.
        let Some(faces) = faces_at.get(&vid).filter(|f| f.len() >= 2) else {
            continue;
        };
        let Some(v) = model.vertices.get(vid) else {
            continue;
        };
        let p = v.point();
        let ends: Vec<Point3> = edges
            .iter()
            .flat_map(|&e| end_at(e, vid))
            .map(|(_, q)| q)
            .collect();
        let curve_gap = ends.iter().map(|q| p.distance(q)).fold(0.0_f64, f64::max);
        let before = worst_off(faces, &p);
        if !(curve_gap > TIGHTEN_ABOVE || (before > TIGHTEN_ABOVE && before.is_finite())) {
            continue;
        }
        let mut candidates: Vec<Point3> = vec![p];
        candidates.extend(ends.iter().copied());
        let n = (ends.len() + 1) as f64;
        let (sx, sy, sz) = ends
            .iter()
            .fold((p.x, p.y, p.z), |(x, y, z), q| (x + q.x, y + q.y, z + q.z));
        candidates.push(Point3::new(sx / n, sy / n, sz / n));
        let mut best = (p, before);
        for start in candidates {
            let mut at = (start, worst_off(faces, &start));
            for _ in 0..16 {
                let Some(q) = project(faces, &at.0) else {
                    break;
                };
                // A Gauss-Newton step, else the mean of the projections; kept
                // only while it lowers the worst distance.
                let k = q.len() as f64;
                let (x, y, z) = q.iter().fold((0.0, 0.0, 0.0), |(x, y, z), (q, _)| {
                    (x + q.x, y + q.y, z + q.z)
                });
                let mean = Point3::new(x / k, y / k, z / k);
                let step = newton(&q, &at.0)
                    .map(|s| (s, worst_off(faces, &s)))
                    .filter(|(_, off)| *off < at.1 * (1.0 - 1e-9))
                    .or_else(|| {
                        let off = worst_off(faces, &mean);
                        (off < at.1 * (1.0 - 1e-9)).then_some((mean, off))
                    });
                match step {
                    Some(next) => at = next,
                    None => break,
                }
                if at.1 <= 1e-12 {
                    break;
                }
            }
            if at.1 < best.1 && p.distance(&at.0) <= TIGHTEN_MOVE_CAP {
                best = at;
            }
        }
        let (to, after) = best;
        if !(after < before) || p.distance(&to) > TIGHTEN_MOVE_CAP {
            continue;
        }
        moves.push(Move {
            vertex: vid,
            from: p,
            to,
            before,
            after,
            faces: faces.iter().copied().collect(),
        });
    }
    if moves.is_empty() {
        return;
    }

    for m in &moves {
        model
            .vertices
            .set_position(m.vertex, m.to.x, m.to.y, m.to.z);
    }
    // Re-fit every edge ending at a moved vertex: (old, new) position.
    let moved: HashMap<u32, (Point3, Point3)> =
        moves.iter().map(|m| (m.vertex, (m.from, m.to))).collect();
    let mut touched: BTreeSet<u32> = BTreeSet::new();
    for m in &moves {
        if let Some(es) = edges_at.get(&m.vertex) {
            touched.extend(es.iter().copied());
        }
    }
    for eid in touched {
        let Some(e) = model.edges.get(eid) else {
            continue;
        };
        let (sv, ev, cid, r) = (e.start_vertex, e.end_vertex, e.curve_id, e.param_range);
        let Some(curve) = model.curves.get(cid) else {
            continue;
        };
        if curve.type_name() == "Line" {
            let (Some(a), Some(b)) = (model.vertices.get(sv), model.vertices.get(ev)) else {
                continue;
            };
            let (a, b) = (a.point(), b.point());
            if a.distance(&b) <= TIGHTEN_ABOVE {
                continue;
            }
            if let Some(slot) = model.curves.get_mut(cid) {
                *slot = Box::new(geometry_engine::primitives::curve::Line::new(a, b));
            }
            continue;
        }
        if sv == ev {
            // A closed edge keeps its whole curve.
            continue;
        }
        let span = r.end - r.start;
        let (Ok(a), Ok(b)) = (curve.evaluate(r.start), curve.evaluate(r.end)) else {
            continue;
        };
        let mut range = r;
        for vid in [sv, ev] {
            let Some(&(from, to)) = moved.get(&vid) else {
                continue;
            };
            // The end of the trimmed curve the vertex WAS at.
            let at_start = from.distance(&a.position) <= from.distance(&b.position);
            let Ok((t, _)) = curve.closest_point(&to, Tolerance::from_distance(1e-10)) else {
                continue;
            };
            let old = if at_start { r.start } else { r.end };
            // Only a re-trim NEAR the old end: a far parameter is another
            // branch of the curve, not this end moved.
            if !((t - old).abs() <= 0.25 * span.abs()) {
                continue;
            }
            if at_start {
                range.start = t;
            } else {
                range.end = t;
            }
        }
        if range.end > range.start && (range.start != r.start || range.end != r.end) {
            if let Some(edge) = model.edges.get_mut(eid) {
                edge.param_range = range;
            }
        }
    }

    let vertex_instance: HashMap<u32, u64> =
        caches.vertices.iter().map(|(n, &vid)| (vid, *n)).collect();
    let face_instance: HashMap<u32, u64> = caches.faces.iter().map(|(n, &f)| (f, *n)).collect();
    for m in moves {
        if let Some(v) = model.vertices.get(m.vertex) {
            if m.after > v.tolerance {
                model
                    .vertices
                    .set_tolerance(m.vertex, m.after * (1.0 + 1e-6));
            }
        }
        let instance = vertex_instance.get(&m.vertex).copied();
        report.healings.push(diagnostics::Healing {
            kind: diagnostics::HealingKind::VertexTightenedOntoFaces,
            entity: "VERTEX_POINT".into(),
            instance: instance.unwrap_or(0),
            deviation: m.before,
            tolerance: m.after,
        });
        let moved_by = m.from.distance(&m.to);
        for f in &m.faces {
            if let Some(&n) = face_instance.get(f) {
                caches.record_lens(
                    n,
                    geometry_engine::lens::Stage::Repair,
                    geometry_engine::lens::EventKind::VertexTightened,
                    instance,
                    "a corner of this face lay off the faces meeting there; it was moved onto them and the edges ending there re-fitted",
                    &[
                        ("moved_mm", moved_by),
                        ("off_faces_before_mm", m.before),
                        ("off_faces_after_mm", m.after),
                    ],
                );
            }
        }
    }
}

/// Reverse the loops of any face that walks MOST of its shared edges the same
/// way as the neighbouring face using each edge. Two faces of a consistently
/// oriented shell walk a shared edge in opposite directions; a face against
/// the majority of its neighbours had its loops written backwards (its sense
/// still agrees with theirs -- its mesh faces out of the solid -- so only the
/// loops are wrong). Each reversal is recorded as a
/// [`diagnostics::HealingKind::LoopReversedToMatchNeighbours`]. Decided on the
/// file's topology before any reversal, in face-id order, so the outcome is
/// deterministic. A seam edge (used twice by one loop) does not vote.
fn reverse_loops_walking_against_neighbours(
    model: &mut geometry_engine::primitives::topology_builder::BRepModel,
    caches: &mut context::ResolutionCaches,
    report: &mut ImportReport,
) {
    use std::collections::HashMap;
    let instance_of: HashMap<u32, u64> = caches
        .faces
        .iter()
        .map(|(inst, &fid)| (fid, *inst))
        .collect();
    // edge -> every (face, walks it start->end) use.
    let mut uses: HashMap<u32, Vec<(u32, bool)>> = HashMap::new();
    let mut face_loops: Vec<(u32, Vec<u32>)> = Vec::new();
    for (fid, face) in model.faces.iter() {
        let loops: Vec<u32> = std::iter::once(face.outer_loop)
            .chain(face.inner_loops.iter().copied())
            .collect();
        for &l in &loops {
            let Some(lp) = model.loops.get(l) else {
                continue;
            };
            let mut count: HashMap<u32, usize> = HashMap::new();
            for &e in &lp.edges {
                *count.entry(e).or_default() += 1;
            }
            for (k, &e) in lp.edges.iter().enumerate() {
                if count.get(&e).copied().unwrap_or(0) > 1 {
                    continue;
                }
                uses.entry(e)
                    .or_default()
                    .push((fid, lp.orientations.get(k).copied().unwrap_or(true)));
            }
        }
        face_loops.push((fid, loops));
    }
    // Per face: shared edges walked the same way as the neighbour, and all shared.
    let mut tally: HashMap<u32, (usize, usize)> = HashMap::new();
    for w in uses.values().filter(|w| w.len() == 2) {
        let same = w[0].1 == w[1].1;
        for &(f, _) in w {
            let t = tally.entry(f).or_default();
            t.1 += 1;
            if same {
                t.0 += 1;
            }
        }
    }
    face_loops.sort_by_key(|(f, _)| *f);
    for (fid, loops) in face_loops {
        let Some(&(same, shared)) = tally.get(&fid) else {
            continue;
        };
        if 2 * same <= shared {
            continue;
        }
        for l in loops {
            if let Some(lp) = model.loops.get_mut(l) {
                lp.reverse();
            }
        }
        report.healings.push(diagnostics::Healing {
            kind: diagnostics::HealingKind::LoopReversedToMatchNeighbours,
            entity: "ADVANCED_FACE".into(),
            instance: instance_of.get(&fid).copied().unwrap_or(0),
            deviation: same as f64,
            tolerance: shared as f64,
        });
        if let Some(&n) = instance_of.get(&fid) {
            caches.record_lens(
                n,
                geometry_engine::lens::Stage::Repair,
                geometry_engine::lens::EventKind::LoopReversedToMatchNeighbours,
                Some(n),
                "walked most of its shared edges the same way as its neighbours; its loops were reversed",
                &[("same_way_edges", same as f64), ("shared_edges", shared as f64)],
            );
        }
    }
}

/// The largest gap between an edge and a face's surface that the importer
/// repairs by raising the edge's tolerance (mm). Beyond it an edge is not on
/// its face and the part is refused, with the gap in the report.
pub const EDGE_GAP_REPAIR_CAP: f64 = 0.01;

/// The farthest the importer moves any point of an edge to put it on the
/// intersection of its two faces (mm). The file's curve is the evidence of
/// WHICH intersection branch the edge is: an edge that would have to move
/// farther is not an approximation of that intersection, and is left alone.
pub const EDGE_REBUILD_CAP: f64 = 0.1;

/// Samples per rebuilt edge to start with; the fitted cubic interpolates
/// them all, and they double until the edge lies on both faces to
/// [`EDGE_REBUILD_TARGET`] or reach [`EDGE_REBUILD_MAX_SAMPLES`].
const EDGE_REBUILD_SAMPLES: usize = 33;
const EDGE_REBUILD_MAX_SAMPLES: usize = 513;

/// How closely a rebuilt edge must lie on both faces (mm).
const EDGE_REBUILD_TARGET: f64 = 1e-6;

/// Golden-section steps per sample across the edge.
const EDGE_REBUILD_ITERATIONS: usize = 120;

/// A closest approach found within this fraction of [`EDGE_REBUILD_CAP`] of
/// the search's bound is taken as the bound itself: no minimum in reach.
const EDGE_REBUILD_BOUND_FRACTION: f64 = 0.99;

/// A rebuilt edge is kept only if its gap is at most this fraction of the
/// file curve's (or, for an edge beyond [`EDGE_GAP_REPAIR_CAP`], if it
/// brings the edge within it).
const EDGE_REBUILD_MIN_GAIN: f64 = 0.1;

/// Rebuild every edge whose 3D curve lies off one of its two faces ON the
/// intersection of those faces: sample the file's curve, carry each sample
/// onto both surfaces by alternating projection, and interpolate the
/// converged points with a cubic whose ends are the edge's vertices.
///
/// An edge is, by definition, where its two faces meet. Writers can still
/// give it a 3D curve that only approximates the intersection -- Open
/// CASCADE keeps the exact edge in pcurves and a per-edge tolerance it does
/// not export, and writes a rough companion curve (CADGenBench part 231:
/// plain cubic Beziers for torus-plane cuts, 0.022 mm off the torus).
/// Kernels that heal this (OCCT's `ShapeFix`, ACIS and Parasolid tolerant
/// edges) widen the edge's tolerance around the wrong curve; this puts the
/// edge where it belongs. Done only when every sample converges onto both
/// surfaces and moves at most [`EDGE_REBUILD_CAP`]; the result is
/// re-measured and kept only if it lies on both faces more closely than the
/// file's curve did. Each rebuild is a
/// [`diagnostics::HealingKind::EdgeRebuiltOnFaceIntersection`] (the gap
/// before as `deviation`, after as `tolerance`) and a Lens event on both
/// faces. What cannot be rebuilt is left to
/// [`raise_edge_tolerances_to_measured_gaps`].
/// Span every `SURFACE_OF_LINEAR_EXTRUSION` over the faces that use it.
///
/// STEP's linear extrusion `C(u) + v V` is unbounded in `v`; the kernel
/// builds it as a `RuledSurface` between `C` and `C + V`, a strip one
/// extrusion vector long whose `v` is clamped to [0, 1]. OCCT writes `V` as
/// a unit vector, so every extruded face more than 1 mm deep lay mostly off
/// its own surface: CADGenBench part 211's edges measured 11 mm off a strip
/// they sit exactly on in the file (OCCT measures 0.03 mm), and the part
/// could not certify.
///
/// A point `C(u) + s d` of the extrusion (`d` the unit direction) has `|s|`
/// at most its distance from the profile, so projecting every boundary point
/// of the faces onto a strip that wide reads each one's offset `s`; the
/// surface is rebuilt between `C + s_min d` and `C + s_max d` (a hair wider),
/// keeping `v` along `+V` so the faces' sense is unchanged.
fn fit_linear_extrusions_to_faces(
    model: &mut geometry_engine::primitives::topology_builder::BRepModel,
    caches: &mut context::ResolutionCaches,
) {
    use geometry_engine::math::{Matrix4, Point3, Tolerance};
    use geometry_engine::primitives::surface::{RuledSurface, Surface};
    use std::collections::HashMap;
    const EDGE_SAMPLES: usize = 9;
    let extrusions = std::mem::take(&mut caches.linear_extrusions);
    if extrusions.is_empty() {
        return;
    }
    let mut faces_of: HashMap<u32, Vec<u32>> = HashMap::new();
    for (fid, face) in model.faces.iter() {
        if extrusions.contains_key(&face.surface_id) {
            faces_of.entry(face.surface_id).or_default().push(fid);
        }
    }
    let translated = |c: &dyn geometry_engine::primitives::curve::Curve,
                      by: geometry_engine::math::Vector3| {
        c.transform(&Matrix4::translation(by.x, by.y, by.z))
    };
    for (sid, (profile, vector)) in &extrusions {
        let Some(fids) = faces_of.get(sid) else {
            continue;
        };
        let Ok(dir) = vector.normalize() else {
            continue;
        };
        let mut points: Vec<Point3> = Vec::new();
        for &fid in fids {
            let Some(face) = model.faces.get(fid) else {
                continue;
            };
            for l in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
                let Some(lp) = model.loops.get(l) else {
                    continue;
                };
                for &e in &lp.edges {
                    let Some(edge) = model.edges.get(e) else {
                        continue;
                    };
                    for k in 0..EDGE_SAMPLES {
                        let t = k as f64 / (EDGE_SAMPLES - 1) as f64;
                        if let Ok(p) = edge.evaluate(t, &model.curves) {
                            points.push(p);
                        }
                    }
                }
            }
        }
        let profile_pts: Vec<Point3> = (0..=32)
            .filter_map(|k| profile.point_at(k as f64 / 32.0).ok())
            .collect();
        let (Some(&origin), false) = (profile_pts.first(), points.is_empty()) else {
            continue;
        };
        let farthest = |ps: &[Point3]| {
            ps.iter()
                .map(|p| (*p - origin).magnitude())
                .fold(0.0, f64::max)
        };
        let reach = farthest(&points) + farthest(&profile_pts) + 1.0;
        let wide = RuledSurface::new(
            translated(profile.as_ref(), dir * -reach),
            translated(profile.as_ref(), dir * reach),
        );
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for p in &points {
            if let Ok((_, v)) = wide.closest_point(p, Tolerance::default()) {
                let s = -reach + 2.0 * reach * v;
                lo = lo.min(s);
                hi = hi.max(s);
            }
        }
        if !(hi > lo) {
            continue;
        }
        let margin = 1e-3 * (hi - lo) + 1e-6;
        let fitted = RuledSurface::new(
            translated(profile.as_ref(), dir * (lo - margin)),
            translated(profile.as_ref(), dir * (hi + margin)),
        );
        model.surfaces.replace(*sid, Box::new(fitted));
    }
}

/// Whether an edge `gap` mm off its faces is beyond any rebuild. Every
/// rebuilt sample stays within [`EDGE_REBUILD_CAP`] of the file's curve and
/// the result is kept only within [`EDGE_GAP_REPAIR_CAP`] of both faces, so
/// a gap above their sum cannot be closed; the factor of ten leaves room for
/// the interpolated curve between samples. Attempting such edges anyway --
/// up to 1025 samples each, each walked and projected -- was most of the
/// import time of CADGenBench part 211, whose misplaced edges lie 11-124 mm
/// off their faces.
fn rebuild_cannot_reach_cap(gap: f64) -> bool {
    gap > 10.0 * (EDGE_GAP_REPAIR_CAP + EDGE_REBUILD_CAP)
}

fn rebuild_edges_onto_face_intersections(
    model: &mut geometry_engine::primitives::topology_builder::BRepModel,
    caches: &mut context::ResolutionCaches,
    report: &mut ImportReport,
) {
    use geometry_engine::math::{Point3, Tolerance};
    use std::collections::{BTreeMap, HashMap};
    let tol = Tolerance::default();
    // Edge -> the distinct faces using it. A seam (used twice by one face)
    // or an edge with any other count is not a two-face intersection.
    let mut users: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (fid, face) in model.faces.iter() {
        for l in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
            let Some(lp) = model.loops.get(l) else {
                continue;
            };
            for &e in &lp.edges {
                users.entry(e).or_default().push(fid);
            }
        }
    }
    let face_instance: HashMap<u32, u64> = caches.faces.iter().map(|(n, &f)| (f, *n)).collect();
    let edge_instance: HashMap<u32, u64> = caches.edges.iter().map(|(n, &e)| (e, *n)).collect();
    // Each edge's rebuild reads only its own curve, its two faces' surfaces
    // and its vertices -- none of which another edge's rebuild changes -- so
    // every candidate is computed in parallel and the results are applied in
    // edge-id order exactly as the serial walk applied them. (The rebuild
    // was 73 s of CADGenBench part 208's import, one edge after another.)
    let candidates: Vec<(u32, Vec<u32>)> = users.into_iter().collect();
    let rebuilt: Vec<_> = {
        use rayon::prelude::*;
        let model: &geometry_engine::primitives::topology_builder::BRepModel = model;
        candidates
            .par_iter()
            .map(|(eid, faces)| {
                let eid = *eid;
                let compute = || {
                    let [fa, fb] = faces[..] else {
                        return None;
                    };
                    if fa == fb {
                        return None;
                    }
                    let (Some(face_a), Some(face_b)) = (model.faces.get(fa), model.faces.get(fb)) else {
                        return None;
                    };
                    let (Some(sa), Some(sb)) = (
                        model.surfaces.get(face_a.surface_id),
                        model.surfaces.get(face_b.surface_id),
                    ) else {
                        return None;
                    };
                    let Some(edge) = model.edges.get(eid) else {
                        return None;
                    };
                    // A closed edge (a full circle: one vertex at both ends) is rebuilt
                    // like any other -- its samples run round and both ends are that
                    // vertex. Fillet rims on cylinders and tori are such circles, and
                    // skipping them left most of CADGenBench's off-face edges unrepaired.
                    let declared = edge.tolerance;
                    // The distance from `p` to a surface, and the surface point.
                    let foot = |s: &dyn geometry_engine::primitives::surface::Surface, p: Point3| {
                        s.closest_point(&p, tol)
                            .ok()
                            .and_then(|(u, v)| s.point_at(u, v).ok())
                            .map(|q| (q, (q - p).magnitude()))
                    };
                    let gap_of = |p: Point3| -> Option<f64> { Some(foot(sa, p)?.1.max(foot(sb, p)?.1)) };
                    let sample = |n: usize| -> Option<Vec<Point3>> {
                        (0..n)
                            .map(|k| edge.evaluate(k as f64 / (n - 1) as f64, &model.curves).ok())
                            .collect()
                    };
                    let Some(probe) = sample(EDGE_REBUILD_SAMPLES) else {
                        return None;
                    };
                    let Some(before) = probe
                        .iter()
                        .map(|&p| gap_of(p))
                        .try_fold(0.0_f64, |m, g| g.map(|g| m.max(g)))
                    else {
                        return None;
                    };
                    if !(before > declared.max(1e-7)) || rebuild_cannot_reach_cap(before) {
                        return None;
                    }
                    let (Some(vs), Some(ve)) = (
                        model.vertices.get(edge.start_vertex).map(|v| v.position),
                        model.vertices.get(edge.end_vertex).map(|v| v.position),
                    ) else {
                        return None;
                    };
                    // Carry `n` samples onto both surfaces and interpolate them; `None`
                    // when any sample does not converge or would move too far.
                    use geometry_engine::primitives::curve::Curve;
                    let attempt = |n: usize| -> Option<(
                        geometry_engine::primitives::curve::NurbsCurve,
                        f64,
                        f64,
                        f64,
                    )> {
                        let samples = sample(n)?;
                        let mut on_both = Vec::with_capacity(n);
                        for (i, &p) in samples.iter().enumerate() {
                            // The ends become the edge's vertices below; nothing to walk
                            // (and at a pole or apex there is no step across to take:
                            // CADGenBench part 205).
                            if i == 0 || i + 1 == samples.len() {
                                on_both.push(p);
                                continue;
                            }
                            // Walk ACROSS the edge on the surface the file's curve lies
                            // nearer (its "home" face) to the point that comes closest to
                            // the other face. Where the faces cross that point is on
                            // both; where they touch TANGENTIALLY -- a fillet running
                            // onto a flat face -- it is the contact line, which
                            // alternating projection never reaches (both steps are near
                            // the shared normal, the error across the edge never shrinks:
                            // CADGenBench part 205); where they truly do not meet it is
                            // their real separation, which becomes the edge's tolerance.
                            let (home, other) = if foot(sa, p)?.1 <= foot(sb, p)?.1 {
                                (sa, sb)
                            } else {
                                (sb, sa)
                            };
                            let (u0, v0) = home.closest_point(&p, tol).ok()?;
                            let e = home.evaluate_full(u0, v0).ok()?;
                            let prev = samples[i.saturating_sub(1)];
                            let next = samples[(i + 1).min(samples.len() - 1)];
                            let along = (next - prev).normalize().ok()?;
                            let across = e.normal.cross(&along).normalize().ok()?;
                            // The parameter step that moves along `across` on `home`.
                            let (g11, g12, g22) = (e.du.dot(&e.du), e.du.dot(&e.dv), e.dv.dot(&e.dv));
                            let (r1, r2) = (e.du.dot(&across), e.dv.dot(&across));
                            let det = g11 * g22 - g12 * g12;
                            if !(det.abs() > 1e-300) {
                                return None;
                            }
                            let (du, dv) = ((g22 * r1 - g12 * r2) / det, (g11 * r2 - g12 * r1) / det);
                            let on_home = |s: f64| home.point_at(u0 + s * du, v0 + s * dv).ok();
                            let gap = |s: f64| on_home(s).and_then(|q| foot(other, q)).map(|(_, d)| d);
                            // Golden-section search for the closest approach.
                            let phi = 0.5 * (5f64.sqrt() - 1.0);
                            let (mut lo, mut hi) = (-EDGE_REBUILD_CAP, EDGE_REBUILD_CAP);
                            let mut x1 = hi - phi * (hi - lo);
                            let mut x2 = lo + phi * (hi - lo);
                            let (mut f1, mut f2) = (gap(x1)?, gap(x2)?);
                            for _ in 0..EDGE_REBUILD_ITERATIONS {
                                if f1 <= f2 {
                                    hi = x2;
                                    x2 = x1;
                                    f2 = f1;
                                    x1 = hi - phi * (hi - lo);
                                    f1 = gap(x1)?;
                                } else {
                                    lo = x1;
                                    x1 = x2;
                                    f1 = f2;
                                    x2 = lo + phi * (hi - lo);
                                    f2 = gap(x2)?;
                                }
                                if hi - lo < 1e-13 * (1.0 + EDGE_REBUILD_CAP) {
                                    break;
                                }
                            }
                            // A minimum AT the search bound is not a closest approach: the
                            // faces run alongside each other without meeting within reach
                            // (CADGenBench part 229: two faces 3e-3 mm apart, the gap
                            // shrinking by a tenth over the whole 0.1 mm walk). The edge
                            // is then the foot on its home face, and their separation
                            // becomes its tolerance.
                            let mut s_best = 0.5 * (lo + hi);
                            if s_best.abs() >= EDGE_REBUILD_BOUND_FRACTION * EDGE_REBUILD_CAP {
                                s_best = 0.0;
                            }
                            let q = on_home(s_best)?;
                            if (q - p).magnitude() > EDGE_REBUILD_CAP {
                                return None;
                            }
                            on_both.push(q);
                        }
                        // The ends are the edge's vertices: the edges meeting there use them.
                        let last = on_both.len() - 1;
                        on_both[0] = Point3::new(vs[0], vs[1], vs[2]);
                        on_both[last] = Point3::new(ve[0], ve[1], ve[2]);
                        let fit = geometry_engine::math::nurbs::interpolate_nurbs_curve(
                            &on_both,
                            3,
                            geometry_engine::math::nurbs::ParameterizationType::ChordLength,
                        )
                        .ok()?;
                        let knots = fit.knots.values().to_vec();
                        let (k0, k1) = (*knots.first()?, *knots.last()?);
                        let curve = geometry_engine::primitives::curve::NurbsCurve::new(
                            fit.degree,
                            fit.control_points.clone(),
                            fit.weights.clone(),
                            knots,
                        )
                        .ok()?;
                        // Measured between the interpolated samples too.
                        let m = 4 * (n - 1);
                        let after = (0..=m)
                            .map(|k| {
                                curve
                                    .point_at(k0 + (k1 - k0) * k as f64 / m as f64)
                                    .ok()
                                    .and_then(gap_of)
                            })
                            .try_fold(0.0_f64, |acc, g| g.map(|g| acc.max(g)))?;
                        Some((curve, k0, k1, after))
                    };
                    // Double the samples until the rebuilt edge lies on both faces to
                    // the target, or the attempts run out; keep the best.
                    let target = EDGE_REBUILD_TARGET.max(declared);
                    let mut best: Option<(
                        geometry_engine::primitives::curve::NurbsCurve,
                        f64,
                        f64,
                        f64,
                    )> = None;
                    let mut n = EDGE_REBUILD_SAMPLES;
                    while n <= EDGE_REBUILD_MAX_SAMPLES {
                        let Some(got) = attempt(n) else { break };
                        let done = got.3 <= target;
                        if best.as_ref().is_none_or(|b| got.3 < b.3) {
                            best = Some(got);
                        }
                        if done {
                            break;
                        }
                        n = 2 * n - 1;
                    }
                    if std::env::var_os("ROSHERA_REBUILD_TRACE").is_some() {
                        eprintln!(
                            "[rebuild] edge {eid}: before {before:.3e} best after {:?} (n up to {n})",
                            best.as_ref().map(|b| b.3)
                        );
                    }
                    let Some((curve, k0, k1, after)) = best else {
                        return None;
                    };
                    // Kept only when it lies on both faces far more closely than the
                    // file's curve: on a tangent contact the rebuilt edge need not be
                    // exact, but it must be a real improvement, and within the repair
                    // cap (its tolerance is what it reached).
                    // An edge beyond the repair cap refuses the part, so any rebuild that
                    // brings it within the cap is kept.
                    let gain = after <= before * EDGE_REBUILD_MIN_GAIN || before > EDGE_GAP_REPAIR_CAP;
                    if !(gain && after <= EDGE_GAP_REPAIR_CAP) {
                        return None;
                    }
                    Some((fa, fb, curve, k0, k1, before, after))
                };
                (eid, compute())
            })
            .collect()
    };
    for (eid, rebuilt) in rebuilt {
        let Some((fa, fb, curve, k0, k1, before, after)) = rebuilt else {
            continue;
        };
        let curve_id = model.curves.add(Box::new(curve));
        let Some(edge) = model.edges.get_mut(eid) else {
            continue;
        };
        edge.curve_id = curve_id;
        edge.param_range = geometry_engine::primitives::curve::ParameterRange::new(k0, k1);
        edge.orientation = geometry_engine::primitives::edge::EdgeOrientation::Forward;
        edge.pcurves.clear();
        edge.invalidate_length_cache();
        if after > edge.tolerance {
            edge.set_tolerance(after * (1.0 + 1e-6));
        }
        let instance = edge_instance.get(&eid).copied().unwrap_or(0);
        report.healings.push(diagnostics::Healing {
            kind: diagnostics::HealingKind::EdgeRebuiltOnFaceIntersection,
            entity: "EDGE_CURVE".into(),
            instance,
            deviation: before,
            tolerance: after,
        });
        for f in [fa, fb] {
            if let Some(&n) = face_instance.get(&f) {
                caches.record_lens(
                    n,
                    geometry_engine::lens::Stage::Repair,
                    geometry_engine::lens::EventKind::EdgeRebuiltOnIntersection,
                    Some(instance),
                    "the file's edge curve lay off this face; the edge was rebuilt on the intersection of its two faces",
                    &[("gap_before_mm", before), ("gap_after_mm", after)],
                );
            }
        }
    }
}

/// Measure every edge's gap from the ANALYTIC surfaces of the faces using it
/// (plane, cylinder, cone, sphere, torus -- the surfaces the validator judges
/// exactly) and, where it exceeds the edge's declared tolerance but not
/// [`EDGE_GAP_REPAIR_CAP`], raise the tolerance to the gap and record a
/// [`diagnostics::HealingKind::EdgeToleranceRaised`]. A gap beyond the cap is
/// left alone and reported as a warning. Writers state one model-wide
/// accuracy (OCCT writes the AVERAGE tolerance), but individual edges --
/// intersection curves, approximated splines -- sit farther off; every
/// exchange reader records the real per-edge tolerance, and so does this,
/// openly.
fn raise_edge_tolerances_to_measured_gaps(
    model: &mut geometry_engine::primitives::topology_builder::BRepModel,
    caches: &mut context::ResolutionCaches,
    report: &mut ImportReport,
) {
    use std::collections::HashMap;
    let face_instance: HashMap<u32, u64> = caches.faces.iter().map(|(n, &fid)| (fid, *n)).collect();
    // Per edge: each face using it and the edge's gap from THAT face, so a
    // raise is recorded on the face it is off, not its neighbours.
    let mut edge_faces: HashMap<u32, Vec<(u32, f64)>> = HashMap::new();
    use geometry_engine::math::Tolerance;
    let instance_of: std::collections::HashMap<u32, u64> = caches
        .edges
        .iter()
        .map(|(inst, &eid)| (eid, *inst))
        .collect();
    let mut gap: std::collections::HashMap<u32, f64> = std::collections::HashMap::new();
    for (fid, face) in model.faces.iter() {
        let Some(surface) = model.surfaces.get(face.surface_id) else {
            continue;
        };
        if !matches!(
            surface.type_name(),
            "Plane" | "Cylinder" | "Cone" | "Sphere" | "Torus"
        ) {
            continue;
        }
        for l in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
            let Some(lp) = model.loops.get(l) else {
                continue;
            };
            for &eid in &lp.edges {
                let Some(edge) = model.edges.get(eid) else {
                    continue;
                };
                let Some(curve) = model.curves.get(edge.curve_id) else {
                    continue;
                };
                let r = edge.param_range;
                let mut worst = 0.0_f64;
                // The validator judges the edge's VERTICES as well as its
                // curve (validation.rs, the analytic edge-on-face check), so
                // the gap is measured on both: measured on the curve alone,
                // a vertex sitting farther off the face than the curve kept
                // the edge failing after its tolerance was "repaired"
                // (CADGenBench parts 125, 205).
                let vertex_points = [edge.start_vertex, edge.end_vertex]
                    .into_iter()
                    .filter_map(|v| model.vertices.get(v).map(|x| x.point()));
                let curve_points = (0..=8).filter_map(|k| {
                    let t = r.start + (r.end - r.start) * k as f64 / 8.0;
                    curve.evaluate(t).ok().map(|p| p.position)
                });
                for p in vertex_points.chain(curve_points) {
                    if let Ok((u, v)) = surface.closest_point(&p, Tolerance::default()) {
                        if let Ok(q) = surface.point_at(u, v) {
                            worst = worst.max(p.distance(&q));
                        }
                    }
                }
                let g = gap.entry(eid).or_insert(0.0);
                *g = g.max(worst);
                edge_faces.entry(eid).or_default().push((fid, worst));
            }
        }
    }
    let mut ids: Vec<u32> = gap.keys().copied().collect();
    ids.sort_unstable();
    for eid in ids {
        let g = gap[&eid];
        let Some(edge) = model.edges.get_mut(eid) else {
            continue;
        };
        let declared = edge.tolerance;
        if !(g > declared) {
            continue;
        }
        let instance = instance_of.get(&eid).copied().unwrap_or(0);
        if g <= EDGE_GAP_REPAIR_CAP {
            // A hair above the measured gap, as OCCT does, so the edge is
            // within its tolerance at every measured point.
            edge.set_tolerance(g * (1.0 + 1e-6));
            report.healings.push(diagnostics::Healing {
                kind: diagnostics::HealingKind::EdgeToleranceRaised,
                entity: "EDGE_CURVE".into(),
                instance,
                deviation: g,
                tolerance: declared,
            });
            for (f, off) in edge_faces.get(&eid).into_iter().flatten() {
                if *off <= declared {
                    continue;
                }
                if let Some(&n) = face_instance.get(f) {
                    caches.record_lens(
                        n,
                        geometry_engine::lens::Stage::Repair,
                        geometry_engine::lens::EventKind::EdgeToleranceRaised,
                        Some(instance),
                        "edge lay farther off this face than the file's declared accuracy; its tolerance was raised to the measured gap",
                        &[("gap_mm", *off), ("declared_mm", declared)],
                    );
                }
            }
        } else {
            report.push_warning(diagnostics::Warning {
                severity: diagnostics::Severity::Warn,
                entity: "EDGE_CURVE".into(),
                instance: Some(instance),
                message: format!(
                    "edge lies {g:.3e} mm off its face, beyond the {EDGE_GAP_REPAIR_CAP} mm repair                      cap; left at its declared tolerance {declared:.1e}"
                ),
            });
        }
    }
}

#[cfg(test)]
mod imp1_tests {
    //! IMP1 acceptance tests. These verify the architecture wires up
    //! end-to-end; per-entity coverage is tested in `handlers::tierN`.
    //!
    //! As tiers come online the "expected unsupported" set shrinks —
    //! after IMP2.5 every tier-1 entity is wired, so tests that exercise
    //! the unsupported path use tier-2 entities (`STYLED_ITEM`,
    //! `B_SPLINE_SURFACE_WITH_KNOTS`) that genuinely have no handler.

    use super::*;
    use std::io::Write;

    /// Canonical AP242 schema identifier (long-form MIM_LF). Matches the
    /// emitter default in [`writer::StepApplicationProtocol`].
    const AP242_SCHEMA: &str = "AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF";

    fn write_tmp(contents: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().expect("tmpfile");
        f.write_all(contents.as_bytes()).expect("write tmp");
        f
    }

    /// Wrap `body` in a syntactically-valid STEP envelope using the
    /// supplied schema identifier.
    fn step_envelope(schema: &str, body: &str) -> String {
        format!(
            "ISO-10303-21;\n\
             HEADER;\n\
             FILE_DESCRIPTION(('test'),'2;1');\n\
             FILE_NAME('t.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
             FILE_SCHEMA(('{schema}'));\n\
             ENDSEC;\n\
             DATA;\n\
             {body}\n\
             ENDSEC;\n\
             END-ISO-10303-21;\n"
        )
    }

    fn ap242_step(body: &str) -> String {
        step_envelope(AP242_SCHEMA, body)
    }

    /// Build a self-contained AP242 unit-cube DATA section that
    /// exercises every tier-1 handler from unit through root.
    ///
    /// Duplicated from `handlers::tier1::root::tests::unit_cube_body`
    /// + `root_scaffolding` — the alternative would be making those
    /// helpers `pub(crate)`, which we don't want as a load-bearing
    /// promise of the handler module's surface.
    fn unit_cube_with_root(root_entity: &str) -> String {
        let mut s = String::new();
        // ----- units (mm) -----------------------------------------
        s += "#900=(NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.)LENGTH_UNIT());";
        s += "#901=(NAMED_UNIT(*)PLANE_ANGLE_UNIT()SI_UNIT($,.RADIAN.));";
        s += "#902=(NAMED_UNIT(*)SOLID_ANGLE_UNIT()SI_UNIT($,.STERADIAN.));";
        s += "#903=UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-6),#900,'distance_accuracy_value','closure');";
        s += "#904=(GEOMETRIC_REPRESENTATION_CONTEXT(3)\
                    GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#903))\
                    GLOBAL_UNIT_ASSIGNED_CONTEXT((#900,#901,#902))\
                    REPRESENTATION_CONTEXT('cube','3D'));";
        // ----- cube geometry --------------------------------------
        s += "#1=CARTESIAN_POINT('',(0.,0.,0.));";
        s += "#2=CARTESIAN_POINT('',(1.,0.,0.));";
        s += "#3=CARTESIAN_POINT('',(1.,1.,0.));";
        s += "#4=CARTESIAN_POINT('',(0.,1.,0.));";
        s += "#5=CARTESIAN_POINT('',(0.,0.,1.));";
        s += "#6=CARTESIAN_POINT('',(1.,0.,1.));";
        s += "#7=CARTESIAN_POINT('',(1.,1.,1.));";
        s += "#8=CARTESIAN_POINT('',(0.,1.,1.));";
        s += "#11=VERTEX_POINT('',#1);";
        s += "#12=VERTEX_POINT('',#2);";
        s += "#13=VERTEX_POINT('',#3);";
        s += "#14=VERTEX_POINT('',#4);";
        s += "#15=VERTEX_POINT('',#5);";
        s += "#16=VERTEX_POINT('',#6);";
        s += "#17=VERTEX_POINT('',#7);";
        s += "#18=VERTEX_POINT('',#8);";
        s += "#21=DIRECTION('',(1.,0.,0.));";
        s += "#22=DIRECTION('',(0.,1.,0.));";
        s += "#23=DIRECTION('',(0.,0.,1.));";
        s += "#24=DIRECTION('',(-1.,0.,0.));";
        s += "#25=DIRECTION('',(0.,-1.,0.));";
        s += "#26=DIRECTION('',(0.,0.,-1.));";
        s += "#31=VECTOR('',#21,1.);";
        s += "#32=VECTOR('',#22,1.);";
        s += "#33=VECTOR('',#23,1.);";
        s += "#41=LINE('',#1,#31);";
        s += "#42=LINE('',#1,#32);";
        s += "#43=LINE('',#1,#33);";
        s += "#51=EDGE_CURVE('',#11,#12,#41,.T.);";
        s += "#52=EDGE_CURVE('',#12,#13,#42,.T.);";
        s += "#53=EDGE_CURVE('',#14,#13,#41,.T.);";
        s += "#54=EDGE_CURVE('',#11,#14,#42,.T.);";
        s += "#55=EDGE_CURVE('',#15,#16,#41,.T.);";
        s += "#56=EDGE_CURVE('',#16,#17,#42,.T.);";
        s += "#57=EDGE_CURVE('',#18,#17,#41,.T.);";
        s += "#58=EDGE_CURVE('',#15,#18,#42,.T.);";
        s += "#59=EDGE_CURVE('',#11,#15,#43,.T.);";
        s += "#60=EDGE_CURVE('',#12,#16,#43,.T.);";
        s += "#61=EDGE_CURVE('',#13,#17,#43,.T.);";
        s += "#62=EDGE_CURVE('',#14,#18,#43,.T.);";
        s += "#71=ORIENTED_EDGE('',*,*,#51,.T.);";
        s += "#72=ORIENTED_EDGE('',*,*,#52,.T.);";
        s += "#73=ORIENTED_EDGE('',*,*,#53,.F.);";
        s += "#74=ORIENTED_EDGE('',*,*,#54,.F.);";
        s += "#75=ORIENTED_EDGE('',*,*,#55,.T.);";
        s += "#76=ORIENTED_EDGE('',*,*,#56,.T.);";
        s += "#77=ORIENTED_EDGE('',*,*,#57,.F.);";
        s += "#78=ORIENTED_EDGE('',*,*,#58,.F.);";
        s += "#79=ORIENTED_EDGE('',*,*,#51,.T.);";
        s += "#80=ORIENTED_EDGE('',*,*,#60,.T.);";
        s += "#81=ORIENTED_EDGE('',*,*,#55,.F.);";
        s += "#82=ORIENTED_EDGE('',*,*,#59,.F.);";
        s += "#83=ORIENTED_EDGE('',*,*,#52,.T.);";
        s += "#84=ORIENTED_EDGE('',*,*,#61,.T.);";
        s += "#85=ORIENTED_EDGE('',*,*,#56,.F.);";
        s += "#86=ORIENTED_EDGE('',*,*,#60,.F.);";
        s += "#87=ORIENTED_EDGE('',*,*,#53,.T.);";
        s += "#88=ORIENTED_EDGE('',*,*,#61,.T.);";
        s += "#89=ORIENTED_EDGE('',*,*,#57,.F.);";
        s += "#90=ORIENTED_EDGE('',*,*,#62,.F.);";
        s += "#91=ORIENTED_EDGE('',*,*,#54,.T.);";
        s += "#92=ORIENTED_EDGE('',*,*,#62,.T.);";
        s += "#93=ORIENTED_EDGE('',*,*,#58,.F.);";
        s += "#94=ORIENTED_EDGE('',*,*,#59,.F.);";
        s += "#101=EDGE_LOOP('',(#71,#72,#73,#74));";
        s += "#102=EDGE_LOOP('',(#75,#76,#77,#78));";
        s += "#103=EDGE_LOOP('',(#79,#80,#81,#82));";
        s += "#104=EDGE_LOOP('',(#83,#84,#85,#86));";
        s += "#105=EDGE_LOOP('',(#87,#88,#89,#90));";
        s += "#106=EDGE_LOOP('',(#91,#92,#93,#94));";
        s += "#111=FACE_OUTER_BOUND('',#101,.T.);";
        s += "#112=FACE_OUTER_BOUND('',#102,.T.);";
        s += "#113=FACE_OUTER_BOUND('',#103,.T.);";
        s += "#114=FACE_OUTER_BOUND('',#104,.T.);";
        s += "#115=FACE_OUTER_BOUND('',#105,.T.);";
        s += "#116=FACE_OUTER_BOUND('',#106,.T.);";
        s += "#121=AXIS2_PLACEMENT_3D('',#1,#26,#21);";
        s += "#122=AXIS2_PLACEMENT_3D('',#5,#23,#21);";
        s += "#123=AXIS2_PLACEMENT_3D('',#1,#25,#21);";
        s += "#124=AXIS2_PLACEMENT_3D('',#2,#21,#22);";
        s += "#125=AXIS2_PLACEMENT_3D('',#4,#22,#21);";
        s += "#126=AXIS2_PLACEMENT_3D('',#1,#24,#22);";
        s += "#131=PLANE('',#121);";
        s += "#132=PLANE('',#122);";
        s += "#133=PLANE('',#123);";
        s += "#134=PLANE('',#124);";
        s += "#135=PLANE('',#125);";
        s += "#136=PLANE('',#126);";
        s += "#141=ADVANCED_FACE('',(#111),#131,.T.);";
        s += "#142=ADVANCED_FACE('',(#112),#132,.T.);";
        s += "#143=ADVANCED_FACE('',(#113),#133,.T.);";
        s += "#144=ADVANCED_FACE('',(#114),#134,.T.);";
        s += "#145=ADVANCED_FACE('',(#115),#135,.T.);";
        s += "#146=ADVANCED_FACE('',(#116),#136,.T.);";
        s += "#151=CLOSED_SHELL('',(#141,#142,#143,#144,#145,#146));";
        s += "#161=MANIFOLD_SOLID_BREP('cube',#151);";
        // World origin placement referenced by the root.
        s += "#204=AXIS2_PLACEMENT_3D('world',#1,#23,#21);";
        // Root container.
        s += &format!("#301={root_entity}('cube',(#161,#204),#904);");
        s
    }

    // ---------- IMP1 architecture sanity ----------

    #[tokio::test]
    async fn imports_empty_with_unsupported_for_unknown_entities() {
        // `STYLED_ITEM` is presentation-tier and has no kernel handler;
        // it exercises the Unsupported logging path. Pair it with
        // `CARTESIAN_POINT` (tier-1, supported) so the fixture is
        // realistic — a STYLED_ITEM referencing a representation item.
        // Inputs without a `MANIFOLD_SOLID_BREP` produce no kernel
        // solid by construction. (B_SPLINE_SURFACE_WITH_KNOTS was used
        // here historically but is now handled by tier-2; the
        // assertion against it was removed when tier-2 coverage
        // landed.)
        let src = ap242_step(
            "#1=STYLED_ITEM('label',(),#2);\n\
             #2=CARTESIAN_POINT('',(0.,0.,0.));",
        );
        let f = write_tmp(&src);
        let (model, report) = import_step_to_brep_with_report(f.path()).await.unwrap();
        assert!(
            model.solids.is_empty(),
            "fixture lacks MANIFOLD_SOLID_BREP — no kernel solid"
        );
        assert!(!report.ok, "ok must be false when no solid resolves");
        assert!(
            report.counts.unsupported.contains_key("STYLED_ITEM"),
            "STYLED_ITEM has no tier-1/2 handler"
        );
        assert_eq!(report.schema.as_deref(), Some(AP242_SCHEMA));
        assert_eq!(report.roots_resolved, 0);
        assert_eq!(report.solids_in_roots, 0);
    }

    #[tokio::test]
    async fn rejects_unparseable_file() {
        let f = write_tmp("this is not a STEP file");
        let result = import_step_to_brep_with_report(f.path()).await;
        assert!(result.is_err(), "garbage input must hard-fail");
    }

    #[tokio::test]
    async fn rejects_missing_file() {
        let result =
            import_step_to_brep_with_report(Path::new("/definitely/not/a/real/file/abc.step"))
                .await;
        assert!(result.is_err(), "missing file must hard-fail");
    }

    #[tokio::test]
    async fn imports_file_with_occt_block_comment() {
        // Regression for the alum_extrusion failure mode: a block
        // comment between the header and DATA sections. Uses a
        // genuinely-unsupported entity (`STYLED_ITEM`) so the
        // assertion stays stable as tier coverage grows.
        let src = format!(
            "ISO-10303-21;\n\
             /* META-MATIC-ID: abc123 */\n\
             HEADER;\n\
             FILE_DESCRIPTION(('t'),'2;1');\n\
             FILE_NAME('t.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
             FILE_SCHEMA(('{AP242_SCHEMA}'));\n\
             ENDSEC;\n\
             DATA;\n\
             #1=CARTESIAN_POINT('',(0.,0.,0.));\n\
             #2=STYLED_ITEM('label',(),#1);\n\
             ENDSEC;\n\
             END-ISO-10303-21;\n"
        );
        let f = write_tmp(&src);
        let (_, report) = import_step_to_brep_with_report(f.path()).await.unwrap();
        assert_eq!(report.counts.unsupported.get("STYLED_ITEM"), Some(&1));
        assert_eq!(report.schema.as_deref(), Some(AP242_SCHEMA));
    }

    // ---------- IMP2.5 end-to-end test corpus ----------

    #[tokio::test]
    async fn e2e_unit_cube_via_shape_representation_resolves_root() {
        let src = ap242_step(&unit_cube_with_root("SHAPE_REPRESENTATION"));
        let f = write_tmp(&src);
        let (model, report) = import_step_to_brep_with_report(f.path()).await.unwrap();
        assert_eq!(
            model.solids.len(),
            1,
            "tier-1 SHAPE_REPRESENTATION must yield one solid: {:?}",
            report.warnings
        );
        assert!(report.ok, "ok flag must reflect resolved root");
        assert_eq!(report.roots_resolved, 1);
        assert_eq!(report.solids_in_roots, 1);
        assert_eq!(report.schema.as_deref(), Some(AP242_SCHEMA));
    }

    #[tokio::test]
    async fn e2e_unit_cube_via_advanced_brep_shape_representation_resolves_root() {
        // AP242's canonical B-Rep root container shape.
        let src = ap242_step(&unit_cube_with_root("ADVANCED_BREP_SHAPE_REPRESENTATION"));
        let f = write_tmp(&src);
        let (model, report) = import_step_to_brep_with_report(f.path()).await.unwrap();
        assert_eq!(model.solids.len(), 1);
        assert!(report.ok);
        assert_eq!(report.roots_resolved, 1);
        assert_eq!(report.solids_in_roots, 1);
    }

    #[tokio::test]
    async fn e2e_ap242_schema_detected_from_header() {
        let src = ap242_step("#1=CARTESIAN_POINT('p',(0.,0.,0.));");
        let f = write_tmp(&src);
        let (_, report) = import_step_to_brep_with_report(f.path()).await.unwrap();
        assert_eq!(
            report.schema.as_deref(),
            Some(AP242_SCHEMA),
            "extract_file_schema must surface the AP242 identifier"
        );
    }

    #[tokio::test]
    async fn e2e_ap214_schema_still_imports() {
        // Legacy AP214 files must still import — the importer is
        // schema-agnostic; only the writer pins AP242 by default.
        let src = step_envelope(
            "AUTOMOTIVE_DESIGN",
            &unit_cube_with_root("SHAPE_REPRESENTATION"),
        );
        let f = write_tmp(&src);
        let (model, report) = import_step_to_brep_with_report(f.path()).await.unwrap();
        assert_eq!(model.solids.len(), 1);
        assert_eq!(report.roots_resolved, 1);
        assert_eq!(report.schema.as_deref(), Some("AUTOMOTIVE_DESIGN"));
    }

    #[tokio::test]
    async fn e2e_orphan_solid_without_root_reports_zero_roots() {
        // A MANIFOLD_SOLID_BREP that exists in the DATA section but
        // is not referenced by any SHAPE_REPRESENTATION still
        // produces a kernel solid (topology handlers process it
        // unconditionally), but `roots_resolved` stays 0.
        let mut body = unit_cube_with_root("SHAPE_REPRESENTATION");
        // Strip the root entity — keep only the geometry/topology.
        body = body.replace("#301=SHAPE_REPRESENTATION('cube',(#161,#204),#904);", "");
        let src = ap242_step(&body);
        let f = write_tmp(&src);
        let (model, report) = import_step_to_brep_with_report(f.path()).await.unwrap();
        assert_eq!(model.solids.len(), 1, "orphan solid still materialises");
        assert_eq!(
            report.roots_resolved, 0,
            "no SHAPE_REPRESENTATION ⇒ no roots resolved"
        );
        assert_eq!(report.solids_in_roots, 0);
        // ok stays true because at least one solid is in the model.
        assert!(report.ok);
    }

    #[tokio::test]
    async fn e2e_solid_alongside_unsupported_entities_still_resolves() {
        // Real-world AP242 files routinely carry STYLED_ITEM /
        // PRESENTATION_LAYER_ASSIGNMENT records that tier-1 cannot
        // model. Their presence must not block solid resolution.
        let mut body = unit_cube_with_root("ADVANCED_BREP_SHAPE_REPRESENTATION");
        body += "#401=STYLED_ITEM('shading',(),#161);";
        body += "#402=PRESENTATION_LAYER_ASSIGNMENT('layer','',(#161));";
        let src = ap242_step(&body);
        let f = write_tmp(&src);
        let (model, report) = import_step_to_brep_with_report(f.path()).await.unwrap();
        assert_eq!(model.solids.len(), 1);
        assert!(report.ok);
        assert_eq!(report.roots_resolved, 1);
        assert!(report.counts.unsupported.contains_key("STYLED_ITEM"));
        assert!(report
            .counts
            .unsupported
            .contains_key("PRESENTATION_LAYER_ASSIGNMENT"));
    }

    #[tokio::test]
    async fn e2e_two_roots_both_count_in_roots_resolved() {
        // Two independent SHAPE_REPRESENTATION roots, each pointing
        // at the same MANIFOLD_SOLID_BREP. roots_resolved must
        // count both; solids_in_roots reflects total item slots,
        // not distinct solid ids.
        let mut body = unit_cube_with_root("SHAPE_REPRESENTATION");
        body += "#302=SHAPE_REPRESENTATION('alt',(#161,#204),#904);";
        let src = ap242_step(&body);
        let f = write_tmp(&src);
        let (model, report) = import_step_to_brep_with_report(f.path()).await.unwrap();
        assert_eq!(model.solids.len(), 1);
        assert!(report.ok);
        assert_eq!(report.roots_resolved, 2);
        assert_eq!(report.solids_in_roots, 2);
    }

    /// The route path imports WITHOUT certifying (it certifies once, in the
    /// document, after the merge -- certifying here too ran every check
    /// twice and produced two verdicts). An uncertified report must say so
    /// and must not claim `ok`: an unverified import is not a verified one.
    #[test]
    fn an_uncertified_import_says_so_and_is_not_ok_yet() {
        let src = ap242_step(&unit_cube_with_root("SHAPE_REPRESENTATION"));
        let (model, report) = import_step_text_uncertified(&src, "t.step").expect("parse");
        assert_eq!(model.solids.len(), 1);
        assert!(
            !report.certified,
            "the report must say it is not certified yet"
        );
        assert!(
            report.validation.is_empty(),
            "no verdict before certification"
        );
        assert!(report.structurally_complete, "the cube built whole");
        assert!(!report.ok, "an unverified import must not claim ok");
    }

    /// Certifying afterwards -- the same helper the certifying import uses --
    /// fills one verdict per solid and folds soundness into `ok`.
    #[test]
    fn certifying_later_gives_the_same_report_as_certifying_at_import() {
        let src = ap242_step(&unit_cube_with_root("SHAPE_REPRESENTATION"));
        let (_, eager) = import_step_text_with_report(&src, "t.step").expect("parse");
        let (mut model, mut deferred) =
            import_step_text_uncertified(&src, "t.step").expect("parse");
        let ids: Vec<_> = model.solids.iter().map(|(sid, _)| sid).collect();
        let certs = model.certify_solids(&ids);
        apply_certificates(&mut deferred, &certs);
        assert!(deferred.certified && eager.certified);
        assert_eq!(deferred.ok, eager.ok);
        assert_eq!(deferred.validation.len(), eager.validation.len());
        for (d, e) in deferred.validation.iter().zip(&eager.validation) {
            assert_eq!(
                (d.sound, d.brep_valid, d.watertight),
                (e.sound, e.brep_valid, e.watertight)
            );
        }
    }

    /// One unsound part makes the whole import not ok.
    #[test]
    fn an_unsound_certificate_makes_the_import_not_ok() {
        let src = ap242_step(&unit_cube_with_root("SHAPE_REPRESENTATION"));
        let (mut model, mut report) = import_step_text_uncertified(&src, "t.step").expect("parse");
        let ids: Vec<_> = model.solids.iter().map(|(sid, _)| sid).collect();
        let mut certs = model.certify_solids(&ids);
        for (_, cert) in &mut certs {
            cert.watertight = false;
        }
        apply_certificates(&mut report, &certs);
        assert!(report.certified);
        assert!(!report.ok, "an unsound part must make the import not ok");
        assert!(!report.validation[0].sound);
    }

    /// The rebuild is skipped only for a gap it provably cannot close: an
    /// edge within the rebuild's reach (every sample may move up to
    /// EDGE_REBUILD_CAP, and the result must land within
    /// EDGE_GAP_REPAIR_CAP) is still attempted, and part 211's misplaced
    /// edges (11 mm and more off their faces) are not.
    #[test]
    fn only_a_gap_beyond_the_rebuilds_reach_skips_the_rebuild() {
        for reachable in [
            1e-6,
            EDGE_GAP_REPAIR_CAP,
            EDGE_REBUILD_CAP,
            EDGE_GAP_REPAIR_CAP + EDGE_REBUILD_CAP,
        ] {
            assert!(
                !rebuild_cannot_reach_cap(reachable),
                "a {reachable} mm gap is within the rebuild's reach and must be attempted"
            );
        }
        for hopeless in [11.0, 123.86] {
            assert!(
                rebuild_cannot_reach_cap(hopeless),
                "a {hopeless} mm gap cannot be closed by a {EDGE_REBUILD_CAP} mm move"
            );
        }
    }

    /// A face on a SURFACE_OF_LINEAR_EXTRUSION that reaches 12 mm along a
    /// unit extrusion vector lies on its surface: the strip the kernel
    /// builds is one vector long, and before the fit the far edge measured
    /// 11 mm off it (CADGenBench part 211).
    #[test]
    fn a_face_deeper_than_its_extrusion_vector_lies_on_its_surface() {
        use geometry_engine::math::{Matrix4, Point3, Tolerance, Vector3};
        use geometry_engine::primitives::curve::{Curve, Line};
        use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
        use geometry_engine::primitives::face::{Face, FaceOrientation};
        use geometry_engine::primitives::r#loop::{Loop, LoopType};
        use geometry_engine::primitives::surface::RuledSurface;
        use geometry_engine::primitives::topology_builder::BRepModel;

        let mut model = BRepModel::new();
        // Profile: a 10 mm line along x; extruded along +z by a UNIT vector.
        let profile: Box<dyn Curve> = Box::new(Line::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(10.0, 0.0, 0.0),
        ));
        let unit = Vector3::new(0.0, 0.0, 1.0);
        let strip = RuledSurface::new(
            profile.clone_box(),
            profile.transform(&Matrix4::translation(0.0, 0.0, 1.0)),
        );
        let sid = model.surfaces.add(Box::new(strip));
        let mut caches = context::ResolutionCaches::default();
        caches.linear_extrusions.insert(sid, (profile, unit));

        // The face: the rectangle x 0..10, z 0..12 on that extrusion.
        let corners = [
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(10.0, 0.0, 0.0),
            Point3::new(10.0, 0.0, 12.0),
            Point3::new(0.0, 0.0, 12.0),
        ];
        let vs: Vec<_> = corners
            .iter()
            .map(|p| model.vertices.add(p.x, p.y, p.z))
            .collect();
        let mut outer = Loop::new(0, LoopType::Outer);
        for k in 0..4 {
            let c = model
                .curves
                .add(Box::new(Line::new(corners[k], corners[(k + 1) % 4])));
            let e = model.edges.add(Edge::new_auto_range(
                0,
                vs[k],
                vs[(k + 1) % 4],
                c,
                EdgeOrientation::Forward,
            ));
            outer.add_edge(e, true);
        }
        let lid = model.loops.add(outer);
        model
            .faces
            .add(Face::new(0, sid, lid, FaceOrientation::Forward));

        fit_linear_extrusions_to_faces(&mut model, &mut caches);

        let surface = model.surfaces.get(sid).expect("surface");
        let worst = (0..=12)
            .map(|k| {
                let p = Point3::new(5.0, 0.0, f64::from(k));
                let (u, v) = surface
                    .closest_point(&p, Tolerance::default())
                    .expect("projects");
                surface.point_at(u, v).expect("evaluates").distance(&p)
            })
            .fold(0.0, f64::max);
        assert!(
            worst < 1e-9,
            "points of the face up to 12 mm along the extrusion lie {worst:.3e} mm off its surface"
        );
    }
}
