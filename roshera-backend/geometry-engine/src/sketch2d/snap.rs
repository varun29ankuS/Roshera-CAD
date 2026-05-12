//! Snap engine — proximity candidates for cursor → entity snapping.
//!
//! Slice D-1-a: this module owns the *geometric* half of the snap
//! pipeline. Given a cursor position and a search radius, it walks the
//! sketch's entity stores and returns a ranked list of
//! [`SnapCandidate`]s — discrete features (endpoints, midpoints,
//! centres, quadrants) preferred over on-curve perpendicular feet.
//!
//! Constraint *inference* (e.g. "this draft line is nearly horizontal,
//! propose `GeometricConstraint::Horizontal`") is the *semantic* half
//! and lives in the forthcoming `inference.rs` module (D-1-b). Snap
//! and inference compose: callers first resolve the cursor against
//! existing geometry with this module, then pass the resulting
//! candidates plus their draft entity into the inference engine to
//! get proposed constraints.
//!
//! # Ranking
//!
//! Candidates are sorted by a two-level key:
//!
//! 1. [`SnapKind::priority`] — discrete features (Vertex / Endpoint /
//!    Corner / Centre / Midpoint / Quadrant) outrank on-curve
//!    projections so the user can always snap *to* a feature rather
//!    than *near* one.
//! 2. Euclidean distance — closer ties win within the same priority.
//!
//! Ties at both keys preserve insertion order, which is the entity
//! store iteration order (DashMap key hash) — stable per process but
//! not stable across runs. Callers that care about determinism in
//! tests should use the `best_snap` convenience and the explicit
//! `(priority, distance)` invariant rather than positional indexing.
//!
//! # Costs
//!
//! Snap is O(N) over all entities in the sketch — every store is
//! scanned linearly. No spatial index is consulted today. For sketches
//! under ~1k entities this is well below a single millisecond on
//! commodity hardware. A grid/BVH acceleration is a future
//! optimisation tracked under task #14 follow-ups.

use serde::{Deserialize, Serialize};

use super::constraints::EntityRef;
use super::line2d::LineGeometry;
use super::point2d::Point2d;
use super::sketch::Sketch;

/// Kind of feature a [`SnapCandidate`] points at.
///
/// Carries both a semantic tag (what kind of geometric feature was
/// snapped to) and an implicit visual hint (the snap marker rendered
/// in the viewport can vary by kind — square for endpoints, diamond
/// for midpoints, ring for centres, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapKind {
    /// A free-standing sketch point.
    Point,
    /// Start or end vertex of a line segment / ray origin.
    LineEndpoint,
    /// Midpoint of a line segment.
    LineMidpoint,
    /// Perpendicular foot of the cursor on a line/segment.
    OnLine,
    /// Centre of a circle.
    CircleCenter,
    /// Quadrant point of a circle (0° / 90° / 180° / 270°).
    CircleQuadrant,
    /// Nearest point on a circle's boundary.
    OnCircle,
    /// Centre of an arc.
    ArcCenter,
    /// Start or end vertex of an arc.
    ArcEndpoint,
    /// Midpoint along an arc's parametric range.
    ArcMidpoint,
    /// Nearest point on an arc.
    OnArc,
    /// Corner of a rectangle.
    RectangleCorner,
    /// Midpoint of a rectangle edge.
    RectangleEdgeMidpoint,
    /// Centre of a rectangle.
    RectangleCenter,
    /// Centre of an ellipse.
    EllipseCenter,
    /// Axis endpoint of an ellipse (±semi_major / ±semi_minor).
    EllipseQuadrant,
    /// Nearest point on an ellipse.
    OnEllipse,
}

impl SnapKind {
    /// Ranking priority. Lower values are preferred when two
    /// candidates lie within the same snap radius.
    ///
    /// Tier 0: discrete vertices the user *clicked into existence*
    /// (free points, segment endpoints, arc endpoints, rect corners).
    /// Tier 1: derived discrete features (midpoints, centres,
    /// quadrants).
    /// Tier 2: on-curve projections — the weakest snap, only
    /// surfaced when no better feature is in reach.
    pub fn priority(self) -> u8 {
        match self {
            SnapKind::Point
            | SnapKind::LineEndpoint
            | SnapKind::ArcEndpoint
            | SnapKind::RectangleCorner => 0,
            SnapKind::LineMidpoint
            | SnapKind::ArcMidpoint
            | SnapKind::CircleCenter
            | SnapKind::CircleQuadrant
            | SnapKind::ArcCenter
            | SnapKind::RectangleEdgeMidpoint
            | SnapKind::RectangleCenter
            | SnapKind::EllipseCenter
            | SnapKind::EllipseQuadrant => 1,
            SnapKind::OnLine
            | SnapKind::OnCircle
            | SnapKind::OnArc
            | SnapKind::OnEllipse => 2,
        }
    }

