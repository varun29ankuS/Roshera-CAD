// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A planar face's self-overlap check sampled each curved boundary edge with
//! a fixed 16 chords. Where two curves leave a corner at a shallow angle --
//! here two radius-50 arcs half a micron apart, one sweeping 5 mm and the
//! other 25 mm -- the chords of the longer one cut across the thin wedge and
//! cross the shorter one's, although the curves never meet: a false
//! "self-overlaps" that refused CADGenBench parts 131, 134 and 249.

use export_engine::ExportEngine;
use geometry_engine::operations::geometry_validity::planar_face_self_intersects;

fn envelope(body: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('wedge'),'2;1');\n\
         FILE_NAME('wedge.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n{body}\nENDSEC;\n\
         END-ISO-10303-21;\n"
    )
}

/// The wedge in z = 0: P0 (0, 0) -- arc A, centre (0, -50), clockwise 0.1
/// rad -- P1 -- line -- P2 -- arc B, centre (0, gap - 50), counter-clockwise
/// up to -- P3 (0, gap) -- the 0.5 micrometre edge back to P0.
fn wedge() -> String {
    let gap = 5e-4;
    let a1 = std::f64::consts::FRAC_PI_2 - 0.1;
    let p1 = (50.0 * a1.cos(), -50.0 + 50.0 * a1.sin());
    let b0 = std::f64::consts::FRAC_PI_2 - 0.5;
    let p2 = (50.0 * b0.cos(), gap - 50.0 + 50.0 * b0.sin());
    let (d1x, d1y) = (p2.0 - p1.0, p2.1 - p1.1);
    let l1 = (d1x * d1x + d1y * d1y).sqrt();
    format!(
        "#1=CARTESIAN_POINT('',(0.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#3=DIRECTION('',(1.,0.,0.));
#4=AXIS2_PLACEMENT_3D('',#1,#2,#3);
#5=PLANE('',#4);
#11=CARTESIAN_POINT('',(0.,0.,0.));
#12=CARTESIAN_POINT('',({p1x:.15},{p1y:.15},0.));
#13=CARTESIAN_POINT('',({p2x:.15},{p2y:.15},0.));
#14=CARTESIAN_POINT('',(0.,{gap:.15},0.));
#21=VERTEX_POINT('',#11);
#22=VERTEX_POINT('',#12);
#23=VERTEX_POINT('',#13);
#24=VERTEX_POINT('',#14);
#30=CARTESIAN_POINT('',(0.,-50.,0.));
#31=DIRECTION('',(0.,0.,-1.));
#32=DIRECTION('',(0.,1.,0.));
#33=AXIS2_PLACEMENT_3D('',#30,#31,#32);
#34=CIRCLE('',#33,50.);
#35=CARTESIAN_POINT('',(0.,{cby:.15},0.));
#36=AXIS2_PLACEMENT_3D('',#35,#2,#3);
#37=CIRCLE('',#36,50.);
#38=DIRECTION('',({d1x:.15},{d1y:.15},0.));
#39=VECTOR('',#38,1.);
#40=LINE('',#12,#39);
#41=DIRECTION('',(0.,-1.,0.));
#42=VECTOR('',#41,1.);
#43=LINE('',#14,#42);
#51=EDGE_CURVE('',#21,#22,#34,.T.);
#52=EDGE_CURVE('',#22,#23,#40,.T.);
#53=EDGE_CURVE('',#23,#24,#37,.T.);
#54=EDGE_CURVE('',#24,#21,#43,.T.);
#61=ORIENTED_EDGE('',*,*,#51,.T.);
#62=ORIENTED_EDGE('',*,*,#52,.T.);
#63=ORIENTED_EDGE('',*,*,#53,.T.);
#64=ORIENTED_EDGE('',*,*,#54,.T.);
#70=EDGE_LOOP('',(#61,#62,#63,#64));
#71=FACE_OUTER_BOUND('',#70,.T.);
#80=ADVANCED_FACE('',(#71),#5,.T.);
#90=OPEN_SHELL('',(#80));
#91=SHELL_BASED_SURFACE_MODEL('',(#90));",
        p1x = p1.0,
        p1y = p1.1,
        p2x = p2.0,
        p2y = p2.1,
        cby = gap - 50.0,
        d1x = d1x / l1,
        d1y = d1y / l1,
    )
}

#[test]
fn a_thin_wedge_between_two_arcs_is_not_self_overlapping() {
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(&wedge()), "wedge")
        .expect("imports");
    let faces: Vec<u32> = model.faces.iter().map(|(id, _)| id).collect();
    assert_eq!(faces.len(), 1, "one face: {:#?}", report.warnings);
    assert!(
        !planar_face_self_intersects(&model, faces[0]),
        "the wedge's boundary does not cross itself"
    );
}
