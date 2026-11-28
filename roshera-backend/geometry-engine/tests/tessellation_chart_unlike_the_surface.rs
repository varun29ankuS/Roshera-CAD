// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! COVERAGE of two kinds of face whose chart is unlike the surface, meshed
//! whole through the public path: no facet of the mesh may be wound against
//! the surface it lies on (the certificate's `off_surface_facets`), and the
//! mesh must meet the face's edges.
//!
//! * A FAN: a three-sided patch written as a four-sided NURBS with one side
//!   collapsed to a point, twisting as it leaves the point.
//! * A BOWED STRIP: a narrow face whose long parameter lines bow sideways
//!   within the surface.
//!
//! These two fixtures mesh soundly with the cures below switched on or off
//! (measured: identical facet counts either way), so they prove nothing
//! about them and are kept only so the ordinary cases cannot be broken. The
//! regression tests -- each RED with its cure off -- are in
//! `src/tessellation/curved_cdt_bent_chart_tests.rs`:
//! `a_point_under_two_facets_of_a_fan_is_walked_out` for the bowed chart
//! (CADGenBench part 240, face 406) and the `TaperedChart` tests for the
//! fan (part 240, face 905).

use geometry_engine::harness::watertight::tessellation_quality_of_mesh;
use geometry_engine::math::nurbs::NurbsSurface;
use geometry_engine::math::Point3;
use geometry_engine::primitives::curve::{Curve, NurbsCurve, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
use geometry_engine::primitives::face::{Face, FaceOrientation};
use geometry_engine::primitives::r#loop::{Loop, LoopType};
use geometry_engine::primitives::shell::{Shell, ShellType};
use geometry_engine::primitives::surface::GeneralNurbsSurface;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::tessellation::edge_cache::EdgeSampleCache;
use geometry_engine::tessellation::{tessellate_shell, TessellationParams, TriangleMesh};
use std::collections::{BTreeMap, BTreeSet};

/// A clamped uniform knot vector for `count` control points of `degree`.
fn clamped_knots(count: usize, degree: usize) -> Vec<f64> {
    let spans = count - degree;
    let mut knots = vec![0.0; degree + 1];
    knots.extend((1..spans).map(|k| k as f64 / spans as f64));
    knots.extend(std::iter::repeat(1.0).take(degree + 1));
    knots
}

/// A B-spline curve on the control points given: the boundary curve of a
/// clamped B-spline surface along one side of its control grid.
fn side(points: Vec<Point3>, degree: usize) -> (Box<dyn Curve>, ParameterRange) {
    let knots = clamped_knots(points.len(), degree);
    let weights = vec![1.0; points.len()];
    (
        Box::new(NurbsCurve::new(degree, points, weights, knots).expect("side curve")),
        ParameterRange::new(0.0, 1.0),
    )
}

/// A one-face open shell on the clamped B-spline surface through the
/// control `grid` (`grid[i][j]`: i along u, j along v), bounded by the
/// sides of the grid that have length. `sides`: each side to walk, in loop
/// order, as its control points from its start to its end.
fn patch_face(
    grid: Vec<Vec<Point3>>,
    degree_u: usize,
    degree_v: usize,
    sides: Vec<(Vec<Point3>, usize)>,
) -> (BRepModel, Shell, Vec<u32>) {
    let mut model = BRepModel::new();
    let weights = vec![vec![1.0; grid[0].len()]; grid.len()];
    let knots_u = clamped_knots(grid.len(), degree_u);
    let knots_v = clamped_knots(grid[0].len(), degree_v);
    let nurbs = NurbsSurface::new(grid, weights, knots_u, knots_v, degree_u, degree_v)
        .expect("patch surface");
    let surface = model.surfaces.add(Box::new(GeneralNurbsSurface { nurbs }));
    let corners: Vec<u32> = sides
        .iter()
        .map(|(points, _)| model.vertices.add(points[0].x, points[0].y, points[0].z))
        .collect();
    let count = sides.len();
    let mut outer = Loop::new(0, LoopType::Outer);
    let mut edges = Vec::new();
    for (k, (points, degree)) in sides.into_iter().enumerate() {
        let (curve, range) = side(points, degree);
        let curve = model.curves.add(curve);
        let edge = model.edges.add(Edge::new(
            0,
            corners[k],
            corners[(k + 1) % count],
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
    (model, shell, edges)
}

/// Mesh the face and return (facets, facets wound against the surface,
/// mesh boundary segments that are not edge-sample segments or the reverse).
fn mesh_and_judge(
    model: &BRepModel,
    shell: &Shell,
    edges: &[u32],
    params: &TessellationParams,
) -> (usize, usize, usize) {
    let cache = EdgeSampleCache::new(params);
    let mut mesh = TriangleMesh::new();
    tessellate_shell(shell, model, params, &cache, &mut mesh);
    let quality = tessellation_quality_of_mesh(model, &mesh);
    let off = quality.map_or(0, |q| q.off_surface_facets);

    let key = |p: Point3| {
        (
            (p.x * 1e7).round() as i64,
            (p.y * 1e7).round() as i64,
            (p.z * 1e7).round() as i64,
        )
    };
    type Key = (i64, i64, i64);
    let mut uses: BTreeMap<(Key, Key), usize> = BTreeMap::new();
    for t in &mesh.triangles {
        let [a, b, c] = t.map(|i| mesh.vertices[i as usize].position);
        for (p, q) in [(a, b), (b, c), (c, a)] {
            let (x, y) = (key(p), key(q));
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
            if x != y {
                edge_segments.insert((x.min(y), x.max(y)));
            }
        }
    }
    let mismatched = mesh_boundary.symmetric_difference(&edge_segments).count();
    (mesh.triangles.len(), off, mismatched)
}

// ─────────────────────────────────── the fan ──────────────────────────────────

/// A fan: every control point of row 0 is the apex; the rows after it lie on
/// a cone of half-angle `half_angle` about +Z at growing distance, each row
/// turned further round the axis than the last (`twist` radians per unit of
/// distance), spanning `sweep` radians. Returns the face and its edges
/// (out along one side, round the rim, back along the other).
fn twisted_fan(
    rows: usize,
    columns: usize,
    reach: f64,
    half_angle: f64,
    sweep: f64,
    twist: f64,
) -> (BRepModel, Shell, Vec<u32>) {
    let apex = Point3::new(0.0, 0.0, 0.0);
    let at = |i: usize, j: usize| {
        // Rows crowd toward the apex, as a blend patch's do.
        let distance = reach * (i as f64 / (rows - 1) as f64).powi(2);
        let angle = sweep * j as f64 / (columns - 1) as f64 + twist * distance;
        Point3::new(
            apex.x + distance * half_angle.sin() * angle.cos(),
            apex.y + distance * half_angle.sin() * angle.sin(),
            apex.z + distance * half_angle.cos(),
        )
    };
    let grid: Vec<Vec<Point3>> = (0..rows)
        .map(|i| (0..columns).map(|j| at(i, j)).collect())
        .collect();
    let out: Vec<Point3> = (0..rows).map(|i| grid[i][0]).collect();
    let rim: Vec<Point3> = grid[rows - 1].clone();
    let back: Vec<Point3> = (0..rows).rev().map(|i| grid[i][columns - 1]).collect();
    patch_face(grid, 2, 2, vec![(out, 2), (rim, 2), (back, 2)])
}

/// COVERAGE (see the file's header).
#[test]
fn coverage_a_twisted_fan_has_no_facet_wound_against_it() {
    let (model, shell, edges) = twisted_fan(9, 7, 4.0, 0.5, 1.2, 1.5);
    let (facets, off, mismatched) =
        mesh_and_judge(&model, &shell, &edges, &TessellationParams::coarse());
    assert!(facets > 0, "the fan is meshed");
    assert_eq!(mismatched, 0, "its boundary is its edges' samples");
    assert_eq!(off, 0, "no facet of {facets} is wound against the surface");
}

// ──────────────────────────────── the bowed strip ─────────────────────────────

/// A strip `length` long and `width` wide whose long parameter lines bow
/// sideways by `bow` over the last `bend` of its length, on a surface arched
/// `arch` high so it is not a plane. Returns the face and its four edges.
fn bowed_strip(
    rows: usize,
    columns: usize,
    length: f64,
    width: f64,
    bow: f64,
    bend: f64,
    arch: f64,
) -> (BRepModel, Shell, Vec<u32>) {
    let at = |i: usize, j: usize| {
        let s = i as f64 / (rows - 1) as f64;
        let x = length * s;
        // Straight until the last `bend` of the length, then swinging
        // sideways.
        let into_bend = ((s - (1.0 - bend)) / bend).max(0.0);
        let y = width * j as f64 / (columns - 1) as f64 + bow * into_bend * into_bend;
        // Arched along its length and tilted across it: not a plane.
        let z = arch * (std::f64::consts::PI * s).sin() + 0.2 * arch * y;
        Point3::new(x, y, z)
    };
    let grid: Vec<Vec<Point3>> = (0..rows)
        .map(|i| (0..columns).map(|j| at(i, j)).collect())
        .collect();
    let near: Vec<Point3> = (0..rows).map(|i| grid[i][0]).collect();
    let end: Vec<Point3> = grid[rows - 1].clone();
    let far: Vec<Point3> = (0..rows).rev().map(|i| grid[i][columns - 1]).collect();
    let start: Vec<Point3> = grid[0].iter().rev().copied().collect();
    patch_face(grid, 2, 1, vec![(near, 2), (end, 1), (far, 2), (start, 1)])
}

/// COVERAGE (see the file's header).
#[test]
fn coverage_a_bowed_strip_has_no_facet_wound_against_it() {
    let (model, shell, edges) = bowed_strip(17, 2, 4.0, 0.06, 0.4, 0.1, 0.5);
    let (facets, off, mismatched) =
        mesh_and_judge(&model, &shell, &edges, &TessellationParams::coarse());
    assert!(facets > 0, "the strip is meshed");
    assert_eq!(mismatched, 0, "its boundary is its edges' samples");
    assert_eq!(off, 0, "no facet of {facets} is wound against the surface");
}
