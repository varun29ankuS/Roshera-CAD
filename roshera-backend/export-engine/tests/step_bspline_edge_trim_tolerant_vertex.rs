// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A B-spline edge whose vertex sits a little off its curve is still only
//! the part of the curve between its vertices.
//!
//! The data is CADGenBench part 211, entities #40167 and #40241: one cubic
//! Bezier written twice, split into two edges at vertex #40168, which lies
//! 1.543e-3 from the curve at parameter 0.50796 (OCCT keeps a vertex
//! tolerance of 3e-2 there). The importer used to read a vertex more than
//! 1e-3 off the curve as broken data and give the edge the WHOLE curve, so
//! both edges covered the same 2.25 mm border, the face outline overlapped
//! itself and the face meshed to nothing.

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

/// Part 211's vertices: the curve's start (#40142), the split vertex
/// (#40169) and the curve's end (#40243).
const CURVE_START: [f64; 3] = [-66.95223078229, 11.305233721842, 85.0018178386];
const SPLIT: [f64; 3] = [-66.92608919764, 12.447177967623, 85.062782750361];
const CURVE_END: [f64; 3] = [-66.90005690079, 13.553907804111, 84.993115705895];

/// How far the split vertex is from the curve, measured on the file's data.
const SPLIT_OFF_CURVE: f64 = 1.5434e-3;
/// The curve parameter nearest the split vertex.
const SPLIT_PARAMETER: f64 = 0.50796;

/// Part 211's Bezier (#40171 and its twin #40245), as STEP, under the id `id`
/// with its control points numbered from `id + 1`.
fn bezier(id: u32) -> String {
    format!(
        "#{p1}=CARTESIAN_POINT('',(-66.95223078259,11.305233721615,85.001817838646));
#{p2}=CARTESIAN_POINT('',(-66.93443949526,12.070299533359,85.087994137742));
#{p3}=CARTESIAN_POINT('',(-66.91782284724,12.788557182711,85.085339989781));
#{p4}=CARTESIAN_POINT('',(-66.90005712819,13.553907816026,84.993115760977));
#{id}=B_SPLINE_CURVE_WITH_KNOTS('',3,(#{p1},#{p2},#{p3},#{p4}),.UNSPECIFIED.,.F.,.F.,(4,4),(0.,1.),.PIECEWISE_BEZIER_KNOTS.);",
        p1 = id + 1,
        p2 = id + 2,
        p3 = id + 3,
        p4 = id + 4
    )
}

fn vertex(id: u32, p: [f64; 3]) -> String {
    format!(
        "#{pid}=CARTESIAN_POINT('',({:.17},{:.17},{:.17}));\n#{id}=VERTEX_POINT('',#{pid});",
        p[0],
        p[1],
        p[2],
        pid = id + 1
    )
}

/// The two edges as part 211 writes them: #300 from the curve's start to the
/// split vertex, #400 from the split vertex to the curve's end, each on its
/// own copy of the Bezier, both `.T.`. `split` is where the shared vertex is.
fn two_edges_split_at(split: [f64; 3]) -> String {
    [
        bezier(100),
        bezier(200),
        vertex(10, CURVE_START),
        vertex(20, split),
        vertex(30, CURVE_END),
        "#300=EDGE_CURVE('',#10,#20,#100,.T.);".to_string(),
        "#400=EDGE_CURVE('',#20,#30,#200,.T.);".to_string(),
    ]
    .join("\n")
}

fn dist(p: Point3, q: [f64; 3]) -> f64 {
    ((p.x - q[0]).powi(2) + (p.y - q[1]).powi(2) + (p.z - q[2]).powi(2)).sqrt()
}

