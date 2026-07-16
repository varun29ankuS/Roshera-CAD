//! 2D polygon-polygon clipping for coplanar boolean face handling.
//!
//! Thin shim over the [`i_overlay`] crate. Provides the two entry
//! points the kernel's coplanar imprint-merge path needs:
//!
//! * [`intersect_polygons`] — geometric intersection `A ∩ B`, returned
//!   as a list of closed boundary loops.
//! * [`partition_boundaries`] — the cutting curves for the
//!   imprint-then-merge pipeline: sub-segments of A's boundary that
//!   lie inside B, and sub-segments of B's boundary inside A.
//!
//! # Why i_overlay
//!
//! The previous implementation was a hand-rolled Greiner-Hormann
//! clipper. It handled proper-crossing configurations correctly but
//! rejected every degenerate input (shared vertex, vertex-on-edge,
//! collinear edge overlap) with [`OperationError::InvalidGeometry`].
//! Real CAD workloads hit these constantly — snapped sketch points,
//! axis-aligned stacks, two extrusions sharing a corner. The named
//! fix in the old module was "Hormann-Agathos perturbation", which is
//! research-grade work for a fragility that production polygon
//! kernels eliminated decades ago by using integer-coordinate cores.
//!
//! `i_overlay` is a pure-Rust polygon overlay engine with an integer
//! core (i32 scaled coordinates) and a thin float wrapper. Same
//! architectural choice as the `cdt` crate that closed the
//! multi-hole tessellator robustness story. Handles every degeneracy
//! class natively.
//!
//! # Algorithm
//!
//! For [`intersect_polygons`]: hand both polygons to
//! `i_overlay::float::single::SingleFloatOverlay::overlay` with
//! [`OverlayRule::Intersect`] and `FillRule::EvenOdd`, then flatten
//! the returned `Shapes` into a `Vec<Vec<Point2d>>` of boundary
//! loops.
//!
//! For [`partition_boundaries`]: compute `A ∩ B` first. Each edge of
//! every output contour is a sub-segment of either A's or B's input
//! boundary (or both, when A and B share a collinear edge). Classify
//! each output edge by midpoint-to-polyline distance:
//!
//! * midpoint within `tolerance.distance()` of A's boundary
//!   and not of B's → A-edge inside B → `a_inside_b`
//! * midpoint within `tolerance.distance()` of B's boundary
//!   and not of A's → B-edge inside A → `b_inside_a`
//! * midpoint within `tolerance.distance()` of both → shared edge,
//!   no cut needed (the boundaries are already coincident on this
//!   segment, downstream imprint handles it via the SSI path)
//!
//! # References
//!
//! - <https://crates.io/crates/i_overlay>
//! - Greiner & Hormann (1998). *Efficient Clipping of Arbitrary
//!   Polygons*. ACM TOG 17(2), 71-83.
//! - Hormann & Agathos (2001). *The Point in Polygon Problem for
//!   Arbitrary Polygons*. Computational Geometry 20(3), 131-144.

use super::{OperationError, OperationResult};
use crate::math::Tolerance;
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;

/// 2D point in clip-space. Owns its coordinates; intentionally distinct
/// from `math::Point2` to keep this module free of the trim-nurbs
/// dependency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point2d {
    pub x: f64,
    pub y: f64,
}

impl Point2d {
    /// Construct a point.
    #[inline]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Squared 2D distance, useful when an exact distance is not needed.
    #[inline]
    fn distance_sq(self, other: Self) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        dx * dx + dy * dy
    }
}

/// Result of [`intersect_polygons`].
#[derive(Debug, Clone, Default)]
pub struct PolygonClipResult {
    /// Connected components of `A ∩ B`. Each inner `Vec<Point2d>` is a
    /// closed loop with the implicit edge from the last point back to
    /// the first; the loops are CCW-oriented when the input polygons
    /// are CCW-oriented.
    pub regions: Vec<Vec<Point2d>>,
}

impl PolygonClipResult {
    /// `true` if the two input polygons have no overlap.
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }
}

/// Per-face boundary partition from [`partition_boundaries`].
#[derive(Debug, Clone, Default)]
pub struct BoundaryPartition {
    /// Sub-segments of `polygon_a`'s edges that lie strictly inside
    /// `polygon_b`. Each `(Point2d, Point2d)` is an open line segment.
    pub a_inside_b: Vec<(Point2d, Point2d)>,
    /// Sub-segments of `polygon_b`'s edges that lie strictly inside
    /// `polygon_a`. Same encoding as `a_inside_b`.
    pub b_inside_a: Vec<(Point2d, Point2d)>,
}

