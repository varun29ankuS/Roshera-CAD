// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A planar face overlaps itself when two parts of its boundary pass through
//! each other. Two CONSECUTIVE edges crossing inside the tolerance zone of
//! the vertex they share are not that: within the zone their order is not
//! defined, the vertex is where they meet. An imported part's edges carry
//! tolerances of up to 0.05 mm, and at a sharp corner their curves' ends,
//! a micrometre apart, cross a few micrometres from the vertex. Everything
//! else is still an overlap: edges that are not neighbours (wherever they
//! cross, a vertex's zone included), an edge crossing itself, and
//! neighbours crossing outside the zone.
//!
//! The outline is read two ways (`JointClosing`). The established way
//! follows each edge up to where the NEXT edge starts; it is judged first,
//! and a face it finds simple is simple. Only if it crosses is each edge
//! followed along its OWN curve to its own end, with the tolerant-vertex
//! rule applied; the face overlaps only if that reading crosses too.
//!
//! What this covers on the CADGenBench corpus, as measured:
//! * part 201 -- certifies. The established reading swung a straight edge,
//!   whose neighbour began 23 micrometres off its end, across a third edge
//!   0.07 mm from the far corner; the own-end reading does not cross.
//! * part 243 -- its planar faces pass (B-Rep valid); the part is still
//!   unsound for other reasons.
//! * part 250 -- kept: simple under the established reading, crossing under
//!   the own-end one, which is why the established one is asked first.
//! * part 211 -- NOT covered. Its consecutive edges cross about 0.03 mm from
//!   their vertex against a zone of 0.02 mm, so it is still an overlap.

