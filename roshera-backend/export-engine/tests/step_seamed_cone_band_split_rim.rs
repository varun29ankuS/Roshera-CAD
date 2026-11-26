// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A countersink is one conical face that runs the whole way round: its loop
//! walks one rim, crosses the seam, walks the other rim back and returns down
//! the seam. Writers leave one rim a single closed circle and split the other
//! into arcs wherever neighbouring faces end on it (CADGenBench part 149: 13
//! such countersinks).
//!
//! The B-Rep validator's geometric orientation check pictures a face by the
//! outline of its loop. It sampled every edge of the loop -- except that a
//! closed edge (one vertex at both ends) could not be sampled, and one closed
//! edge anywhere in the loop threw the whole sampled outline away for the
//! polygon of the loop's VERTICES. For this face that is a handful of points
//! on one rim plus one on the other: not the face. The check read the rim
//! arcs from the wrong side and reported the cone and the flat face beside it
//! as walking their shared arcs the same way, although their loops oppose.
//! The solid is valid.

use export_engine::ExportEngine;

fn envelope(body: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('band'),'2;1');\n\
         FILE_NAME('band.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n{body}\nENDSEC;\n\
         END-ISO-10303-21;\n"
    )
}

/// A frustum about +Z, radius `r0` at z = 0 and `r1` at z = `h`: bottom disk,
/// top disk and ONE conical wall with a seam at 90 degrees. The bottom rim is
/// a single closed circle; the top rim is four arcs meeting at 90, 320, 340
/// and 35 degrees (the uneven split of part 149's countersinks). Two of the
/// arcs are written against the circle and two along it, as writers do: the
/// vertex polygon answered the same for every rim arc whichever way it ran,
/// so arcs running one of the two ways were always read wrong.
fn frustum(r0: f64, r1: f64, h: f64) -> String {
    let half_angle = ((r1 - r0) / h).atan();
    let slant = ((r1 - r0).powi(2) + h * h).sqrt();
    let (sy, sz) = ((r1 - r0) / slant, h / slant);
    let top = |deg: f64| {
        let a = deg.to_radians();
        format!("({:.15},{:.15},{h:.15})", r1 * a.cos(), r1 * a.sin())
    };
    format!(
        "#1=CARTESIAN_POINT('',(0.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#3=DIRECTION('',(1.,0.,0.));
#4=AXIS2_PLACEMENT_3D('',#1,#2,#3);
#5=CONICAL_SURFACE('',#4,{r0:.15},{half_angle:.17});
#6=CARTESIAN_POINT('',(0.,0.,{h:.15}));
#7=AXIS2_PLACEMENT_3D('',#6,#2,#3);
#8=PLANE('',#4);
#9=PLANE('',#7);
#10=CIRCLE('',#4,{r0:.15});
#11=CIRCLE('',#7,{r1:.15});
#20=CARTESIAN_POINT('',(0.,{r0:.15},0.));
#21=CARTESIAN_POINT('',{p90});
#22=CARTESIAN_POINT('',{p320});
#23=CARTESIAN_POINT('',{p340});
#24=CARTESIAN_POINT('',{p35});
#30=VERTEX_POINT('',#20);
#31=VERTEX_POINT('',#21);
#32=VERTEX_POINT('',#22);
#33=VERTEX_POINT('',#23);
#34=VERTEX_POINT('',#24);
#40=DIRECTION('',(0.,{sy:.17},{sz:.17}));
#41=VECTOR('',#40,1.);
#42=LINE('',#20,#41);
#50=EDGE_CURVE('',#30,#30,#10,.T.);
#51=EDGE_CURVE('',#30,#31,#42,.T.);
#52=EDGE_CURVE('',#31,#32,#11,.T.);
#53=EDGE_CURVE('',#33,#32,#11,.F.);
#54=EDGE_CURVE('',#33,#34,#11,.T.);
#55=EDGE_CURVE('',#31,#34,#11,.F.);
#60=ORIENTED_EDGE('',*,*,#50,.T.);
#61=ORIENTED_EDGE('',*,*,#51,.T.);
#62=ORIENTED_EDGE('',*,*,#55,.T.);
#63=ORIENTED_EDGE('',*,*,#54,.F.);
#64=ORIENTED_EDGE('',*,*,#53,.T.);
#65=ORIENTED_EDGE('',*,*,#52,.F.);
#66=ORIENTED_EDGE('',*,*,#51,.F.);
#70=EDGE_LOOP('',(#60,#61,#62,#63,#64,#65,#66));
#71=FACE_OUTER_BOUND('',#70,.T.);
#72=ADVANCED_FACE('',(#71),#5,.T.);
#80=ORIENTED_EDGE('',*,*,#50,.F.);
#81=EDGE_LOOP('',(#80));
#82=FACE_OUTER_BOUND('',#81,.T.);
#83=ADVANCED_FACE('',(#82),#8,.F.);
#90=ORIENTED_EDGE('',*,*,#52,.T.);
#91=ORIENTED_EDGE('',*,*,#53,.F.);
#92=ORIENTED_EDGE('',*,*,#54,.T.);
#93=ORIENTED_EDGE('',*,*,#55,.F.);
#94=EDGE_LOOP('',(#90,#91,#92,#93));
#95=FACE_OUTER_BOUND('',#94,.T.);
#96=ADVANCED_FACE('',(#95),#9,.T.);
#100=CLOSED_SHELL('',(#72,#83,#96));
#101=MANIFOLD_SOLID_BREP('',#100);",
        p90 = top(90.0),
        p320 = top(320.0),
        p340 = top(340.0),
        p35 = top(35.0),
    )
}

fn band_validates(body: &str) {
    let (mut model, report) = ExportEngine::new()
        .import_step_content(&envelope(body), "band")
        .expect("imports");
    let solids: Vec<u32> = model.solids.iter().map(|(id, _)| id).collect();
    assert_eq!(solids.len(), 1, "one solid: {:#?}", report.warnings);
    assert_eq!(model.faces.iter().count(), 3, "two disks and one wall");
    let cert = model.certify_solid(solids[0]);
    assert!(
        cert.brep_valid,
        "the seamed wall is a valid B-Rep: {:?}",
        cert.errors
    );
    assert!(cert.is_sound(), "and sound: {:?}", cert.errors);
}

/// The countersink's own shape: a 45-degree wall.
#[test]
fn a_45_degree_seamed_cone_band_with_a_split_rim_is_valid() {
    band_validates(&frustum(3.0, 5.0, 2.0));
}

/// A shallow wall (about 17 degrees off the axis plane): seen along any of
/// its normals the whole band faces the viewer, so the check does decide
/// every rim arc and must decide it from the true outline.
#[test]
fn a_shallow_seamed_cone_band_with_a_split_rim_is_valid() {
    band_validates(&frustum(3.0, 9.0, 2.0));
}

/// A steep wall (about 14 degrees off the axis).
#[test]
fn a_steep_seamed_cone_band_with_a_split_rim_is_valid() {
    band_validates(&frustum(3.0, 3.5, 2.0));
}
