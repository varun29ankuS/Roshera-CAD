//! Charts that are unlike their surface, and what the mesher does about the
//! facets a sound chart triangulation then turns over:
//!
//! * a point that is on one side of a row of facets in the chart and on the
//!   other side of them on the surface is walked out from under them
//!   ([`walk_points_out_from_under_facets`]);
//! * a face that tapers to a point is given a chart of its own shape
//!   ([`TaperedChart`]), tried where the established chart fails
//!   ([`prefer_tapered_chart`]).

use super::*;
use crate::math::nurbs::NurbsSurface;
use crate::primitives::curve::{Curve, NurbsCurve, ParameterRange};
use crate::primitives::edge::{Edge, EdgeOrientation};
use crate::primitives::face::FaceOrientation;
use crate::primitives::r#loop::Loop;
use crate::primitives::surface::GeneralNurbsSurface;

// ───────────────────────── a chart bowed within its surface ───────────────────

/// The plane z = 0 seen through a chart whose lines of constant `y` are
/// parabolas on it: chart `(x, y)` is the point `(x, y + BOW x^2, 0)`. The
/// surface is flat, so no facet can stand off it; the chart is bent within
/// it, which is all that is being tested.
struct BowedPlane;

const BOW: f64 = 0.4;

impl MeshChart for BowedPlane {
    fn evaluate_full(&self, x: f64, y: f64) -> MathResult<SurfacePoint> {
        Ok(SurfacePoint {
            position: MeshChart::point_at(self, x, y)?,
            du: Vector3::new(1.0, 2.0 * BOW * x, 0.0),
            dv: Vector3::new(0.0, 1.0, 0.0),
            duu: Vector3::new(0.0, 2.0 * BOW, 0.0),
            duv: Vector3::ZERO,
            dvv: Vector3::ZERO,
            normal: Vector3::Z,
            k1: 0.0,
            k2: 0.0,
            dir1: Vector3::X,
            dir2: Vector3::Y,
        })
    }
    fn point_at(&self, x: f64, y: f64) -> MathResult<Point3> {
        Ok(Point3::new(x, y + BOW * x * x, 0.0))
    }
    fn normal_at(&self, _x: f64, _y: f64) -> MathResult<Vector3> {
        Ok(Vector3::Z)
    }
    fn is_developable(&self) -> bool {
        false
    }
    fn type_name(&self) -> &'static str {
        "BowedPlane"
    }
    fn native_uv(&self, x: f64, y: f64) -> (f64, f64) {
        (x, y)
    }
    fn rulings_along_v(&self) -> bool {
        false
    }
}

/// A face on the bowed plane: the chart rectangle 0 <= x <= 1, -1 <= y <= 1,
/// its left side carrying three samples close together (points 5, 6, 7 at
/// y = 0, -0.1, -0.2) and its right side one in the middle (point 2, at
/// y = 0). One interior point, 8, sits just ABOVE the chart line from point
/// 5 to point 2.
///
/// On the surface point 2 is lifted by the bow (to y = 0.4) and the points
/// of the left side are not, so the facet sides fanning from point 2 to the
/// left side slope down to the left -- and point 8, lifted only a quarter as
/// much, lies BELOW the first two of them: under two facets of the fan.
fn fan_with_a_point_under_it() -> (ProjectedLoop, Vec<(f64, f64)>, Vec<[usize; 3]>, Face) {
    let chart = BowedPlane;
    let boundary: Vec<(f64, f64)> = vec![
        (0.0, -1.0), // 0
        (1.0, -1.0), // 1
        (1.0, 0.0),  // 2: the fan's hub
        (1.0, 1.0),  // 3
        (0.0, 1.0),  // 4
        (0.0, 0.0),  // 5
        (0.0, -0.1), // 6
        (0.0, -0.2), // 7
    ];
    let outer = ProjectedLoop {
        points_3d: boundary
            .iter()
            .map(|&(x, y)| MeshChart::point_at(&chart, x, y).expect("point"))
            .collect(),
        points_uv: boundary.clone(),
        loop_type: LoopType::Outer,
    };
    let mut pts2d = boundary;
    pts2d.push((0.5, 0.02)); // 8
                             // Counter-clockwise in the chart, every one.
    let triangles = vec![
        [5, 2, 8], // the facet point 8 is the apex of: turned on the surface
        [2, 3, 8],
        [3, 4, 8],
        [4, 5, 8],
        [2, 5, 6], // the fan from point 2 down the left side
        [2, 6, 7],
        [2, 7, 0],
        [2, 0, 1],
    ];
    let face = Face::new(0, 0, 0, FaceOrientation::Forward);
    (outer, pts2d, triangles, face)
}

