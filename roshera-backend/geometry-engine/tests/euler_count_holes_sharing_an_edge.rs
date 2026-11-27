// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The Euler-Poincare count `V - E + F - R = 2(S - G)` and how a face's
//! loops are read as its boundary (`FaceBoundary`, `EulerTally`).
//!
//! "One inner loop, one ring" holds only while a face's loops are apart.
//! The cases, each with a test below:
//!
//! | loops of one face                                   | reading                                   |
//! |-----------------------------------------------------|-------------------------------------------|
//! | apart                                               | each inner loop a ring                    |
//! | share a VERTEX only (hole touching the outline)     | one boundary component: no ring           |
//! | share a vertex only (two holes touching)            | one component between them: one ring      |
//! | share an EDGE, walked once each way                 | void edge: not boundary, left out of E    |
//! | ... a hole written as two loops                     | one ring                                  |
//! | ... a hole notching the outline                     | no ring                                   |
//! | ... several separate arcs shared                    | all void; one component                   |
//! | share an edge walked the SAME way                   | a defect: counted, solid fails            |
//! | ONE loop walking an edge twice (a seam)             | an edge of the surface, counted, as before|
//!
//! The solid that brought the void edge up: a plate with a boss, bored
//! through off the boss's axis so the bore's rim straddles the boss's foot
//! (`boolean_straddling_rim::f7_straddling_offset_*`). On the plate's top
//! face the hole is the boss's foot circle together with the crescent of the
//! bore outside it, and the boolean writes that one hole as two inner loops
//! -- the whole foot circle, and the crescent -- sharing the arc of the foot
//! circle that lies inside the bore: two edges and the vertex between them,
//! walked once each way, void on both sides. Counting each inner loop a ring
//! gave -2 (even, so it passed, while implying genus 2 for a genus-1 body);
//! joining loops that share a vertex without leaving the void arc out gave
//! -1 (odd: the solid was rejected). With the arc left out the count is
//! `26 - 36 + 12 - 2 = 0`.

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::primitives::curve::{Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
use geometry_engine::primitives::face::{Face, FaceOrientation};
use geometry_engine::primitives::r#loop::{Loop, LoopType};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::Plane;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::validation::{
    euler_tally, face_boundary, validate_model_enhanced, EulerTally, FaceBoundary, ValidationError,
    ValidationLevel,
};
use std::collections::{BTreeMap, BTreeSet};

// ═════════════════════════ one face, read on its own ══════════════════════════

/// One walk of an edge in a loop: (index into the fixture's edges, forward?).
type Walk = (usize, bool);

/// A planar face in z = 0 built from straight edges. `points` are positions,
/// `edges` are (from point, to point), and each of `loops` is a list of
/// walks; the first loop is the outer one. Returns the model, the face, and
/// the edge ids in the order of `edges`.
fn face_of(
    points: &[(f64, f64)],
    edges: &[(usize, usize)],
    loops: &[&[Walk]],
) -> (BRepModel, u32, Vec<u32>) {
    let mut model = BRepModel::new();
    let plane = model.surfaces.add(Box::new(
        Plane::new(Point3::new(0.0, 0.0, 0.0), Vector3::Z, Vector3::X).expect("plane"),
    ));
    let vertices: Vec<u32> = points
        .iter()
        .map(|&(x, y)| model.vertices.add(x, y, 0.0))
        .collect();
    let at = |k: usize| Point3::new(points[k].0, points[k].1, 0.0);
    let edge_ids: Vec<u32> = edges
        .iter()
        .map(|&(a, b)| {
            let curve = model.curves.add(Box::new(Line::new(at(a), at(b))));
            model.edges.add(Edge::new(
                0,
                vertices[a],
                vertices[b],
                curve,
                EdgeOrientation::Forward,
                ParameterRange::new(0.0, 1.0),
            ))
        })
        .collect();
    let mut loop_ids = Vec::new();
    for (k, walks) in loops.iter().enumerate() {
        let mut lp = Loop::new(
            0,
            if k == 0 {
                LoopType::Outer
            } else {
                LoopType::Inner
            },
        );
        for &(edge, forward) in walks.iter() {
            lp.add_edge(edge_ids[edge], forward);
        }
        loop_ids.push(model.loops.add(lp));
    }
    let mut face = Face::new(0, plane, loop_ids[0], FaceOrientation::Forward);
    face.inner_loops.extend(loop_ids[1..].iter().copied());
    let face = model.faces.add(face);
    (model, face, edge_ids)
}