use geometry_engine::math::nurbs::{interpolate_nurbs_curve, ParameterizationType};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::geometry_validity::{
    planar_face_outline_closed, planar_face_self_intersects, tolerant_vertex_crossings,
    JointClosing,
};
use geometry_engine::primitives::curve::{Curve, Line, NurbsCurve, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
use geometry_engine::primitives::face::{Face, FaceOrientation};
use geometry_engine::primitives::r#loop::{Loop, LoopType};
use geometry_engine::primitives::surface::Plane;
use geometry_engine::primitives::topology_builder::BRepModel;

fn through(points: &[Point3]) -> (Box<dyn Curve>, ParameterRange) {
    let fit = interpolate_nurbs_curve(points, 3, ParameterizationType::ChordLength).expect("fit");
    let knots = fit.knots.values().to_vec();
    let range = ParameterRange::new(knots[0], knots[knots.len() - 1]);
    let curve = NurbsCurve::new(
        fit.degree,
        fit.control_points.clone(),
        fit.weights.clone(),
        knots,
    )
    .expect("curve");
    (Box::new(curve), range)
}

fn line(a: (f64, f64), b: (f64, f64)) -> (Box<dyn Curve>, ParameterRange) {
    (
        Box::new(Line::new(
            Point3::new(a.0, a.1, 0.0),
            Point3::new(b.0, b.1, 0.0),
        )),
        ParameterRange::new(0.0, 1.0),
    )
}

/// A planar face in z = 0 whose outer loop is `edges` in order, each given
/// as (start vertex position, curve); edge k ends at the vertex edge k + 1
/// starts from. Every edge carries `tolerance`. Returns the model, the face
/// and the vertex ids in order.
fn planar_face(
    edges: Vec<((f64, f64), (Box<dyn Curve>, ParameterRange))>,
    tolerance: f64,
) -> (BRepModel, u32, Vec<u32>) {
    let mut model = BRepModel::new();
    let plane = model.surfaces.add(Box::new(
        Plane::new(Point3::new(0.0, 0.0, 0.0), Vector3::Z, Vector3::X).expect("plane"),
    ));
    let vertices: Vec<u32> = edges
        .iter()
        .map(|(v, _)| model.vertices.add(v.0, v.1, 0.0))
        .collect();
    let n = edges.len();
    let mut lp = Loop::new(0, LoopType::Outer);
    for (k, (_, (curve, range))) in edges.into_iter().enumerate() {
        let curve = model.curves.add(curve);
        let mut edge = Edge::new(
            0,
            vertices[k],
            vertices[(k + 1) % n],
            curve,
            EdgeOrientation::Forward,
            range,
        );
        edge.set_tolerance(tolerance);
        let id = model.edges.add(edge);
        lp.add_edge(id, true);
    }
    let lp = model.loops.add(lp);
    let face = model
        .faces
        .add(Face::new(0, plane, lp, FaceOrientation::Forward));
    (model, face, vertices)
}

/// A wedge with a sharp corner at the origin: a small arc of radius
/// `0.05 scale` arrives there heading down the Y axis, and a straight edge
/// leaves it up and a little to the right. The straight edge's curve starts
/// `0.001 scale` to the LEFT of the corner vertex, so near the corner the two
/// run through each other: they cross about `0.0062 scale` from the vertex.
fn sharp_corner(scale: f64, tolerance: f64) -> (BRepModel, u32, Vec<u32>) {
    let r = 0.05 * scale;
    let arc: Vec<Point3> = (0..=48)
        .map(|k| {
            let a = std::f64::consts::FRAC_PI_2 * (1.0 - k as f64 / 48.0);
            Point3::new(-r + r * a.cos(), r * a.sin(), 0.0)
        })
        .collect();
    planar_face(
        vec![
            ((-5.0, 5.0), line((-5.0, 5.0), (-r, r))),
            ((-r, r), through(&arc)),
            ((0.0, 0.0), line((-0.001 * scale, 0.0), (1.0, 10.0))),
            ((1.0, 10.0), line((1.0, 10.0), (-5.0, 5.0))),
        ],
        tolerance,
    )
}

/// Inside the vertex's zone: the vertex, not an overlap -- and reported.
#[test]
fn consecutive_edges_crossing_inside_their_vertex_zone_are_the_vertex() {
    let (model, face, vertices) = sharp_corner(1.0, 0.02);
    assert!(
        !planar_face_self_intersects(&model, face),
        "a crossing 6 micrometres from a vertex of tolerance 0.02 is the vertex"
    );
    let reported = tolerant_vertex_crossings(&model, face);
    assert_eq!(reported.len(), 1, "and it is reported: {reported:?}");
    let c = reported[0];
    assert_eq!(c.vertex, vertices[2], "at the corner vertex");
    assert!(
        c.distance > 0.004 && c.distance < 0.009,
        "with how far from it the edges cross: {c:?}"
    );
    assert!((c.tolerance - 0.02).abs() < 1e-12, "and the zone: {c:?}");
}

/// The same edges carrying an exact tolerance: the crossing is outside any
/// zone and the face overlaps itself, as it always did.
#[test]
fn the_same_crossing_with_exact_edges_is_an_overlap() {
    let (model, face, _) = sharp_corner(1.0, 1e-6);
    assert!(planar_face_self_intersects(&model, face));
    assert!(tolerant_vertex_crossings(&model, face).is_empty());
}

/// Ten times the size: the edges cross 0.06 mm from the vertex. No tolerance
/// reaches that far -- the zone is held to 0.05 mm whatever an edge claims.
#[test]
fn a_crossing_beyond_the_ceiling_is_an_overlap_whatever_the_tolerance() {
    let (model, face, _) = sharp_corner(10.0, 0.2);
    assert!(planar_face_self_intersects(&model, face));
    assert!(tolerant_vertex_crossings(&model, face).is_empty());
}

/// Edges that are not neighbours crossing: a bow-tie. Tolerances do not
/// enter into it.
#[test]
fn edges_that_are_not_neighbours_crossing_is_an_overlap() {
    let (model, face, _) = planar_face(
        vec![
            ((0.0, 0.0), line((0.0, 0.0), (10.0, 10.0))),
            ((10.0, 10.0), line((10.0, 10.0), (10.0, 0.0))),
            ((10.0, 0.0), line((10.0, 0.0), (0.0, 10.0))),
            ((0.0, 10.0), line((0.0, 10.0), (0.0, 0.0))),
        ],
        0.05,
    );
    assert!(planar_face_self_intersects(&model, face));
    assert!(tolerant_vertex_crossings(&model, face).is_empty());
}

/// Edges that are not neighbours crossing INSIDE a vertex's zone. Edge 0
/// comes down the Y axis to A at the origin; edge 1 is 0.01 long, from A to
/// B; edge 2 leaves B up and to the left and passes through edge 0 at
/// (0, 0.005) -- 5 micrometres from A, 11 from B, every edge carrying a
/// tolerance of 0.05. Edges 0 and 2 share no vertex, so no vertex is where
/// they meet: the zone excuses neighbours only, and this is an overlap.
#[test]
fn edges_that_are_not_neighbours_crossing_inside_a_vertex_zone_is_an_overlap() {
    let (model, face, _) = planar_face(
        vec![
            ((0.0, 10.0), line((0.0, 10.0), (0.0, 0.0))),
            ((0.0, 0.0), line((0.0, 0.0), (0.01, 0.0))),
            ((0.01, 0.0), line((0.01, 0.0), (-10.0, 5.0))),
            ((-10.0, 5.0), line((-10.0, 5.0), (0.0, 10.0))),
        ],
        0.05,
    );
    assert!(
        planar_face_self_intersects(&model, face),
        "edges 0 and 2 are not neighbours; crossing near a vertex does not make them so"
    );
    assert!(
        tolerant_vertex_crossings(&model, face).is_empty(),
        "and it is not reported as a tolerant vertex"
    );
    // The reading the rule lives in says the same, on its own.
    let own_ends = planar_face_outline_closed(&model, face, JointClosing::OwnEnds);
    assert!(own_ends.self_overlaps && own_ends.at_tolerant_vertices.is_empty());
}

/// A sliver between the X axis and the parabola y = 0.01 x^2, closed by a
/// short upright edge at x = 10 whose curve starts 0.02 above the end of the
/// bottom edge. The bottom edge is the X axis from 0 to 10: it does not
/// swing up to meet its neighbour, and does not cross the parabola.
#[test]
fn a_straight_edge_is_its_own_line_not_a_chord_to_its_neighbours_start() {
    let parabola: Vec<Point3> = (0..=64)
        .map(|k| {
            let x = 10.0 * (1.0 - k as f64 / 64.0);
            Point3::new(x, 0.01 * x * x, 0.0)
        })
        .collect();
    let (model, face, _) = planar_face(
        vec![
            ((0.0, 0.0), line((0.0, 0.0), (10.0, 0.0))),
            ((10.0, 0.0), line((10.0, 0.02), (10.0, 1.0))),
            ((10.0, 1.0), through(&parabola)),
        ],
        1e-6,
    );
    assert!(
        !planar_face_self_intersects(&model, face),
        "the bottom edge was swung onto its neighbour's start and read as crossing the parabola"
    );
    assert!(tolerant_vertex_crossings(&model, face).is_empty());
}

/// A needle of two edges: a straight one down the Y axis to the origin, and
/// a curve back up that starts 1.2 micrometres to the right of where the
/// straight one ends and crosses over it a little way up. Closed the
/// established way -- the straight edge run to where the curve starts --
/// the outline does not cross, and that reading stands: the second way of
/// closing a joint is asked only when the first one crosses (CADGenBench
/// part 250, which certified before and must still).
#[test]
fn an_outline_that_does_not_cross_the_established_way_is_simple() {
    let back: Vec<Point3> = (0..=64)
        .map(|k| {
            let y = 10.0 * k as f64 / 64.0;
            Point3::new(
                0.0012 * (1.0 - y / 10.0) - 0.05 * y * (10.0 - y) / 25.0,
                y,
                0.0,
            )
        })
        .collect();
    let (model, face, _) = planar_face(
        vec![
            ((0.0, 10.0), line((0.0, 10.0), (0.0, 0.0))),
            ((0.0, 0.0), through(&back)),
        ],
        1e-6,
    );
    assert!(!planar_face_self_intersects(&model, face));
    assert!(tolerant_vertex_crossings(&model, face).is_empty());
    // Why the order matters, each reading on its own: the established one
    // does not cross and the own-end one does. Were the own-end reading
    // asked first, or alone, this face would be called self-overlapping.
    let established = planar_face_outline_closed(&model, face, JointClosing::OntoNextStart);
    assert!(!established.self_overlaps, "established reading: simple");
    let own_ends = planar_face_outline_closed(&model, face, JointClosing::OwnEnds);
    assert!(
        own_ends.self_overlaps,
        "own-end reading, alone, crosses: the fixture would not show the order otherwise"
    );
}