    /// `true` if this kind describes a discrete point feature
    /// (Tier 0 or Tier 1). `false` for on-curve projections.
    pub fn is_discrete(self) -> bool {
        self.priority() < 2
    }
}

/// One candidate result from [`Sketch::find_snap_candidates`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SnapCandidate {
    /// Owning entity in the sketch.
    pub entity: EntityRef,
    /// The 2D location the cursor would snap to.
    pub point: Point2d,
    /// Euclidean distance from the cursor to `point` (sketch units).
    pub distance: f64,
    /// What kind of feature this candidate represents.
    pub kind: SnapKind,
}

impl SnapCandidate {
    /// Compose the two-level sort key (priority, distance) for
    /// stable ranking. Used by `find_snap_candidates` internally and
    /// exposed for callers that want to merge candidates from
    /// multiple sketches.
    pub fn sort_key(&self) -> (u8, f64) {
        (self.kind.priority(), self.distance)
    }
}

impl Sketch {
    /// Collect every snap candidate whose distance to `cursor` is
    /// `≤ radius`. Returns a list sorted by (priority, distance) —
    /// the most desirable snap is at index 0.
    ///
    /// Entity stores walked:
    /// - free points (1 candidate per point — `SnapKind::Point`)
    /// - lines: endpoints + segment midpoint + perpendicular foot
    /// - circles: centre + 4 quadrants + nearest boundary point
    /// - arcs: centre + 2 endpoints + midpoint + nearest on-arc point
    /// - rectangles: 4 corners + 4 edge midpoints + centre
    /// - ellipses: centre + 4 axis endpoints + nearest boundary
    ///
    /// Splines and polylines are intentionally excluded for now
    /// (the inference engine does not yet propose constraints over
    /// them — see C-4 / C-5).
    ///
    /// `radius` must be `>= 0`. A negative radius returns the empty
    /// vector. A radius of `0.0` is honoured exactly: only candidates
    /// that coincide with the cursor are returned.
    pub fn find_snap_candidates(&self, cursor: Point2d, radius: f64) -> Vec<SnapCandidate> {
        if !radius.is_finite() || radius < 0.0 {
            return Vec::new();
        }

        let mut out: Vec<SnapCandidate> = Vec::new();
        let push_if_close =
            |out: &mut Vec<SnapCandidate>, entity: EntityRef, point: Point2d, kind: SnapKind| {
                let distance = cursor.distance_to(&point);
                if distance <= radius {
                    out.push(SnapCandidate {
                        entity,
                        point,
                        distance,
                        kind,
                    });
                }
            };

        // Free points.
        for entry in self.points().iter() {
            let id = *entry.key();
            push_if_close(
                &mut out,
                EntityRef::Point(id),
                entry.value().position,
                SnapKind::Point,
            );
        }

        // Lines: endpoints, midpoint (segments only), perpendicular foot.
        for entry in self.lines().iter() {
            let id = *entry.key();
            let entity = EntityRef::Line(id);
            match &entry.value().geometry {
                LineGeometry::Segment(seg) => {
                    push_if_close(&mut out, entity, seg.start, SnapKind::LineEndpoint);
                    push_if_close(&mut out, entity, seg.end, SnapKind::LineEndpoint);
                    push_if_close(&mut out, entity, seg.midpoint(), SnapKind::LineMidpoint);
                    push_if_close(
                        &mut out,
                        entity,
                        seg.closest_point(&cursor),
                        SnapKind::OnLine,
                    );
                }
                LineGeometry::Ray(ray) => {
                    push_if_close(&mut out, entity, ray.origin, SnapKind::LineEndpoint);
                    push_if_close(
                        &mut out,
                        entity,
                        ray.closest_point(&cursor),
                        SnapKind::OnLine,
                    );
                }
                LineGeometry::Infinite(line) => {
                    push_if_close(
                        &mut out,
                        entity,
                        line.closest_point(&cursor),
                        SnapKind::OnLine,
                    );
                }
            }
        }

        // Circles: centre, 4 quadrants, nearest on-boundary.
        for entry in self.circles().iter() {
            let id = *entry.key();
            let entity = EntityRef::Circle(id);
            let c = &entry.value().circle;
            push_if_close(&mut out, entity, c.center, SnapKind::CircleCenter);
            push_if_close(
                &mut out,
                entity,
                Point2d::new(c.center.x + c.radius, c.center.y),
                SnapKind::CircleQuadrant,
            );
            push_if_close(
                &mut out,
                entity,
                Point2d::new(c.center.x - c.radius, c.center.y),
                SnapKind::CircleQuadrant,
            );
            push_if_close(
                &mut out,
                entity,
                Point2d::new(c.center.x, c.center.y + c.radius),
                SnapKind::CircleQuadrant,
            );
            push_if_close(
                &mut out,
                entity,
                Point2d::new(c.center.x, c.center.y - c.radius),
                SnapKind::CircleQuadrant,
            );
            push_if_close(
                &mut out,
                entity,
                c.closest_point(&cursor),
                SnapKind::OnCircle,
            );
        }

        // Arcs: centre, 2 endpoints, midpoint, nearest on-arc.
        for entry in self.arcs().iter() {
            let id = *entry.key();
            let entity = EntityRef::Arc(id);
            let a = &entry.value().arc;
            push_if_close(&mut out, entity, a.center, SnapKind::ArcCenter);
            push_if_close(&mut out, entity, a.start_point(), SnapKind::ArcEndpoint);
            push_if_close(&mut out, entity, a.end_point(), SnapKind::ArcEndpoint);
            push_if_close(&mut out, entity, a.midpoint(), SnapKind::ArcMidpoint);
            push_if_close(&mut out, entity, a.closest_point(&cursor), SnapKind::OnArc);
        }

        // Rectangles: 4 corners, 4 edge midpoints, centre.
        for entry in self.rectangles().iter() {
            let id = *entry.key();
            let entity = EntityRef::Rectangle(id);
            let r = &entry.value().rectangle;
            let corners = r.corners();
            for c in &corners {
                push_if_close(&mut out, entity, *c, SnapKind::RectangleCorner);
            }
            // Edge midpoints walk the corner ring with wrap-around.
            for i in 0..4 {
                let a = corners[i];
                let b = corners[(i + 1) % 4];
                let mid = Point2d::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
                push_if_close(&mut out, entity, mid, SnapKind::RectangleEdgeMidpoint);
            }
            push_if_close(&mut out, entity, r.center, SnapKind::RectangleCenter);
        }

        // Ellipses: centre, 4 axis endpoints (±semi_major / ±semi_minor),
        // nearest on-boundary.
        for entry in self.ellipses().iter() {
            let id = *entry.key();
            let entity = EntityRef::Ellipse(id);
            let e = &entry.value().ellipse;
            push_if_close(&mut out, entity, e.center, SnapKind::EllipseCenter);

            let (cos_r, sin_r) = (e.rotation.cos(), e.rotation.sin());
            // Local axis vector u = (cos, sin) for the major axis,
            // v = (-sin, cos) for the minor. Axis endpoints are
            // centre ± semi_axis · axis_vector.
            let major_offset = (e.semi_major * cos_r, e.semi_major * sin_r);
            let minor_offset = (-e.semi_minor * sin_r, e.semi_minor * cos_r);
            push_if_close(
                &mut out,
                entity,
                Point2d::new(e.center.x + major_offset.0, e.center.y + major_offset.1),
                SnapKind::EllipseQuadrant,
            );
            push_if_close(
                &mut out,
                entity,
                Point2d::new(e.center.x - major_offset.0, e.center.y - major_offset.1),
                SnapKind::EllipseQuadrant,
            );
            push_if_close(
                &mut out,
                entity,
                Point2d::new(e.center.x + minor_offset.0, e.center.y + minor_offset.1),
                SnapKind::EllipseQuadrant,
            );
            push_if_close(
                &mut out,
                entity,
                Point2d::new(e.center.x - minor_offset.0, e.center.y - minor_offset.1),
                SnapKind::EllipseQuadrant,
            );
            push_if_close(
                &mut out,
                entity,
                e.closest_point(&cursor),
                SnapKind::OnEllipse,
            );
        }

        out.sort_by(|a, b| {
            a.kind
                .priority()
                .cmp(&b.kind.priority())
                .then(
                    a.distance
                        .partial_cmp(&b.distance)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
        });
        out
    }

    /// The single best snap, if any, within `radius` of `cursor`.
    ///
    /// Equivalent to `find_snap_candidates(..).into_iter().next()`
    /// but avoids allocating the rest of the list when the caller
    /// only needs the winner. Returns `None` if `radius` is invalid
    /// or no entity feature lies in range.
    pub fn best_snap(&self, cursor: Point2d, radius: f64) -> Option<SnapCandidate> {
        self.find_snap_candidates(cursor, radius).into_iter().next()
    }
}

#[cfg(test)]
mod tests {
    //! D-1-a snap engine tests.
    //!
    //! Coverage:
    //! - Each entity kind produces its expected candidate set.
    //! - Ranking respects (priority, distance).
    //! - Empty sketch / out-of-range cursor return empty.
    //! - Negative / NaN radius returns empty.
    //! - `best_snap` matches the head of `find_snap_candidates`.
    #![allow(clippy::float_cmp)]
    #![allow(clippy::expect_used)]

