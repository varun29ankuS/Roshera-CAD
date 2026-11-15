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
    let (resolved, roots_with_solids, total_root_solids, caches) = {
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
    raise_edge_tolerances_to_measured_gaps(&mut model, &caches, &mut report);
    reverse_loops_walking_against_neighbours(&mut model, &caches, &mut report);

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
    caches: &context::ResolutionCaches,
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
    }
}

/// The largest gap between an edge and a face's surface that the importer
/// repairs by raising the edge's tolerance (mm). Beyond it an edge is not on
/// its face and the part is refused, with the gap in the report.
pub const EDGE_GAP_REPAIR_CAP: f64 = 0.01;

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
    caches: &context::ResolutionCaches,
    report: &mut ImportReport,
) {
    use geometry_engine::math::Tolerance;
    let instance_of: std::collections::HashMap<u32, u64> = caches
        .edges
        .iter()
        .map(|(inst, &eid)| (eid, *inst))
        .collect();
    let mut gap: std::collections::HashMap<u32, f64> = std::collections::HashMap::new();
    for (_, face) in model.faces.iter() {
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
                for k in 0..=8 {
                    let t = r.start + (r.end - r.start) * k as f64 / 8.0;
                    let Ok(p) = curve.evaluate(t) else { continue };
                    let p = p.position;
                    if let Ok((u, v)) = surface.closest_point(&p, Tolerance::default()) {
                        if let Ok(q) = surface.point_at(u, v) {
                            worst = worst.max(p.distance(&q));
                        }
                    }
                }
                let g = gap.entry(eid).or_insert(0.0);
                *g = g.max(worst);
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
}