/// Validate a polygon input — at least three vertices required.
fn require_simple_polygon(poly: &[Point2d], name: &str) -> OperationResult<()> {
    if poly.len() < 3 {
        return Err(OperationError::InvalidInput {
            parameter: name.to_string(),
            expected: "at least 3 vertices".to_string(),
            received: format!("{} vertices", poly.len()),
        });
    }
    Ok(())
}

/// Convert a `&[Point2d]` polygon to the `Vec<[f64; 2]>` contour form
/// `i_overlay` expects.
#[inline]
fn to_contour(poly: &[Point2d]) -> Vec<[f64; 2]> {
    poly.iter().map(|p| [p.x, p.y]).collect()
}

/// Compute the geometric intersection `A ∩ B` of two simple polygons.
///
/// Both inputs are interpreted as closed CCW-oriented polygons with an
/// implicit closing edge between the last and first vertex. Returns the
/// closed boundary loops of the overlap region.
///
/// Backed by `i_overlay`'s float API. Degenerate inputs (shared
/// vertices, vertex-on-edge, collinear edges) are handled natively;
/// they no longer surface as [`OperationError::InvalidGeometry`].
///
/// # Errors
///
/// - [`OperationError::InvalidInput`] when either polygon has fewer
///   than three vertices.
pub fn intersect_polygons(
    polygon_a: &[Point2d],
    polygon_b: &[Point2d],
    _tolerance: &Tolerance,
) -> OperationResult<PolygonClipResult> {
    require_simple_polygon(polygon_a, "polygon_a")?;
    require_simple_polygon(polygon_b, "polygon_b")?;

    let subj: Vec<Vec<[f64; 2]>> = vec![to_contour(polygon_a)];
    let clip: Vec<[f64; 2]> = to_contour(polygon_b);

    let shapes = subj.overlay(&clip, OverlayRule::Intersect, FillRule::EvenOdd);

    let mut regions: Vec<Vec<Point2d>> = Vec::new();
    for shape in shapes {
        for contour in shape {
            if contour.len() < 3 {
                continue;
            }
            let region: Vec<Point2d> = contour.iter().map(|p| Point2d::new(p[0], p[1])).collect();
            regions.push(region);
        }
    }

    Ok(PolygonClipResult { regions })
}

/// Partition each polygon's boundary by the other polygon's interior.
///
/// For two simple polygons that overlap, this returns:
/// - the sub-segments of A's edges lying strictly inside B
///   (`a_inside_b`) — these are the cuts that split face B into
///   "interior to A" and "exterior to A" parts;
/// - the sub-segments of B's edges lying strictly inside A
///   (`b_inside_a`) — these split face A.
///
/// # Algorithm
///
/// Compute `A ∩ B` via [`intersect_polygons`]. Every edge of every
/// resulting boundary loop lies on A's boundary, B's boundary, or
/// both. Classify by midpoint distance to each input polyline and
/// route accordingly.
///
/// # Errors
///
/// - [`OperationError::InvalidInput`] when either polygon has fewer
///   than three vertices.
pub fn partition_boundaries(
    polygon_a: &[Point2d],
    polygon_b: &[Point2d],
    tolerance: &Tolerance,
) -> OperationResult<BoundaryPartition> {
    require_simple_polygon(polygon_a, "polygon_a")?;
    require_simple_polygon(polygon_b, "polygon_b")?;

    let eps = tolerance.distance().max(f64::EPSILON);
    let eps_sq = eps * eps;

    let intersection = intersect_polygons(polygon_a, polygon_b, tolerance)?;

    let mut a_inside_b: Vec<(Point2d, Point2d)> = Vec::new();
    let mut b_inside_a: Vec<(Point2d, Point2d)> = Vec::new();

    for region in &intersection.regions {
        let n = region.len();
        if n < 2 {
            continue;
        }
        for i in 0..n {
            let s = region[i];
            let e = region[(i + 1) % n];
            if s.distance_sq(e) <= eps_sq {
                continue;
            }
            let mid = Point2d::new(0.5 * (s.x + e.x), 0.5 * (s.y + e.y));
            let on_a = point_on_polyline(mid, polygon_a, eps_sq);
            let on_b = point_on_polyline(mid, polygon_b, eps_sq);
            match (on_a, on_b) {
                (true, true) | (false, false) => {
                    // (true, true): A and B share a coincident sub-edge
                    // here — no imprint cut needed, the boundaries
                    // already align.
                    // (false, false): defensive — every A∩B boundary
                    // edge should lie on at least one input polygon.
                    // If neither classifier matches, drop the segment
                    // rather than mis-route it.
                    continue;
                }
                (true, false) => a_inside_b.push((s, e)),
                (false, true) => b_inside_a.push((s, e)),
            }
        }
    }

    Ok(BoundaryPartition {
        a_inside_b,
        b_inside_a,
    })
}