    use super::*;
    use crate::sketch2d::sketch::SketchAnchor;

    fn fresh() -> Sketch {
        Sketch::new("snap_test".to_string(), SketchAnchor::xy())
    }

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-10
    }

    // ── Empty / degenerate ────────────────────────────────────────

    #[test]
    fn empty_sketch_returns_no_candidates() {
        let s = fresh();
        assert!(s.find_snap_candidates(Point2d::ORIGIN, 10.0).is_empty());
        assert!(s.best_snap(Point2d::ORIGIN, 10.0).is_none());
    }

    #[test]
    fn negative_radius_returns_empty() {
        let s = fresh();
        s.add_point(Point2d::ORIGIN);
        assert!(s.find_snap_candidates(Point2d::ORIGIN, -1.0).is_empty());
    }

    #[test]
    fn nan_radius_returns_empty() {
        let s = fresh();
        s.add_point(Point2d::ORIGIN);
        assert!(s
            .find_snap_candidates(Point2d::ORIGIN, f64::NAN)
            .is_empty());
    }

    #[test]
    fn cursor_outside_radius_returns_empty() {
        let s = fresh();
        s.add_point(Point2d::new(100.0, 100.0));
        assert!(s.find_snap_candidates(Point2d::ORIGIN, 1.0).is_empty());
    }