fn boundary_of(model: &BRepModel, face: u32) -> FaceBoundary {
    face_boundary(model, model.faces.get(face).expect("face"))
}

/// The 10 x 10 square every face below has as its outline: points 0 to 3,
/// edges 0 to 3, walked forward.
const SQUARE_POINTS: [(f64, f64); 4] = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
const SQUARE_EDGES: [(usize, usize); 4] = [(0, 1), (1, 2), (2, 3), (3, 0)];
const SQUARE_LOOP: [Walk; 4] = [(0, true), (1, true), (2, true), (3, true)];

fn with_square(
    points: &[(f64, f64)],
    edges: &[(usize, usize)],
) -> (Vec<(f64, f64)>, Vec<(usize, usize)>) {
    (
        SQUARE_POINTS.iter().chain(points).copied().collect(),
        SQUARE_EDGES.iter().chain(edges).copied().collect(),
    )
}

/// Loops apart: the inner loop is a ring, as it always was.
#[test]
fn a_hole_apart_from_the_outline_is_a_ring() {
    // A triangle well inside the square: points 4, 5, 6; edges 4, 5, 6.
    let (points, edges) = with_square(
        &[(2.0, 2.0), (3.0, 6.0), (6.0, 3.0)],
        &[(4, 5), (5, 6), (6, 4)],
    );
    let (model, face, _) = face_of(
        &points,
        &edges,
        &[&SQUARE_LOOP, &[(4, true), (5, true), (6, true)]],
    );
    assert_eq!(
        boundary_of(&model, face),
        FaceBoundary {
            rings: 1,
            void_edges: vec![],
            same_way_shared_edges: vec![],
        }
    );
}

/// Loops sharing a VERTEX only (CADGenBench part 248: a hole circle tangent
/// to its face's outline). The region is a disk pinched at that vertex: one
/// boundary component, no ring. Nothing is left out of V or E.
#[test]
fn a_hole_touching_the_outline_at_a_vertex_is_not_a_ring() {
    // The triangle's first corner IS the square's corner at the origin.
    let (points, edges) = with_square(&[(3.0, 6.0), (6.0, 3.0)], &[(0, 4), (4, 5), (5, 0)]);
    let (model, face, _) = face_of(
        &points,
        &edges,
        &[&SQUARE_LOOP, &[(4, true), (5, true), (6, true)]],
    );
    assert_eq!(
        boundary_of(&model, face),
        FaceBoundary {
            rings: 0,
            void_edges: vec![],
            same_way_shared_edges: vec![],
        }
    );
}

/// Two holes touching each other at a vertex, both apart from the outline:
/// together they are one boundary component, so the face has ONE ring, not
/// two.
#[test]
fn two_holes_touching_at_a_vertex_are_one_ring() {
    // Two triangles meeting at (5, 5): points 4 (the shared one), 5-6, 7-8.
    let (points, edges) = with_square(
        &[(5.0, 5.0), (2.0, 4.0), (2.0, 6.0), (8.0, 6.0), (8.0, 4.0)],
        &[(4, 5), (5, 6), (6, 4), (4, 7), (7, 8), (8, 4)],
    );
    let (model, face, _) = face_of(
        &points,
        &edges,
        &[
            &SQUARE_LOOP,
            &[(4, true), (5, true), (6, true)],
            &[(7, true), (8, true), (9, true)],
        ],
    );
    assert_eq!(
        boundary_of(&model, face),
        FaceBoundary {
            rings: 1,
            void_edges: vec![],
            same_way_shared_edges: vec![],
        }
    );
}

