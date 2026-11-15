// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! An EDGE_CURVE on a B-spline is the part of the curve BETWEEN its two
//! vertices, not the whole curve. STEP writers commonly reuse one long
//! intersection curve for several edges, each a piece of it. The importer
//! used to give every such edge the curve's whole domain: one CADGenBench
//! part had an edge running 1.77 mm past its own vertex, so the face outline
//! never closed and the face was refused.

use export_engine::ExportEngine;
use geometry_engine::math::Point3;

fn envelope(body: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('trim'),'2;1');\n\
         FILE_NAME('trim.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n{body}\nENDSEC;\n\
         END-ISO-10303-21;\n"
    )
}

/// The quadratic Bezier (0,0,0) - (5,5,0) - (10,0,0) on [0, 1]:
/// C(t) = (10 t, 10 t (1 - t), 0).
fn on_curve(t: f64) -> [f64; 3] {
    [10.0 * t, 10.0 * t * (1.0 - t), 0.0]
}

/// One EDGE_CURVE on that curve from `a` to `b` with the given sense.
fn edge_step(a: [f64; 3], b: [f64; 3], sense: &str) -> String {
    format!(
        "#1=CARTESIAN_POINT('',(0.,0.,0.));
#2=CARTESIAN_POINT('',(5.,5.,0.));
#3=CARTESIAN_POINT('',(10.,0.,0.));
#4=B_SPLINE_CURVE_WITH_KNOTS('',2,(#1,#2,#3),.UNSPECIFIED.,.F.,.F.,(3,3),(0.,1.),.UNSPECIFIED.);
#11=CARTESIAN_POINT('',({:.17},{:.17},{:.17}));
#12=CARTESIAN_POINT('',({:.17},{:.17},{:.17}));
#21=VERTEX_POINT('',#11);
#22=VERTEX_POINT('',#12);
#31=EDGE_CURVE('',#21,#22,#4,{sense});",
        a[0], a[1], a[2], b[0], b[1], b[2]
    )
}

fn dist(p: Point3, q: [f64; 3]) -> f64 {
    ((p.x - q[0]).powi(2) + (p.y - q[1]).powi(2) + (p.z - q[2]).powi(2)).sqrt()
}

/// The edge's parameter range ends exactly at its two vertices: whichever
/// way round, the curve at one end of the range is one vertex and at the
/// other end is the other.
fn assert_edge_spans_its_vertices(a: [f64; 3], b: [f64; 3], sense: &str) {
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(&edge_step(a, b, sense)), "trim")
        .expect("imports");
    let edges: Vec<_> = model.edges.iter().collect();
    assert_eq!(edges.len(), 1, "one edge: {:#?}", report.warnings);
    let (_, edge) = edges[0];
    let curve = model.curves.get(edge.curve_id).expect("curve");
    let r = edge.param_range;
    let p0 = curve.evaluate(r.start).expect("evaluates").position;
    let p1 = curve.evaluate(r.end).expect("evaluates").position;
    let straight = dist(p0, a).max(dist(p1, b));
    let crossed = dist(p0, b).max(dist(p1, a));
    assert!(
        straight.min(crossed) < 1e-9,
        "sense {sense}: the range [{}, {}] runs {p0:?} to {p1:?}, not between the vertices {a:?} and {b:?}",
        r.start,
        r.end
    );
}

#[test]
fn a_bspline_edge_runs_only_between_its_vertices() {
    // From the curve's start to its middle, both senses.
    assert_edge_spans_its_vertices(on_curve(0.0), on_curve(0.5), ".T.");
    assert_edge_spans_its_vertices(on_curve(0.5), on_curve(0.0), ".F.");
    // A piece from inside the curve, both senses.
    assert_edge_spans_its_vertices(on_curve(0.25), on_curve(0.75), ".T.");
    assert_edge_spans_its_vertices(on_curve(0.75), on_curve(0.25), ".F.");
}

#[test]
fn a_bspline_edge_over_the_whole_curve_keeps_the_whole_domain() {
    assert_edge_spans_its_vertices(on_curve(0.0), on_curve(1.0), ".T.");
    assert_edge_spans_its_vertices(on_curve(1.0), on_curve(0.0), ".F.");
}

/// One EDGE_CURVE on the closed square polyline (0,0) (10,0) (10,10) (0,10)
/// back to (0,0), a degree-1 B-spline on [0, 1] whose seam is at (0,0,0).
fn closed_edge_step(a: [f64; 3], b: [f64; 3], sense: &str) -> String {
    format!(
        "#1=CARTESIAN_POINT('',(0.,0.,0.));
#2=CARTESIAN_POINT('',(10.,0.,0.));
#3=CARTESIAN_POINT('',(10.,10.,0.));
#5=CARTESIAN_POINT('',(0.,10.,0.));
#4=B_SPLINE_CURVE_WITH_KNOTS('',1,(#1,#2,#3,#5,#1),.UNSPECIFIED.,.T.,.F.,(2,1,1,1,2),(0.,0.25,0.5,0.75,1.),.UNSPECIFIED.);
#11=CARTESIAN_POINT('',({:.17},{:.17},{:.17}));
#12=CARTESIAN_POINT('',({:.17},{:.17},{:.17}));
#21=VERTEX_POINT('',#11);
#22=VERTEX_POINT('',#12);
#31=EDGE_CURVE('',#21,#22,#4,{sense});",
        a[0], a[1], a[2], b[0], b[1], b[2]
    )
}

/// The middle of the edge's range is at `mid`: the edge took the right half
/// of the closed curve, not the other one.
fn assert_closed_edge_passes(a: [f64; 3], b: [f64; 3], sense: &str, mid: [f64; 3]) {
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(&closed_edge_step(a, b, sense)), "closed")
        .expect("imports");
    let edges: Vec<_> = model.edges.iter().collect();
    assert_eq!(edges.len(), 1, "one edge: {:#?}", report.warnings);
    let (_, edge) = edges[0];
    let curve = model.curves.get(edge.curve_id).expect("curve");
    let r = edge.param_range;
    let m = curve
        .evaluate(0.5 * (r.start + r.end))
        .expect("evaluates")
        .position;
    assert!(
        dist(m, mid) < 1e-9,
        "{a:?} -> {b:?} sense {sense}: range [{}, {}] passes {m:?}, want {mid:?}",
        r.start,
        r.end
    );
}

#[test]
fn a_closed_bspline_edge_from_its_seam_takes_the_half_its_sense_runs() {
    let (seam, far) = ([0.0, 0.0, 0.0], [10.0, 10.0, 0.0]);
    // Along the curve from the seam: through (10,0,0).
    assert_closed_edge_passes(seam, far, ".T.", [10.0, 0.0, 0.0]);
    assert_closed_edge_passes(far, seam, ".F.", [10.0, 0.0, 0.0]);
    // Along the curve back to the seam: through (0,10,0).
    assert_closed_edge_passes(far, seam, ".T.", [0.0, 10.0, 0.0]);
    assert_closed_edge_passes(seam, far, ".F.", [0.0, 10.0, 0.0]);
}