/// True when `p` lies within `sqrt(eps_sq)` of any segment of the
/// closed polyline `poly`. Used to classify which input polygon an
/// output `A ∩ B` boundary edge came from.
fn point_on_polyline(p: Point2d, poly: &[Point2d], eps_sq: f64) -> bool {
    let n = poly.len();
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        if point_to_segment_distance_sq(p, a, b) <= eps_sq {
            return true;
        }
    }
    false
}

/// Squared distance from `p` to the segment `a..b`. Foot is clamped to
/// the segment's closed extent so the result is the true point-to-
/// segment distance, not the point-to-infinite-line distance.
fn point_to_segment_distance_sq(p: Point2d, a: Point2d, b: Point2d) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len_sq = dx * dx + dy * dy;
    if len_sq <= f64::EPSILON {
        return p.distance_sq(a);
    }
    let t = ((p.x - a.x) * dx + (p.y - a.y) * dy) / len_sq;
    let t = t.clamp(0.0, 1.0);
    let foot_x = a.x + t * dx;
    let foot_y = a.y + t * dy;
    let fx = p.x - foot_x;
    let fy = p.y - foot_y;
    fx * fx + fy * fy
}

/// 2D point-in-polygon test by ray casting along +x. Returns `true`
/// when `p` lies strictly inside the polygon `poly` (boundary points
/// are not classified — callers should handle on-boundary checks
/// separately if needed).
///
/// This was the campaign's original exact ray cast; Slice 2 hoisted its
/// algorithm into `math::exact_predicates::point_in_polygon_2d_by` as the
/// SINGLE exact entry point every pipeline copy now delegates to. The
/// zero-allocation accessor form is used here because `poly` carries
/// `Point2d`s, not `(f64, f64)` tuples.
fn point_in_polygon(p: Point2d, poly: &[Point2d], _eps: f64) -> bool {
    crate::math::exact_predicates::point_in_polygon_2d_by(
        poly.len(),
        |i| (poly[i].x, poly[i].y),
        p.x,
        p.y,
    )
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Tolerance;

    fn tol() -> Tolerance {
        Tolerance::from_distance(1e-6)
    }

    fn p(x: f64, y: f64) -> Point2d {
        Point2d::new(x, y)
    }

    /// Signed area of a closed polygon (CCW = positive).
    fn signed_area(poly: &[Point2d]) -> f64 {
        let n = poly.len();
        let mut sum = 0.0;
        for i in 0..n {
            let p0 = poly[i];
            let p1 = poly[(i + 1) % n];
            sum += (p1.x - p0.x) * (p1.y + p0.y);
        }
        -sum * 0.5
    }

    #[test]
    fn disjoint_polygons_return_empty() {
        let a = vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)];
        let b = vec![p(5.0, 5.0), p(6.0, 5.0), p(6.0, 6.0), p(5.0, 6.0)];
        let result = intersect_polygons(&a, &b, &tol()).expect("clip ok");
        assert!(result.is_empty(), "disjoint should produce no regions");
    }

    #[test]
    fn fully_contained_a_in_b_returns_a() {
        // 1×1 square inside a 10×10 square.
        let a = vec![p(2.0, 2.0), p(3.0, 2.0), p(3.0, 3.0), p(2.0, 3.0)];
        let b = vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0)];
        let result = intersect_polygons(&a, &b, &tol()).expect("clip ok");
        assert_eq!(result.regions.len(), 1);
        let area = signed_area(&result.regions[0]).abs();
        assert!(
            (area - 1.0).abs() < 1e-9,
            "expected area 1.0, got {area:.6}"
        );
    }

    #[test]
    fn fully_contained_b_in_a_returns_b() {
        let a = vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0)];
        let b = vec![p(2.0, 2.0), p(3.0, 2.0), p(3.0, 3.0), p(2.0, 3.0)];
        let result = intersect_polygons(&a, &b, &tol()).expect("clip ok");
        assert_eq!(result.regions.len(), 1);
        let area = signed_area(&result.regions[0]).abs();
        assert!(
            (area - 1.0).abs() < 1e-9,
            "expected area 1.0, got {area:.6}"
        );
    }

    #[test]
    fn proper_crossing_two_squares_overlap() {
        // Two unit squares offset by (0.5, 0.5) — overlap is a
        // 0.5×0.5 square of area 0.25.
        let a = vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)];
        let b = vec![p(0.5, 0.5), p(1.5, 0.5), p(1.5, 1.5), p(0.5, 1.5)];
        let result = intersect_polygons(&a, &b, &tol()).expect("clip ok");
        assert_eq!(result.regions.len(), 1);
        let area = signed_area(&result.regions[0]).abs();
        assert!(
            (area - 0.25).abs() < 1e-9,
            "expected area 0.25, got {area:.6}"
        );
    }

    #[test]
    fn proper_crossing_triangle_and_square() {
        let square = vec![p(0.0, 0.0), p(4.0, 0.0), p(4.0, 4.0), p(0.0, 4.0)];
        let triangle = vec![p(-1.0, 2.0), p(2.0, -2.0), p(5.0, 2.0)];
        let result = intersect_polygons(&square, &triangle, &tol()).expect("clip ok");
        assert_eq!(result.regions.len(), 1);
        let area = signed_area(&result.regions[0]).abs();
        assert!(area > 0.0, "expected positive area, got {area:.6}");
        assert!(
            result.regions[0].len() >= 3,
            "overlap polygon must have ≥3 vertices, got {}",
            result.regions[0].len()
        );
    }

    #[test]
    fn rejects_polygon_with_fewer_than_three_vertices() {
        let a = vec![p(0.0, 0.0), p(1.0, 0.0)];
        let b = vec![p(0.0, 0.0), p(1.0, 0.0), p(0.5, 1.0)];
        let result = intersect_polygons(&a, &b, &tol());
        assert!(matches!(result, Err(OperationError::InvalidInput { .. })));
    }

    #[test]
    fn accepts_shared_vertex_configuration() {
        // Two triangles sharing the vertex (0,0). Old Greiner-Hormann
        // rejected this as InvalidGeometry; i_overlay handles it.
        let a = vec![p(0.0, 0.0), p(1.0, 0.0), p(0.0, 1.0)];
        let b = vec![p(0.0, 0.0), p(-1.0, 0.0), p(0.0, -1.0)];
        let result = intersect_polygons(&a, &b, &tol()).expect("clip ok");
        // Touch-at-vertex configurations have zero-area intersection
        // and no regions emitted; this just verifies we don't error.
        assert!(
            result.is_empty() || result.regions.iter().all(|r| signed_area(r).abs() < 1e-6),
            "shared-vertex touching should be empty or zero-area"
        );
    }

    #[test]
    fn accepts_vertex_on_edge_configuration() {
        // Triangle b's vertex sits on a's bottom edge.
        // Old: InvalidGeometry. New: handled by i_overlay's integer core.
        let a = vec![p(0.0, 0.0), p(4.0, 0.0), p(2.0, 4.0)];
        let b = vec![p(2.0, 0.0), p(3.0, -1.0), p(1.0, -1.0)];
        let _result = intersect_polygons(&a, &b, &tol()).expect("clip ok");
        // The triangles touch along an edge but b lies below the x-axis
        // so there is no positive-area overlap. Pass condition is simply
        // that no error is raised.
    }

    #[test]
    fn partition_two_overlapping_squares_yields_l_shaped_cuts() {
        // A = unit square at origin, B = unit square at (0.5, 0.5).
        // A's right + top edges have segments inside B; B's left +
        // bottom edges have segments inside A.
        let a = vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)];
        let b = vec![p(0.5, 0.5), p(1.5, 0.5), p(1.5, 1.5), p(0.5, 1.5)];
        let part = partition_boundaries(&a, &b, &tol()).expect("partition ok");
        assert_eq!(
            part.a_inside_b.len(),
            2,
            "expected 2 A-edge sub-segments inside B, got {}",
            part.a_inside_b.len()
        );
        assert_eq!(
            part.b_inside_a.len(),
            2,
            "expected 2 B-edge sub-segments inside A, got {}",
            part.b_inside_a.len()
        );
        // Verify (1, 1) appears among the a_inside_b endpoints — both
        // halves of the L meet at A's top-right corner.
        let endpoints: Vec<Point2d> = part
            .a_inside_b
            .iter()
            .flat_map(|(s, e)| vec![*s, *e])
            .collect();
        assert!(
            endpoints
                .iter()
                .any(|p| (p.x - 1.0).abs() < 1e-9 && (p.y - 1.0).abs() < 1e-9),
            "expected vertex (1, 1) in a_inside_b endpoint set, got {endpoints:?}"
        );
    }

    #[test]
    fn partition_disjoint_polygons_yields_empty() {
        let a = vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)];
        let b = vec![p(5.0, 5.0), p(6.0, 5.0), p(6.0, 6.0), p(5.0, 6.0)];
        let part = partition_boundaries(&a, &b, &tol()).expect("partition ok");
        assert!(part.a_inside_b.is_empty());
        assert!(part.b_inside_a.is_empty());
    }

    #[test]
    fn partition_segments_have_midpoints_inside_other_polygon() {
        // Property test: every output sub-segment's midpoint must lie
        // inside (or on) the opposing polygon.
        let a = vec![p(0.0, 0.0), p(4.0, 0.0), p(4.0, 4.0), p(0.0, 4.0)];
        let b = vec![p(2.0, 2.0), p(6.0, 2.0), p(6.0, 6.0), p(2.0, 6.0)];
        let part = partition_boundaries(&a, &b, &tol()).expect("partition ok");

        let eps = 1e-6;
        let eps_sq = eps * eps;
        for (s, e) in &part.a_inside_b {
            let mid = Point2d::new(0.5 * (s.x + e.x), 0.5 * (s.y + e.y));
            assert!(
                point_in_polygon(mid, &b, eps) || point_on_polyline(mid, &b, eps_sq),
                "a_inside_b segment ({s:?}, {e:?}) midpoint {mid:?} not in B"
            );
        }
        for (s, e) in &part.b_inside_a {
            let mid = Point2d::new(0.5 * (s.x + e.x), 0.5 * (s.y + e.y));
            assert!(
                point_in_polygon(mid, &a, eps) || point_on_polyline(mid, &a, eps_sq),
                "b_inside_a segment ({s:?}, {e:?}) midpoint {mid:?} not in A"
            );
        }
    }

    #[test]
    fn boundary_loop_winding_is_ccw() {
        // Two overlapping CCW squares — i_overlay outputs CCW.
        let a = vec![p(0.0, 0.0), p(2.0, 0.0), p(2.0, 2.0), p(0.0, 2.0)];
        let b = vec![p(1.0, 1.0), p(3.0, 1.0), p(3.0, 3.0), p(1.0, 3.0)];
        let result = intersect_polygons(&a, &b, &tol()).expect("clip ok");
        assert_eq!(result.regions.len(), 1);
        let signed = signed_area(&result.regions[0]);
        assert!(
            signed > 0.0,
            "expected CCW overlap, got signed area {signed}"
        );
    }

    #[test]
    fn accepts_two_overlapping_hexagons_at_shared_vertex() {
        // Production regression: regular hexagons centred at (0,0) and
        // (1,0), both circumradius 1.0. They share a vertex at
        // (0.5, sqrt(3)/2) — the exact configuration that broke
        // union_overlapping_polyline_hexagons.
        let h = (3.0_f64).sqrt() * 0.5;
        let a = vec![
            p(1.0, 0.0),
            p(0.5, h),
            p(-0.5, h),
            p(-1.0, 0.0),
            p(-0.5, -h),
            p(0.5, -h),
        ];
        let b: Vec<Point2d> = a.iter().map(|q| Point2d::new(q.x + 1.0, q.y)).collect();
        let part = partition_boundaries(&a, &b, &tol()).expect("partition ok");
        // The hexagons overlap on a lens-shaped region; both
        // partitions must be non-empty.
        assert!(
            !part.a_inside_b.is_empty(),
            "a_inside_b should not be empty for proper overlap"
        );
        assert!(
            !part.b_inside_a.is_empty(),
            "b_inside_a should not be empty for proper overlap"
        );
    }
}