/// Two squares side by side inside the outline, sharing the edge between
/// them: points 4-7 the left one (2,2)-(5,5), 8-9 the right one's far side.
/// Edge 5 (index 5 below: from (5,2) up to (5,5)) is the shared one.
fn two_squares_sharing_an_edge(second_walks_it_forward: bool) -> (BRepModel, u32, Vec<u32>) {
    let (points, edges) = with_square(
        &[
            (2.0, 2.0),
            (5.0, 2.0),
            (5.0, 5.0),
            (2.0, 5.0),
            (8.0, 2.0),
            (8.0, 5.0),
        ],
        &[
            (4, 5), // 4
            (5, 6), // 5: the shared edge
            (6, 7), // 6
            (7, 4), // 7
            (5, 8), // 8
            (8, 9), // 9
            (9, 6), // 10
        ],
    );
    // The left square walks the shared edge forward.
    let left: [Walk; 4] = [(4, true), (5, true), (6, true), (7, true)];
    // The right square, the same way round as the left one, comes DOWN the
    // shared edge; turned the other way round it goes up it.
    let right_opposite: [Walk; 4] = [(8, true), (9, true), (10, true), (5, false)];
    let right_same: [Walk; 4] = [(5, true), (10, false), (9, false), (8, false)];
    face_of(
        &points,
        &edges,
        &[
            &SQUARE_LOOP,
            &left,
            if second_walks_it_forward {
                &right_same
            } else {
                &right_opposite
            },
        ],
    )
}

/// THE case: a hole written as two loops that share an edge, walked once
/// each way. The edge has a hole on both sides: it is void, and the two
/// loops are one ring.
#[test]
fn an_edge_two_loops_walk_opposite_ways_is_void_and_they_are_one_ring() {
    let (model, face, edges) = two_squares_sharing_an_edge(false);
    assert_eq!(
        boundary_of(&model, face),
        FaceBoundary {
            rings: 1,
            void_edges: vec![edges[5]],
            same_way_shared_edges: vec![],
        }
    );
}

/// The same edge walked the SAME way by both loops. No region has two holes
/// on one side of an edge: this is a defect, not the void case. The edge is
/// not left out; it is named.
#[test]
fn an_edge_two_loops_walk_the_same_way_is_not_void() {
    let (model, face, edges) = two_squares_sharing_an_edge(true);
    let boundary = boundary_of(&model, face);
    assert_eq!(boundary.void_edges, Vec::<u32>::new());
    assert_eq!(boundary.same_way_shared_edges, vec![edges[5]]);
}

/// A hole sharing an edge with the OUTLINE, walked opposite ways: a notch
/// written as a hole. The shared edge has the hole on one side and the
/// outside on the other: void. What is left is one boundary component.
#[test]
fn a_hole_sharing_an_edge_with_the_outline_is_a_notch_not_a_ring() {
    // The outline's bottom side in three pieces; the hole stands on the
    // middle one.
    let points = [
        (0.0, 0.0),   // 0
        (4.0, 0.0),   // 1
        (6.0, 0.0),   // 2
        (10.0, 0.0),  // 3
        (10.0, 10.0), // 4
        (0.0, 10.0),  // 5
        (6.0, 2.0),   // 6
        (4.0, 2.0),   // 7
    ];
    let edges = [
        (0, 1), // 0
        (1, 2), // 1: shared
        (2, 3), // 2
        (3, 4), // 3
        (4, 5), // 4
        (5, 0), // 5
        (1, 7), // 6
        (7, 6), // 7
        (6, 2), // 8
    ];
    let (model, face, ids) = face_of(
        &points,
        &edges,
        &[
            &[
                (0, true),
                (1, true),
                (2, true),
                (3, true),
                (4, true),
                (5, true),
            ],
            // (4,0) up, across, down, and back along the shared edge.
            &[(6, true), (7, true), (8, true), (1, false)],
        ],
    );
    assert_eq!(
        boundary_of(&model, face),
        FaceBoundary {
            rings: 0,
            void_edges: vec![ids[1]],
            same_way_shared_edges: vec![],
        }
    );
}