fn facings(
    triangles: &[[usize; 3]],
    pts2d: &[(f64, f64)],
    outer: &ProjectedLoop,
    face: &Face,
) -> Vec<f64> {
    let mut cache = FidelityCache::default();
    triangles
        .iter()
        .map(|&t| {
            facet_facing(t, pts2d, outer, &[], &BowedPlane, face, 1, &mut cache).expect("facing")
        })
        .collect()
}

/// The fixture is what it says: sound in the chart, one facet turned on the
/// surface, and a single swap of that facet's long side still leaves one
/// turned (the point is under the next facet of the fan too).
#[test]
fn the_point_is_under_two_facets_of_the_fan() {
    let (outer, pts2d, triangles, face) = fan_with_a_point_under_it();
    for t in &triangles {
        let (a, b, c) = (pts2d[t[0]], pts2d[t[1]], pts2d[t[2]]);
        assert!(
            (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0) > 0.0,
            "{t:?} is counter-clockwise in the chart"
        );
    }
    let before = facings(&triangles, &pts2d, &outer, &face);
    assert_eq!(
        before.iter().filter(|&&f| f < 0.0).count(),
        1,
        "one facet turned on the surface: {before:?}"
    );
    // One swap of the side from 5 to 2: the facets (8, 5, 6) and (8, 6, 2).
    let mut swapped = triangles.clone();
    swapped[0] = [8, 5, 6];
    swapped[4] = [8, 6, 2];
    let after = facings(&swapped, &pts2d, &outer, &face);
    assert_eq!(
        after.iter().filter(|&&f| f < 0.0).count(),
        1,
        "still one turned after the one swap: {after:?}"
    );
}

/// The point is walked out from under the fan: two swaps, each leaving less
/// turned over than before, and none turned at the end. The same eight
/// facets' worth of points; nothing added, nothing removed; no facet left
/// standing edge-on.
#[test]
fn a_point_under_two_facets_of_a_fan_is_walked_out() {
    let (outer, pts2d, triangles, face) = fan_with_a_point_under_it();
    let mut cache = FidelityCache::default();
    let (walked, turned) = flip_edges_under_turned_facets(
        &outer,
        &[],
        &pts2d,
        triangles.clone(),
        &BowedPlane,
        &face,
        1,
        &mut cache,
    );
    assert_eq!(turned, 0, "no facet is left turned: {walked:?}");
    assert_eq!(walked.len(), triangles.len(), "the same number of facets");
    let after = facings(&walked, &pts2d, &outer, &face);
    assert!(
        after.iter().all(|&f| f >= FLIPPED_FACET_MIN_FACING),
        "every facet faces the surface squarely: {after:?}"
    );
    // The area is the face's, as before: the facets still tile it.
    let area = |tris: &[[usize; 3]]| -> f64 {
        tris.iter()
            .map(|t| {
                let p = |i: usize| {
                    resolve_position_3d(i, &outer, &[], &pts2d, &BowedPlane).expect("position")
                };
                0.5 * (p(t[1]) - p(t[0])).cross(&(p(t[2]) - p(t[0]))).magnitude()
            })
            .sum()
    };
    assert!(
        (area(&walked) - 2.0).abs() < 1e-12,
        "the facets cover the face (area 2) exactly once: {}",
        area(&walked)
    );
}

// ─────────────────────────────── a face that tapers ───────────────────────────

/// A clamped uniform knot vector for `count` control points of `degree`.
fn clamped_knots(count: usize, degree: usize) -> Vec<f64> {
    let spans = count - degree;
    let mut knots = vec![0.0; degree + 1];
    knots.extend((1..spans).map(|k| k as f64 / spans as f64));
    knots.extend(std::iter::repeat(1.0).take(degree + 1));
    knots
}

