// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Many STEP writers split a cylinder's or cone's wall into two half faces
//! joined along two straight seams. Each half wraps 180 degrees. The B-Rep
//! validator's geometric orientation check decides which side of an edge is
//! a face's inside by projecting the loop onto the plane at that edge; seen
//! from a seam of a half-cylinder the projection folds flat (both arcs land on
//! one line), the answer is arbitrary, and it reported the two halves as
//! walking their seams the same way although their loops oppose (CADGenBench
//! parts 218 and 231: halved chamfer cones). The solid is valid.

use export_engine::ExportEngine;

fn envelope(body: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('halves'),'2;1');\n\
         FILE_NAME('halves.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n{body}\nENDSEC;\n\
         END-ISO-10303-21;\n"
    )
}

/// A radius-5, height-10 cylinder about +Z: bottom and top disks, the wall
/// in two halves (x >= 0 and x <= 0) joined along the seams at (0, +-5).
const HALVED_CYLINDER: &str = "#1=CARTESIAN_POINT('',(0.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#3=DIRECTION('',(1.,0.,0.));
#4=AXIS2_PLACEMENT_3D('',#1,#2,#3);
#5=CYLINDRICAL_SURFACE('',#4,5.);
#6=CARTESIAN_POINT('',(0.,0.,10.));
#7=AXIS2_PLACEMENT_3D('',#6,#2,#3);
#8=PLANE('',#4);
#9=PLANE('',#7);
#10=CIRCLE('',#4,5.);
#11=CIRCLE('',#7,5.);
#21=CARTESIAN_POINT('',(0.,-5.,0.));
#22=CARTESIAN_POINT('',(0.,5.,0.));
#23=CARTESIAN_POINT('',(0.,-5.,10.));
#24=CARTESIAN_POINT('',(0.,5.,10.));
#31=VERTEX_POINT('',#21);
#32=VERTEX_POINT('',#22);
#33=VERTEX_POINT('',#23);
#34=VERTEX_POINT('',#24);
#40=VECTOR('',#2,1.);
#41=LINE('',#21,#40);
#42=LINE('',#22,#40);
#51=EDGE_CURVE('',#31,#32,#10,.T.);
#52=EDGE_CURVE('',#32,#31,#10,.T.);
#53=EDGE_CURVE('',#33,#34,#11,.T.);
#54=EDGE_CURVE('',#34,#33,#11,.T.);
#55=EDGE_CURVE('',#31,#33,#41,.T.);
#56=EDGE_CURVE('',#32,#34,#42,.T.);
#61=ORIENTED_EDGE('',*,*,#51,.T.);
#62=ORIENTED_EDGE('',*,*,#56,.T.);
#63=ORIENTED_EDGE('',*,*,#53,.F.);
#64=ORIENTED_EDGE('',*,*,#55,.F.);
#70=EDGE_LOOP('',(#61,#62,#63,#64));
#71=FACE_OUTER_BOUND('',#70,.T.);
#72=ADVANCED_FACE('',(#71),#5,.T.);
#65=ORIENTED_EDGE('',*,*,#52,.T.);
#66=ORIENTED_EDGE('',*,*,#55,.T.);
#67=ORIENTED_EDGE('',*,*,#54,.F.);
#68=ORIENTED_EDGE('',*,*,#56,.F.);
#73=EDGE_LOOP('',(#65,#66,#67,#68));
#74=FACE_OUTER_BOUND('',#73,.T.);
#75=ADVANCED_FACE('',(#74),#5,.T.);
#81=ORIENTED_EDGE('',*,*,#51,.F.);
#82=ORIENTED_EDGE('',*,*,#52,.F.);
#83=EDGE_LOOP('',(#81,#82));
#84=FACE_OUTER_BOUND('',#83,.T.);
#85=ADVANCED_FACE('',(#84),#8,.F.);
#86=ORIENTED_EDGE('',*,*,#53,.T.);
#87=ORIENTED_EDGE('',*,*,#54,.T.);
#88=EDGE_LOOP('',(#86,#87));
#89=FACE_OUTER_BOUND('',#88,.T.);
#90=ADVANCED_FACE('',(#89),#9,.T.);
#100=CLOSED_SHELL('',(#72,#75,#85,#90));
#101=MANIFOLD_SOLID_BREP('',#100);";

/// The same part with a 45-degree cone for its wall (radius 5 at the bottom,
/// 10 at the top) -- the shape of the halved chamfer cones on CADGenBench
/// parts 218 and 231. Seen along the normal at one seam, the far seam of a
/// 45-degree half-cone is exactly edge-on, the projected outline pinches and
/// crosses, and the inside of the face was read on the wrong side.
fn halved_cone() -> String {
    let h = std::f64::consts::FRAC_1_SQRT_2;
    HALVED_CYLINDER
        .replace(
            "#5=CYLINDRICAL_SURFACE('',#4,5.);",
            "#5=CONICAL_SURFACE('',#4,5.,0.78539816339744828);",
        )
        .replace("#11=CIRCLE('',#7,5.);", "#11=CIRCLE('',#7,10.);")
        .replace(
            "#23=CARTESIAN_POINT('',(0.,-5.,10.));",
            "#23=CARTESIAN_POINT('',(0.,-10.,5.));",
        )
        .replace(
            "#24=CARTESIAN_POINT('',(0.,5.,10.));",
            "#24=CARTESIAN_POINT('',(0.,10.,5.));",
        )
        .replace(
            "#6=CARTESIAN_POINT('',(0.,0.,10.));",
            "#6=CARTESIAN_POINT('',(0.,0.,5.));",
        )
        .replace(
            "#40=VECTOR('',#2,1.);
#41=LINE('',#21,#40);
#42=LINE('',#22,#40);",
            &format!(
                "#43=DIRECTION('',(0.,-{h:.17},{h:.17}));
#44=DIRECTION('',(0.,{h:.17},{h:.17}));
                 #45=VECTOR('',#43,1.);
#46=VECTOR('',#44,1.);
                 #41=LINE('',#21,#45);
#42=LINE('',#22,#46);"
            ),
        )
}

#[test]
fn a_cylinder_wall_written_as_two_halves_is_valid() {
    halves_validate(HALVED_CYLINDER);
}

#[test]
fn a_45_degree_cone_wall_written_as_two_halves_is_valid() {
    let text = halved_cone();
    assert!(
        text.contains("CONICAL_SURFACE") && text.contains("#45=VECTOR"),
        "the cone fixture was built"
    );
    halves_validate(&text);
}

fn halves_validate(body: &str) {
    let (mut model, report) = ExportEngine::new()
        .import_step_content(&envelope(body), "halves")
        .expect("imports");
    let solids: Vec<u32> = model.solids.iter().map(|(id, _)| id).collect();
    assert_eq!(solids.len(), 1, "one solid: {:#?}", report.warnings);
    assert_eq!(
        model.faces.iter().count(),
        4,
        "two disks and two half walls"
    );
    let cert = model.certify_solid(solids[0]);
    assert!(
        cert.brep_valid,
        "the halved wall is a valid B-Rep: {:?}",
        cert.errors
    );
    assert!(cert.is_sound(), "and sound: {:?}", cert.errors);
}