/// Two loops sharing SEVERAL separate arcs: a bar, and an arch standing on
/// its two ends. Both shared edges are void. What is left of the two loops
/// still hangs together at the arcs' end vertices: one boundary component,
/// one ring.
///
/// (Between the bar and the arch is a pocket bounded by edges that are NOT
/// shared. If that pocket is material, the face is in two pieces -- it and
/// the region outside -- which no count of one face describes; `F` is then
/// one short and a solid carrying such a face comes out odd and is rejected,
/// which is the right verdict for a face that is not one region.)
#[test]
fn two_loops_sharing_two_separate_arcs_leave_both_out() {
    let (points, edges) = with_square(
        &[
            (2.0, 2.0), // 4
            (8.0, 2.0), // 5
            (8.0, 4.0), // 6
            (6.0, 4.0), // 7
            (4.0, 4.0), // 8
            (2.0, 4.0), // 9
            (4.0, 5.0), // 10
            (6.0, 5.0), // 11
            (8.0, 7.0), // 12
            (2.0, 7.0), // 13
        ],
        &[
            (4, 5),   // 4
            (5, 6),   // 5
            (6, 7),   // 6: shared (right)
            (7, 8),   // 7: the bar's top, under the arch
            (8, 9),   // 8: shared (left)
            (9, 4),   // 9
            (8, 10),  // 10
            (10, 11), // 11
            (11, 7),  // 12
            (6, 12),  // 13
            (12, 13), // 14
            (13, 9),  // 15
        ],
    );
    let bar: [Walk; 6] = [
        (4, true),
        (5, true),
        (6, true),
        (7, true),
        (8, true),
        (9, true),
    ];
    // The arch: from (2,4) along the left shared edge backwards, up over
    // the pocket, along the right shared edge backwards, and round the top.
    let arch: [Walk; 8] = [
        (8, false),
        (10, true),
        (11, true),
        (12, true),
        (6, false),
        (13, true),
        (14, true),
        (15, true),
    ];
    let (model, face, ids) = face_of(&points, &edges, &[&SQUARE_LOOP, &bar, &arch]);
    let mut shared = vec![ids[6], ids[8]];
    shared.sort_unstable();
    assert_eq!(
        boundary_of(&model, face),
        FaceBoundary {
            rings: 1,
            void_edges: shared,
            same_way_shared_edges: vec![],
        }
    );
}

/// A SEAM is one loop walking an edge twice, once each way. It is not what
/// the void rule is about -- that takes two DIFFERENT loops -- and it is
/// read exactly as before: here the outline carries a slit, and a hole apart
/// from it is still one ring.
#[test]
fn an_edge_one_loop_walks_twice_is_not_void() {
    let (points, edges) = with_square(
        &[(3.0, 3.0), (6.0, 6.0), (8.0, 6.0), (7.0, 8.0)],
        &[(0, 4), (5, 6), (6, 7), (7, 5)],
    );
    let (model, face, _) = face_of(
        &points,
        &edges,
        &[
            // Round the square, then out along the slit and back.
            &[
                (0, true),
                (1, true),
                (2, true),
                (3, true),
                (4, true),
                (4, false),
            ],
            &[(5, true), (6, true), (7, true)],
        ],
    );
    assert_eq!(
        boundary_of(&model, face),
        FaceBoundary {
            rings: 1,
            void_edges: vec![],
            same_way_shared_edges: vec![],
        }
    );
}

// ═══════════════════════════════ whole solids ═════════════════════════════════

fn solid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        other => panic!("a solid: {other:?}"),
    }
}

/// The count as it was before loops were read as boundary components,
/// made here independently of the kernel's: every vertex and edge any loop
/// walks, every inner loop a ring. Returns (V, E, F, R, faces with no edges,
/// edges some single loop walks twice).
fn plain_count(m: &BRepModel, s: SolidId) -> (usize, usize, usize, usize, usize, usize) {
    let solid = m.solids.get(s).expect("solid");
    let (mut vertices, mut edges) = (BTreeSet::new(), BTreeSet::new());
    let (mut faces, mut rings, mut seamless, mut seams) = (0, 0, 0, 0);
    for shell in solid.all_shells() {
        for &fid in &m.shells.get(shell).expect("shell").faces {
            let face = m.faces.get(fid).expect("face");
            faces += 1;
            rings += face.inner_loops.len();
            let mut walked = 0;
            for lid in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
                let lp = m.loops.get(lid).expect("loop");
                let mut times: BTreeMap<u32, usize> = BTreeMap::new();
                for &eid in &lp.edges {
                    walked += 1;
                    *times.entry(eid).or_default() += 1;
                    edges.insert(eid);
                    let e = m.edges.get(eid).expect("edge");
                    vertices.insert(e.start_vertex);
                    vertices.insert(e.end_vertex);
                }
                seams += times.values().filter(|&&n| n == 2).count();
            }
            if walked == 0 {
                seamless += 1;
            }
        }
    }
    (vertices.len(), edges.len(), faces, rings, seamless, seams)
}

