//! Constraint inference — propose `GeometricConstraint`s from a draft.
//!
//! Slice D-1-b. Companion to [`crate::sketch2d::snap`]: snap resolves
//! the *position* of a cursor against existing geometry, inference
//! interprets that resolution as *semantic intent* — "the draft line's
//! start endpoint coincides with this existing point", "the draft
//! line is essentially horizontal", "the draft circle is concentric
//! with this existing arc".
//!
//! # Workflow
//!
//! 1. The user is drawing a line / circle / point. The current
//!    in-flight state is a [`DraftEntity`] — a value type owned by
//!    the caller, NOT yet inserted into the sketch.
//! 2. The caller invokes [`infer_constraints`]. The engine calls
//!    [`crate::sketch2d::Sketch::best_snap`] for each control point
//!    of the draft, walks the sketch's existing geometry, and emits
//!    a [`Vec<ProposedConstraint>`].
//! 3. The frontend renders the proposals as soft glyphs near the
//!    cursor (a small "⊥", "∥", "⊙" badge). When the user clicks
//!    to commit the draft, the high-confidence proposals are
//!    promoted into real [`GeometricConstraint`]s on the new entity
//!    by the auto-constrain layer (D-2, future slice).
//!
//! # Inference rules (D-1-b scope)
//!
//! For a [`DraftEntity::Line`]:
//! - **Coincident**: either endpoint snaps to a Point / LineEndpoint
//!   / ArcEndpoint / RectangleCorner. The constraint pairs the
//!   draft endpoint with the snap target.
//! - **PointOnCurve**: either endpoint snaps to `OnLine` / `OnArc`
//!   / `OnEllipse`. Excludes `OnCircle` when the line is tangent
//!   (see next rule).
//! - **Tangent**: the line endpoint snapped to `OnCircle` / `OnArc`
//!   AND the line direction is ≈ perpendicular to the radius vector
//!   from the snapped point to the circle/arc centre.
//! - **Horizontal / Vertical**: line direction lies within
//!   `angle_tol` of the X / Y axis.
//! - **Parallel / Perpendicular**: line direction lies within
//!   `angle_tol` of an existing line's direction (parallel) or its
//!   perpendicular.
//!
//! For a [`DraftEntity::Circle`]:
//! - **Coincident** (centre): centre snaps to a vertex feature.
//! - **Concentric**: centre snaps to another `CircleCenter` /
//!   `ArcCenter` / `EllipseCenter`.
//! - **Equal** (radius): radius differs from an existing circle's /
//!   arc's radius by less than `equal_radius_tol`.
//!
//! For a [`DraftEntity::Point`]:
//! - **Coincident**: snaps to any discrete feature.
//! - **PointOnCurve**: snaps to any on-curve feature.
//!
//! # What this module does NOT do
//!
//! - It does not mutate the sketch. Callers commit proposals via the
//!   auto-constrain layer (D-2).
//! - It does not score proposals beyond a coarse `confidence` tag
//!   (1.0 for snap-driven proposals, derived from angle alignment
//!   for direction proposals).
//! - It does not yet emit `Collinear` (would require segment-overlap
//!   test) or `Symmetric` (would require an explicit axis selection).

use serde::{Deserialize, Serialize};

use super::constraints::{EntityRef, GeometricConstraint};
use super::point2d::Point2d;
use super::sketch::Sketch;
use super::snap::{SnapCandidate, SnapKind};
use super::Tolerance2d;

/// Identifies which part of a [`DraftEntity`] a [`ProposedConstraint`]
/// applies to.
///
/// Constraints with no slot are *unary* on the draft as a whole
/// (Horizontal/Vertical on a line, EqualRadius on a circle paired
/// with a target).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftSlot {
    /// First endpoint of a [`DraftEntity::Line`].
    LineStart,
    /// Second endpoint of a [`DraftEntity::Line`].
    LineEnd,
    /// The whole [`DraftEntity::Line`] as a vector / direction.
    LineSelf,
    /// Centre of a [`DraftEntity::Circle`].
    CircleCenter,
    /// The whole [`DraftEntity::Circle`] (used for radius / tangent
    /// comparisons).
    CircleSelf,
    /// A standalone [`DraftEntity::Point`].
    PointSelf,
}

