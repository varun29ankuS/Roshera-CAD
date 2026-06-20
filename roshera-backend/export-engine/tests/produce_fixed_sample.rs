//! One-off: produce a FIXED sample STEP for FreeCAD/Parasolid/ACIS
//! confirmation. Run explicitly:
//!   cargo test -p export-engine --test produce_fixed_sample -- --ignored --nocapture
use geometry_engine::math::{Tolerance, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::primitives::box_primitive::{BoxParameters, BoxPrimitive};
use geometry_engine::primitives::cylinder_primitive::{CylinderParameters, CylinderPrimitive};
use geometry_engine::primitives::primitive_traits::Primitive;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};
use std::path::Path;

#[tokio::test]
#[ignore = "manual: writes to the user's Downloads folder"]
async fn produce_fixed_sample() {
    let mut model = BRepModel::new();
    let box_id =
        BoxPrimitive::create(BoxParameters::new(40.0, 40.0, 20.0).unwrap(), &mut model).unwrap();
    let cyl_id =
        CylinderPrimitive::create(CylinderParameters::new(6.0, 60.0).unwrap(), &mut model).unwrap();
    translate(
        &mut model,
        vec![cyl_id],
        Vector3::new(0.0, 0.0, 1.0),
        -30.0,
        TransformOptions::default(),
    )
    .unwrap();
    let result = boolean_operation(
        &mut model,
        box_id,
        cyl_id,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .unwrap();
    let v =
        validate_solid_scoped(&model, result, Tolerance::default(), ValidationLevel::Standard);
    assert!(v.is_valid, "source must be valid: {:?}", v.errors);

    let out_dir = Path::new("C:/Users/Varun Sharma/Downloads");
    let engine = export_engine::ExportEngine::with_output_directory(
        out_dir.to_string_lossy().to_string(),
    );
    let filename = engine
        .export_step(&model, "ROSHERA_FIXED_export")
        .await
        .expect("export");
    let path = out_dir.join(&filename);
    eprintln!("WROTE {}", path.display());

    // Prove the fixed file re-imports VALID (the oracle, no FreeCAD needed).
    let (imported, report) =
        export_engine::formats::step::import_step_to_brep_with_report(&path)
            .await
            .expect("import");
    let sid = imported.solids.iter().next().map(|(id, _)| id).unwrap();
    let rv =
        validate_solid_scoped(&imported, sid, Tolerance::default(), ValidationLevel::Standard);
    eprintln!(
        "RE-IMPORT ok={} valid={} errors={}",
        report.ok,
        rv.is_valid,
        rv.errors.len()
    );
    assert!(rv.is_valid, "fixed sample must re-import valid");
}