fn assert_counted_as_before(label: &str, m: &BRepModel, s: SolidId) -> (EulerTally, usize) {
    let tally = euler_tally(m, m.solids.get(s).expect("solid"));
    let (v, e, f, r, seamless, seams) = plain_count(m, s);
    assert_eq!(
        (
            tally.vertices,
            tally.edges,
            tally.faces,
            tally.rings,
            tally.seamless_closed_faces
        ),
        (v, e, f, r, seamless),
        "{label}: counted exactly as before: {tally:?}"
    );
    assert_eq!(
        (
            tally.void_edges,
            tally.void_vertices,
            tally.same_way_shared_edges
        ),
        (0, 0, 0),
        "{label}: nothing left out: {tally:?}"
    );
    let result = validate_model_enhanced(m, Tolerance::default(), ValidationLevel::Standard);
    assert!(result.errors.is_empty(), "{label}: {:?}", result.errors);
    (tally, seams)
}

/// The primitives with a seam -- one loop walking the closing edge twice --
/// and the others: the count is what it was, edge for edge.
#[test]
fn seamed_and_plain_primitives_are_counted_exactly_as_before() {
    let origin = Point3::new(0.0, 0.0, 0.0);

    let mut m = BRepModel::new();
    let s = solid(
        TopologyBuilder::new(&mut m)
            .create_cylinder_3d(origin, Vector3::Z, 5.0, 12.0)
            .unwrap(),
    );
    let (tally, seams) = assert_counted_as_before("cylinder", &m, s);
    assert!(
        seams >= 1,
        "the cylinder's wall is a seamed face (one loop walks its seam twice), or this test \
         does not test a seam: {tally:?}"
    );
    assert_eq!(tally.characteristic(), 2, "cylinder: genus 0: {tally:?}");

    let mut m = BRepModel::new();
    let s = solid(
        TopologyBuilder::new(&mut m)
            .create_cone_3d(origin, Vector3::Z, 6.0, 2.0, 9.0)
            .unwrap(),
    );
    let (tally, seams) = assert_counted_as_before("cone frustum", &m, s);
    assert!(seams >= 1, "the cone's wall is a seamed face: {tally:?}");
    assert_eq!(tally.characteristic(), 2, "cone: genus 0: {tally:?}");

    let mut m = BRepModel::new();
    let s = solid(
        TopologyBuilder::new(&mut m)
            .create_torus_3d(origin, Vector3::Z, 20.0, 4.0)
            .unwrap(),
    );
    let (tally, _) = assert_counted_as_before("torus", &m, s);
    assert_eq!(tally.characteristic() % 2, 0, "torus: {tally:?}");

    let mut m = BRepModel::new();
    let s = solid(
        TopologyBuilder::new(&mut m)
            .create_sphere_3d(origin, 7.0)
            .unwrap(),
    );
    let (tally, _) = assert_counted_as_before("sphere", &m, s);
    assert_eq!(tally.characteristic() % 2, 0, "sphere: {tally:?}");

    let mut m = BRepModel::new();
    let s = solid(
        TopologyBuilder::new(&mut m)
            .create_box_3d(4.0, 5.0, 6.0)
            .unwrap(),
    );
    let (tally, seams) = assert_counted_as_before("box", &m, s);
    assert_eq!(seams, 0);
    assert_eq!(tally.characteristic(), 2, "box: genus 0: {tally:?}");
}

/// Plate 60 x 60 x 10 (z from 0 to 10) united with a boss of radius 15 and
/// height 20 standing on z = 0, then bored through by a cylinder of radius 8
/// whose axis is 10 off the boss's: the bore's rim crosses the boss's foot.
fn plate_and_boss_bored_off_axis() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let plate = solid(
        TopologyBuilder::new(&mut m)
            .create_box_3d(60.0, 60.0, 10.0)
            .unwrap(),
    );
    translate(
        &mut m,
        vec![plate],
        Vector3::Z,
        5.0,
        TransformOptions::default(),
    )
    .unwrap();
    let boss = solid(
        TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 15.0, 20.0)
            .unwrap(),
    );
    let united = boolean_operation(
        &mut m,
        plate,
        boss,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("union");
    let bore = solid(
        TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(10.0, 0.0, -3.0), Vector3::Z, 8.0, 26.0)
            .unwrap(),
    );
    let bored = boolean_operation(
        &mut m,
        united,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("difference");
    (m, bored)
}