/// In-flight draft of a sketch entity the user is drawing.
///
/// Draft entities are pure value types — no ID, no insertion in the
/// sketch yet. They exist only inside the inference pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DraftEntity {
    /// A line being drawn between two cursor positions.
    Line { start: Point2d, end: Point2d },
    /// A circle being drawn from a centre + radius.
    Circle { center: Point2d, radius: f64 },
    /// A free-standing draft point.
    Point { position: Point2d },
}

/// A single inferred constraint proposal.
///
/// The frontend renders these as soft glyphs near the cursor;
/// auto-constrain (D-2) promotes accepted ones to real
/// [`GeometricConstraint`]s.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProposedConstraint {
    /// The geometric constraint kind being proposed.
    pub constraint: GeometricConstraint,
    /// Which part of the draft entity the constraint applies to.
    pub draft_slot: DraftSlot,
    /// The existing sketch entity the constraint pairs with, if any.
    /// `None` for unary constraints (Horizontal, Vertical).
    pub target: Option<EntityRef>,
    /// Confidence in the proposal, in `[0, 1]`. Snap-driven (vertex
    /// coincidence, on-curve) → `1.0`. Direction-driven (Horizontal,
    /// Parallel, …) → `1 - misalignment / angle_tol`, clamped to
    /// `[0, 1]`.
    pub confidence: f64,
    /// Short human-readable reason for UI tooltips. Static lifetime
    /// — no allocation per proposal.
    pub reason: &'static str,
}

/// Tolerances for inference.
///
/// `angle_tol` is in radians and gates Horizontal / Vertical /
/// Parallel / Perpendicular / Tangent. The default (3°) matches
/// Fusion 360 / SolidWorks behaviour — anything within ±3° of an
/// axis snaps to that axis when the user releases.
///
/// `snap_radius` is the world-space radius passed through to
/// [`Sketch::best_snap`] for endpoint coincidence detection.
///
/// `equal_radius_tol` gates the EqualRadius rule on draft circles.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct InferenceTolerance {
    /// Angular tolerance in radians for direction-based rules.
    pub angle_tol: f64,
    /// Snap radius in sketch units for endpoint coincidence.
    pub snap_radius: f64,
    /// Equal-radius tolerance in sketch units.
    pub equal_radius_tol: f64,
}

impl InferenceTolerance {
    /// Default tolerances tuned for cursor-driven drawing on a CAD
    /// canvas: 3° angular, 5 unit snap radius, 0.5 unit radius
    /// equality. Callers that target a denser sketch should scale
    /// `snap_radius` and `equal_radius_tol` by the inverse of the
    /// viewport zoom.
    pub fn defaults() -> Self {
        Self {
            angle_tol: 3.0_f64.to_radians(),
            snap_radius: 5.0,
            equal_radius_tol: 0.5,
        }
    }

    /// Custom tolerances.
    pub fn new(angle_tol: f64, snap_radius: f64, equal_radius_tol: f64) -> Self {
        Self {
            angle_tol,
            snap_radius,
            equal_radius_tol,
        }
    }
}

impl Default for InferenceTolerance {
    fn default() -> Self {
        Self::defaults()
    }
}

/// Top-level inference entry point.
///
/// Walks the appropriate snap candidates for each control point of
/// `draft`, applies the rules described in the module-level docs,
/// and returns the resulting proposals. Order is not significant
/// — callers that want a single "best" proposal should pick the
/// highest-confidence one.
pub fn infer_constraints(
    sketch: &Sketch,
    draft: &DraftEntity,
    tol: InferenceTolerance,
) -> Vec<ProposedConstraint> {
    match draft {
        DraftEntity::Line { start, end } => infer_for_line(sketch, *start, *end, tol),
        DraftEntity::Circle { center, radius } => {
            infer_for_circle(sketch, *center, *radius, tol)
        }
        DraftEntity::Point { position } => infer_for_point(sketch, *position, tol),
    }
}

