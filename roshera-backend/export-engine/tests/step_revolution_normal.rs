// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A STEP SURFACE_OF_REVOLUTION's normal is fixed by ISO 10303-42: u is the
//! ROTATION angle, v the swept curve's parameter, and the normal is
//! dσ/du × dσ/dv. A face's `same_sense` flag is relative to THAT normal. The
//! kernel's surface of revolution parameterises the other way round (u along
//! the profile, v the angle), which reverses du × dv, so an imported face
//! has to come in with the outward side STEP meant -- not the opposite one.
//! One CADGenBench part's hole walls (revolution faces) all read as
//! inconsistently oriented against the plates around them.

use export_engine::ExportEngine;

fn envelope(body: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('rev'),'2;1');\n\
         FILE_NAME('rev.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n{body}\nENDSEC;\n\
         END-ISO-10303-21;\n"
    )
}

/// A quarter of a radius-10 wall about +Z, 5 tall, as a revolution of the
/// straight profile (10,0,0) -> (10,0,5). ISO 10303-42: dσ/du (rotation) at
/// the profile is +Y, dσ/dv (up the profile) is +Z, so the surface normal
/// is +Y x +Z = +X: AWAY from the axis. With `same_sense = .T.` the face's
/// outward side is away from the axis.
const WALL: &str = "#1=CARTESIAN_POINT('',(10.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#3=VECTOR('',#2,1.);
#4=LINE('',#1,#3);
#5=CARTESIAN_POINT('',(0.,0.,0.));
#6=AXIS1_PLACEMENT('',#5,#2);
#7=SURFACE_OF_REVOLUTION('',#4,#6);
#11=CARTESIAN_POINT('',(10.,0.,0.));
#12=CARTESIAN_POINT('',(0.,10.,0.));
#13=CARTESIAN_POINT('',(0.,10.,5.));
#14=CARTESIAN_POINT('',(10.,0.,5.));
#21=VERTEX_POINT('',#11);
#22=VERTEX_POINT('',#12);
#23=VERTEX_POINT('',#13);
#24=VERTEX_POINT('',#14);
#30=DIRECTION('',(1.,0.,0.));
#31=AXIS2_PLACEMENT_3D('',#5,#2,#30);
#32=CIRCLE('',#31,10.);
#33=CARTESIAN_POINT('',(0.,0.,5.));
#34=AXIS2_PLACEMENT_3D('',#33,#2,#30);
#35=CIRCLE('',#34,10.);
#36=CARTESIAN_POINT('',(0.,10.,0.));
#37=VECTOR('',#2,1.);
#38=LINE('',#36,#37);
#51=EDGE_CURVE('',#21,#22,#32,.T.);
#52=EDGE_CURVE('',#22,#23,#38,.T.);
#53=EDGE_CURVE('',#24,#23,#35,.T.);
#54=EDGE_CURVE('',#21,#24,#4,.T.);
#61=ORIENTED_EDGE('',*,*,#51,.T.);
#62=ORIENTED_EDGE('',*,*,#52,.T.);
#63=ORIENTED_EDGE('',*,*,#53,.F.);
#64=ORIENTED_EDGE('',*,*,#54,.F.);
#70=EDGE_LOOP('',(#61,#62,#63,#64));
#71=FACE_OUTER_BOUND('',#70,.T.);
#80=ADVANCED_FACE('',(#71),#7,.T.);
#90=OPEN_SHELL('',(#80));
#91=SHELL_BASED_SURFACE_MODEL('',(#90));";

#[test]
fn an_imported_revolution_face_faces_the_way_step_says() {
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(WALL), "rev")
        .expect("imports");
    let faces: Vec<_> = model.faces.iter().collect();
    assert_eq!(faces.len(), 1, "one face: {:#?}", report.warnings);
    let (_, face) = faces[0];
    let surface = model.surfaces.get(face.surface_id).expect("surface");
    // The face's outward normal at a point of the wall, 45 degrees round.
    let p = geometry_engine::math::Point3::new(
        10.0 * std::f64::consts::FRAC_1_SQRT_2,
        10.0 * std::f64::consts::FRAC_1_SQRT_2,
        2.5,
    );
    let (u, v) = surface
        .closest_point(&p, geometry_engine::math::Tolerance::default())
        .expect("projects");
    let n = surface.normal_at(u, v).expect("normal") * face.orientation.sign();
    let away = geometry_engine::math::Vector3::new(p.x, p.y, 0.0);
    assert!(
        n.dot(&away) > 0.0,
        "{}: the face's outward normal {n:?} points toward the axis; STEP says away",
        surface.type_name()
    );
}

/// The writer is the importer's inverse: a kernel vase (one smooth revolution
/// wall, a bore, two caps) exported to STEP and read back keeps its outer
/// wall facing AWAY from the axis. Together with the test above (the
/// importer reads ISO's convention), this pins the WRITER to ISO too: a
/// writer that emitted the kernel's own axis would round-trip a wall that
/// OCCT and every other reader see inside out.
#[tokio::test]
async fn an_exported_revolution_wall_reads_back_facing_out() {
    use geometry_engine::math::{Point3, Tolerance, Vector3};
    use geometry_engine::operations::revolve::{revolve_spline_meridian, RevolveOptions};
    use geometry_engine::primitives::topology_builder::BRepModel;

    let mut model = BRepModel::new();
    let wall = [(5.0, 0.0), (3.0, 4.0), (4.0, 8.0), (7.0, 12.0)];
    let opts = RevolveOptions {
        axis_origin: Point3::ZERO,
        axis_direction: Vector3::Z,
        angle: std::f64::consts::TAU,
        segments: 48,
        ..Default::default()
    };
    revolve_spline_meridian(&mut model, &wall, 2.0, opts).expect("vase");

    let temp = tempfile::TempDir::new().expect("tmp");
    let engine = ExportEngine::with_output_directory(temp.path().to_string_lossy().to_string());
    let filename = engine.export_step(&model, "vase").await.expect("export");
    let text = std::fs::read_to_string(temp.path().join(&filename)).expect("read");
    let (back, _report) = ExportEngine::new()
        .import_step_content(&text, "vase")
        .expect("re-imports");

    let mut walls = 0;
    for (_, face) in back.faces.iter() {
        let Some(surface) = back.surfaces.get(face.surface_id) else {
            continue;
        };
        if surface.type_name() != "SurfaceOfRevolution" {
            continue;
        }
        walls += 1;
        // A point on the outer wall at mid height, on +X.
        let p = Point3::new(3.5, 0.0, 6.0);
        let (u, v) = surface
            .closest_point(&p, Tolerance::default())
            .expect("projects");
        let q = surface.point_at(u, v).expect("evaluates");
        let n = surface.normal_at(u, v).expect("normal") * face.orientation.sign();
        let away = Vector3::new(q.x, q.y, 0.0);
        assert!(
            n.dot(&away) > 0.0,
            "the re-imported outer wall faces the axis at {q:?} (normal {n:?})"
        );
    }
    assert_eq!(walls, 1, "the vase has one revolution wall");
}