/// The first face of `solid` that has void edges: (face, its loop walks of
/// each void edge as (loop id, position in the loop)).
fn face_with_void_edges(m: &BRepModel, solid: SolidId) -> Option<(u32, Vec<u32>)> {
    let shell = m.shells.get(m.solids.get(solid)?.outer_shell)?;
    shell.faces.iter().find_map(|&fid| {
        let boundary = face_boundary(m, m.faces.get(fid)?);
        (!boundary.void_edges.is_empty()).then_some((fid, boundary.void_edges))
    })
}

fn euler_errors(m: &BRepModel) -> Vec<ValidationError> {
    validate_model_enhanced(m, Tolerance::default(), ValidationLevel::Standard)
        .errors
        .into_iter()
        .filter(|e| format!("{e:?}").contains("Euler"))
        .collect()
}

/// The regression the red gate caught (`f7_straddling_offset_{10,12,14}`):
/// a correct solid whose hole is written as two loops sharing an arc.
#[test]
fn a_hole_written_as_two_loops_sharing_edges_passes_the_euler_count() {
    let (m, bored) = plate_and_boss_bored_off_axis();

    // The structure this test is about is really there. If the boolean
    // stops writing the hole this way, this fixture no longer tests the
    // count and must be replaced, not left passing for another reason.
    let (face, void) = face_with_void_edges(&m, bored)
        .expect("the plate's top face carries its hole as two inner loops sharing edges");
    assert_eq!(
        void.len(),
        2,
        "face {face}: the two loops share the foot-circle arc inside the bore: {void:?}"
    );

    let tally = euler_tally(&m, m.solids.get(bored).expect("solid"));
    assert_eq!(
        (tally.void_edges, tally.void_vertices),
        (2, 1),
        "the arc's two edges and the vertex between them are left out: {tally:?}"
    );
    assert_eq!(
        tally.characteristic(),
        0,
        "one through bore: genus 1, 2(S - G) = 0: {tally:?}"
    );

    let result = validate_model_enhanced(&m, Tolerance::default(), ValidationLevel::Standard);
    assert!(
        euler_errors(&m).is_empty(),
        "a correct solid is rejected by the Euler-Poincare count: {:?}",
        result.errors
    );
    assert!(
        result.errors.is_empty(),
        "and it has no other validation error: {:?}",
        result.errors
    );
}

/// The same solid with one of the two loops made to walk the shared arc the
/// SAME way as the other. That is not the void case and must not be excused
/// as one: the edges stay counted and the solid fails the count, as it did
/// before the void rule existed.
#[test]
fn the_same_edges_walked_the_same_way_still_fail_the_euler_count() {
    let (mut m, bored) = plate_and_boss_bored_off_axis();
    let (face, void) = face_with_void_edges(&m, bored).expect("the face with the shared arc");
    // Flip the shared edges' direction in the LAST inner loop that walks
    // them (the crescent).
    let crescent = *m
        .faces
        .get(face)
        .expect("face")
        .inner_loops
        .last()
        .expect("inner loops");
    let lp = m.loops.get_mut(crescent).expect("loop");
    let mut flipped = 0;
    for i in 0..lp.edges.len() {
        if void.contains(&lp.edges[i]) {
            lp.orientations[i] = !lp.orientations[i];
            flipped += 1;
        }
    }
    assert_eq!(flipped, 2, "the crescent walks both shared edges");

    let tally = euler_tally(&m, m.solids.get(bored).expect("solid"));
    assert_eq!(
        (
            tally.void_edges,
            tally.void_vertices,
            tally.same_way_shared_edges
        ),
        (0, 0, 2),
        "walked the same way they are not void, and are named: {tally:?}"
    );
    assert!(
        tally.characteristic() % 2 != 0,
        "nothing is left out, and the count is odd: {tally:?}"
    );
    assert!(
        !euler_errors(&m).is_empty(),
        "and the solid is rejected by the Euler-Poincare count"
    );
}