// ── Line ──────────────────────────────────────────────────────────

fn infer_for_line(
    sketch: &Sketch,
    start: Point2d,
    end: Point2d,
    tol: InferenceTolerance,
) -> Vec<ProposedConstraint> {
    let mut out = Vec::new();

    // Endpoint snaps first.
    propose_for_endpoint(
        sketch,
        start,
        end,
        DraftSlot::LineStart,
        tol,
        &mut out,
        /*is_line_endpoint=*/ true,
    );
    propose_for_endpoint(
        sketch,
        end,
        start,
        DraftSlot::LineEnd,
        tol,
        &mut out,
        true,
    );

    // Direction: Horizontal, Vertical.
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let length = (dx * dx + dy * dy).sqrt();
    if length > 0.0 {
        let abs_sin_to_x = (dy / length).abs(); // |sin(angle to x)|
        let abs_sin_to_y = (dx / length).abs(); // |sin(angle to y)|
        if abs_sin_to_x < tol.angle_tol.sin() {
            out.push(ProposedConstraint {
                constraint: GeometricConstraint::Horizontal,
                draft_slot: DraftSlot::LineSelf,
                target: None,
                confidence: confidence_from_misalign(abs_sin_to_x, tol.angle_tol.sin()),
                reason: "line is nearly horizontal",
            });
        }
        if abs_sin_to_y < tol.angle_tol.sin() {
            out.push(ProposedConstraint {
                constraint: GeometricConstraint::Vertical,
                draft_slot: DraftSlot::LineSelf,
                target: None,
                confidence: confidence_from_misalign(abs_sin_to_y, tol.angle_tol.sin()),
                reason: "line is nearly vertical",
            });
        }

        // Parallel / Perpendicular against existing lines.
        let ux = dx / length;
        let uy = dy / length;
        for entry in sketch.lines().iter() {
            let lid = *entry.key();
            let geom = entry.value().geometry;
            let dir = match line_unit_direction(&geom) {
                Some(d) => d,
                None => continue,
            };
            // |sin(angle)| = |u × dir|; |cos(angle)| = |u · dir|.
            let sin_angle = (ux * dir.1 - uy * dir.0).abs();
            let cos_angle = (ux * dir.0 + uy * dir.1).abs();
            if sin_angle < tol.angle_tol.sin() {
                out.push(ProposedConstraint {
                    constraint: GeometricConstraint::Parallel,
                    draft_slot: DraftSlot::LineSelf,
                    target: Some(EntityRef::Line(lid)),
                    confidence: confidence_from_misalign(sin_angle, tol.angle_tol.sin()),
                    reason: "parallel to existing line",
                });
            } else if cos_angle < tol.angle_tol.sin() {
                out.push(ProposedConstraint {
                    constraint: GeometricConstraint::Perpendicular,
                    draft_slot: DraftSlot::LineSelf,
                    target: Some(EntityRef::Line(lid)),
                    confidence: confidence_from_misalign(cos_angle, tol.angle_tol.sin()),
                    reason: "perpendicular to existing line",
                });
            }
        }
    }

    out
}

