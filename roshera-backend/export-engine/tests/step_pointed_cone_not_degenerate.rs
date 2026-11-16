// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A cone that runs to its point is written as one loop: its base circle (a
//! closed edge, one vertex at both ends) and the seam line up to the apex and
//! back. The validator measured a loop's extent from each edge's stored START
//! vertex only -- here the circle's vertex three times -- and rejected the
//! cone as a face of zero size (CADGenBench parts 120, 126, 147: "outer loop
//! spans only 0.000e0").

use export_engine::ExportEngine;

fn envelope(body: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('cone'),'2;1');\n\
         FILE_NAME('cone.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n{body}\nENDSEC;\n\
         END-ISO-10303-21;\n"
    )
}

/// A cone of base radius 5 at z = 0 coming to a point at (0, 0, 10), on a
/// base disk. The cone's axis points from the base toward the apex's side
/// negatively, so its radius shrinks to zero at z = 10.
fn pointed_cone() -> String {
    let semi = (5.0f64 / 10.0).atan();
    format!(
        "#1=CARTESIAN_POINT('',(0.,0.,0.));
#2=DIRECTION('',(0.,0.,-1.));
#3=DIRECTION('',(1.,0.,0.));
#4=AXIS2_PLACEMENT_3D('',#1,#2,#3);
#5=CONICAL_SURFACE('',#4,5.,{semi:.17});
#6=DIRECTION('',(0.,0.,1.));
#7=AXIS2_PLACEMENT_3D('',#1,#6,#3);
#8=PLANE('',#7);
#11=CARTESIAN_POINT('',(5.,0.,0.));
#12=CARTESIAN_POINT('',(0.,0.,10.));
#21=VERTEX_POINT('',#11);
#22=VERTEX_POINT('',#12);
#31=CIRCLE('',#7,5.);
#32=DIRECTION('',(-0.44721359549995793,0.,0.89442719099991586));
#33=VECTOR('',#32,1.);
#34=LINE('',#11,#33);
#41=EDGE_CURVE('',#21,#21,#31,.T.);
#42=EDGE_CURVE('',#21,#22,#34,.T.);
#51=ORIENTED_EDGE('',*,*,#41,.T.);
#52=ORIENTED_EDGE('',*,*,#42,.T.);
#53=ORIENTED_EDGE('',*,*,#42,.F.);
#54=EDGE_LOOP('',(#51,#52,#53));
#55=FACE_OUTER_BOUND('',#54,.T.);
#56=ADVANCED_FACE('',(#55),#5,.T.);
#61=ORIENTED_EDGE('',*,*,#41,.F.);
#62=EDGE_LOOP('',(#61));
#63=FACE_OUTER_BOUND('',#62,.T.);
#64=ADVANCED_FACE('',(#63),#8,.F.);
#70=CLOSED_SHELL('',(#56,#64));
#71=MANIFOLD_SOLID_BREP('',#70);",
    )
}

#[test]
fn a_cone_running_to_its_point_is_not_a_degenerate_face() {
    let (mut model, report) = ExportEngine::new()
        .import_step_content(&envelope(&pointed_cone()), "cone")
        .expect("imports");
    let solids: Vec<u32> = model.solids.iter().map(|(id, _)| id).collect();
    assert_eq!(solids.len(), 1, "one solid: {:#?}", report.warnings);
    let cert = model.certify_solid(solids[0]);
    let degenerate: Vec<_> = cert
        .errors
        .iter()
        .filter(|e| e.contains("is degenerate"))
        .collect();
    assert!(
        degenerate.is_empty(),
        "the cone face spans its base circle and apex: {degenerate:?}"
    );
}