/// Each imported edge as (parameter range start, range end, the curve there
/// at the start, at the end), ordered along the curve.
fn imported_spans(body: &str) -> (Vec<(f64, f64, Point3, Point3)>, Vec<String>) {
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(body), "trim")
        .expect("imports");
    let mut spans: Vec<(f64, f64, Point3, Point3)> = model
        .edges
        .iter()
        .map(|(_, edge)| {
            let curve = model.curves.get(edge.curve_id).expect("curve");
            let r = edge.param_range;
            (
                r.start,
                r.end,
                curve.evaluate(r.start).expect("evaluates").position,
                curve.evaluate(r.end).expect("evaluates").position,
            )
        })
        .collect();
    spans.sort_by(|a, b| a.0.partial_cmp(&b.0).expect("finite"));
    let warnings = report
        .warnings
        .iter()
        .map(|w| format!("#{:?} {}: {}", w.instance, w.entity, w.message))
        .collect();
    (spans, warnings)
}

#[test]
fn an_edge_whose_vertex_is_a_little_off_its_curve_is_trimmed_at_the_nearest_point() {
    let (spans, warnings) = imported_spans(&two_edges_split_at(SPLIT));
    assert_eq!(spans.len(), 2, "both edges import: {warnings:#?}");
    let (first, second) = (spans[0], spans[1]);

    // The first edge runs from the curve's start to the point nearest the
    // split vertex, the second from there to the curve's end: together they
    // cover the curve once.
    assert!(first.0.abs() < 1e-12, "first edge starts at 0: {first:?}");
    assert!(
        (first.1 - SPLIT_PARAMETER).abs() < 1e-4,
        "first edge ends at the split, not at {}",
        first.1
    );
    assert!(
        (second.0 - SPLIT_PARAMETER).abs() < 1e-4,
        "second edge starts at the split, not at {}",
        second.0
    );
    assert!(
        (second.1 - 1.0).abs() < 1e-12,
        "second edge ends at 1: {second:?}"
    );
    assert!(
        (first.1 - second.0).abs() < 1e-9,
        "the two edges meet at one parameter: {} and {}",
        first.1,
        second.0
    );

    // The trimmed ends are the curve's nearest points to the vertex: as far
    // from it as the vertex is from the curve, no farther.
    for (what, p) in [
        ("first edge's end", first.3),
        ("second edge's start", second.2),
    ] {
        let off = dist(p, SPLIT);
        assert!(
            (off - SPLIT_OFF_CURVE).abs() < 1e-6,
            "{what} is {off:e} from the split vertex; the vertex is {SPLIT_OFF_CURVE:e} off the curve"
        );
    }
}

#[test]
fn the_distance_of_a_trimmed_vertex_from_its_curve_is_reported() {
    let (_, warnings) = imported_spans(&two_edges_split_at(SPLIT));
    let told: Vec<&String> = warnings
        .iter()
        .filter(|w| w.contains("off its curve") && w.contains("1.543e-3") && w.contains("trimmed"))
        .collect();
    assert_eq!(
        told.len(),
        2,
        "each edge says how far its vertex is from the curve: {warnings:#?}"
    );
    assert!(
        warnings.iter().all(|w| !w.contains("whole curve")),
        "no edge is widened to the whole curve: {warnings:#?}"
    );
}

#[test]
fn an_edge_whose_vertex_is_nowhere_near_its_curve_is_refused_not_widened() {
    // The split vertex moved 1 mm off the curve, whose two pieces are 1.14
    // and 1.11 mm long: farther off than half of either piece. That is not
    // a tolerant vertex; no trim is honest.
    let far = [SPLIT[0] + 1.0, SPLIT[1], SPLIT[2]];
    let (spans, warnings) = imported_spans(&two_edges_split_at(far));
    assert!(
        spans.is_empty(),
        "neither edge is imported: {spans:?} {warnings:#?}"
    );
    let refused = warnings
        .iter()
        .filter(|w| {
            w.contains("EDGE_CURVE") && w.contains("off its curve") && w.contains("refused")
        })
        .count();
    assert_eq!(refused, 2, "each refusal is reported: {warnings:#?}");
}
