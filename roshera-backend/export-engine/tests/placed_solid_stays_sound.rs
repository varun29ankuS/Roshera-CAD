// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A rigid placement must not change whether a solid is sound.
//!
//! Found 2026-11-05 importing the robot-arm assembly: once its 634
//! occurrences were placed, parts that imported SOUND at their own origin
//! (e.g. 17 of 18 four-face pins) came out B-Rep-valid but NOT watertight
//! after `transform_solid` moved them (29 of 199). A move is an isometry; it
//! cannot honestly open a mesh.

use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::transform::{transform_solid, TransformOptions};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn solid_of(id: GeometryId) -> u32 {
    match id {
        GeometryId::Solid(s) => s,
        other => panic!("expected a solid, got {other:?}"),
    }
}

/// Rotation by `angle` about the unit axis `(x, y, z)`, then translation.
fn placement(axis: [f64; 3], angle: f64, t: [f64; 3]) -> Matrix4 {
    let [x, y, z] = axis;
    let (s, c) = angle.sin_cos();
    let k = 1.0 - c;
    Matrix4::from_cols(
        Vector3::new(c + x * x * k, y * x * k + z * s, z * x * k - y * s),
        Vector3::new(x * y * k - z * s, c + y * y * k, z * y * k + x * s),
        Vector3::new(x * z * k + y * s, y * z * k - x * s, c + z * z * k),
        Vector3::new(t[0], t[1], t[2]),
    )
}

fn assert_placement_keeps_soundness(
    build: impl Fn(&mut TopologyBuilder) -> GeometryId,
    what: &str,
) {
    let cases = [
        (
            "translate",
            placement([0.0, 0.0, 1.0], 0.0, [250.0, -40.0, 900.0]),
        ),
        (
            "rotate-z 90",
            placement(
                [0.0, 0.0, 1.0],
                std::f64::consts::FRAC_PI_2,
                [0.0, 0.0, 0.0],
            ),
        ),
        (
            "rotate-x 90 + move",
            placement(
                [1.0, 0.0, 0.0],
                std::f64::consts::FRAC_PI_2,
                [120.0, 35.0, -60.0],
            ),
        ),
        (
            "oblique",
            placement(
                [1.0 / 3f64.sqrt(), 1.0 / 3f64.sqrt(), 1.0 / 3f64.sqrt()],
                0.7,
                [512.5, 77.25, 300.0],
            ),
        ),
    ];
    for (label, m) in cases {
        let mut model = BRepModel::new();
        let sid = {
            let mut b = TopologyBuilder::new(&mut model);
            solid_of(build(&mut b))
        };
        assert!(
            model.certify_solid(sid).is_sound(),
            "{what}: precondition — sound before the move"
        );
        transform_solid(&mut model, sid, m, TransformOptions::default()).expect("transform");
        let cert = model.certify_solid(sid);
        assert!(
            cert.is_sound(),
            "{what} after {label}: a rigid move must keep the solid sound \
             (brep_valid={} watertight={} manifold={} oriented={}): {:?}",
            cert.brep_valid,
            cert.watertight,
            cert.manifold,
            cert.oriented,
            cert.errors
        );
    }
}

/// The same move applied to a solid that came IN through STEP (the robot-arm
/// case): export a kernel solid, import it back, then place it.
async fn assert_imported_placement_keeps_soundness(
    build: impl Fn(&mut TopologyBuilder) -> GeometryId,
    what: &str,
) {
    let mut source = BRepModel::new();
    {
        let mut b = TopologyBuilder::new(&mut source);
        build(&mut b);
    }
    let path = std::env::temp_dir().join(format!(
        "roshera_placed_sound_{}_{}.step",
        what,
        std::process::id()
    ));
    export_engine::formats::step::export_brep_to_step(&source, &path)
        .await
        .expect("export");
    let text = std::fs::read_to_string(&path).expect("read back");
    let _ = std::fs::remove_file(&path);
    let m = placement(
        [1.0, 0.0, 0.0],
        std::f64::consts::FRAC_PI_2,
        [120.0, 35.0, -60.0],
    );
    let (mut model, report) = export_engine::ExportEngine::new()
        .import_step_content(&text, what)
        .expect("import");
    assert!(report.ok, "{what}: imports sound: {:?}", report.validation);
    let sid = model.solids.iter().map(|(id, _)| id).next().expect("solid");
    transform_solid(&mut model, sid, m, TransformOptions::default()).expect("transform");
    let cert = model.certify_solid(sid);
    assert!(
        cert.is_sound(),
        "imported {what} after a rigid move must stay sound \
         (brep_valid={} watertight={} manifold={} oriented={}): {:?}",
        cert.brep_valid,
        cert.watertight,
        cert.manifold,
        cert.oriented,
        cert.errors
    );
}

#[tokio::test]
async fn an_imported_cylinder_placed_stays_sound() {
    assert_imported_placement_keeps_soundness(
        |b| {
            b.create_cylinder_3d(
                Point3::new(0.0, 0.0, 0.0),
                Vector3::new(0.0, 0.0, 1.0),
                4.0,
                25.0,
            )
            .unwrap()
        },
        "cylinder",
    )
    .await;
}