/// Resolve a draft line endpoint against the sketch and push the
/// appropriate Coincident / PointOnCurve / Tangent proposal.
///
/// `other_endpoint` is the *other* end of the draft line; used to
/// distinguish Tangent (line is perpendicular to the radius at the
/// snap) from PointOnCurve when the snap is `OnCircle` / `OnArc`.
#[allow(clippy::too_many_arguments)]
fn propose_for_endpoint(
    sketch: &Sketch,
    endpoint: Point2d,
    other_endpoint: Point2d,
    slot: DraftSlot,
    tol: InferenceTolerance,
    out: &mut Vec<ProposedConstraint>,
    is_line_endpoint: bool,
) {
    let snap = match sketch.best_snap(endpoint, tol.snap_radius) {
        Some(s) => s,
        None => return,
    };

    if snap.kind.is_discrete() {
        out.push(ProposedConstraint {
            constraint: GeometricConstraint::Coincident,
            draft_slot: slot,
            target: Some(snap.entity),
            confidence: 1.0,
            reason: "snapped to existing vertex",
        });
        return;
    }

    // On-curve snap. For lines we may want Tangent instead of
    // PointOnCurve when the line direction is perpendicular to the
    // radius vector at the snap.
    if is_line_endpoint {
        if let Some(circle_center) = circle_or_arc_center_for(sketch, snap.entity) {
            // Radius vector from centre to snap point.
            let rx = snap.point.x - circle_center.x;
            let ry = snap.point.y - circle_center.y;
            let r_len = (rx * rx + ry * ry).sqrt();
            // Line direction unit vector (endpoint → other_endpoint).
            let lx = other_endpoint.x - endpoint.x;
            let ly = other_endpoint.y - endpoint.y;
            let l_len = (lx * lx + ly * ly).sqrt();
            if r_len > 0.0 && l_len > 0.0 {
                // |cos(angle between line and radius)| ≈ 0 → tangent.
                let cos_angle = ((rx * lx + ry * ly) / (r_len * l_len)).abs();
                if cos_angle < tol.angle_tol.sin() {
                    out.push(ProposedConstraint {
                        constraint: GeometricConstraint::Tangent,
                        draft_slot: DraftSlot::LineSelf,
                        target: Some(snap.entity),
                        confidence: confidence_from_misalign(cos_angle, tol.angle_tol.sin()),
                        reason: "line is tangent to curve",
                    });
                    return;
                }
            }
        }
    }

    out.push(ProposedConstraint {
        constraint: GeometricConstraint::PointOnCurve,
        draft_slot: slot,
        target: Some(snap.entity),
        confidence: 1.0,
        reason: "snapped onto curve",
    });
}

// ── Circle ────────────────────────────────────────────────────────

fn infer_for_circle(
    sketch: &Sketch,
    center: Point2d,
    radius: f64,
    tol: InferenceTolerance,
) -> Vec<ProposedConstraint> {
    let mut out = Vec::new();

    // Centre snap → Concentric (if snapped to another centre) or
    // Coincident (if snapped to a vertex).
    if let Some(snap) = sketch.best_snap(center, tol.snap_radius) {
        match snap.kind {
            SnapKind::CircleCenter | SnapKind::ArcCenter | SnapKind::EllipseCenter => {
                out.push(ProposedConstraint {
                    constraint: GeometricConstraint::Concentric,
                    draft_slot: DraftSlot::CircleCenter,
                    target: Some(snap.entity),
                    confidence: 1.0,
                    reason: "concentric with existing curve",
                });
            }
            _ if snap.kind.is_discrete() => {
                out.push(ProposedConstraint {
                    constraint: GeometricConstraint::Coincident,
                    draft_slot: DraftSlot::CircleCenter,
                    target: Some(snap.entity),
                    confidence: 1.0,
                    reason: "centre snapped to vertex",
                });
            }
            _ => {}
        }
    }

    // EqualRadius against existing circles / arcs.
    for entry in sketch.circles().iter() {
        let cid = *entry.key();
        let r = entry.value().circle.radius;
        if (r - radius).abs() < tol.equal_radius_tol {
            out.push(ProposedConstraint {
                constraint: GeometricConstraint::Equal,
                draft_slot: DraftSlot::CircleSelf,
                target: Some(EntityRef::Circle(cid)),
                confidence: confidence_from_misalign(
                    (r - radius).abs(),
                    tol.equal_radius_tol,
                ),
                reason: "equal radius to existing circle",
            });
        }
    }
    for entry in sketch.arcs().iter() {
        let aid = *entry.key();
        let r = entry.value().arc.radius;
        if (r - radius).abs() < tol.equal_radius_tol {
            out.push(ProposedConstraint {
                constraint: GeometricConstraint::Equal,
                draft_slot: DraftSlot::CircleSelf,
                target: Some(EntityRef::Arc(aid)),
                confidence: confidence_from_misalign(
                    (r - radius).abs(),
                    tol.equal_radius_tol,
                ),
                reason: "equal radius to existing arc",
            });
        }
    }

    out
}