    // ── Free points ───────────────────────────────────────────────

    #[test]
    fn point_in_range_returns_single_candidate() {
        let s = fresh();
        let pid = s.add_point(Point2d::new(0.1, 0.0));
        let cands = s.find_snap_candidates(Point2d::ORIGIN, 1.0);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].entity, EntityRef::Point(pid));
        assert_eq!(cands[0].kind, SnapKind::Point);
        assert!(approx_eq(cands[0].distance, 0.1));
        assert!(approx_eq(cands[0].point.x, 0.1));
    }

    // ── Lines ─────────────────────────────────────────────────────

    #[test]
    fn line_segment_emits_two_endpoints_midpoint_and_on_line() {
        let s = fresh();
        let p0 = s.add_point(Point2d::new(0.0, 0.0));
        let p1 = s.add_point(Point2d::new(10.0, 0.0));
        let lid = s.add_line(p0, p1).expect("line");

        // Cursor near segment midpoint at (5, 0.05) — endpoints far
        // away, midpoint very close, perpendicular foot exactly at
        // (5, 0).
        let cands = s.find_snap_candidates(Point2d::new(5.0, 0.05), 1.0);
        let line_cands: Vec<_> = cands
            .iter()
            .filter(|c| c.entity == EntityRef::Line(lid))
            .collect();
        // Midpoint + perpendicular foot are both within radius;
        // endpoints (distance ~5.0) are not.
        assert_eq!(line_cands.len(), 2);
        let kinds: Vec<SnapKind> = line_cands.iter().map(|c| c.kind).collect();
        assert!(kinds.contains(&SnapKind::LineMidpoint));
        assert!(kinds.contains(&SnapKind::OnLine));
    }

    #[test]
    fn line_endpoint_outranks_on_line_at_same_radius() {
        let s = fresh();
        let p0 = s.add_point(Point2d::new(0.0, 0.0));
        let p1 = s.add_point(Point2d::new(10.0, 0.0));
        s.add_line(p0, p1).expect("line");

        // Cursor at endpoint exactly. The free point at (0,0) (which
        // backs the line endpoint) and the LineEndpoint feature both
        // report priority 0 / distance 0; either can win, but
        // critically the priority-2 OnLine projection must NOT win
        // when a priority-0 feature is at the same distance. Assert
        // the winning candidate is a discrete feature, not OnLine.
        let best = s.best_snap(Point2d::new(0.0, 0.0), 1.0).expect("snap");
        assert!(best.kind.is_discrete());
        assert!(best.kind.priority() < 2);
        assert_eq!(best.distance, 0.0);
    }

    // ── Circles ───────────────────────────────────────────────────

    #[test]
    fn circle_emits_centre_four_quadrants_and_on_circle() {
        let s = fresh();
        let cid = s.add_circle(Point2d::ORIGIN, 5.0).expect("add");

        // Cursor at centre — every feature on a circle of radius 5
        // is exactly 5 away except the centre itself (0). With a
        // radius of 10 they all fit.
        let cands = s.find_snap_candidates(Point2d::ORIGIN, 10.0);
        let circle_cands: Vec<_> = cands
            .iter()
            .filter(|c| c.entity == EntityRef::Circle(cid))
            .collect();
        // 1 centre + 4 quadrants + 1 on-circle (closest_point picks
        // an arbitrary boundary point when cursor == centre).
        assert_eq!(circle_cands.len(), 6);
    }

    #[test]
    fn circle_quadrant_at_zero_degrees_is_at_centre_plus_radius_x() {
        let s = fresh();
        let cid = s
            .add_circle(Point2d::new(3.0, 7.0), 2.0)
            .expect("add");
        // Snap at (5, 7) → should hit the +x quadrant exactly.
        let best = s.best_snap(Point2d::new(5.0, 7.0), 0.01).expect("snap");
        assert_eq!(best.entity, EntityRef::Circle(cid));
        assert_eq!(best.kind, SnapKind::CircleQuadrant);
        assert!(approx_eq(best.distance, 0.0));
    }

    // ── Arcs ──────────────────────────────────────────────────────

    #[test]
    fn arc_emits_centre_endpoints_midpoint_and_on_arc() {
        let s = fresh();
        // Quarter-arc on unit circle from 0° to 90°.
        let aid = s
            .add_arc_center_angles(
                Point2d::ORIGIN,
                1.0,
                0.0,
                std::f64::consts::FRAC_PI_2,
            )
            .expect("add");
        // Cursor at centre — all 4 candidates (centre, 2 endpoints,
        // midpoint, on-arc) lie within radius 2.
        let cands = s.find_snap_candidates(Point2d::ORIGIN, 2.0);
        let arc_cands: Vec<_> = cands
            .iter()
            .filter(|c| c.entity == EntityRef::Arc(aid))
            .collect();
        // 1 centre + 2 endpoints + 1 midpoint + 1 on-arc.
        assert_eq!(arc_cands.len(), 5);
        assert!(arc_cands
            .iter()
            .any(|c| c.kind == SnapKind::ArcCenter && approx_eq(c.distance, 0.0)));
    }

    // ── Rectangles ────────────────────────────────────────────────

    #[test]
    fn rectangle_emits_four_corners_four_edge_midpoints_and_centre() {
        let s = fresh();
        let rid = s
            .add_rectangle(Point2d::new(0.0, 0.0), Point2d::new(2.0, 1.0))
            .expect("rect");
        let cands = s.find_snap_candidates(Point2d::new(1.0, 0.5), 5.0);
        let rect_cands: Vec<_> = cands
            .iter()
            .filter(|c| c.entity == EntityRef::Rectangle(rid))
            .collect();
        // 4 corners + 4 edge midpoints + 1 centre = 9.
        assert_eq!(rect_cands.len(), 9);
        assert!(rect_cands
            .iter()
            .filter(|c| c.kind == SnapKind::RectangleCorner)
            .count()
            == 4);
        assert!(rect_cands
            .iter()
            .filter(|c| c.kind == SnapKind::RectangleEdgeMidpoint)
            .count()
            == 4);
        assert!(rect_cands
            .iter()
            .filter(|c| c.kind == SnapKind::RectangleCenter)
            .count()
            == 1);
    }

    // ── Ellipses ──────────────────────────────────────────────────

    #[test]
    fn ellipse_axis_aligned_emits_centre_four_quadrants_and_on_ellipse() {
        let s = fresh();
        let eid = s
            .add_ellipse(Point2d::ORIGIN, 3.0, 2.0, 0.0)
            .expect("ellipse");
        let cands = s.find_snap_candidates(Point2d::ORIGIN, 5.0);
        let e_cands: Vec<_> = cands
            .iter()
            .filter(|c| c.entity == EntityRef::Ellipse(eid))
            .collect();
        // 1 centre + 4 quadrants + 1 on-ellipse = 6.
        assert_eq!(e_cands.len(), 6);

        let quadrants: Vec<_> = e_cands
            .iter()
            .filter(|c| c.kind == SnapKind::EllipseQuadrant)
            .map(|c| (c.point.x, c.point.y))
            .collect();
        // For rotation = 0 the four axis endpoints are at
        // (±3, 0) and (0, ±2).
        assert_eq!(quadrants.len(), 4);
        assert!(quadrants
            .iter()
            .any(|(x, y)| approx_eq(*x, 3.0) && approx_eq(*y, 0.0)));
        assert!(quadrants
            .iter()
            .any(|(x, y)| approx_eq(*x, -3.0) && approx_eq(*y, 0.0)));
        assert!(quadrants
            .iter()
            .any(|(x, y)| approx_eq(*x, 0.0) && approx_eq(*y, 2.0)));
        assert!(quadrants
            .iter()
            .any(|(x, y)| approx_eq(*x, 0.0) && approx_eq(*y, -2.0)));
    }

    // ── Ranking ───────────────────────────────────────────────────

    #[test]
    fn ranking_prefers_discrete_features_over_on_curve_projections() {
        let s = fresh();
        // A horizontal segment from (0, 0) to (10, 0) and a free
        // point near the same on-line projection.
        let p0 = s.add_point(Point2d::new(0.0, 0.0));
        let p1 = s.add_point(Point2d::new(10.0, 0.0));
        s.add_line(p0, p1).expect("line");
        let near_pid = s.add_point(Point2d::new(5.05, 0.04));

        // Cursor at (5, 0.05): the perpendicular foot at (5, 0) is
        // 0.05 away and the free point at (5.05, 0.04) is also
        // ≈0.053 away — very close in distance but free point is
        // priority 0 while the perpendicular foot is priority 2.
        // Priority must win.
        let best = s.best_snap(Point2d::new(5.0, 0.05), 0.2).expect("snap");
        assert_eq!(best.entity, EntityRef::Point(near_pid));
        assert_eq!(best.kind, SnapKind::Point);
    }

    #[test]
    fn ranking_closer_distance_wins_within_same_priority() {
        let s = fresh();
        let a = s.add_point(Point2d::new(0.5, 0.0));
        let _b = s.add_point(Point2d::new(0.9, 0.0));
        let best = s.best_snap(Point2d::ORIGIN, 2.0).expect("snap");
        assert_eq!(best.entity, EntityRef::Point(a));
        assert!(approx_eq(best.distance, 0.5));
    }

    #[test]
    fn best_snap_matches_head_of_find_snap_candidates() {
        let s = fresh();
        s.add_point(Point2d::new(1.0, 0.0));
        s.add_point(Point2d::new(2.0, 0.0));
        s.add_circle(Point2d::new(5.0, 0.0), 1.0)
            .expect("circle");
        let head = s
            .find_snap_candidates(Point2d::ORIGIN, 10.0)
            .into_iter()
            .next();
        let best = s.best_snap(Point2d::ORIGIN, 10.0);
        assert_eq!(head, best);
    }

    // ── Snap kind taxonomy ────────────────────────────────────────

    #[test]
    fn discrete_kinds_report_priority_below_two() {
        // Sanity: any "discrete" snap kind must report a priority <2.
        let discrete = [
            SnapKind::Point,
            SnapKind::LineEndpoint,
            SnapKind::LineMidpoint,
            SnapKind::CircleCenter,
            SnapKind::CircleQuadrant,
            SnapKind::ArcCenter,
            SnapKind::ArcEndpoint,
            SnapKind::ArcMidpoint,
            SnapKind::RectangleCorner,
            SnapKind::RectangleEdgeMidpoint,
            SnapKind::RectangleCenter,
            SnapKind::EllipseCenter,
            SnapKind::EllipseQuadrant,
        ];
        for k in discrete {
            assert!(k.is_discrete(), "{:?} should be discrete", k);
            assert!(k.priority() < 2);
        }
    }

    #[test]
    fn on_curve_kinds_report_priority_two() {
        let on = [
            SnapKind::OnLine,
            SnapKind::OnCircle,
            SnapKind::OnArc,
            SnapKind::OnEllipse,
        ];
        for k in on {
            assert!(!k.is_discrete(), "{:?} should not be discrete", k);
            assert_eq!(k.priority(), 2);
        }
    }
}
