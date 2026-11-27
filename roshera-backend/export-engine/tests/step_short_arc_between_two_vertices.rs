// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! An `EDGE_CURVE` on a circle runs from its start vertex to its end vertex
//! in the edge's sense. It is the WHOLE circle only when both ends are the
//! same vertex. The importer decided that by distance instead -- two ends
//! closer than the file's stated accuracy were "the same point" -- so an arc
//! shorter than that accuracy between two different vertices was built as
//! the full circle it lies on: CADGenBench part 247 has a 6 micrometre arc of
//! a 5 metre circle, part 248 a 3 micrometre arc that became the "giant
//! triangles", part 250 two more. The loop then walks a whole circle where
//! the file has a hair of one.

use export_engine::ExportEngine;

fn envelope(body: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('arc'),'2;1');\n\
         FILE_NAME('arc.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n{body}\nENDSEC;\n\
         END-ISO-10303-21;\n"
    )
}

/// A radius-5, height-10 cylinder about +Z whose bottom rim is two edges:
/// from vertex A at angle 0 to vertex B at `split` radians, and from B the
/// rest of the way round back to A. `rim` is the bottom rim's curve entity
/// (a circle or an ellipse of both semi-axes 5). The top rim is one closed
/// circle and the wall has a seam at angle 0.
fn cylinder_with_split_rim(split: f64, rim: &str) -> String {
    let (bx, by) = (5.0 * split.cos(), 5.0 * split.sin());
    format!(
        "#1=CARTESIAN_POINT('',(0.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#3=DIRECTION('',(1.,0.,0.));
#4=AXIS2_PLACEMENT_3D('',#1,#2,#3);
#5=CYLINDRICAL_SURFACE('',#4,5.);
#6=CARTESIAN_POINT('',(0.,0.,10.));
#7=AXIS2_PLACEMENT_3D('',#6,#2,#3);
#8=PLANE('',#4);
#9=PLANE('',#7);
#10={rim};
#11=CIRCLE('',#7,5.);
#20=CARTESIAN_POINT('',(5.,0.,0.));
#21=CARTESIAN_POINT('',({bx:.17},{by:.17},0.));
#22=CARTESIAN_POINT('',(5.,0.,10.));
#30=VERTEX_POINT('',#20);
#31=VERTEX_POINT('',#21);
#32=VERTEX_POINT('',#22);
#40=VECTOR('',#2,1.);
#41=LINE('',#20,#40);
#50=EDGE_CURVE('',#30,#31,#10,.T.);
#51=EDGE_CURVE('',#31,#30,#10,.T.);
#52=EDGE_CURVE('',#32,#32,#11,.T.);
#53=EDGE_CURVE('',#30,#32,#41,.T.);
#60=ORIENTED_EDGE('',*,*,#50,.T.);
#61=ORIENTED_EDGE('',*,*,#51,.T.);
#62=ORIENTED_EDGE('',*,*,#53,.T.);
#63=ORIENTED_EDGE('',*,*,#52,.F.);
#64=ORIENTED_EDGE('',*,*,#53,.F.);
#70=EDGE_LOOP('',(#60,#61,#62,#63,#64));
#71=FACE_OUTER_BOUND('',#70,.T.);
#72=ADVANCED_FACE('',(#71),#5,.T.);
#80=ORIENTED_EDGE('',*,*,#51,.F.);
#81=ORIENTED_EDGE('',*,*,#50,.F.);
#82=EDGE_LOOP('',(#80,#81));
#83=FACE_OUTER_BOUND('',#82,.T.);
#84=ADVANCED_FACE('',(#83),#8,.F.);
#90=ORIENTED_EDGE('',*,*,#52,.T.);
#91=EDGE_LOOP('',(#90));
#92=FACE_OUTER_BOUND('',#91,.T.);
#93=ADVANCED_FACE('',(#92),#9,.T.);
#100=CLOSED_SHELL('',(#72,#84,#93));
#101=MANIFOLD_SOLID_BREP('',#100);"
    )
}

const CIRCLE: &str = "CIRCLE('',#4,5.)";
const ELLIPSE: &str = "ELLIPSE('',#4,5.,5.)";

/// The length of every edge the import made, longest first.
fn edge_lengths(body: &str) -> (Vec<f64>, bool) {
    let (mut model, report) = ExportEngine::new()
        .import_step_content(&envelope(body), "arc")
        .expect("imports");
    let solids: Vec<u32> = model.solids.iter().map(|(id, _)| id).collect();
    assert_eq!(solids.len(), 1, "one solid: {:#?}", report.warnings);
    let mut lengths = Vec::new();
    for (_, lp) in model.loops.iter() {
        for &e in &lp.edges {
            let edge = model.edges.get(e).expect("edge");
            let mut length = 0.0;
            let mut last = edge.evaluate(0.0, &model.curves).expect("start");
            for k in 1..=256 {
                let p = edge
                    .evaluate(k as f64 / 256.0, &model.curves)
                    .expect("point");
                length += (p - last).magnitude();
                last = p;
            }
            lengths.push(length);
        }
    }
    lengths.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let sound = model.certify_solid(solids[0]).is_sound();
    (lengths, sound)
}

/// The calibration case: the rim split a quarter of the way round has always
/// been read as a quarter and three quarters.
#[test]
fn a_rim_split_at_a_quarter_turn_is_a_quarter_and_three_quarters() {
    let (lengths, sound) = edge_lengths(&cylinder_with_split_rim(
        std::f64::consts::FRAC_PI_2,
        CIRCLE,
    ));
    assert!(sound);
    let full = 2.0 * std::f64::consts::PI * 5.0;
    assert!(
        lengths.iter().any(|l| (l - 0.25 * full).abs() < 1e-2)
            && lengths.iter().any(|l| (l - 0.75 * full).abs() < 1e-2),
        "{lengths:?}"
    );
}

/// The rim split 0.5 micrometre from its start: a hair of arc, and all the
/// rest. No edge of the bottom rim may be a second whole circle, and the
/// solid is sound.
#[test]
fn an_arc_shorter_than_the_files_accuracy_is_not_the_whole_circle() {
    let (lengths, sound) = edge_lengths(&cylinder_with_split_rim(1e-7, CIRCLE));
    let full = 2.0 * std::f64::consts::PI * 5.0;
    // The wall and the bottom disk each walk the bottom rim once, and the
    // wall and the top disk the top rim: four edge uses the length of the
    // circle, not six.
    let whole_circles = lengths.iter().filter(|l| (**l - full).abs() < 1e-2).count();
    assert_eq!(
        whole_circles, 4,
        "the hair of arc was built as a circle of its own: {lengths:?}"
    );
    assert!(sound, "and the cylinder is sound");
}

/// The same rim written as an ELLIPSE (of equal semi-axes).
#[test]
fn an_elliptical_arc_shorter_than_the_files_accuracy_is_not_the_whole_ellipse() {
    let (lengths, sound) = edge_lengths(&cylinder_with_split_rim(1e-7, ELLIPSE));
    let full = 2.0 * std::f64::consts::PI * 5.0;
    let whole = lengths.iter().filter(|l| (**l - full).abs() < 1e-2).count();
    assert_eq!(
        whole, 4,
        "the hair of arc was built as an ellipse of its own: {lengths:?}"
    );
    assert!(sound, "and the cylinder is sound");
}