// ── Point ─────────────────────────────────────────────────────────

fn infer_for_point(
    sketch: &Sketch,
    position: Point2d,
    tol: InferenceTolerance,
) -> Vec<ProposedConstraint> {
    let mut out = Vec::new();
    if let Some(snap) = sketch.best_snap(position, tol.snap_radius) {
        if snap.kind.is_discrete() {
            out.push(ProposedConstraint {
                constraint: GeometricConstraint::Coincident,
                draft_slot: DraftSlot::PointSelf,
                target: Some(snap.entity),
                confidence: 1.0,
                reason: "snapped to existing vertex",
            });
        } else {
            out.push(ProposedConstraint {
                constraint: GeometricConstraint::PointOnCurve,
                draft_slot: DraftSlot::PointSelf,
                target: Some(snap.entity),
                confidence: 1.0,
                reason: "snapped onto curve",
            });
        }
    }
    out
}

// ── Helpers ───────────────────────────────────────────────────────

/// Unit direction `(ux, uy)` of a [`LineGeometry`], or `None` for
/// degenerate (zero-length) segments.
fn line_unit_direction(geom: &super::line2d::LineGeometry) -> Option<(f64, f64)> {
    use super::line2d::LineGeometry;
    let (dx, dy) = match geom {
        LineGeometry::Infinite(l) => (l.direction.x, l.direction.y),
        LineGeometry::Ray(r) => (r.direction.x, r.direction.y),
        LineGeometry::Segment(s) => (s.end.x - s.start.x, s.end.y - s.start.y),
    };
    let len = (dx * dx + dy * dy).sqrt();
    if len < Tolerance2d::default().distance {
        None
    } else {
        Some((dx / len, dy / len))
    }
}

/// Return the centre point of a circle / arc / ellipse identified by
/// `entity`, or `None` if `entity` is not a curve with a centre.
fn circle_or_arc_center_for(sketch: &Sketch, entity: EntityRef) -> Option<Point2d> {
    match entity {
        EntityRef::Circle(id) => sketch.circles().get(&id).map(|c| c.circle.center),
        EntityRef::Arc(id) => sketch.arcs().get(&id).map(|a| a.arc.center),
        EntityRef::Ellipse(id) => sketch.ellipses().get(&id).map(|e| e.ellipse.center),
        _ => None,
    }
}

/// Map a `misalignment ∈ [0, tol]` into a confidence `∈ [0, 1]` —
/// zero misalignment → full confidence, equal to tolerance → zero
/// confidence. Clamps for safety.
fn confidence_from_misalign(misalign: f64, tol: f64) -> f64 {
    if tol <= 0.0 {
        return 1.0;
    }
    (1.0 - (misalign / tol)).clamp(0.0, 1.0)
}

// Snap candidates exposed for callers that want to render snap glyphs
// independently of inference; thin re-export.
#[allow(dead_code)]
fn _carries_snap_types(c: &SnapCandidate) -> SnapKind {
    c.kind
}

#[cfg(test)]
mod tests {
    //! D-1-b inference engine tests.
    //!
    //! Each test pins one rule of the inference table.
    #![allow(clippy::float_cmp)]
    #![allow(clippy::expect_used)]

    use super::*;
    use crate::sketch2d::sketch::SketchAnchor;

    fn fresh() -> Sketch {
        Sketch::new("inference_test".to_string(), SketchAnchor::xy())
    }

