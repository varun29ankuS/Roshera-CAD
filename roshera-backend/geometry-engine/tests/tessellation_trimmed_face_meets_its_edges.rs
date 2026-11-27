// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A trimmed face on a periodic surface is meshed right when two things
//! hold: the mesh's boundary is exactly the samples of the face's own edges
//! (so it meets its neighbours, which mesh from the same samples), and the
//! mesh covers the face's area and no more. Both are checked by one helper,
//! [`assert_meets_its_edges_and_has_area`], on two faces that once failed
//! them:
//!
//! * a small patch of a torus, claimed by a mesher built for something else
//!   (CADGenBench part 136);
//! * a sector of a cone that includes the apex, whose outline was unwrapped
//!   the wrong way round the pole (CADGenBench part 244).

use geometry_engine::math::nurbs::{interpolate_nurbs_curve, ParameterizationType};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::curve::{Curve, Line, NurbsCurve, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
use geometry_engine::primitives::face::{Face, FaceOrientation};
use geometry_engine::primitives::r#loop::{Loop, LoopType};
use geometry_engine::primitives::shell::{Shell, ShellType};
use geometry_engine::primitives::surface::{Cone, Surface, Torus};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::tessellation::edge_cache::EdgeSampleCache;
use geometry_engine::tessellation::{tessellate_shell, TessellationParams, TriangleMesh};
use std::collections::{BTreeMap, BTreeSet};

// ───────────────────────────── shared fixture parts ───────────────────────────

/// A cubic curve through `points`, with its parameter range.
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

/// A one-face open shell on `surface`: the face's outer loop is `parts` in
/// order, each (start vertex, end vertex, curve, range), walked forward.
/// Returns the shell and the edge ids in loop order.
fn one_face_shell(
    model: &mut BRepModel,
    surface: u32,
    parts: Vec<(u32, u32, Box<dyn Curve>, ParameterRange)>,
) -> (Shell, Vec<u32>) {
    let mut outer = Loop::new(0, LoopType::Outer);
    let mut edges = Vec::new();
    for (start, end, curve, range) in parts {
        let curve = model.curves.add(curve);
        let edge = model.edges.add(Edge::new(
            0,
            start,
            end,
            curve,
            EdgeOrientation::Forward,
            range,
        ));
        outer.add_edge(edge, true);
        edges.push(edge);
    }
    let outer = model.loops.add(outer);
    let face = model
        .faces
        .add(Face::new(0, surface, outer, FaceOrientation::Forward));
    let mut shell = Shell::new(0, ShellType::Open);
    shell.add_face(face);
    (shell, edges)
}

/// THE check, shared by every face in this file. Meshes `shell` (one face
/// bounded by `edges`) at the coarse setting and asserts:
///
/// 1. the mesh is not empty;
/// 2. the mesh's boundary -- the facet edges used by exactly one facet -- is
///    exactly the set of segments between consecutive samples of `edges`,
///    taken from the same sample cache the mesher used. A missing segment is
///    an open edge against the neighbour; an extra one is a hole or a facet
///    that left the face;
/// 3. the facets' total area is within `area_tolerance` (a fraction) of
///    `true_area`, so the mesh is the face and not a larger or smaller piece
///    of the surface that happens to share its outline.
fn assert_meets_its_edges_and_has_area(
    label: &str,
    model: &BRepModel,
    shell: &Shell,
    edges: &[u32],
    true_area: f64,
    area_tolerance: f64,
) {
    let params = TessellationParams::coarse();
    let cache = EdgeSampleCache::new(&params);
    let mut mesh = TriangleMesh::new();
    tessellate_shell(shell, model, &params, &cache, &mut mesh);
    assert!(!mesh.triangles.is_empty(), "{label}: the face is meshed");

    // Points are compared at 0.1 micrometre: the mesh's boundary vertices
    // are the cache's samples themselves, not recomputed.
    let key = |p: Point3| {
        (
            (p.x * 1e7).round() as i64,
            (p.y * 1e7).round() as i64,
            (p.z * 1e7).round() as i64,
        )
    };
    type Key = (i64, i64, i64);
    let mut uses: BTreeMap<(Key, Key), usize> = BTreeMap::new();
    let mut area = 0.0;
    for t in &mesh.triangles {
        let [a, b, c] = t.map(|i| mesh.vertices[i as usize].position);
        area += 0.5 * (b - a).cross(&(c - a)).magnitude();
        for (p, q) in [(a, b), (b, c), (c, a)] {
            let (x, y) = (key(p), key(q));
            // A facet side of no length (two corners on a pole) is not a
            // boundary of anything.
            if x != y {
                *uses.entry((x.min(y), x.max(y))).or_default() += 1;
            }
        }
    }
    let mesh_boundary: BTreeSet<(Key, Key)> = uses
        .iter()
        .filter(|(_, &n)| n == 1)
        .map(|(e, _)| *e)
        .collect();
    let mut edge_segments = BTreeSet::new();
    for &edge in edges {
        let samples = cache.get_or_compute(edge, model);
        for pair in samples.windows(2) {
            let (x, y) = (key(pair[0]), key(pair[1]));
            edge_segments.insert((x.min(y), x.max(y)));
        }
    }
    assert_eq!(
        mesh_boundary,
        edge_segments,
        "{label}: the mesh's boundary is the edges' samples ({} mesh boundary edges against {} \
         sample segments)",
        mesh_boundary.len(),
        edge_segments.len()
    );
    assert!(
        (area - true_area).abs() < area_tolerance * true_area,
        "{label}: it covers the face, no more and no less: {area} against {true_area}"
    );
}

// ──────────────────────────── a small torus patch ─────────────────────────────
//
// A torus face with one outer loop was handed to the mesher built for a
// boolean's rim-poke bump (a grid of the whole torus kept where it is inside
// the face, stitched to the cut oval). On a small imported patch that stitch
// does not find the outline: part 136 got 3421 facets on a patch
// 0.02 x 0.04 rad, meeting none of the face's edges. That mesher's result is
// now kept only if its boundary is the face's outline.

const MAJOR: f64 = 20.0;
const MINOR: f64 = 4.0;

/// The corners of the patch in the torus's (u, v): a triangle with two sides
/// along the parameter lines and one across them.
const CORNERS: [(f64, f64); 3] = [(0.10, 0.20), (0.12, 0.20), (0.12, 0.24)];

fn torus_patch() -> (BRepModel, Shell, Vec<u32>) {
    let mut model = BRepModel::new();
    let torus = Torus::new(Point3::new(0.0, 0.0, 0.0), Vector3::Z, MAJOR, MINOR).expect("torus");
    let side = |a: (f64, f64), b: (f64, f64)| -> Vec<Point3> {
        (0..=32)
            .map(|k| {
                let t = k as f64 / 32.0;
                torus
                    .point_at(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
                    .expect("point")
            })
            .collect()
    };
    let sides: Vec<Vec<Point3>> = (0..3)
        .map(|k| side(CORNERS[k], CORNERS[(k + 1) % 3]))
        .collect();
    let surface = model.surfaces.add(Box::new(torus));
    let vertices: Vec<u32> = sides
        .iter()
        .map(|s| model.vertices.add(s[0].x, s[0].y, s[0].z))
        .collect();
    let parts = sides
        .iter()
        .enumerate()
        .map(|(k, points)| {
            let (curve, range) = through(points);
            (vertices[k], vertices[(k + 1) % 3], curve, range)
        })
        .collect();
    let (shell, edges) = one_face_shell(&mut model, surface, parts);
    (model, shell, edges)
}

/// The patch's true area: the torus's area element r (R + r cos v)
/// integrated over the (u, v) triangle -- for each u, v runs from the bottom
/// side up to the slanted one.
fn torus_patch_area() -> f64 {
    let (a, b, c) = (CORNERS[0], CORNERS[1], CORNERS[2]);
    let n = 4000;
    let mut sum = 0.0;
    for i in 0..n {
        let u = a.0 + (b.0 - a.0) * (i as f64 + 0.5) / n as f64;
        let top = a.1 + (c.1 - a.1) * (u - a.0) / (c.0 - a.0);
        sum += MINOR * (MAJOR * (top - a.1) + MINOR * (top.sin() - a.1.sin()));
    }
    sum * (b.0 - a.0) / n as f64
}

/// RED without the outline check on the rim-poke mesher's result
/// (`patch_boundary_is_the_outline`): the patch comes back with no facets.
#[test]
fn a_small_torus_patch_meets_its_edges_and_has_its_own_area() {
    let (model, shell, edges) = torus_patch();
    assert_meets_its_edges_and_has_area(
        "torus patch",
        &model,
        &shell,
        &edges,
        torus_patch_area(),
        0.02,
    );
}

// ─────────────────────── a cone sector that has the apex ──────────────────────
//
// A drill point is a cone written as two halves, each bounded by two straight
// sides meeting at the apex and half of the rim. The apex is a pole of the
// cone's chart: its angle is arbitrary. The step across it was taken "the
// nearer way round", which is wrong for a sector wider than half a turn and
// undetermined at exactly half; a sector read the wrong way was unwrapped a
// whole turn out and meshed by an untrimmed grid that met none of its
// neighbours (part 244: nine drill points, 27 faces with open edges). The
// samples off the pole are now unwrapped against each other and the loop's
// closing decides which way round the pole went.

const HALF_ANGLE: f64 = 1.0297442586766545; // 59 degrees: a 118-degree drill point
const HEIGHT: f64 = 3.0;

/// A sector of a cone with its apex at the origin opening along +Z, between
/// the azimuths `from` and `from + sweep`: the model, its shell, its edges
/// (side down, side up, rim) and the rim radius.
fn cone_sector(from: f64, sweep: f64) -> (BRepModel, Shell, Vec<u32>, f64) {
    let mut model = BRepModel::new();
    let apex_point = Point3::new(0.0, 0.0, 0.0);
    let cone = Cone::new(apex_point, Vector3::Z, HALF_ANGLE).expect("cone");
    let surface = model.surfaces.add(Box::new(cone));
    let radius = HEIGHT * HALF_ANGLE.tan();
    let rim = |a: f64| Point3::new(radius * a.cos(), radius * a.sin(), HEIGHT);
    let (start, end) = (rim(from), rim(from + sweep));
    let apex = model.vertices.add(0.0, 0.0, 0.0);
    let v_start = model.vertices.add(start.x, start.y, start.z);
    let v_end = model.vertices.add(end.x, end.y, end.z);
    let arc: Vec<Point3> = (0..=64)
        .map(|k| rim(from + sweep * k as f64 / 64.0))
        .collect();
    let (arc_curve, arc_range) = through(&arc);
    let unit = ParameterRange::new(0.0, 1.0);
    // Walked as the file walks it: down one side to the apex, up the other,
    // round the rim -- so the apex sample follows a sample at the far
    // azimuth.
    let parts: Vec<(u32, u32, Box<dyn Curve>, ParameterRange)> = vec![
        (v_end, apex, Box::new(Line::new(end, apex_point)), unit),
        (apex, v_start, Box::new(Line::new(apex_point, start)), unit),
        (v_start, v_end, arc_curve, arc_range),
    ];
    let (shell, edges) = one_face_shell(&mut model, surface, parts);
    (model, shell, edges, radius)
}

fn assert_cone_sector_meets_its_edges(from: f64, sweep: f64) {
    let (model, shell, edges, radius) = cone_sector(from, sweep);
    // The sector's lateral area: sweep r l / 2.
    let slant = (radius * radius + HEIGHT * HEIGHT).sqrt();
    assert_meets_its_edges_and_has_area(
        &format!("cone sector from {from} sweeping {sweep}"),
        &model,
        &shell,
        &edges,
        0.5 * sweep * radius * slant,
        0.03,
    );
}

/// Regression test for the unwrap across the pole
/// (`unwrap_past_pole_samples`): wider than half a turn, the nearer way
/// round the apex is the wrong one, and only the loop's closing says so.
/// RED with the unwrap not called.
#[test]
fn a_cone_sector_over_half_a_turn_meets_its_edges() {
    assert_cone_sector_meets_its_edges(1.5, 3.4);
    assert_cone_sector_meets_its_edges(0.3, 4.5);
}

/// Regression test for the same unwrap: a sector narrower than half a turn.
/// RED with the unwrap not called (the outline is then unwrapped against
/// the apex sample's arbitrary angle and the whole cone is meshed). It does
/// not need the closing repair: the nearer way round the apex is the right
/// one here.
#[test]
fn a_cone_sector_under_half_a_turn_meets_its_edges() {
    assert_cone_sector_meets_its_edges(0.4, 2.0);
}

/// COVERAGE, not a regression test: it passes with the unwrap across the
/// pole on or off. Exactly half a turn (the drill point's two halves),
/// either side of azimuth zero. At exactly half, "the nearer way round" is
/// a coin toss on rounding; in this fixture it happens to fall the right
/// way either way, so this does not prove the fix -- the two tests above
/// do. Kept because it is the shape part 244 actually has.
#[test]
fn coverage_half_a_cone_meets_its_edges() {
    let quarter = std::f64::consts::FRAC_PI_2;
    assert_cone_sector_meets_its_edges(-quarter, 2.0 * quarter);
    assert_cone_sector_meets_its_edges(quarter, 2.0 * quarter);
}