/// The control grid of a fan: row 0 is the apex five times over; the rows
/// after it lie at growing distance on a cone about +Z, spanning 1.2 rad.
/// With `collapsed` false the first row is given the width of the second,
/// so the patch is an ordinary four-sided one.
fn fan_grid(collapsed: bool) -> Vec<Vec<Point3>> {
    let (rows, columns, reach, half_angle, sweep) = (6usize, 5usize, 4.0f64, 0.5f64, 1.2f64);
    (0..rows)
        .map(|i| {
            let s = i as f64 / (rows - 1) as f64;
            let distance = if collapsed {
                reach * s
            } else {
                reach * (0.5 + 0.5 * s)
            };
            (0..columns)
                .map(|j| {
                    let angle = sweep * j as f64 / (columns - 1) as f64;
                    Point3::new(
                        distance * half_angle.sin() * angle.cos(),
                        distance * half_angle.sin() * angle.sin(),
                        distance * half_angle.cos(),
                    )
                })
                .collect()
        })
        .collect()
}

fn patch(grid: Vec<Vec<Point3>>) -> GeneralNurbsSurface {
    let weights = vec![vec![1.0; grid[0].len()]; grid.len()];
    let (knots_u, knots_v) = (
        clamped_knots(grid.len(), 2),
        clamped_knots(grid[0].len(), 2),
    );
    GeneralNurbsSurface {
        nurbs: NurbsSurface::new(grid, weights, knots_u, knots_v, 2, 2).expect("patch"),
    }
}

/// The corners of the whole patch in its own chart.
const WHOLE_PATCH: [(f64, f64); 4] = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];

/// In its tapered chart the fan is a wedge: a step across it near the point
/// is as long in the chart as it is on the surface, where in the patch's
/// own chart the same step across is the same length at the point as at
/// the rim.
#[test]
fn a_fan_is_a_wedge_in_its_tapered_chart() {
    let surface = patch(fan_grid(true));
    let chart = TaperedChart::for_face(&surface, &WHOLE_PATCH).expect("a fan tapers");
    assert!(chart.along_u, "it runs along u, from the point to the rim");
    let on_surface = |a: (f64, f64), b: (f64, f64)| {
        (Surface::point_at(&surface, b.0, b.1).expect("b")
            - Surface::point_at(&surface, a.0, a.1).expect("a"))
        .magnitude()
    };
    let in_chart = |a: (f64, f64), b: (f64, f64)| {
        let (p, q) = (chart.to_chart(a), chart.to_chart(b));
        (q.0 - p.0).hypot(q.1 - p.1)
    };
    // Steps across (in v) and along (in u), near the point and at the rim.
    for (a, b) in [
        ((0.05, 0.40), (0.05, 0.45)),
        ((0.95, 0.40), (0.95, 0.45)),
        ((0.05, 0.50), (0.07, 0.50)),
        ((0.90, 0.50), (0.92, 0.50)),
    ] {
        let ratio = in_chart(a, b) / on_surface(a, b);
        assert!(
            (0.5..2.0).contains(&ratio),
            "the step {a:?} -> {b:?} is {ratio} times as long in the chart as on the surface"
        );
    }
    // In the patch's own chart the two steps across are the same length;
    // on the surface they differ by the fan's taper.
    let taper = on_surface((0.95, 0.40), (0.95, 0.45)) / on_surface((0.05, 0.40), (0.05, 0.45));
    assert!(taper > 10.0, "the fixture tapers: {taper}");
    // And the chart is a chart: every point comes back from it.
    for &(u, v) in &[(0.02, 0.1), (0.3, 0.9), (0.77, 0.33), (1.0, 1.0)] {
        let (x, y) = chart.to_chart((u, v));
        let (u2, v2) = chart.native_uv(x, y);
        assert!(
            (u2 - u).abs() < 1e-9 && (v2 - v).abs() < 1e-9,
            "({u}, {v}) comes back as ({u2}, {v2})"
        );
    }
}

/// A four-sided patch of nearly even width has no tapered chart: its own
/// chart, or the rescaled one, already has its shape.
#[test]
fn a_face_of_even_width_has_no_tapered_chart() {
    let surface = patch(fan_grid(false));
    assert!(TaperedChart::for_face(&surface, &WHOLE_PATCH).is_none());
}