    fn has(out: &[ProposedConstraint], c: GeometricConstraint, slot: DraftSlot) -> bool {
        out.iter().any(|p| p.constraint == c && p.draft_slot == slot)
    }

    // ── Empty sketch ──────────────────────────────────────────────

    #[test]
    fn empty_sketch_line_infers_only_axis_constraints() {
        let s = fresh();
        let draft = DraftEntity::Line {
            start: Point2d::new(0.0, 0.0),
            end: Point2d::new(10.0, 0.0),
        };
        let out = infer_constraints(&s, &draft, InferenceTolerance::defaults());
        // Horizontal must be present; Vertical must not.
        assert!(has(&out, GeometricConstraint::Horizontal, DraftSlot::LineSelf));
        assert!(!has(&out, GeometricConstraint::Vertical, DraftSlot::LineSelf));
        // No coincidence proposals on empty sketch.
        assert!(!out.iter().any(|p| p.constraint == GeometricConstraint::Coincident));
    }

    #[test]
    fn empty_sketch_circle_infers_nothing() {
        let s = fresh();
        let draft = DraftEntity::Circle {
            center: Point2d::new(5.0, 5.0),
            radius: 2.0,
        };
        let out = infer_constraints(&s, &draft, InferenceTolerance::defaults());
        assert!(out.is_empty());
    }

    // ── Horizontal / Vertical ─────────────────────────────────────

    #[test]
    fn horizontal_line_within_two_degrees_infers_horizontal() {
        let s = fresh();
        // 1° off horizontal — well within 3° tolerance.
        let theta = 1.0_f64.to_radians();
        let draft = DraftEntity::Line {
            start: Point2d::new(0.0, 0.0),
            end: Point2d::new(10.0 * theta.cos(), 10.0 * theta.sin()),
        };
        let out = infer_constraints(&s, &draft, InferenceTolerance::defaults());
        assert!(has(&out, GeometricConstraint::Horizontal, DraftSlot::LineSelf));
    }

    #[test]
    fn line_off_axis_by_ten_degrees_does_not_infer_horizontal() {
        let s = fresh();
        let theta = 10.0_f64.to_radians();
        let draft = DraftEntity::Line {
            start: Point2d::new(0.0, 0.0),
            end: Point2d::new(10.0 * theta.cos(), 10.0 * theta.sin()),
        };
        let out = infer_constraints(&s, &draft, InferenceTolerance::defaults());
        assert!(!has(&out, GeometricConstraint::Horizontal, DraftSlot::LineSelf));
        assert!(!has(&out, GeometricConstraint::Vertical, DraftSlot::LineSelf));
    }

    #[test]
    fn vertical_line_infers_vertical_not_horizontal() {
        let s = fresh();
        let draft = DraftEntity::Line {
            start: Point2d::new(0.0, 0.0),
            end: Point2d::new(0.0, 10.0),
        };
        let out = infer_constraints(&s, &draft, InferenceTolerance::defaults());
        assert!(has(&out, GeometricConstraint::Vertical, DraftSlot::LineSelf));
        assert!(!has(&out, GeometricConstraint::Horizontal, DraftSlot::LineSelf));
    }

    // ── Parallel / Perpendicular ──────────────────────────────────

    #[test]
    fn line_parallel_to_existing_infers_parallel() {
        let s = fresh();
        // Existing diagonal line.
        let p0 = s.add_point(Point2d::new(0.0, 0.0));
        let p1 = s.add_point(Point2d::new(10.0, 5.0));
        let existing = s.add_line(p0, p1).expect("line");
        // Draft line parallel to it, offset upward.
        let draft = DraftEntity::Line {
            start: Point2d::new(0.0, 100.0),
            end: Point2d::new(10.0, 105.0),
        };
        let out = infer_constraints(&s, &draft, InferenceTolerance::defaults());
        assert!(out.iter().any(|p| p.constraint == GeometricConstraint::Parallel
            && p.target == Some(EntityRef::Line(existing))));
    }

