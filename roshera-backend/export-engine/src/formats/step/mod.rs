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

pub mod context;
pub mod diagnostics;
pub mod dispatch;
pub mod handlers;
pub mod parser;
pub mod registry;
pub mod writer;

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

    // Parse → index → dispatch.
    let exchange = parser::parse_step(&text, &path_str)?;
    let registry = EntityRegistry::build(&exchange);
    let mut model = BRepModel::new();
    let mut report = ImportReport::new();

    // Extract FILE_SCHEMA from the header, if present, for the report.
    report.schema = extract_file_schema(&exchange);

    let mut dispatch = EntityDispatch::new();
    handlers::register_all(&mut dispatch);

    let (resolved, roots_with_solids, total_root_solids) = {
        let mut ctx = ImportContext::new(&mut model, &mut report);
        let resolved = dispatch.run_all(&registry, &mut ctx);
        let roots_with_solids = ctx.caches.roots.values().filter(|v| !v.is_empty()).count();
        let total_root_solids: usize = ctx.caches.roots.values().map(|v| v.len()).sum();
        (resolved, roots_with_solids, total_root_solids)
    };
    report.roots_resolved = roots_with_solids;
    report.solids_in_roots = total_root_solids;
    report.ok = resolved > 0 && !model.solids.is_empty();
    Ok((model, report))
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
        // Use entities that genuinely have no handler in tier-1.
        // `STYLED_ITEM` and `B_SPLINE_SURFACE_WITH_KNOTS` are both
        // tier-2+ and exercise the Unsupported logging path without
        // depending on a phase that may grow coverage later.
        let src = ap242_step(
            "#1=STYLED_ITEM('label',(),#2);\n\
             #2=B_SPLINE_SURFACE_WITH_KNOTS('s',1,1,((#3,#3),(#3,#3)),.UNSPECIFIED.,.F.,.F.,.F.,(2,2),(2,2),(0.,1.),(0.,1.),.UNSPECIFIED.);\n\
             #3=CARTESIAN_POINT('',(0.,0.,0.));",
        );
        let f = write_tmp(&src);
        let (model, report) = import_step_to_brep_with_report(f.path()).await.unwrap();
        assert!(
            model.solids.is_empty(),
            "STYLED_ITEM/B_SPLINE inputs yield no kernel solid"
        );
        assert!(!report.ok, "ok must be false when no solid resolves");
        assert!(
            report.counts.unsupported.contains_key("STYLED_ITEM"),
            "STYLED_ITEM has no tier-1 handler"
        );
        assert!(
            report
                .counts
                .unsupported
                .contains_key("B_SPLINE_SURFACE_WITH_KNOTS"),
            "B_SPLINE_SURFACE_WITH_KNOTS has no tier-1 handler"
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
        let result = import_step_to_brep_with_report(Path::new(
            "/definitely/not/a/real/file/abc.step",
        ))
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
        body = body
            .replace(
                "#301=SHAPE_REPRESENTATION('cube',(#161,#204),#904);",
                "",
            );
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
        assert!(
            report
                .counts
                .unsupported
                .contains_key("PRESENTATION_LAYER_ASSIGNMENT")
        );
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
}