/// A tube (pin with a through-hole, mesh χ = 0) — the robot-arm part shape
/// whose placed copies opened 200 / 400 mesh edges.
fn tube(b: &mut TopologyBuilder) -> GeometryId {
    use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    let outer = solid_of(
        b.create_cylinder_3d(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            6.0,
            20.0,
        )
        .unwrap(),
    );
    let inner = solid_of(
        b.create_cylinder_3d(
            Point3::new(0.0, 0.0, -1.0),
            Vector3::new(0.0, 0.0, 1.0),
            3.0,
            22.0,
        )
        .unwrap(),
    );
    let tube = boolean_operation(
        b.model,
        outer,
        inner,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("tube = outer − inner");
    GeometryId::Solid(tube)
}

/// The exact robot-arm placement of a `DIN 988 - S4 x 8` shim: an axis
/// permutation (x→y, y→z, z→x) carrying ~1e-15 float noise, plus a small
/// offset. Column-major, copied from the import.
fn robot_arm_shim_placement() -> Matrix4 {
    Matrix4::from_cols(
        Vector3::new(3.368_283_463_715_094_3e-18, -3.556_507_964_626_848e-15, 1.0),
        Vector3::new(1.0, 3.565_523_944_469_19e-16, -3.368_283_463_713_826_2e-18),
        Vector3::new(-3.565_523_944_469_19e-16, 1.0, 3.556_507_964_626_848e-15),
        Vector3::new(
            -28.999_999_999_999_652,
            28.999_999_999_999_858,
            7.000_000_000_000_043,
        ),
    )
}

/// A DIN 988 S4×8 shim: Ø8 outside, Ø4 bore, `thickness` thick.
fn shim(thickness: f64) -> impl Fn(&mut TopologyBuilder) -> GeometryId {
    move |b: &mut TopologyBuilder| {
        use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
        let outer = solid_of(
            b.create_cylinder_3d(
                Point3::new(0.0, 0.0, 0.0),
                Vector3::new(0.0, 0.0, 1.0),
                4.0,
                thickness,
            )
            .unwrap(),
        );
        let inner = solid_of(
            b.create_cylinder_3d(
                Point3::new(0.0, 0.0, -1.0),
                Vector3::new(0.0, 0.0, 1.0),
                2.0,
                thickness + 2.0,
            )
            .unwrap(),
        );
        GeometryId::Solid(
            boolean_operation(
                b.model,
                outer,
                inner,
                BooleanOp::Difference,
                BooleanOptions::default(),
            )
            .expect("shim = disc − bore"),
        )
    }
}

fn assert_moved_by(model: &mut BRepModel, sid: u32, m: Matrix4, what: &str) {
    assert!(
        model.certify_solid(sid).is_sound(),
        "{what}: sound before the move"
    );
    transform_solid(model, sid, m, TransformOptions::default()).expect("transform");
    let cert = model.certify_solid(sid);
    assert!(
        cert.is_sound(),
        "{what}: the robot-arm shim placement must keep it sound \
         (brep_valid={} watertight={}): {:?}",
        cert.brep_valid,
        cert.watertight,
        cert.errors
    );
}

#[test]
fn a_thin_shim_placed_like_the_robot_arm_stays_sound() {
    for thickness in [1.0, 0.3, 0.1] {
        let mut model = BRepModel::new();
        let sid = {
            let mut b = TopologyBuilder::new(&mut model);
            solid_of(shim(thickness)(&mut b))
        };
        assert_moved_by(
            &mut model,
            sid,
            robot_arm_shim_placement(),
            &format!("kernel shim t={thickness}"),
        );
    }
}

#[tokio::test]
async fn an_imported_thin_shim_placed_like_the_robot_arm_stays_sound() {
    for thickness in [1.0, 0.3, 0.1] {
        let mut source = BRepModel::new();
        {
            let mut b = TopologyBuilder::new(&mut source);
            shim(thickness)(&mut b);
        }
        let path = std::env::temp_dir().join(format!(
            "roshera_shim_{}_{}.step",
            thickness,
            std::process::id()
        ));
        export_engine::formats::step::export_brep_to_step(&source, &path)
            .await
            .expect("export");
        let text = std::fs::read_to_string(&path).expect("read back");
        let _ = std::fs::remove_file(&path);
        let (mut model, report) = export_engine::ExportEngine::new()
            .import_step_content(&text, "shim")
            .expect("import");
        assert!(report.ok, "shim t={thickness} imports sound");
        let sid = model.solids.iter().map(|(id, _)| id).next().expect("solid");
        assert_moved_by(
            &mut model,
            sid,
            robot_arm_shim_placement(),
            &format!("imported shim t={thickness}"),
        );
    }
}

/// The REAL robot-arm `DIN 988 - S6 x 12` shim (written out by Roshera's
/// exporter from the imported part) and the exact placement the assembly gave
/// it. Its closed circles carry their vertex on the far side of the circle
/// from the curve's `t = 0` — legal STEP, and what the synthetic fixtures
/// above never had.
#[test]
fn the_robot_arm_shim_stays_sound_when_placed() {
    let text = include_str!("fixtures/din988_s6x12_shim.step");
    let (mut model, report) = export_engine::ExportEngine::new()
        .import_step_content(text, "din988_s6x12_shim")
        .expect("import");
    assert!(report.ok, "the shim imports sound: {:?}", report.validation);
    let sid = model.solids.iter().map(|(id, _)| id).next().expect("solid");
    // Column-major, as the assembly computed it.
    let m = Matrix4::from_cols(
        Vector3::new(3.368_283_463_710_766_3e-18, -7.238_073_559_509_98e-16, 1.0),
        Vector3::new(1.0, 3.565_523_944_469_19e-16, -3.368_283_463_710_508_2e-18),
        Vector3::new(-3.565_523_944_469_19e-16, 1.0, 7.238_073_559_509_98e-16),
        Vector3::new(
            24.999_999_999_999_943,
            59.999_999_999_999_77,
            7.439_347_520_989_107e-14,
        ),
    );
    assert_moved_by(&mut model, sid, m, "robot-arm S6x12 shim");
}

/// A chamfer band as SolidWorks writes it: a cone face bounded by TWO closed
/// circles (outer + inner loop, no seam edge), angle in degrees. Every
/// DIN 912 cap screw / DIN 555 nut in the robot arm has one. The cone mesher
/// read "outer loop has one edge" as a pointed (apex) cone and fanned the
/// face to an apex that is not on it: 200+ open mesh edges, unsound.
#[test]
fn a_two_circle_chamfer_band_meshes_watertight_and_stays_sound_placed() {
    let text = include_str!("fixtures/chamfer_frustum_two_circles.step");
    let (mut model, report) = export_engine::ExportEngine::new()
        .import_step_content(text, "chamfer_frustum_two_circles")
        .expect("import");
    let sid = model.solids.iter().map(|(id, _)| id).next().expect("solid");
    let cert = model.certify_solid(sid);
    assert!(
        report.ok && cert.is_sound(),
        "the frustum imports sound (watertight={} oriented={}): {:?} / {:?}",
        cert.watertight,
        cert.oriented,
        cert.errors,
        report.warnings
    );
    assert_moved_by(
        &mut model,
        sid,
        placement([0.3, 0.4, 0.866_025_403_784_438_6], 1.1, [40.0, -15.0, 7.0]),
        "two-circle chamfer frustum",
    );
}

/// The REAL robot-arm `DIN 912 - M6 x 25` socket-head cap screw (cut out of
/// the assembly file with its product chain and units). The bottom of its
/// hex socket is a 120° drill-point cone whose single loop is the six curves
/// where the hex walls meet it: it winds once round the cone's axis with the
/// apex INSIDE the face. Curved-CDT cannot chart a loop around the apex
/// (`CrossingFixedEdge`), and the untrimmed-grid fallback then meshed the
/// whole uncut cone, leaving the socket walls with nothing to meet: 362 open
/// mesh edges, unsound. 252 fastener uses in the arm were this part family.
#[test]
fn the_robot_arm_cap_screw_imports_sound_and_stays_sound_placed() {
    let text = include_str!("fixtures/din912_m6x25_cap_screw.step");
    let (mut model, report) = export_engine::ExportEngine::new()
        .import_step_content(text, "din912_m6x25_cap_screw")
        .expect("import");
    let sid = model.solids.iter().map(|(id, _)| id).next().expect("solid");
    let cert = model.certify_solid(sid);
    assert!(
        cert.is_sound(),
        "the cap screw imports sound (watertight={} oriented={} self-x-free={}): {:?} / {:?}",
        cert.watertight,
        cert.oriented,
        cert.self_intersection_free,
        cert.errors,
        report.warnings
    );
    assert_moved_by(
        &mut model,
        sid,
        placement([0.3, 0.4, 0.866_025_403_784_438_6], 1.1, [40.0, -15.0, 7.0]),
        "DIN 912 M6x25 cap screw",
    );
}

#[test]
fn a_placed_tube_stays_sound() {
    assert_placement_keeps_soundness(tube, "tube");
}

#[tokio::test]
async fn an_imported_tube_placed_stays_sound() {
    assert_imported_placement_keeps_soundness(tube, "tube").await;
}

#[tokio::test]
async fn an_imported_box_placed_stays_sound() {
    assert_imported_placement_keeps_soundness(
        |b| b.create_box_3d(10.0, 20.0, 30.0).unwrap(),
        "box",
    )
    .await;
}

#[test]
fn a_placed_box_stays_sound() {
    assert_placement_keeps_soundness(|b| b.create_box_3d(10.0, 20.0, 30.0).unwrap(), "box");
}

#[test]
fn a_placed_cylinder_stays_sound() {
    assert_placement_keeps_soundness(
        |b| {
            b.create_cylinder_3d(
                Point3::new(0.0, 0.0, 0.0),
                Vector3::new(0.0, 0.0, 1.0),
                4.0,
                25.0,
            )
            .unwrap()
        },
        "cylinder",
    );
}