/// The fan as a face: bounded by its two sides and its rim.
fn fan_face() -> (BRepModel, u32) {
    let grid = fan_grid(true);
    let (rows, columns) = (grid.len(), grid[0].len());
    let mut model = BRepModel::new();
    let out: Vec<Point3> = (0..rows).map(|i| grid[i][0]).collect();
    let rim: Vec<Point3> = grid[rows - 1].clone();
    let back: Vec<Point3> = (0..rows).rev().map(|i| grid[i][columns - 1]).collect();
    let surface = model.surfaces.add(Box::new(patch(grid)));
    let sides = [out, rim, back];
    let corners: Vec<u32> = sides
        .iter()
        .map(|points| model.vertices.add(points[0].x, points[0].y, points[0].z))
        .collect();
    let mut outer = Loop::new(0, LoopType::Outer);
    for (k, points) in sides.into_iter().enumerate() {
        let knots = clamped_knots(points.len(), 2);
        let weights = vec![1.0; points.len()];
        let curve: Box<dyn Curve> =
            Box::new(NurbsCurve::new(2, points, weights, knots).expect("side"));
        let curve = model.curves.add(curve);
        let edge = model.edges.add(Edge::new(
            0,
            corners[k],
            corners[(k + 1) % 3],
            curve,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ));
        outer.add_edge(edge, true);
    }
    let outer = model.loops.add(outer);
    let face = model
        .faces
        .add(Face::new(0, surface, outer, FaceOrientation::Forward));
    (model, face)
}

/// What [`prefer_tapered_chart`] hands back for the fan face given an
/// established attempt with this outcome and an empty mesh.
fn fan_after(established: Result<usize, CurvedCdtError>) -> (usize, Option<usize>) {
    let (model, face) = fan_face();
    let face = model.faces.get(face).expect("face");
    let surface = model.surfaces.get(face.surface_id).expect("surface");
    let params = TessellationParams::coarse();
    let cache = EdgeSampleCache::new(&params);
    let (outer, inners, _) =
        run_boundary_projection(face, &model, &cache, surface).expect("projection");
    let all_uv = outer.points_uv.clone();
    let (mesh, outcome, attempt) = prefer_tapered_chart(
        surface,
        face,
        &params,
        Some((outer, inners)),
        &all_uv,
        TriangleMesh::new(),
        established,
        ChartAttempt::default(),
    );
    (
        mesh.triangles.len(),
        outcome.ok().map(|turned| least_turned(turned, &attempt)),
    )
}

/// Where the established chart could not mesh the face at all, the tapered
/// one is tried and its mesh kept.
#[test]
fn a_fan_its_established_chart_could_not_mesh_is_meshed_in_its_tapered_chart() {
    let (facets, turned) = fan_after(Err(CurvedCdtError::ProjectionFailed));
    assert!(facets > 0, "the tapered chart's mesh is kept");
    assert_eq!(turned, Some(0), "and it has no facet turned");
}

/// Where the established chart left facets turned, the tapered one is
/// tried and kept because it leaves fewer.
#[test]
fn a_fan_its_established_chart_left_turned_facets_on_is_meshed_in_its_tapered_chart() {
    let (facets, turned) = fan_after(Ok(3));
    assert!(facets > 0, "the tapered chart's mesh is kept");
    assert_eq!(
        turned,
        Some(0),
        "with none turned against the established three"
    );
}

/// Where the established chart meshed the face cleanly, it stands: the
/// tapered chart is not tried. (Here the "established" mesh handed in is
/// empty, so an empty mesh coming back shows it was left alone.)
#[test]
fn a_face_its_established_chart_meshed_cleanly_is_left_alone() {
    let (facets, turned) = fan_after(Ok(0));
    assert_eq!((facets, turned), (0, Some(0)));
}

// ─────────────────────── a coordinate too small to triangulate ────────────────

/// A boundary point a denormal off a chart axis (here 1e-60 off y = 0) is
/// on the axis. The triangulator refuses any non-zero coordinate below
/// 2^-142 outright, which refused the whole face.
#[test]
fn a_coordinate_a_denormal_off_zero_is_triangulated_as_zero() {
    let pts = [(0.0, -1.0e-60), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
    let segments = [[0, 1], [1, 2], [2, 3], [3, 0]];
    let (triangles, missing) = triangulate_segments(&pts, &segments).expect("triangulated");
    assert_eq!(triangles.len(), 2, "the square is two triangles");
    assert!(missing.is_empty(), "every boundary segment is kept");
}