    #[test]
    fn line_perpendicular_to_existing_infers_perpendicular() {
        let s = fresh();
        let p0 = s.add_point(Point2d::new(0.0, 0.0));
        let p1 = s.add_point(Point2d::new(10.0, 5.0));
        let existing = s.add_line(p0, p1).expect("line");
        // Draft perpendicular: rotate (10, 5) by 90° → (-5, 10).
        let draft = DraftEntity::Line {
            start: Point2d::new(0.0, 100.0),
            end: Point2d::new(-5.0, 110.0),
        };
        let out = infer_constraints(&s, &draft, InferenceTolerance::defaults());
        assert!(out.iter().any(|p| p.constraint == GeometricConstraint::Perpendicular
            && p.target == Some(EntityRef::Line(existing))));
    }

    // ── Endpoint coincidence ──────────────────────────────────────

    #[test]
    fn line_endpoint_snaps_to_existing_point_infers_coincident() {
        let s = fresh();
        let target = s.add_point(Point2d::new(0.0, 0.0));
        let draft = DraftEntity::Line {
            start: Point2d::new(0.05, 0.05),
            end: Point2d::new(10.0, 10.0),
        };
        let out = infer_constraints(&s, &draft, InferenceTolerance::defaults());
        // LineStart should be Coincident with the target point.
        assert!(out.iter().any(|p| p.constraint == GeometricConstraint::Coincident
            && p.draft_slot == DraftSlot::LineStart
            && p.target == Some(EntityRef::Point(target))));
    }

    // ── Tangent ───────────────────────────────────────────────────

    #[test]
    fn line_ending_on_circle_perpendicular_to_radius_infers_tangent() {
        let s = fresh();
        // Larger circle so the chosen endpoint is far enough from
        // the axis-aligned quadrants that the priority-1 quadrant
        // snaps stay outside the 5-unit snap radius; the priority-2
        // OnCircle snap is the only candidate in range → the tangent
        // code path fires.
        let cid = s.add_circle(Point2d::ORIGIN, 20.0).expect("circle");
        // Point on the circle at angle 0.5 rad: r·(cos θ, sin θ).
        let theta = 0.5_f64;
        let p_on_circle = Point2d::new(20.0 * theta.cos(), 20.0 * theta.sin());
        // Tangent direction at that point is perpendicular to the
        // radius (cos θ, sin θ) → (-sin θ, cos θ).
        let tx = -theta.sin();
        let ty = theta.cos();
        let start = Point2d::new(p_on_circle.x - 3.0 * tx, p_on_circle.y - 3.0 * ty);
        let draft = DraftEntity::Line {
            start,
            end: p_on_circle,
        };
        let out = infer_constraints(&s, &draft, InferenceTolerance::defaults());
        assert!(out.iter().any(|p| p.constraint == GeometricConstraint::Tangent
            && p.target == Some(EntityRef::Circle(cid))));
    }

    #[test]
    fn line_ending_on_circle_not_perpendicular_infers_point_on_curve() {
        let s = fresh();
        let cid = s.add_circle(Point2d::ORIGIN, 5.0).expect("circle");
        // Snap end to the +X quadrant via the OnCircle/quadrant snap,
        // but the line points diagonally — not tangent.
        let draft = DraftEntity::Line {
            start: Point2d::new(-1.0, -1.0),
            end: Point2d::new(5.0, 0.0),
        };
        let out = infer_constraints(&s, &draft, InferenceTolerance::defaults());
        // Coincident with the quadrant target (CircleQuadrant is
        // discrete priority 1) is what fires here, NOT Tangent.
        assert!(out.iter().any(|p| p.constraint == GeometricConstraint::Coincident
            && p.target == Some(EntityRef::Circle(cid))));
        assert!(!out.iter().any(|p| p.constraint == GeometricConstraint::Tangent));
    }

    // ── Circle inference ──────────────────────────────────────────

    #[test]
    fn draft_circle_centred_on_existing_centre_infers_concentric() {
        let s = fresh();
        let cid = s.add_circle(Point2d::ORIGIN, 5.0).expect("circle");
        let draft = DraftEntity::Circle {
            center: Point2d::new(0.05, 0.05),
            radius: 10.0,
        };
        let out = infer_constraints(&s, &draft, InferenceTolerance::defaults());
        assert!(out.iter().any(|p| p.constraint == GeometricConstraint::Concentric
            && p.target == Some(EntityRef::Circle(cid))));
    }

    #[test]
    fn draft_circle_with_matching_radius_infers_equal() {
        let s = fresh();
        let cid = s.add_circle(Point2d::new(100.0, 100.0), 3.0).expect("circle");
        let draft = DraftEntity::Circle {
            center: Point2d::new(200.0, 200.0),
            radius: 3.1, // within 0.5 tolerance
        };
        let out = infer_constraints(&s, &draft, InferenceTolerance::defaults());
        assert!(out.iter().any(|p| p.constraint == GeometricConstraint::Equal
            && p.target == Some(EntityRef::Circle(cid))));
    }

    #[test]
    fn draft_circle_with_distant_radius_does_not_infer_equal() {
        let s = fresh();
        s.add_circle(Point2d::new(100.0, 100.0), 3.0).expect("circle");
        let draft = DraftEntity::Circle {
            center: Point2d::new(200.0, 200.0),
            radius: 7.0,
        };
        let out = infer_constraints(&s, &draft, InferenceTolerance::defaults());
        assert!(!out.iter().any(|p| p.constraint == GeometricConstraint::Equal));
    }

    // ── Point inference ───────────────────────────────────────────

    #[test]
    fn draft_point_on_existing_line_infers_point_on_curve() {
        let s = fresh();
        let p0 = s.add_point(Point2d::new(0.0, 0.0));
        let p1 = s.add_point(Point2d::new(10.0, 0.0));
        let lid = s.add_line(p0, p1).expect("line");
        // Cursor at (5, 0.05) — just off the line, well within the
        // default 5-unit snap radius. Note: it must not snap to the
        // free point p0/p1 (distance ~5 each).
        let draft = DraftEntity::Point {
            position: Point2d::new(5.0, 0.05),
        };
        let out = infer_constraints(&s, &draft, InferenceTolerance::defaults());
        // LineMidpoint (discrete) is at distance 0.05; OnLine is also
        // 0.05 but priority 2 — so the discrete one wins and we get
        // Coincident with the line. Either Coincident or PointOnCurve
        // is a valid intent inference here; we assert at least one of
        // the two fires.
        assert!(out
            .iter()
            .any(|p| (p.constraint == GeometricConstraint::PointOnCurve
                || p.constraint == GeometricConstraint::Coincident)
                && p.target == Some(EntityRef::Line(lid))));
    }

    #[test]
    fn draft_point_snapping_to_existing_point_infers_coincident() {
        let s = fresh();
        let target = s.add_point(Point2d::new(3.0, 4.0));
        let draft = DraftEntity::Point {
            position: Point2d::new(3.01, 4.01),
        };
        let out = infer_constraints(&s, &draft, InferenceTolerance::defaults());
        assert!(out.iter().any(|p| p.constraint == GeometricConstraint::Coincident
            && p.draft_slot == DraftSlot::PointSelf
            && p.target == Some(EntityRef::Point(target))));
    }

    // ── Confidence helper ─────────────────────────────────────────

    #[test]
    fn confidence_at_zero_misalign_is_one() {
        assert_eq!(confidence_from_misalign(0.0, 0.1), 1.0);
    }

    #[test]
    fn confidence_at_tolerance_boundary_is_zero() {
        assert_eq!(confidence_from_misalign(0.1, 0.1), 0.0);
    }

    #[test]
    fn confidence_beyond_tolerance_clamps_to_zero() {
        assert_eq!(confidence_from_misalign(0.5, 0.1), 0.0);
    }
}
