//! Sketch operations — trim / extend / offset / mirror / patterns
//! (SKETCH-DCM #45 Slice 6, spec §3.4).
//!
//! Design contract shared by every op:
//!
//! 1. **Validate first, mutate second.** A typed refusal
//!    ([`SketchOpError`]) must leave the sketch byte-identical — all
//!    geometry computation and support checks run before the first
//!    entity is created or deleted.
//! 2. **Maintained, not one-shot.** Each op mints the constraints
//!    that make the solver PRESERVE its result: trim/extend mint
//!    `PointOnCurve` contact at the cut/extension, offset mints the
//!    Slice-6-enforced `Offset`/`OffsetDistance` pairs, mirror mints
//!    `Symmetric` (+ `Equal` radius), patterns mint the
//!    `Equal`-chain / `Distance`-spacing / `Angle`-spoke web the
//!    Slice-2/3 decomposition handles well.
//! 3. **Provenance from day one.** Every minted entity records an
//!    [`EntityProvenance`] on the sketch (persistent-ids campaign #11
//!    note: the 3D pattern ops shipped without lineage — the 2D ops
//!    must not repeat that debt).
//! 4. **Re-certifiable.** Ops leave the sketch in a state
//!    `certify_sketch` / `analyze_dofs` account for exactly; the
//!    slice gates hand-count the DOF arithmetic.
//!
//! Scope boundaries (typed refusals, spec §3.4 / §3.6): offset
//! handles line/arc loops and lone circles — NURBS/ellipse offset
//! approximation is out of scope; patterns maintain point/circle
//! instances (the hole-pattern flagship); mirror requires a
//! construction axis and shared-endpoint arcs.

use super::constraints::{
    Constraint, ConstraintId, ConstraintPriority, DimensionalConstraint, EntityRef,
    GeometricConstraint,
};
use super::line2d::LineGeometry;
use super::sketch_topology::{AnalyticLoop, ProfileEdge, ProfileExtractor, SketchTopology};
use super::{
    Arc2d, Circle2d, Line2d, Line2dId, Point2d, Point2dId, Sketch, Sketch2dError, Tolerance2d,
    Vector2d,
};
use serde::{Deserialize, Serialize};
use std::f64::consts::{PI, TAU};

/// Positional tolerance for op-level joins and interior filtering.
/// Sketch ops work on user-scale geometry (millimetre-class), so this
/// sits well above f64 noise and well below any feature size.
const OP_EPS: f64 = 1e-9;
/// Join tolerance for tangent-continuous offset corners: natural
/// offset endpoints closer than this share one junction point.
const JOIN_EPS: f64 = 1e-6;

// ── Outcome model ───────────────────────────────────────────────────

/// Which sketch operation minted an entity / produced an outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SketchOpKind {
    Trim,
    Extend,
    Offset,
    Mirror,
    LinearPattern,
    CircularPattern,
    /// Instances along a spline/arc rail (SKETCH-DCM #45 Slice 7).
    CurvePattern,
    /// Vogel phyllotaxis spiral: r = c·√n, θ stepped by the exact
    /// golden angle (SKETCH-DCM #45 Slice 7 — biomimicry).
    PhyllotaxisPattern,
}

/// Op lineage of one minted entity (stored on the sketch, queryable
/// via [`Sketch::provenance_of`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityProvenance {
    /// The operation that minted the entity.
    pub op: SketchOpKind,
    /// The source entity it derives from (`None` for support geometry
    /// with no single source, e.g. offset corner arcs and pattern
    /// guide lines).
    pub source: Option<EntityRef>,
    /// Pattern instance index (1-based; the source itself is
    /// instance 0). `None` outside patterns.
    pub instance: Option<usize>,
}

/// Typed result of a sketch operation: everything an agent (or the
/// timeline event) needs to know about what changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SketchOpOutcome {
    /// The operation performed.
    pub op: SketchOpKind,
    /// Entities minted by the op, in creation order.
    pub created: Vec<EntityRef>,
    /// Entities removed (trim targets, pruned orphan endpoints).
    pub deleted: Vec<EntityRef>,
    /// Entities mutated in place (extend's line + moved endpoint).
    pub modified: Vec<EntityRef>,
    /// Maintenance constraints minted by the op.
    pub constraints_added: Vec<ConstraintId>,
    /// Constraints dropped because they referenced a deleted entity.
    pub constraints_removed: Vec<ConstraintId>,
    /// Provenance records minted (mirrors what was stored on the
    /// sketch, so the caller need not re-query).
    pub provenance: Vec<(EntityRef, EntityProvenance)>,
}

impl SketchOpOutcome {
    fn new(op: SketchOpKind) -> Self {
        Self {
            op,
            created: Vec::new(),
            deleted: Vec::new(),
            modified: Vec::new(),
            constraints_added: Vec::new(),
            constraints_removed: Vec::new(),
            provenance: Vec::new(),
        }
    }
}

/// Typed refusals — an op either fully applies or reports exactly why
/// it cannot, leaving the sketch untouched.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SketchOpError {
    /// A referenced entity does not exist in the sketch.
    #[error("{op}: entity not found: {entity}")]
    EntityNotFound { op: &'static str, entity: String },
    /// The entity kind (or its configuration) is outside the op's
    /// honest envelope.
    #[error("{op}: unsupported: {reason}")]
    Unsupported { op: &'static str, reason: String },
    /// The op needs an intersection that does not exist.
    #[error("{op}: no intersection: {reason}")]
    NoIntersection { op: &'static str, reason: String },
    /// A scalar input is out of domain.
    #[error("{op}: invalid parameter {parameter}: {reason}")]
    InvalidParameter {
        op: &'static str,
        parameter: &'static str,
        reason: String,
    },
    /// The offset distance exceeds what the loop's local feature
    /// sizes admit (an edge or arc would vanish or invert).
    #[error("offset: distance {distance} exceeds local feature size: {reason}")]
    OffsetTooLarge { distance: f64, reason: String },
    /// Mirror requires a construction-line axis (spec §3.4).
    #[error("{op}: the axis must be a construction line")]
    AxisNotConstruction { op: &'static str },
    /// A kernel-level failure while materialising the result.
    #[error(transparent)]
    Sketch(#[from] Sketch2dError),
}

// ── Shared curve utilities ──────────────────────────────────────────

/// Materialised op-level curve of an entity, read from LIVE geometry
/// (shared endpoints / centers resolve through their points).
#[derive(Debug, Clone, Copy)]
enum OpCurve {
    Segment { a: Point2d, b: Point2d },
    Arc(Arc2d),
    Circle { c: Point2d, r: f64 },
}

fn norm_angle(a: f64) -> f64 {
    a.rem_euclid(TAU)
}

/// CCW sweep from `a0` to `a1` in [0, 2π).
fn ccw_sweep(a0: f64, a1: f64) -> f64 {
    (a1 - a0).rem_euclid(TAU)
}

fn op_curve(
    sketch: &Sketch,
    entity: &EntityRef,
    op: &'static str,
) -> Result<OpCurve, SketchOpError> {
    match entity {
        EntityRef::Line(id) => {
            let entry = sketch
                .lines()
                .get(id)
                .ok_or_else(|| SketchOpError::EntityNotFound {
                    op,
                    entity: entity.to_string(),
                })?;
            // Live endpoint positions when the segment is derived from
            // shared points; the stored segment otherwise.
            if let Some((sa, sb)) = entry.endpoints {
                drop(entry);
                let a = sketch
                    .get_point(&sa)
                    .ok_or_else(|| SketchOpError::EntityNotFound {
                        op,
                        entity: sa.to_string(),
                    })?;
                let b = sketch
                    .get_point(&sb)
                    .ok_or_else(|| SketchOpError::EntityNotFound {
                        op,
                        entity: sb.to_string(),
                    })?;
                return Ok(OpCurve::Segment { a, b });
            }
            match &entry.geometry {
                LineGeometry::Segment(seg) => Ok(OpCurve::Segment {
                    a: seg.start,
                    b: seg.end,
                }),
                _ => Err(SketchOpError::Unsupported {
                    op,
                    reason: format!(
                        "{entity} is an unbounded line (ray/infinite); ops need a segment"
                    ),
                }),
            }
        }
        EntityRef::Arc(id) => {
            let entry = sketch
                .arcs()
                .get(id)
                .ok_or_else(|| SketchOpError::EntityNotFound {
                    op,
                    entity: entity.to_string(),
                })?;
            Ok(OpCurve::Arc(entry.arc))
        }
        EntityRef::Circle(id) => {
            let entry = sketch
                .circles()
                .get(id)
                .ok_or_else(|| SketchOpError::EntityNotFound {
                    op,
                    entity: entity.to_string(),
                })?;
            let r = entry.circle.radius;
            drop(entry);
            let c =
                sketch
                    .circle_center_position(id)
                    .ok_or_else(|| SketchOpError::EntityNotFound {
                        op,
                        entity: entity.to_string(),
                    })?;
            Ok(OpCurve::Circle { c, r })
        }
        other => Err(SketchOpError::Unsupported {
            op,
            reason: format!(
                "{other} is not a trim/extend curve (supported: line segment, arc, circle)"
            ),
        }),
    }
}

/// Unclamped carrier parameter of `p` projected onto segment (a, b)
/// (t = 0 at `a`, 1 at `b`).
fn seg_param(a: &Point2d, b: &Point2d, p: &Point2d) -> f64 {
    let d = Vector2d::from_points(a, b);
    let len2 = d.magnitude_squared();
    if len2 < OP_EPS * OP_EPS {
        return 0.0;
    }
    Vector2d::from_points(a, p).dot(&d) / len2
}

/// Whether `p` lies within the curve's own extent (assumes `p` is on
/// the carrier).
fn on_extent(curve: &OpCurve, p: &Point2d) -> bool {
    match curve {
        OpCurve::Segment { a, b } => {
            let t = seg_param(a, b, p);
            (-OP_EPS..=1.0 + OP_EPS).contains(&t)
        }
        OpCurve::Arc(arc) => arc.contains_angle(arc.center.angle_to(p)),
        OpCurve::Circle { .. } => true,
    }
}

fn carrier_circle(curve: &OpCurve, op: &'static str) -> Result<Circle2d, SketchOpError> {
    let (c, r) = match curve {
        OpCurve::Arc(arc) => (arc.center, arc.radius),
        OpCurve::Circle { c, r } => (*c, *r),
        OpCurve::Segment { .. } => {
            return Err(SketchOpError::Unsupported {
                op,
                reason: "internal: segment has no carrier circle".to_string(),
            })
        }
    };
    Circle2d::new(c, r).map_err(SketchOpError::from)
}

/// Extent-respecting intersection points of two op curves.
fn intersections(
    a: &OpCurve,
    b: &OpCurve,
    op: &'static str,
) -> Result<Vec<Point2d>, SketchOpError> {
    let carrier_points: Vec<Point2d> = match (a, b) {
        (OpCurve::Segment { a: a0, b: a1 }, OpCurve::Segment { a: b0, b: b1 }) => {
            let d1 = Vector2d::from_points(a0, a1);
            let d2 = Vector2d::from_points(b0, b1);
            let cross = d1.cross(&d2);
            if cross.abs() < OP_EPS {
                Vec::new() // parallel carriers
            } else {
                let dp = Vector2d::from_points(a0, b0);
                let t = dp.cross(&d2) / cross;
                vec![Point2d::new(a0.x + t * d1.x, a0.y + t * d1.y)]
            }
        }
        (OpCurve::Segment { a: s0, b: s1 }, other) | (other, OpCurve::Segment { a: s0, b: s1 }) => {
            let circle = carrier_circle(other, op)?;
            let dir = Vector2d::from_points(s0, s1);
            circle.intersect_line(s0, &dir).unwrap_or_default()
        }
        (ca, cb) => {
            let c1 = carrier_circle(ca, op)?;
            let c2 = carrier_circle(cb, op)?;
            c1.intersect_circle(&c2).unwrap_or_default()
        }
    };
    Ok(carrier_points
        .into_iter()
        .filter(|p| on_extent(a, p) && on_extent(b, p))
        .collect())
}

/// Is the point referenced by any surviving entity or constraint (or
/// pinned by the user)? Used by trim's orphan pruning.
fn point_in_use(sketch: &Sketch, pid: &Point2dId) -> bool {
    if let Some(p) = sketch.points().get(pid) {
        if p.is_fixed {
            return true;
        }
    } else {
        return false;
    }
    if sketch
        .lines()
        .iter()
        .any(|e| matches!(e.endpoints, Some((a, b)) if a == *pid || b == *pid))
    {
        return true;
    }
    if sketch.arcs().iter().any(|e| {
        matches!(e.endpoints, Some((a, b)) if a == *pid || b == *pid)
            || e.center_point == Some(*pid)
    }) {
        return true;
    }
    if sketch
        .circles()
        .iter()
        .any(|e| e.center_point == Some(*pid))
    {
        return true;
    }
    !sketch
        .get_constraints_by_entity(&EntityRef::Point(*pid))
        .is_empty()
}

fn mint_point(
    sketch: &Sketch,
    pos: Point2d,
    op: SketchOpKind,
    source: Option<EntityRef>,
    instance: Option<usize>,
    outcome: &mut SketchOpOutcome,
) -> Point2dId {
    let id = sketch.add_point(pos);
    let eref = EntityRef::Point(id);
    let prov = EntityProvenance {
        op,
        source,
        instance,
    };
    sketch.set_provenance(eref, prov.clone());
    outcome.created.push(eref);
    outcome.provenance.push((eref, prov));
    id
}

fn record_created(
    sketch: &Sketch,
    eref: EntityRef,
    op: SketchOpKind,
    source: Option<EntityRef>,
    instance: Option<usize>,
    outcome: &mut SketchOpOutcome,
) {
    let prov = EntityProvenance {
        op,
        source,
        instance,
    };
    sketch.set_provenance(eref, prov.clone());
    outcome.created.push(eref);
    outcome.provenance.push((eref, prov));
}

fn mint_constraint(
    sketch: &Sketch,
    constraint: Constraint,
    outcome: &mut SketchOpOutcome,
) -> ConstraintId {
    let id = sketch.add_constraint(constraint);
    outcome.constraints_added.push(id);
    id
}

fn point_on_curve(point: Point2dId, curve: EntityRef) -> Constraint {
    Constraint::new_geometric(
        GeometricConstraint::PointOnCurve,
        vec![EntityRef::Point(point), curve],
        ConstraintPriority::High,
    )
}

// ── trim ────────────────────────────────────────────────────────────

/// Cut away the span of `target` that contains `pick`, bounded by its
/// intersections with `cutter` (or the target's own ends). The
/// surviving spans become new entities re-using the original endpoint
/// points where they survive; each new cut point is held ON the
/// cutter by a minted `PointOnCurve` constraint, so the trim is
/// maintained under later solves. Constraints referencing the deleted
/// target are dropped and reported in
/// [`SketchOpOutcome::constraints_removed`].
///
/// Supported targets: line segments, arcs, circles (a circle needs at
/// least two intersections and becomes an arc). Supported cutters:
/// line segments, arcs, circles.
pub fn trim(
    sketch: &Sketch,
    target: &EntityRef,
    cutter: &EntityRef,
    pick: Point2d,
) -> Result<SketchOpOutcome, SketchOpError> {
    const OP: &str = "trim";
    if target == cutter {
        return Err(SketchOpError::InvalidParameter {
            op: OP,
            parameter: "cutter",
            reason: "cutter and target are the same entity".to_string(),
        });
    }
    let cutter_curve = op_curve(sketch, cutter, OP)?;
    let target_curve = op_curve(sketch, target, OP)?;
    let hits = intersections(&target_curve, &cutter_curve, OP)?;

    match (target, target_curve) {
        (EntityRef::Line(line_id), OpCurve::Segment { a, b }) => {
            // Interior cut parameters along the target.
            let mut ts: Vec<f64> = hits
                .iter()
                .map(|p| seg_param(&a, &b, p))
                .filter(|t| (1e-6..=1.0 - 1e-6).contains(t))
                .collect();
            ts.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
            ts.dedup_by(|x, y| (*x - *y).abs() < 1e-9);
            if ts.is_empty() {
                return Err(SketchOpError::NoIntersection {
                    op: OP,
                    reason: format!("{cutter} does not cross the interior of {target}"),
                });
            }
            let tp = seg_param(&a, &b, &pick).clamp(0.0, 1.0);
            let lo = ts.iter().copied().rfind(|t| *t < tp);
            let hi = ts.iter().copied().find(|t| *t > tp);
            if lo.is_none() && hi.is_none() {
                return Err(SketchOpError::InvalidParameter {
                    op: OP,
                    parameter: "pick",
                    reason: "pick sits exactly on the only intersection — ambiguous span"
                        .to_string(),
                });
            }

            let entry =
                sketch
                    .lines()
                    .get(line_id)
                    .ok_or_else(|| SketchOpError::EntityNotFound {
                        op: OP,
                        entity: target.to_string(),
                    })?;
            let endpoint_ids = entry.endpoints;
            drop(entry);

            let mut outcome = SketchOpOutcome::new(SketchOpKind::Trim);
            outcome.constraints_removed = sketch
                .get_constraints_by_entity(target)
                .iter()
                .map(|c| c.id)
                .collect();
            sketch.delete_line(line_id)?;
            outcome.deleted.push(*target);

            let at = |t: f64| Point2d::new(a.x + t * (b.x - a.x), a.y + t * (b.y - a.y));
            let make_end = |t: f64, outcome: &mut SketchOpOutcome| -> Point2dId {
                if t <= OP_EPS {
                    if let Some((sa, _)) = endpoint_ids {
                        return sa;
                    }
                } else if t >= 1.0 - OP_EPS {
                    if let Some((_, sb)) = endpoint_ids {
                        return sb;
                    }
                }
                let pid = mint_point(
                    sketch,
                    at(t),
                    SketchOpKind::Trim,
                    Some(*target),
                    None,
                    outcome,
                );
                // Maintained contact: the cut point rides the cutter.
                mint_constraint(sketch, point_on_curve(pid, *cutter), outcome);
                pid
            };

            let mut survivors: Vec<(f64, f64)> = Vec::new();
            if let Some(t) = lo {
                survivors.push((0.0, t));
            }
            if let Some(t) = hi {
                survivors.push((t, 1.0));
            }
            let mut kept_original = [false, false];
            for (t0, t1) in survivors {
                let start_id = make_end(t0, &mut outcome);
                let end_id = make_end(t1, &mut outcome);
                if t0 <= OP_EPS {
                    kept_original[0] = true;
                }
                if t1 >= 1.0 - OP_EPS {
                    kept_original[1] = true;
                }
                let lid = sketch.add_line(start_id, end_id)?;
                record_created(
                    sketch,
                    EntityRef::Line(lid),
                    SketchOpKind::Trim,
                    Some(*target),
                    None,
                    &mut outcome,
                );
            }

            // Prune endpoints the trim orphaned (nothing else uses
            // them and the user never pinned them).
            if let Some((sa, sb)) = endpoint_ids {
                for (kept, pid) in [(kept_original[0], sa), (kept_original[1], sb)] {
                    if !kept && !point_in_use(sketch, &pid) {
                        sketch.delete_point(&pid)?;
                        outcome.deleted.push(EntityRef::Point(pid));
                    }
                }
            }
            Ok(outcome)
        }
        (EntityRef::Circle(circle_id), OpCurve::Circle { c, r }) => {
            let mut angles: Vec<f64> = hits.iter().map(|p| norm_angle(c.angle_to(p))).collect();
            angles.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
            angles.dedup_by(|x, y| (*x - *y).abs() < 1e-9);
            if angles.len() < 2 {
                return Err(SketchOpError::NoIntersection {
                    op: OP,
                    reason: format!(
                        "trimming a closed circle needs at least two intersections with \
                         {cutter}; found {}",
                        angles.len()
                    ),
                });
            }
            let ap = norm_angle(c.angle_to(&pick));
            // The removed span is the cyclic interval (prev, next)
            // containing the pick; the survivor runs CCW next → prev.
            let next = angles
                .iter()
                .copied()
                .find(|t| *t > ap)
                .unwrap_or(angles[0]);
            let prev = angles
                .iter()
                .copied()
                .rfind(|t| *t < ap)
                .unwrap_or(*angles.last().unwrap_or(&next));
            let sweep = ccw_sweep(next, prev);
            if sweep < 1e-9 {
                return Err(SketchOpError::InvalidParameter {
                    op: OP,
                    parameter: "pick",
                    reason: "pick coincides with an intersection — ambiguous span".to_string(),
                });
            }

            let center_point = sketch.circles().get(circle_id).and_then(|e| e.center_point);

            let mut outcome = SketchOpOutcome::new(SketchOpKind::Trim);
            outcome.constraints_removed = sketch
                .get_constraints_by_entity(target)
                .iter()
                .map(|c| c.id)
                .collect();
            sketch.delete_circle(circle_id)?;
            outcome.deleted.push(*target);

            let at = |t: f64| Point2d::new(c.x + r * t.cos(), c.y + r * t.sin());
            let start_id = mint_point(
                sketch,
                at(next),
                SketchOpKind::Trim,
                Some(*target),
                None,
                &mut outcome,
            );
            mint_constraint(sketch, point_on_curve(start_id, *cutter), &mut outcome);
            let end_id = mint_point(
                sketch,
                at(prev),
                SketchOpKind::Trim,
                Some(*target),
                None,
                &mut outcome,
            );
            mint_constraint(sketch, point_on_curve(end_id, *cutter), &mut outcome);
            let aid = sketch.add_arc(start_id, end_id, r, true, sweep > PI)?;
            record_created(
                sketch,
                EntityRef::Arc(aid),
                SketchOpKind::Trim,
                Some(*target),
                None,
                &mut outcome,
            );

            // A shared center point the circle owned exclusively is
            // now orphaned.
            if let Some(cp) = center_point {
                if !point_in_use(sketch, &cp) {
                    sketch.delete_point(&cp)?;
                    outcome.deleted.push(EntityRef::Point(cp));
                }
            }
            Ok(outcome)
        }
        (EntityRef::Arc(arc_id), OpCurve::Arc(arc)) => {
            let total = arc.sweep_angle();
            if total < 1e-9 {
                return Err(SketchOpError::Unsupported {
                    op: OP,
                    reason: "degenerate arc sweep".to_string(),
                });
            }
            let fraction = |p: &Point2d| -> f64 {
                let theta = norm_angle(arc.center.angle_to(p));
                let swept = if arc.ccw {
                    ccw_sweep(arc.start_angle, theta)
                } else {
                    ccw_sweep(theta, arc.start_angle)
                };
                let f = swept / total;
                if f <= 1.0 {
                    f
                } else {
                    // Outside the span: clamp to the nearer end
                    // (cyclically).
                    let past_end = f - 1.0;
                    let before_start = TAU / total - f;
                    if past_end < before_start {
                        1.0
                    } else {
                        0.0
                    }
                }
            };
            let mut ts: Vec<f64> = hits
                .iter()
                .map(&fraction)
                .filter(|t| (1e-6..=1.0 - 1e-6).contains(t))
                .collect();
            ts.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
            ts.dedup_by(|x, y| (*x - *y).abs() < 1e-9);
            if ts.is_empty() {
                return Err(SketchOpError::NoIntersection {
                    op: OP,
                    reason: format!("{cutter} does not cross the interior of {target}"),
                });
            }
            let tp = fraction(&pick);
            let lo = ts.iter().copied().rfind(|t| *t < tp);
            let hi = ts.iter().copied().find(|t| *t > tp);
            if lo.is_none() && hi.is_none() {
                return Err(SketchOpError::InvalidParameter {
                    op: OP,
                    parameter: "pick",
                    reason: "pick sits exactly on the only intersection — ambiguous span"
                        .to_string(),
                });
            }

            let endpoint_ids = sketch.arcs().get(arc_id).and_then(|e| e.endpoints);

            let mut outcome = SketchOpOutcome::new(SketchOpKind::Trim);
            outcome.constraints_removed = sketch
                .get_constraints_by_entity(target)
                .iter()
                .map(|c| c.id)
                .collect();
            sketch.delete_arc(arc_id)?;
            outcome.deleted.push(*target);

            let angle_at = |t: f64| {
                if arc.ccw {
                    norm_angle(arc.start_angle + t * total)
                } else {
                    norm_angle(arc.start_angle - t * total)
                }
            };
            let pos_at = |t: f64| {
                let ang = angle_at(t);
                Point2d::new(
                    arc.center.x + arc.radius * ang.cos(),
                    arc.center.y + arc.radius * ang.sin(),
                )
            };
            let make_end = |t: f64, outcome: &mut SketchOpOutcome| -> Point2dId {
                if t <= OP_EPS {
                    if let Some((sa, _)) = endpoint_ids {
                        return sa;
                    }
                } else if t >= 1.0 - OP_EPS {
                    if let Some((_, sb)) = endpoint_ids {
                        return sb;
                    }
                }
                let pid = mint_point(
                    sketch,
                    pos_at(t),
                    SketchOpKind::Trim,
                    Some(*target),
                    None,
                    outcome,
                );
                mint_constraint(sketch, point_on_curve(pid, *cutter), outcome);
                pid
            };

            let mut survivors: Vec<(f64, f64)> = Vec::new();
            if let Some(t) = lo {
                survivors.push((0.0, t));
            }
            if let Some(t) = hi {
                survivors.push((t, 1.0));
            }
            let mut kept_original = [false, false];
            for (t0, t1) in survivors {
                let start_id = make_end(t0, &mut outcome);
                let end_id = make_end(t1, &mut outcome);
                if t0 <= OP_EPS {
                    kept_original[0] = true;
                }
                if t1 >= 1.0 - OP_EPS {
                    kept_original[1] = true;
                }
                let sub_sweep = (t1 - t0) * total;
                let aid = sketch.add_arc(start_id, end_id, arc.radius, arc.ccw, sub_sweep > PI)?;
                record_created(
                    sketch,
                    EntityRef::Arc(aid),
                    SketchOpKind::Trim,
                    Some(*target),
                    None,
                    &mut outcome,
                );
            }
            if let Some((sa, sb)) = endpoint_ids {
                for (kept, pid) in [(kept_original[0], sa), (kept_original[1], sb)] {
                    if !kept && !point_in_use(sketch, &pid) {
                        sketch.delete_point(&pid)?;
                        outcome.deleted.push(EntityRef::Point(pid));
                    }
                }
            }
            Ok(outcome)
        }
        _ => Err(SketchOpError::Unsupported {
            op: OP,
            reason: format!("{target} cannot be trimmed (supported: line segment, arc, circle)"),
        }),
    }
}

// ── extend ──────────────────────────────────────────────────────────

/// Which end of a line segment an [`extend`] moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineEnd {
    Start,
    End,
}

/// Extend the chosen end of a shared-endpoint line segment along its
/// own carrier to the NEAREST forward intersection with `boundary`
/// (respecting the boundary's extent). The moved endpoint is then
/// held on the boundary by a minted `PointOnCurve` constraint.
///
/// Arcs are not extendable this slice (typed refuse): growing an
/// arc's angular span against its endpoint-derived representation is
/// follow-up work.
pub fn extend(
    sketch: &Sketch,
    line: &Line2dId,
    end: LineEnd,
    boundary: &EntityRef,
) -> Result<SketchOpOutcome, SketchOpError> {
    const OP: &str = "extend";
    if matches!(boundary, EntityRef::Line(id) if id == line) {
        return Err(SketchOpError::InvalidParameter {
            op: OP,
            parameter: "boundary",
            reason: "boundary and target are the same entity".to_string(),
        });
    }
    let entry = sketch
        .lines()
        .get(line)
        .ok_or_else(|| SketchOpError::EntityNotFound {
            op: OP,
            entity: EntityRef::Line(*line).to_string(),
        })?;
    let Some((sa, sb)) = entry.endpoints else {
        return Err(SketchOpError::Unsupported {
            op: OP,
            reason: "extend needs a shared-endpoint segment".to_string(),
        });
    };
    drop(entry);
    let (moving_id, fixed_id) = match end {
        LineEnd::Start => (sa, sb),
        LineEnd::End => (sb, sa),
    };
    let moving = sketch
        .get_point(&moving_id)
        .ok_or_else(|| SketchOpError::EntityNotFound {
            op: OP,
            entity: moving_id.to_string(),
        })?;
    let anchor = sketch
        .get_point(&fixed_id)
        .ok_or_else(|| SketchOpError::EntityNotFound {
            op: OP,
            entity: fixed_id.to_string(),
        })?;
    let dir = Vector2d::from_points(&anchor, &moving);
    let dir = dir.normalize().map_err(|_| SketchOpError::Unsupported {
        op: OP,
        reason: "degenerate (zero-length) segment".to_string(),
    })?;

    // Forward hits: carrier line of the segment against the boundary,
    // then keep those strictly beyond the moving end.
    let boundary_curve = op_curve(sketch, boundary, OP)?;
    let far = Point2d::new(moving.x + dir.x, moving.y + dir.y);
    let carrier = OpCurve::Segment { a: anchor, b: far };
    // Widen the carrier segment artificially: intersections() filters
    // by segment extent, but an extension target lies beyond it — use
    // a carrier long enough to reach any boundary hit.
    let reach = {
        let (bmin, bmax) = match boundary_curve {
            OpCurve::Segment { a, b } => (a, b),
            OpCurve::Arc(arc) => (
                Point2d::new(arc.center.x - arc.radius, arc.center.y - arc.radius),
                Point2d::new(arc.center.x + arc.radius, arc.center.y + arc.radius),
            ),
            OpCurve::Circle { c, r } => (
                Point2d::new(c.x - r, c.y - r),
                Point2d::new(c.x + r, c.y + r),
            ),
        };
        let d1 = moving.distance_to(&bmin);
        let d2 = moving.distance_to(&bmax);
        d1.max(d2) + 1.0
    };
    let carrier = match carrier {
        OpCurve::Segment { a, .. } => OpCurve::Segment {
            a,
            b: Point2d::new(moving.x + dir.x * reach, moving.y + dir.y * reach),
        },
        other => other,
    };
    let hits = intersections(&carrier, &boundary_curve, OP)?;
    let best = hits
        .into_iter()
        .filter_map(|p| {
            let s = Vector2d::from_points(&moving, &p).dot(&dir);
            (s > 1e-9).then_some((s, p))
        })
        .min_by(|(s1, _), (s2, _)| s1.partial_cmp(s2).unwrap_or(std::cmp::Ordering::Equal));
    let Some((_, hit)) = best else {
        return Err(SketchOpError::NoIntersection {
            op: OP,
            reason: format!("{boundary} has no intersection ahead of the extended end"),
        });
    };

    sketch.update_point(&moving_id, hit)?;
    // Re-sync the stored segment so pre-solve readers (topology, ops)
    // see the extended geometry immediately.
    if let Some(mut e) = sketch.lines().get_mut(line) {
        if let LineGeometry::Segment(seg) = &mut e.value_mut().geometry {
            match end {
                LineEnd::Start => seg.start = hit,
                LineEnd::End => seg.end = hit,
            }
        }
    }

    let mut outcome = SketchOpOutcome::new(SketchOpKind::Extend);
    outcome.modified.push(EntityRef::Line(*line));
    outcome.modified.push(EntityRef::Point(moving_id));
    mint_constraint(sketch, point_on_curve(moving_id, *boundary), &mut outcome);
    Ok(outcome)
}

// ── offset ──────────────────────────────────────────────────────────

/// One primitive of the offset loop under construction.
#[derive(Debug, Clone)]
enum OffsetPrim {
    Line {
        start: Point2d,
        end: Point2d,
        /// Original (walk-oriented) direction — inversion guard.
        dir: Vector2d,
        source: Line2dId,
        /// Walk order is reversed w.r.t. the source entity's stored
        /// endpoint order: the minted entity flips back so the
        /// `Offset` correspondence rows compare like ends.
        reversed: bool,
    },
    Arc {
        center: Point2d,
        radius: f64,
        start_angle: f64,
        end_angle: f64,
        ccw: bool,
        original_sweep: f64,
        source: Option<super::Arc2dId>,
        /// `true` for inserted corner arcs (radius-pinned support
        /// geometry, no source pair).
        corner: bool,
    },
}

impl OffsetPrim {
    fn start_pos(&self) -> Point2d {
        match self {
            OffsetPrim::Line { start, .. } => *start,
            OffsetPrim::Arc {
                center,
                radius,
                start_angle,
                ..
            } => Point2d::new(
                center.x + radius * start_angle.cos(),
                center.y + radius * start_angle.sin(),
            ),
        }
    }
    fn end_pos(&self) -> Point2d {
        match self {
            OffsetPrim::Line { end, .. } => *end,
            OffsetPrim::Arc {
                center,
                radius,
                end_angle,
                ..
            } => Point2d::new(
                center.x + radius * end_angle.cos(),
                center.y + radius * end_angle.sin(),
            ),
        }
    }
    fn sweep(&self) -> f64 {
        match self {
            OffsetPrim::Line { .. } => 0.0,
            OffsetPrim::Arc {
                start_angle,
                end_angle,
                ccw,
                ..
            } => {
                if *ccw {
                    ccw_sweep(*start_angle, *end_angle)
                } else {
                    ccw_sweep(*end_angle, *start_angle)
                }
            }
        }
    }
    /// Travel direction entering / leaving the primitive.
    fn dir_at_start(&self) -> Vector2d {
        match self {
            OffsetPrim::Line { dir, .. } => *dir,
            OffsetPrim::Arc {
                start_angle, ccw, ..
            } => arc_tangent(*start_angle, *ccw),
        }
    }
    fn dir_at_end(&self) -> Vector2d {
        match self {
            OffsetPrim::Line { dir, .. } => *dir,
            OffsetPrim::Arc { end_angle, ccw, .. } => arc_tangent(*end_angle, *ccw),
        }
    }
}

fn arc_tangent(angle: f64, ccw: bool) -> Vector2d {
    if ccw {
        Vector2d::new(-angle.sin(), angle.cos())
    } else {
        Vector2d::new(angle.sin(), -angle.cos())
    }
}

/// Offset the closed loop containing `seed` by `distance`: positive
/// ENLARGES the loop (outward), negative shrinks it. Line/arc loops
/// offset edge-by-edge with arc corner insertion at separating
/// (convex) corners and carrier trimming at closing (reflex) corners;
/// a lone circle offsets to a concentric circle. The result is
/// MAINTAINED: each source/offset line pair gets the Slice-6-enforced
/// `Offset` + `OffsetDistance` constraints, offset arcs get the
/// radial-gap `OffsetDistance`, and corner arcs get `Radius(|d|)` —
/// the DOF arithmetic is exact for loops whose arcs join lines (the
/// spec's "line-arc loops first" class).
///
/// Typed refusals: spline/ellipse loops (NURBS offset approximation
/// is out of scope — spec §3.4), rectangle/polyline loop edges
/// (explode them into lines first), and distances that would invert
/// or vanish an edge ([`SketchOpError::OffsetTooLarge`]).
pub fn offset(
    sketch: &Sketch,
    seed: &EntityRef,
    distance: f64,
) -> Result<SketchOpOutcome, SketchOpError> {
    const OP: &str = "offset";
    if !distance.is_finite() || distance.abs() < OP_EPS {
        return Err(SketchOpError::InvalidParameter {
            op: OP,
            parameter: "distance",
            reason: format!("must be finite and non-zero (got {distance})"),
        });
    }
    let topology = SketchTopology::analyze(sketch, &Tolerance2d::default())?;
    let loop_index = topology
        .loops()
        .iter()
        .position(|lp| {
            lp.edges
                .iter()
                .any(|&ei| topology.edges().get(ei).map(|e| e.entity) == Some(*seed))
        })
        .ok_or_else(|| SketchOpError::Unsupported {
            op: OP,
            reason: format!("{seed} is not part of a closed profile loop"),
        })?;
    let sketch_loop = &topology.loops()[loop_index];

    // Source entity per walk edge (same order analytic_loop_edges
    // walks) + the "line/arc loops first" envelope check.
    let mut edge_entities = Vec::with_capacity(sketch_loop.edges.len());
    for &ei in &sketch_loop.edges {
        let entity = topology.edges().get(ei).map(|e| e.entity).ok_or_else(|| {
            SketchOpError::Unsupported {
                op: OP,
                reason: "loop references a missing topology edge".to_string(),
            }
        })?;
        match entity {
            EntityRef::Line(_) | EntityRef::Arc(_) | EntityRef::Circle(_) => {}
            other => {
                return Err(SketchOpError::Unsupported {
                    op: OP,
                    reason: format!(
                        "offset v1 maintains line/arc loops; {other} edges are not \
                         offsetable (explode rectangles/polylines into lines first)"
                    ),
                })
            }
        }
        edge_entities.push(entity);
    }

    let typed = match ProfileExtractor::analytic_loop_edges(sketch, &topology, sketch_loop)? {
        AnalyticLoop::Edges(edges) => edges,
        AnalyticLoop::Unsupported { entity, .. } => {
            return Err(SketchOpError::Unsupported {
                op: OP,
                reason: format!(
                    "loop contains {entity}, which has no analytic offset — NURBS/ellipse \
                     offset approximation is explicitly out of scope (spec §3.4)"
                ),
            })
        }
    };

    // Lone circle: concentric offset.
    if let [ProfileEdge::Circle { radius, .. }] = typed.as_slice() {
        let EntityRef::Circle(circle_id) = edge_entities[0] else {
            return Err(SketchOpError::Unsupported {
                op: OP,
                reason: "circle loop with a non-circle entity".to_string(),
            });
        };
        let new_r = radius + distance;
        if new_r < OP_EPS {
            return Err(SketchOpError::OffsetTooLarge {
                distance,
                reason: format!("circle of radius {radius} would vanish"),
            });
        }
        let center_point = sketch
            .circles()
            .get(&circle_id)
            .and_then(|e| e.center_point);
        let mut outcome = SketchOpOutcome::new(SketchOpKind::Offset);
        let new_circle = match center_point {
            Some(cp) => sketch.add_circle_centered(cp, new_r)?,
            None => {
                let c = sketch.circle_center_position(&circle_id).ok_or_else(|| {
                    SketchOpError::EntityNotFound {
                        op: OP,
                        entity: edge_entities[0].to_string(),
                    }
                })?;
                sketch.add_circle(c, new_r)?
            }
        };
        record_created(
            sketch,
            EntityRef::Circle(new_circle),
            SketchOpKind::Offset,
            Some(edge_entities[0]),
            None,
            &mut outcome,
        );
        if center_point.is_none() {
            // No structural concentricity — maintain it numerically.
            mint_constraint(
                sketch,
                Constraint::new_geometric(
                    GeometricConstraint::Offset,
                    vec![edge_entities[0], EntityRef::Circle(new_circle)],
                    ConstraintPriority::High,
                ),
                &mut outcome,
            );
        }
        mint_constraint(
            sketch,
            Constraint::new_dimensional(
                DimensionalConstraint::OffsetDistance(distance.abs()),
                vec![edge_entities[0], EntityRef::Circle(new_circle)],
                ConstraintPriority::High,
            ),
            &mut outcome,
        );
        return Ok(outcome);
    }

    // Multi-edge loop. Signed offset along the LEFT of travel: for a
    // CCW walk the interior is left, so outward = −distance to the
    // left; for a CW walk outward = +distance to the left. The walk
    // winding is `SketchLoop::is_ccw` — geometric truth since the
    // follow-ups-A root fix (exact predicate over the walk polygon
    // threaded with the curved edges' interior witnesses), so the op
    // no longer computes its own shoelace to dodge the legacy
    // inverted convention.
    let walk_ccw = sketch_loop.is_ccw;
    let delta_left = if walk_ccw { -distance } else { distance };

    // 1. Offset every edge along its normal (no corners yet).
    let mut prims: Vec<OffsetPrim> = Vec::with_capacity(typed.len());
    for (k, edge) in typed.iter().enumerate() {
        match edge {
            ProfileEdge::Line { start, end } => {
                let s = Point2d::new(start[0], start[1]);
                let e = Point2d::new(end[0], end[1]);
                let d = Vector2d::from_points(&s, &e);
                let d_hat = d.normalize().map_err(|_| SketchOpError::Unsupported {
                    op: OP,
                    reason: "degenerate loop edge".to_string(),
                })?;
                let n_left = Vector2d::new(-d_hat.y, d_hat.x);
                let off = n_left.scale(delta_left);
                let EntityRef::Line(src) = edge_entities[k] else {
                    return Err(SketchOpError::Unsupported {
                        op: OP,
                        reason: "line edge with a non-line entity".to_string(),
                    });
                };
                // Does the walk traverse the source entity forward?
                let reversed = match sketch.lines().get(&src).and_then(|e| e.endpoints) {
                    Some((sa, _)) => sketch
                        .get_point(&sa)
                        .map(|p| p.distance_to(&s) > JOIN_EPS)
                        .unwrap_or(false),
                    None => false,
                };
                prims.push(OffsetPrim::Line {
                    start: Point2d::new(s.x + off.x, s.y + off.y),
                    end: Point2d::new(e.x + off.x, e.y + off.y),
                    dir: d_hat,
                    source: src,
                    reversed,
                });
            }
            ProfileEdge::Arc {
                center,
                radius,
                start_angle,
                end_angle,
                ccw,
            } => {
                // The center sits LEFT of travel for a CCW walk;
                // moving the curve left by delta_left therefore
                // shrinks the radius when the center is left.
                let new_r = if *ccw {
                    radius - delta_left
                } else {
                    radius + delta_left
                };
                if new_r < OP_EPS {
                    return Err(SketchOpError::OffsetTooLarge {
                        distance,
                        reason: format!("arc of radius {radius} would vanish or invert"),
                    });
                }
                let source = match edge_entities[k] {
                    EntityRef::Arc(id) => Some(id),
                    _ => {
                        return Err(SketchOpError::Unsupported {
                            op: OP,
                            reason: "arc edge with a non-arc entity".to_string(),
                        })
                    }
                };
                let original_sweep = if *ccw {
                    ccw_sweep(*start_angle, *end_angle)
                } else {
                    ccw_sweep(*end_angle, *start_angle)
                };
                prims.push(OffsetPrim::Arc {
                    center: Point2d::new(center[0], center[1]),
                    radius: new_r,
                    start_angle: *start_angle,
                    end_angle: *end_angle,
                    ccw: *ccw,
                    original_sweep,
                    source,
                    corner: false,
                });
            }
            ProfileEdge::Circle { .. } => {
                return Err(SketchOpError::Unsupported {
                    op: OP,
                    reason: "a full circle inside a multi-edge loop is not offsetable".to_string(),
                })
            }
            // Unreachable — spline loop edges refuse at the
            // entity-kind gate above; the typed refusal documents that
            // NURBS offset approximation stays out of the honest
            // envelope (SKETCH-DCM #45 Slice 7 directive note).
            ProfileEdge::Nurbs { .. } => {
                return Err(SketchOpError::Unsupported {
                    op: OP,
                    reason: "NURBS loop edges have no honest analytic offset —                              out of scope, typed refusal"
                        .to_string(),
                })
            }
        }
    }

    // 2. Resolve corners: separating corners get an inserted arc
    //    centered at the original vertex; closing corners trim both
    //    carriers to their intersection. Everything is computed
    //    BEFORE any sketch mutation.
    let n = prims.len();
    let original_vertex = |k: usize| -> Point2d {
        // Original (pre-offset) shared vertex between edge k and k+1,
        // from the typed edges.
        match &typed[k] {
            ProfileEdge::Line { end, .. } => Point2d::new(end[0], end[1]),
            ProfileEdge::Arc {
                center,
                radius,
                end_angle,
                ..
            } => Point2d::new(
                center[0] + radius * end_angle.cos(),
                center[1] + radius * end_angle.sin(),
            ),
            ProfileEdge::Circle { .. } => Point2d::new(0.0, 0.0), // unreachable: refused above
            ProfileEdge::Nurbs { control_points, .. } => control_points
                .last()
                .map(|p| Point2d::new(p[0], p[1]))
                .unwrap_or(Point2d::new(0.0, 0.0)), // unreachable: refused above
        }
    };
    #[derive(Debug, Clone, Copy)]
    enum Corner {
        Join,
        Gap { vertex: Point2d },
        TrimTo(Point2d),
    }
    let mut corners: Vec<Corner> = Vec::with_capacity(n);
    for k in 0..n {
        let next = (k + 1) % n;
        let e_end = prims[k].end_pos();
        let s_next = prims[next].start_pos();
        if e_end.distance_to(&s_next) < JOIN_EPS {
            corners.push(Corner::Join);
            continue;
        }
        let turn = prims[k].dir_at_end().cross(&prims[next].dir_at_start());
        if turn * delta_left < 0.0 {
            corners.push(Corner::Gap {
                vertex: original_vertex(k),
            });
        } else {
            // Closing corner: intersect the two offset carriers and
            // trim both to the intersection nearest the corner.
            let mid = e_end.midpoint(&s_next);
            let carrier_hits: Vec<Point2d> = match (&prims[k], &prims[next]) {
                (
                    OffsetPrim::Line {
                        start: a0, end: a1, ..
                    },
                    OffsetPrim::Line {
                        start: b0, end: b1, ..
                    },
                ) => {
                    let (l1, l2) = (
                        Line2d::from_points(a0, a1).map_err(SketchOpError::from)?,
                        Line2d::from_points(b0, b1).map_err(SketchOpError::from)?,
                    );
                    l1.intersect(&l2).map(|p| vec![p]).unwrap_or_default()
                }
                (
                    OffsetPrim::Line {
                        start: a0, end: a1, ..
                    },
                    OffsetPrim::Arc { center, radius, .. },
                )
                | (
                    OffsetPrim::Arc { center, radius, .. },
                    OffsetPrim::Line {
                        start: a0, end: a1, ..
                    },
                ) => {
                    let circle = Circle2d::new(*center, *radius).map_err(SketchOpError::from)?;
                    circle
                        .intersect_line(a0, &Vector2d::from_points(a0, a1))
                        .unwrap_or_default()
                }
                (
                    OffsetPrim::Arc {
                        center: c1,
                        radius: r1,
                        ..
                    },
                    OffsetPrim::Arc {
                        center: c2,
                        radius: r2,
                        ..
                    },
                ) => {
                    let (k1, k2) = (
                        Circle2d::new(*c1, *r1).map_err(SketchOpError::from)?,
                        Circle2d::new(*c2, *r2).map_err(SketchOpError::from)?,
                    );
                    k1.intersect_circle(&k2).unwrap_or_default()
                }
            };
            let best = carrier_hits.into_iter().min_by(|p, q| {
                p.distance_to(&mid)
                    .partial_cmp(&q.distance_to(&mid))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let Some(p) = best else {
                return Err(SketchOpError::OffsetTooLarge {
                    distance,
                    reason: "closing corner has no carrier intersection — adjacent edges \
                             would be consumed"
                        .to_string(),
                });
            };
            corners.push(Corner::TrimTo(p));
        }
    }

    // Apply the trims to the prims (still no sketch mutation).
    for k in 0..n {
        if let Corner::TrimTo(p) = corners[k] {
            let next = (k + 1) % n;
            set_prim_end(&mut prims[k], p);
            set_prim_start(&mut prims[next], p);
        }
    }
    // Validate: no inverted line, no inflated/degenerate arc.
    for prim in &prims {
        match prim {
            OffsetPrim::Line {
                start, end, dir, ..
            } => {
                if Vector2d::from_points(start, end).dot(dir) < OP_EPS {
                    return Err(SketchOpError::OffsetTooLarge {
                        distance,
                        reason: "an offset edge inverted (its trimmed length is not \
                                 positive along the original direction)"
                            .to_string(),
                    });
                }
            }
            OffsetPrim::Arc { original_sweep, .. } => {
                let sweep = prim.sweep();
                if sweep < OP_EPS || sweep > original_sweep + 1e-6 {
                    return Err(SketchOpError::OffsetTooLarge {
                        distance,
                        reason: "an offset arc degenerated (trimmed sweep is empty or \
                                 exceeds the original span)"
                            .to_string(),
                    });
                }
            }
        }
    }

    // Interleave the corner arcs into the final primitive ring.
    let mut finals: Vec<OffsetPrim> = Vec::with_capacity(2 * n);
    for k in 0..n {
        finals.push(prims[k].clone());
        if let Corner::Gap { vertex } = corners[k] {
            let e_end = prims[k].end_pos();
            let s_next = prims[(k + 1) % n].start_pos();
            let a0 = vertex.angle_to(&e_end);
            let a1 = vertex.angle_to(&s_next);
            let ccw = Vector2d::from_points(&vertex, &e_end)
                .cross(&Vector2d::from_points(&vertex, &s_next))
                > 0.0;
            finals.push(OffsetPrim::Arc {
                center: vertex,
                radius: distance.abs(),
                start_angle: a0,
                end_angle: a1,
                ccw,
                original_sweep: TAU,
                source: None,
                corner: true,
            });
        }
    }

    // 3. Materialise: junction points are shared between consecutive
    //    primitives; entities + maintenance constraints + provenance.
    let mut outcome = SketchOpOutcome::new(SketchOpKind::Offset);
    let m = finals.len();
    let mut junction_ids: Vec<Point2dId> = Vec::with_capacity(m);
    for k in 0..m {
        // Junction between prim k and prim k+1 (cyclic) — one point.
        let e_end = finals[k].end_pos();
        let s_next = finals[(k + 1) % m].start_pos();
        debug_assert!(e_end.distance_to(&s_next) < 1e-6);
        let pid = mint_point(
            sketch,
            e_end,
            SketchOpKind::Offset,
            None,
            None,
            &mut outcome,
        );
        junction_ids.push(pid);
    }
    let start_id = |k: usize| -> Point2dId {
        // Prim k starts at the junction AFTER prim k−1.
        junction_ids[(k + m - 1) % m]
    };
    for (k, prim) in finals.iter().enumerate() {
        let (p_start, p_end) = (start_id(k), junction_ids[k]);
        match prim {
            OffsetPrim::Line {
                source, reversed, ..
            } => {
                let lid = if *reversed {
                    sketch.add_line(p_end, p_start)?
                } else {
                    sketch.add_line(p_start, p_end)?
                };
                record_created(
                    sketch,
                    EntityRef::Line(lid),
                    SketchOpKind::Offset,
                    Some(EntityRef::Line(*source)),
                    None,
                    &mut outcome,
                );
                mint_constraint(
                    sketch,
                    Constraint::new_geometric(
                        GeometricConstraint::Offset,
                        vec![EntityRef::Line(*source), EntityRef::Line(lid)],
                        ConstraintPriority::High,
                    ),
                    &mut outcome,
                );
                mint_constraint(
                    sketch,
                    Constraint::new_dimensional(
                        DimensionalConstraint::OffsetDistance(distance.abs()),
                        vec![EntityRef::Line(*source), EntityRef::Line(lid)],
                        ConstraintPriority::High,
                    ),
                    &mut outcome,
                );
            }
            OffsetPrim::Arc {
                radius,
                ccw,
                source,
                corner,
                ..
            } => {
                let sweep = prim.sweep();
                let aid = sketch.add_arc(p_start, p_end, *radius, *ccw, sweep > PI)?;
                let source_ref = source.map(EntityRef::Arc);
                record_created(
                    sketch,
                    EntityRef::Arc(aid),
                    SketchOpKind::Offset,
                    source_ref,
                    None,
                    &mut outcome,
                );
                if *corner {
                    // Support geometry: the corner fillet's radius IS
                    // the offset distance; its endpoints are shared
                    // with the adjacent (already maintained) edges.
                    mint_constraint(
                        sketch,
                        Constraint::new_dimensional(
                            DimensionalConstraint::Radius(distance.abs()),
                            vec![EntityRef::Arc(aid)],
                            ConstraintPriority::High,
                        ),
                        &mut outcome,
                    );
                } else if let Some(src) = source_ref {
                    // Radial gap to the source arc. Concentricity is
                    // NOT double-minted: for the line-arc loop class
                    // the arc's endpoints are already pinned by the
                    // adjacent line pairs, and adding the 2-row
                    // concentric `Offset` would over-count the DOF
                    // arithmetic (documented in the slice report;
                    // all-arc loops therefore keep residual freedom,
                    // reported honestly by the certificate).
                    mint_constraint(
                        sketch,
                        Constraint::new_dimensional(
                            DimensionalConstraint::OffsetDistance(distance.abs()),
                            vec![src, EntityRef::Arc(aid)],
                            ConstraintPriority::High,
                        ),
                        &mut outcome,
                    );
                }
            }
        }
    }
    Ok(outcome)
}

fn set_prim_end(prim: &mut OffsetPrim, p: Point2d) {
    match prim {
        OffsetPrim::Line { end, .. } => *end = p,
        OffsetPrim::Arc {
            center, end_angle, ..
        } => *end_angle = norm_angle(center.angle_to(&p)),
    }
}

fn set_prim_start(prim: &mut OffsetPrim, p: Point2d) {
    match prim {
        OffsetPrim::Line { start, .. } => *start = p,
        OffsetPrim::Arc {
            center,
            start_angle,
            ..
        } => *start_angle = norm_angle(center.angle_to(&p)),
    }
}

// ── mirror ──────────────────────────────────────────────────────────

/// Mirror `entities` about a CONSTRUCTION line, minting `Symmetric`
/// constraints per mirrored point (and `Equal` radii for circles /
/// arcs) so the mirror is MAINTAINED by the solver — edit the source
/// and the copy follows. Shared points among the selected entities
/// are mirrored once.
///
/// Supported: points, shared-endpoint lines, circles, shared-endpoint
/// arcs. The axis must be marked construction (spec §3.4) — a typed
/// refusal otherwise.
pub fn mirror(
    sketch: &Sketch,
    entities: &[EntityRef],
    axis: &Line2dId,
) -> Result<SketchOpOutcome, SketchOpError> {
    const OP: &str = "mirror";
    let axis_entry = sketch
        .lines()
        .get(axis)
        .ok_or_else(|| SketchOpError::EntityNotFound {
            op: OP,
            entity: EntityRef::Line(*axis).to_string(),
        })?;
    if !axis_entry.is_construction {
        return Err(SketchOpError::AxisNotConstruction { op: OP });
    }
    // Axis carrier (live endpoint positions when derived).
    let (a0, d) = match (&axis_entry.geometry, axis_entry.endpoints) {
        (_, Some((sa, sb))) => {
            drop(axis_entry);
            let pa = sketch
                .get_point(&sa)
                .ok_or_else(|| SketchOpError::EntityNotFound {
                    op: OP,
                    entity: sa.to_string(),
                })?;
            let pb = sketch
                .get_point(&sb)
                .ok_or_else(|| SketchOpError::EntityNotFound {
                    op: OP,
                    entity: sb.to_string(),
                })?;
            (pa, Vector2d::from_points(&pa, &pb))
        }
        (LineGeometry::Segment(seg), None) => (seg.start, seg.direction()),
        (LineGeometry::Infinite(line), None) => (line.point, line.direction),
        (LineGeometry::Ray(ray), None) => (ray.origin, ray.direction),
    };
    let d_hat = d.normalize().map_err(|_| SketchOpError::Unsupported {
        op: OP,
        reason: "degenerate mirror axis".to_string(),
    })?;
    let n_hat = Vector2d::new(-d_hat.y, d_hat.x);
    let reflect = |p: &Point2d| -> Point2d {
        let off = 2.0 * Vector2d::from_points(&a0, p).dot(&n_hat);
        Point2d::new(p.x - off * n_hat.x, p.y - off * n_hat.y)
    };

    // Pre-validate every entity BEFORE mutating.
    let mut seen = std::collections::HashSet::new();
    let mut plan: Vec<EntityRef> = Vec::new();
    for e in entities {
        if !seen.insert(*e) {
            continue;
        }
        match e {
            EntityRef::Point(id) => {
                if sketch.get_point(id).is_none() {
                    return Err(SketchOpError::EntityNotFound {
                        op: OP,
                        entity: e.to_string(),
                    });
                }
            }
            EntityRef::Line(id) => {
                let entry =
                    sketch
                        .lines()
                        .get(id)
                        .ok_or_else(|| SketchOpError::EntityNotFound {
                            op: OP,
                            entity: e.to_string(),
                        })?;
                if entry.endpoints.is_none() {
                    return Err(SketchOpError::Unsupported {
                        op: OP,
                        reason: format!("{e}: mirror needs shared-endpoint segments"),
                    });
                }
            }
            EntityRef::Circle(id) => {
                if sketch.circles().get(id).is_none() {
                    return Err(SketchOpError::EntityNotFound {
                        op: OP,
                        entity: e.to_string(),
                    });
                }
            }
            EntityRef::Arc(id) => {
                let entry = sketch
                    .arcs()
                    .get(id)
                    .ok_or_else(|| SketchOpError::EntityNotFound {
                        op: OP,
                        entity: e.to_string(),
                    })?;
                if entry.endpoints.is_none() {
                    return Err(SketchOpError::Unsupported {
                        op: OP,
                        reason: format!("{e}: mirror needs shared-endpoint arcs"),
                    });
                }
            }
            other => {
                return Err(SketchOpError::Unsupported {
                    op: OP,
                    reason: format!(
                        "{other} is not mirrorable (supported: point, line, circle, arc)"
                    ),
                })
            }
        }
        plan.push(*e);
    }

    let mut outcome = SketchOpOutcome::new(SketchOpKind::Mirror);
    let mut point_map: std::collections::HashMap<Point2dId, Point2dId> =
        std::collections::HashMap::new();
    let axis_ref = EntityRef::Line(*axis);

    // Mirror a point once, minting its Symmetric maintenance pair.
    macro_rules! mirror_point {
        ($pid:expr) => {{
            let pid = $pid;
            match point_map.get(&pid) {
                Some(m) => *m,
                None => {
                    let pos =
                        sketch
                            .get_point(&pid)
                            .ok_or_else(|| SketchOpError::EntityNotFound {
                                op: OP,
                                entity: pid.to_string(),
                            })?;
                    let mid = mint_point(
                        sketch,
                        reflect(&pos),
                        SketchOpKind::Mirror,
                        Some(EntityRef::Point(pid)),
                        None,
                        &mut outcome,
                    );
                    mint_constraint(
                        sketch,
                        Constraint::new_geometric(
                            GeometricConstraint::Symmetric,
                            vec![EntityRef::Point(pid), EntityRef::Point(mid), axis_ref],
                            ConstraintPriority::High,
                        ),
                        &mut outcome,
                    );
                    point_map.insert(pid, mid);
                    mid
                }
            }
        }};
    }

    for e in plan {
        match e {
            EntityRef::Point(id) => {
                let _ = mirror_point!(id);
            }
            EntityRef::Line(id) => {
                let (sa, sb) = sketch
                    .lines()
                    .get(&id)
                    .and_then(|entry| entry.endpoints)
                    .ok_or_else(|| SketchOpError::EntityNotFound {
                        op: OP,
                        entity: e.to_string(),
                    })?;
                let ma = mirror_point!(sa);
                let mb = mirror_point!(sb);
                let lid = sketch.add_line(ma, mb)?;
                record_created(
                    sketch,
                    EntityRef::Line(lid),
                    SketchOpKind::Mirror,
                    Some(e),
                    None,
                    &mut outcome,
                );
            }
            EntityRef::Circle(id) => {
                let (center_point, radius) =
                    {
                        let entry = sketch.circles().get(&id).ok_or_else(|| {
                            SketchOpError::EntityNotFound {
                                op: OP,
                                entity: e.to_string(),
                            }
                        })?;
                        (entry.center_point, entry.circle.radius)
                    };
                let cid = match center_point {
                    Some(cp) => {
                        let mcp = mirror_point!(cp);
                        sketch.add_circle_centered(mcp, radius)?
                    }
                    None => {
                        let c = sketch.circle_center_position(&id).ok_or_else(|| {
                            SketchOpError::EntityNotFound {
                                op: OP,
                                entity: e.to_string(),
                            }
                        })?;
                        let cid = sketch.add_circle(reflect(&c), radius)?;
                        // No shared center to reflect structurally —
                        // maintain the center numerically.
                        mint_constraint(
                            sketch,
                            Constraint::new_geometric(
                                GeometricConstraint::Symmetric,
                                vec![e, EntityRef::Circle(cid), axis_ref],
                                ConstraintPriority::High,
                            ),
                            &mut outcome,
                        );
                        cid
                    }
                };
                record_created(
                    sketch,
                    EntityRef::Circle(cid),
                    SketchOpKind::Mirror,
                    Some(e),
                    None,
                    &mut outcome,
                );
                mint_constraint(
                    sketch,
                    Constraint::new_geometric(
                        GeometricConstraint::Equal,
                        vec![e, EntityRef::Circle(cid)],
                        ConstraintPriority::High,
                    ),
                    &mut outcome,
                );
            }
            EntityRef::Arc(id) => {
                let (endpoints, arc) =
                    {
                        let entry = sketch.arcs().get(&id).ok_or_else(|| {
                            SketchOpError::EntityNotFound {
                                op: OP,
                                entity: e.to_string(),
                            }
                        })?;
                        (entry.endpoints, entry.arc)
                    };
                let Some((sa, sb)) = endpoints else {
                    return Err(SketchOpError::Unsupported {
                        op: OP,
                        reason: format!("{e}: mirror needs shared-endpoint arcs"),
                    });
                };
                let ma = mirror_point!(sa);
                let mb = mirror_point!(sb);
                let sweep = arc.sweep_angle();
                let aid = sketch.add_arc(ma, mb, arc.radius, !arc.ccw, sweep > PI)?;
                record_created(
                    sketch,
                    EntityRef::Arc(aid),
                    SketchOpKind::Mirror,
                    Some(e),
                    None,
                    &mut outcome,
                );
                mint_constraint(
                    sketch,
                    Constraint::new_geometric(
                        GeometricConstraint::Equal,
                        vec![e, EntityRef::Arc(aid)],
                        ConstraintPriority::High,
                    ),
                    &mut outcome,
                );
            }
            _ => {}
        }
    }
    Ok(outcome)
}

// ── patterns ────────────────────────────────────────────────────────

/// Anchor point of a pattern source: a point IS its own anchor; a
/// circle anchors at its (possibly newly minted, `Coincident`-tied)
/// center point. Returns `(anchor_id, circle_entity)`.
fn pattern_anchor(
    sketch: &Sketch,
    source: &EntityRef,
    op_kind: SketchOpKind,
    op: &'static str,
    outcome: &mut SketchOpOutcome,
) -> Result<(Point2dId, Option<EntityRef>), SketchOpError> {
    match source {
        EntityRef::Point(id) => Ok((*id, None)),
        EntityRef::Circle(id) => {
            let center_point = sketch
                .circles()
                .get(id)
                .ok_or_else(|| SketchOpError::EntityNotFound {
                    op,
                    entity: source.to_string(),
                })?
                .center_point;
            match center_point.filter(|cp| sketch.get_point(cp).is_some()) {
                Some(cp) => Ok((cp, Some(*source))),
                None => {
                    // Legacy circle: mint an anchor tied to the center.
                    let c = sketch.circle_center_position(id).ok_or_else(|| {
                        SketchOpError::EntityNotFound {
                            op,
                            entity: source.to_string(),
                        }
                    })?;
                    let anchor = mint_point(sketch, c, op_kind, Some(*source), None, outcome);
                    mint_constraint(
                        sketch,
                        Constraint::new_geometric(
                            GeometricConstraint::Coincident,
                            vec![EntityRef::Point(anchor), *source],
                            ConstraintPriority::High,
                        ),
                        outcome,
                    );
                    Ok((anchor, Some(*source)))
                }
            }
        }
        other => Err(SketchOpError::Unsupported {
            op,
            reason: format!(
                "{other} is not a pattern source — v1 patterns maintain point/circle \
                 instances (the hole-pattern class); mirror covers reflective copies"
            ),
        }),
    }
}

fn validate_pattern_sources(
    sketch: &Sketch,
    sources: &[EntityRef],
    op: &'static str,
) -> Result<(), SketchOpError> {
    if sources.is_empty() {
        return Err(SketchOpError::InvalidParameter {
            op,
            parameter: "sources",
            reason: "at least one source entity is required".to_string(),
        });
    }
    for s in sources {
        match s {
            EntityRef::Point(id) => {
                if sketch.get_point(id).is_none() {
                    return Err(SketchOpError::EntityNotFound {
                        op,
                        entity: s.to_string(),
                    });
                }
            }
            EntityRef::Circle(id) => {
                if sketch.circles().get(id).is_none() {
                    return Err(SketchOpError::EntityNotFound {
                        op,
                        entity: s.to_string(),
                    });
                }
            }
            other => {
                return Err(SketchOpError::Unsupported {
                    op,
                    reason: format!(
                        "{other} is not a pattern source — v1 patterns maintain \
                         point/circle instances (the hole-pattern class)"
                    ),
                })
            }
        }
    }
    Ok(())
}

/// Linear pattern: `count` total instances (source = instance 0) of
/// each source, stepped by `(dx, dy)`. Maintenance is the
/// `Equal`-chain form the Slice-2/3 decomposition handles well: a
/// construction guide line through the source pinned to the step
/// direction, `Distance(spacing)` between consecutive instances,
/// `PointOnCurve` on the guide, and an `Equal`-radius chain for
/// circles. Every instance records provenance lineage.
pub fn linear_pattern(
    sketch: &Sketch,
    sources: &[EntityRef],
    count: usize,
    dx: f64,
    dy: f64,
) -> Result<SketchOpOutcome, SketchOpError> {
    const OP: &str = "linear_pattern";
    if count < 2 {
        return Err(SketchOpError::InvalidParameter {
            op: OP,
            parameter: "count",
            reason: format!("need at least 2 instances (got {count})"),
        });
    }
    let spacing = (dx * dx + dy * dy).sqrt();
    if !spacing.is_finite() || spacing < OP_EPS {
        return Err(SketchOpError::InvalidParameter {
            op: OP,
            parameter: "step",
            reason: format!("step vector must be finite and non-zero (got ({dx}, {dy}))"),
        });
    }
    validate_pattern_sources(sketch, sources, OP)?;

    let mut outcome = SketchOpOutcome::new(SketchOpKind::LinearPattern);
    for source in sources {
        let (anchor_id, circle_source) = pattern_anchor(
            sketch,
            source,
            SketchOpKind::LinearPattern,
            OP,
            &mut outcome,
        )?;
        let anchor_pos =
            sketch
                .get_point(&anchor_id)
                .ok_or_else(|| SketchOpError::EntityNotFound {
                    op: OP,
                    entity: anchor_id.to_string(),
                })?;
        let source_radius = match circle_source {
            Some(EntityRef::Circle(id)) => sketch.circles().get(&id).map(|e| e.circle.radius),
            _ => None,
        };

        let mut prev_point = anchor_id;
        let mut prev_circle = circle_source;
        let mut guide: Option<Line2dId> = None;
        for k in 1..count {
            let pos = Point2d::new(anchor_pos.x + k as f64 * dx, anchor_pos.y + k as f64 * dy);
            let pk = mint_point(
                sketch,
                pos,
                SketchOpKind::LinearPattern,
                Some(*source),
                Some(k),
                &mut outcome,
            );
            if k == 1 {
                // Construction guide through the source along the
                // pattern direction, pinned so the chain cannot
                // rotate: Horizontal / Vertical for axis-aligned
                // steps, Slope otherwise.
                let gid = sketch.add_line(anchor_id, pk)?;
                sketch.set_construction(&EntityRef::Line(gid), true)?;
                record_created(
                    sketch,
                    EntityRef::Line(gid),
                    SketchOpKind::LinearPattern,
                    Some(*source),
                    None,
                    &mut outcome,
                );
                let pin = if dy.abs() < OP_EPS {
                    Constraint::new_geometric(
                        GeometricConstraint::Horizontal,
                        vec![EntityRef::Line(gid)],
                        ConstraintPriority::High,
                    )
                } else if dx.abs() < OP_EPS {
                    Constraint::new_geometric(
                        GeometricConstraint::Vertical,
                        vec![EntityRef::Line(gid)],
                        ConstraintPriority::High,
                    )
                } else {
                    Constraint::new_dimensional(
                        DimensionalConstraint::Slope(dy / dx),
                        vec![EntityRef::Line(gid)],
                        ConstraintPriority::High,
                    )
                };
                mint_constraint(sketch, pin, &mut outcome);
                guide = Some(gid);
            } else if let Some(gid) = guide {
                mint_constraint(
                    sketch,
                    point_on_curve(pk, EntityRef::Line(gid)),
                    &mut outcome,
                );
            }
            mint_constraint(
                sketch,
                Constraint::new_dimensional(
                    DimensionalConstraint::Distance(spacing),
                    vec![EntityRef::Point(prev_point), EntityRef::Point(pk)],
                    ConstraintPriority::High,
                ),
                &mut outcome,
            );
            if let (Some(prev), Some(r)) = (prev_circle, source_radius) {
                let ck = sketch.add_circle_centered(pk, r)?;
                record_created(
                    sketch,
                    EntityRef::Circle(ck),
                    SketchOpKind::LinearPattern,
                    Some(*source),
                    Some(k),
                    &mut outcome,
                );
                mint_constraint(
                    sketch,
                    Constraint::new_geometric(
                        GeometricConstraint::Equal,
                        vec![prev, EntityRef::Circle(ck)],
                        ConstraintPriority::High,
                    ),
                    &mut outcome,
                );
                prev_circle = Some(EntityRef::Circle(ck));
            }
            prev_point = pk;
        }
    }
    Ok(outcome)
}

/// Circular pattern: `count` total instances of each source, rotated
/// about the `center` point by multiples of `angle_step`. Maintenance
/// is the spoke web: construction spokes center→instance with an
/// `Equal`-length chain and `Angle(angle_step)` between consecutive
/// spokes, plus the `Equal`-radius chain for circles.
pub fn circular_pattern(
    sketch: &Sketch,
    sources: &[EntityRef],
    center: &Point2dId,
    count: usize,
    angle_step: f64,
) -> Result<SketchOpOutcome, SketchOpError> {
    const OP: &str = "circular_pattern";
    if count < 2 {
        return Err(SketchOpError::InvalidParameter {
            op: OP,
            parameter: "count",
            reason: format!("need at least 2 instances (got {count})"),
        });
    }
    if !angle_step.is_finite() || angle_step.abs() < 1e-12 {
        return Err(SketchOpError::InvalidParameter {
            op: OP,
            parameter: "angle_step",
            reason: format!("must be finite and non-zero (got {angle_step})"),
        });
    }
    let center_pos = sketch
        .get_point(center)
        .ok_or_else(|| SketchOpError::EntityNotFound {
            op: OP,
            entity: center.to_string(),
        })?;
    validate_pattern_sources(sketch, sources, OP)?;
    // Pre-validate radii: every source must be off-center.
    for source in sources {
        let anchor_pos = match source {
            EntityRef::Point(id) => sketch.get_point(id),
            EntityRef::Circle(id) => sketch.circle_center_position(id),
            _ => None,
        };
        let Some(pos) = anchor_pos else {
            return Err(SketchOpError::EntityNotFound {
                op: OP,
                entity: source.to_string(),
            });
        };
        if pos.distance_to(&center_pos) < OP_EPS {
            return Err(SketchOpError::InvalidParameter {
                op: OP,
                parameter: "center",
                reason: format!("{source} coincides with the pattern center"),
            });
        }
    }

    let mut outcome = SketchOpOutcome::new(SketchOpKind::CircularPattern);
    for source in sources {
        let (anchor_id, circle_source) = pattern_anchor(
            sketch,
            source,
            SketchOpKind::CircularPattern,
            OP,
            &mut outcome,
        )?;
        let anchor_pos =
            sketch
                .get_point(&anchor_id)
                .ok_or_else(|| SketchOpError::EntityNotFound {
                    op: OP,
                    entity: anchor_id.to_string(),
                })?;
        let source_radius = match circle_source {
            Some(EntityRef::Circle(id)) => sketch.circles().get(&id).map(|e| e.circle.radius),
            _ => None,
        };

        // Spoke 0: center → source anchor (construction, derived).
        let spoke0 = sketch.add_line(*center, anchor_id)?;
        sketch.set_construction(&EntityRef::Line(spoke0), true)?;
        record_created(
            sketch,
            EntityRef::Line(spoke0),
            SketchOpKind::CircularPattern,
            Some(*source),
            None,
            &mut outcome,
        );

        let rotate = |p: &Point2d, angle: f64| -> Point2d {
            let (s, c) = angle.sin_cos();
            let (vx, vy) = (p.x - center_pos.x, p.y - center_pos.y);
            Point2d::new(
                center_pos.x + c * vx - s * vy,
                center_pos.y + s * vx + c * vy,
            )
        };

        let mut prev_spoke = spoke0;
        let mut prev_circle = circle_source;
        for k in 1..count {
            let pos = rotate(&anchor_pos, k as f64 * angle_step);
            let pk = mint_point(
                sketch,
                pos,
                SketchOpKind::CircularPattern,
                Some(*source),
                Some(k),
                &mut outcome,
            );
            let spoke = sketch.add_line(*center, pk)?;
            sketch.set_construction(&EntityRef::Line(spoke), true)?;
            record_created(
                sketch,
                EntityRef::Line(spoke),
                SketchOpKind::CircularPattern,
                Some(*source),
                Some(k),
                &mut outcome,
            );
            mint_constraint(
                sketch,
                Constraint::new_geometric(
                    GeometricConstraint::Equal,
                    vec![EntityRef::Line(prev_spoke), EntityRef::Line(spoke)],
                    ConstraintPriority::High,
                ),
                &mut outcome,
            );
            mint_constraint(
                sketch,
                Constraint::new_dimensional(
                    DimensionalConstraint::Angle(angle_step),
                    vec![EntityRef::Line(prev_spoke), EntityRef::Line(spoke)],
                    ConstraintPriority::High,
                ),
                &mut outcome,
            );
            if let (Some(prev), Some(r)) = (prev_circle, source_radius) {
                let ck = sketch.add_circle_centered(pk, r)?;
                record_created(
                    sketch,
                    EntityRef::Circle(ck),
                    SketchOpKind::CircularPattern,
                    Some(*source),
                    Some(k),
                    &mut outcome,
                );
                mint_constraint(
                    sketch,
                    Constraint::new_geometric(
                        GeometricConstraint::Equal,
                        vec![prev, EntityRef::Circle(ck)],
                        ConstraintPriority::High,
                    ),
                    &mut outcome,
                );
                prev_circle = Some(EntityRef::Circle(ck));
            }
            prev_spoke = spoke;
        }
    }
    Ok(outcome)
}

// ── generative patterns (SKETCH-DCM #45 Slice 7 — biomimicry) ───────

/// The EXACT golden angle in radians: `2π(1 − 1/φ)` with
/// `φ = (1 + √5)/2` — Vogel's phyllotaxis constant
/// (H. Vogel, "A better way to construct the sunflower head",
/// *Mathematical Biosciences* 44(3-4):179-189, 1979).
/// ≈ 2.399963229728653 rad ≈ 137.50776405003785°. Computed, never a
/// rounded literal.
pub fn golden_angle_rad() -> f64 {
    let phi = (1.0 + 5.0_f64.sqrt()) / 2.0;
    TAU * (1.0 - 1.0 / phi)
}

/// Phyllotaxis pattern: `count` florets per source on the Vogel spiral
/// about `center` — floret n at radius `spacing·√n`, azimuth stepped
/// by the exact golden angle. The SOURCE is floret 1 (its
/// `Distance(center, anchor) = spacing·√1` is minted, so the solver
/// holds it on its own floret radius); florets 2..=count are minted.
///
/// Maintenance web (the Slice-6 pattern scheme): construction spokes
/// center→floret, `Angle(golden angle)` between consecutive spokes,
/// `Distance(center, floretₙ) = spacing·√n` — each floret is fully
/// constrained relative to the center and the previous spoke (2 DOF
/// in, 2 DOF removed; hand-countable). Circle sources add the
/// `Equal`-radius chain, so dimensioning the source re-sizes every
/// floret. Provenance records the floret index n on every instance.
pub fn phyllotaxis_pattern(
    sketch: &Sketch,
    sources: &[EntityRef],
    center: &Point2dId,
    count: usize,
    spacing: f64,
) -> Result<SketchOpOutcome, SketchOpError> {
    const OP: &str = "phyllotaxis_pattern";
    if count < 2 {
        return Err(SketchOpError::InvalidParameter {
            op: OP,
            parameter: "count",
            reason: format!("need at least 2 florets (got {count})"),
        });
    }
    if !spacing.is_finite() || spacing < OP_EPS {
        return Err(SketchOpError::InvalidParameter {
            op: OP,
            parameter: "spacing",
            reason: format!("the Vogel constant c must be finite and positive (got {spacing})"),
        });
    }
    let center_pos = sketch
        .get_point(center)
        .ok_or_else(|| SketchOpError::EntityNotFound {
            op: OP,
            entity: center.to_string(),
        })?;
    validate_pattern_sources(sketch, sources, OP)?;
    // Every source anchor must be off-center: the anchor direction IS
    // the spiral's phase reference.
    for source in sources {
        let anchor_pos = match source {
            EntityRef::Point(id) => sketch.get_point(id),
            EntityRef::Circle(id) => sketch.circle_center_position(id),
            _ => None,
        };
        let Some(pos) = anchor_pos else {
            return Err(SketchOpError::EntityNotFound {
                op: OP,
                entity: source.to_string(),
            });
        };
        if pos.distance_to(&center_pos) < OP_EPS {
            return Err(SketchOpError::InvalidParameter {
                op: OP,
                parameter: "center",
                reason: format!("{source} coincides with the spiral center"),
            });
        }
    }

    let gamma = golden_angle_rad();
    let mut outcome = SketchOpOutcome::new(SketchOpKind::PhyllotaxisPattern);
    for source in sources {
        let (anchor_id, circle_source) = pattern_anchor(
            sketch,
            source,
            SketchOpKind::PhyllotaxisPattern,
            OP,
            &mut outcome,
        )?;
        let anchor_pos =
            sketch
                .get_point(&anchor_id)
                .ok_or_else(|| SketchOpError::EntityNotFound {
                    op: OP,
                    entity: anchor_id.to_string(),
                })?;
        let source_radius = match circle_source {
            Some(EntityRef::Circle(id)) => sketch.circles().get(&id).map(|e| e.circle.radius),
            _ => None,
        };
        let theta1 = (anchor_pos.y - center_pos.y).atan2(anchor_pos.x - center_pos.x);

        // Floret 1 = the source: hold it on its Vogel radius.
        mint_constraint(
            sketch,
            Constraint::new_dimensional(
                DimensionalConstraint::Distance(spacing),
                vec![EntityRef::Point(*center), EntityRef::Point(anchor_id)],
                ConstraintPriority::High,
            ),
            &mut outcome,
        );
        let spoke1 = sketch.add_line(*center, anchor_id)?;
        sketch.set_construction(&EntityRef::Line(spoke1), true)?;
        record_created(
            sketch,
            EntityRef::Line(spoke1),
            SketchOpKind::PhyllotaxisPattern,
            Some(*source),
            None,
            &mut outcome,
        );

        let mut prev_spoke = spoke1;
        let mut prev_circle = circle_source;
        for n in 2..=count {
            let n_f = n as f64;
            let r_n = spacing * n_f.sqrt();
            let theta_n = theta1 + (n_f - 1.0) * gamma;
            let pos = Point2d::new(
                center_pos.x + r_n * theta_n.cos(),
                center_pos.y + r_n * theta_n.sin(),
            );
            let pn = mint_point(
                sketch,
                pos,
                SketchOpKind::PhyllotaxisPattern,
                Some(*source),
                Some(n),
                &mut outcome,
            );
            let spoke = sketch.add_line(*center, pn)?;
            sketch.set_construction(&EntityRef::Line(spoke), true)?;
            record_created(
                sketch,
                EntityRef::Line(spoke),
                SketchOpKind::PhyllotaxisPattern,
                Some(*source),
                Some(n),
                &mut outcome,
            );
            mint_constraint(
                sketch,
                Constraint::new_dimensional(
                    DimensionalConstraint::Distance(r_n),
                    vec![EntityRef::Point(*center), EntityRef::Point(pn)],
                    ConstraintPriority::High,
                ),
                &mut outcome,
            );
            mint_constraint(
                sketch,
                Constraint::new_dimensional(
                    DimensionalConstraint::Angle(gamma),
                    vec![EntityRef::Line(prev_spoke), EntityRef::Line(spoke)],
                    ConstraintPriority::High,
                ),
                &mut outcome,
            );

            if let (Some(prev), Some(r)) = (prev_circle, source_radius) {
                let cn = sketch.add_circle_centered(pn, r)?;
                record_created(
                    sketch,
                    EntityRef::Circle(cn),
                    SketchOpKind::PhyllotaxisPattern,
                    Some(*source),
                    Some(n),
                    &mut outcome,
                );
                mint_constraint(
                    sketch,
                    Constraint::new_geometric(
                        GeometricConstraint::Equal,
                        vec![prev, EntityRef::Circle(cn)],
                        ConstraintPriority::High,
                    ),
                    &mut outcome,
                );
                prev_circle = Some(EntityRef::Circle(cn));
            }
            prev_spoke = spoke;
        }
    }
    Ok(outcome)
}

/// Sampled arc-length parameterisation of a pattern rail: cumulative
/// chord table over the rail's native parameter. PLACEMENT machinery
/// only — the maintained truth is the minted constraints
/// (`PointOnCurve` holds instances exactly on the live rail; the
/// chained `Distance` values are exact scalars measured at
/// placement). 512 spans keep the placement error far below solver
/// tolerance for any rail the constraints can subsequently tighten.
struct RailTable {
    /// (cumulative_arc_length, parameter) knots, ascending.
    knots: Vec<(f64, f64)>,
    total: f64,
}

impl RailTable {
    const SPANS: usize = 512;

    fn for_spline(spline: &super::Spline2d) -> Result<Self, SketchOpError> {
        const OP: &str = "curve_pattern";
        let (u0, u1) = spline.parameter_range();
        let mut knots = Vec::with_capacity(Self::SPANS + 1);
        let mut total = 0.0;
        let mut prev = spline
            .evaluate(u0)
            .map_err(|e| SketchOpError::Unsupported {
                op: OP,
                reason: format!("rail spline does not evaluate: {e}"),
            })?;
        knots.push((0.0, u0));
        for i in 1..=Self::SPANS {
            let u = u0 + (u1 - u0) * (i as f64) / (Self::SPANS as f64);
            let p = spline.evaluate(u).map_err(|e| SketchOpError::Unsupported {
                op: OP,
                reason: format!("rail spline does not evaluate: {e}"),
            })?;
            total += prev.distance_to(&p);
            knots.push((total, u));
            prev = p;
        }
        Ok(Self { knots, total })
    }

    /// Parameter at arc length `s` (linear interpolation between
    /// table knots; `s` must be within `[0, total]`).
    fn parameter_at(&self, s: f64) -> f64 {
        let s = s.clamp(0.0, self.total);
        match self
            .knots
            .binary_search_by(|(len, _)| len.partial_cmp(&s).unwrap_or(std::cmp::Ordering::Less))
        {
            Ok(i) => self.knots[i].1,
            Err(i) => {
                if i == 0 {
                    return self.knots[0].1;
                }
                if i >= self.knots.len() {
                    return self.knots[self.knots.len() - 1].1;
                }
                let (s0, u0) = self.knots[i - 1];
                let (s1, u1) = self.knots[i];
                if s1 - s0 < OP_EPS {
                    u0
                } else {
                    u0 + (u1 - u0) * (s - s0) / (s1 - s0)
                }
            }
        }
    }
}

/// Pattern along a curve: `count` total instances per source
/// (source = instance 0) stepped along a spline or arc rail at
/// arc-length intervals from the source anchor's closest point.
///
/// - `spacing = Some(d)` — arc-length step `d`; the run must fit the
///   rail's remaining length (typed `InvalidParameter` otherwise).
/// - `spacing = None` — the remaining length is divided evenly.
///
/// Maintenance: every instance gets `PointOnCurve` on the rail plus a
/// chained `Distance` to its predecessor whose value is the CHORD
/// measured at placement (documented honestly: arc-length spacing is
/// the placement rule; the maintained invariant is on-rail contact +
/// chord distances, which is what the 2D solver can hold exactly).
/// Circle sources add the `Equal`-radius chain. Rails may be
/// construction geometry (a guide spline that never enters the
/// profile). Supported rails: splines and arcs — other kinds refuse
/// typed.
pub fn curve_pattern(
    sketch: &Sketch,
    sources: &[EntityRef],
    rail: &EntityRef,
    count: usize,
    spacing: Option<f64>,
) -> Result<SketchOpOutcome, SketchOpError> {
    const OP: &str = "curve_pattern";
    if count < 2 {
        return Err(SketchOpError::InvalidParameter {
            op: OP,
            parameter: "count",
            reason: format!("need at least 2 instances (got {count})"),
        });
    }
    if let Some(d) = spacing {
        if !d.is_finite() || d < OP_EPS {
            return Err(SketchOpError::InvalidParameter {
                op: OP,
                parameter: "spacing",
                reason: format!("must be finite and positive (got {d})"),
            });
        }
    }
    validate_pattern_sources(sketch, sources, OP)?;

    // Rail geometry: a position function over an arc-length domain.
    enum Rail {
        Spline(super::Spline2d, RailTable),
        Arc { arc: Arc2d },
    }
    let rail_geom = match rail {
        EntityRef::Spline(id) => {
            let entry = sketch
                .splines()
                .get(id)
                .ok_or_else(|| SketchOpError::EntityNotFound {
                    op: OP,
                    entity: rail.to_string(),
                })?;
            let spline = entry.value().spline.clone();
            drop(entry);
            let table = RailTable::for_spline(&spline)?;
            Rail::Spline(spline, table)
        }
        EntityRef::Arc(id) => {
            let entry = sketch
                .arcs()
                .get(id)
                .ok_or_else(|| SketchOpError::EntityNotFound {
                    op: OP,
                    entity: rail.to_string(),
                })?;
            Rail::Arc {
                arc: entry.value().arc,
            }
        }
        other => {
            return Err(SketchOpError::Unsupported {
                op: OP,
                reason: format!(
                    "{other} is not a supported rail — curve patterns run along \
                     splines and arcs"
                ),
            })
        }
    };
    let rail_length = match &rail_geom {
        Rail::Spline(_, table) => table.total,
        Rail::Arc { arc } => arc.radius * arc.sweep_angle().abs(),
    };
    let position_at = |s: f64| -> Point2d {
        match &rail_geom {
            Rail::Spline(spline, table) => {
                let u = table.parameter_at(s);
                spline
                    .evaluate(u)
                    .unwrap_or(Point2d::new(f64::NAN, f64::NAN))
            }
            Rail::Arc { arc } => {
                let dir = if arc.ccw { 1.0 } else { -1.0 };
                let angle = arc.start_angle + dir * s / arc.radius;
                Point2d::new(
                    arc.center.x + arc.radius * angle.cos(),
                    arc.center.y + arc.radius * angle.sin(),
                )
            }
        }
    };
    // Closest arc length of a point on the rail (dense scan over the
    // same table resolution — placement machinery only).
    let closest_arc_length = |p: &Point2d| -> f64 {
        let mut best_s = 0.0;
        let mut best_d = f64::INFINITY;
        let samples = 512;
        for i in 0..=samples {
            let s = rail_length * (i as f64) / (samples as f64);
            let q = position_at(s);
            let d = (q.x - p.x).powi(2) + (q.y - p.y).powi(2);
            if d < best_d {
                best_d = d;
                best_s = s;
            }
        }
        best_s
    };

    // Validate the fit for EVERY source before mutating anything.
    let mut plans: Vec<(EntityRef, f64)> = Vec::with_capacity(sources.len());
    for source in sources {
        let anchor_pos = match source {
            EntityRef::Point(id) => sketch.get_point(id),
            EntityRef::Circle(id) => sketch.circle_center_position(id),
            _ => None,
        };
        let Some(pos) = anchor_pos else {
            return Err(SketchOpError::EntityNotFound {
                op: OP,
                entity: source.to_string(),
            });
        };
        let s0 = closest_arc_length(&pos);
        let step = match spacing {
            Some(d) => d,
            None => (rail_length - s0) / (count as f64 - 1.0),
        };
        let end = s0 + step * (count as f64 - 1.0);
        if step < OP_EPS || end > rail_length + OP_EPS {
            return Err(SketchOpError::InvalidParameter {
                op: OP,
                parameter: "spacing",
                reason: format!(
                    "{count} instances at step {step:.6} from arc length {s0:.6} need \
                     {end:.6} of rail, but the rail is {rail_length:.6} long"
                ),
            });
        }
        plans.push((*source, s0));
    }

    let mut outcome = SketchOpOutcome::new(SketchOpKind::CurvePattern);
    for (source, s0) in plans {
        let (anchor_id, circle_source) = pattern_anchor(
            sketch,
            &source,
            SketchOpKind::CurvePattern,
            OP,
            &mut outcome,
        )?;
        let source_radius = match circle_source {
            Some(EntityRef::Circle(id)) => sketch.circles().get(&id).map(|e| e.circle.radius),
            _ => None,
        };
        let step = match spacing {
            Some(d) => d,
            None => (rail_length - s0) / (count as f64 - 1.0),
        };

        // The first chord is measured from the anchor's ACTUAL
        // position (the source is untouched by the op and need not
        // sit on the rail) so every minted Distance is exactly
        // satisfied at placement.
        let anchor_actual =
            sketch
                .get_point(&anchor_id)
                .ok_or_else(|| SketchOpError::EntityNotFound {
                    op: OP,
                    entity: anchor_id.to_string(),
                })?;
        let mut prev_point = anchor_id;
        let mut prev_pos = anchor_actual;
        let mut prev_circle = circle_source;
        for k in 1..count {
            let pos = position_at(s0 + step * k as f64);
            if !pos.x.is_finite() || !pos.y.is_finite() {
                return Err(SketchOpError::Unsupported {
                    op: OP,
                    reason: "rail evaluation failed during placement".to_string(),
                });
            }
            let pk = mint_point(
                sketch,
                pos,
                SketchOpKind::CurvePattern,
                Some(source),
                Some(k),
                &mut outcome,
            );
            mint_constraint(sketch, point_on_curve(pk, *rail), &mut outcome);
            let chord = prev_pos.distance_to(&pos);
            mint_constraint(
                sketch,
                Constraint::new_dimensional(
                    DimensionalConstraint::Distance(chord),
                    vec![EntityRef::Point(prev_point), EntityRef::Point(pk)],
                    ConstraintPriority::High,
                ),
                &mut outcome,
            );
            if let (Some(prev), Some(r)) = (prev_circle, source_radius) {
                let ck = sketch.add_circle_centered(pk, r)?;
                record_created(
                    sketch,
                    EntityRef::Circle(ck),
                    SketchOpKind::CurvePattern,
                    Some(source),
                    Some(k),
                    &mut outcome,
                );
                mint_constraint(
                    sketch,
                    Constraint::new_geometric(
                        GeometricConstraint::Equal,
                        vec![prev, EntityRef::Circle(ck)],
                        ConstraintPriority::High,
                    ),
                    &mut outcome,
                );
                prev_circle = Some(EntityRef::Circle(ck));
            }
            prev_point = pk;
            prev_pos = pos;
        }
    }
    Ok(outcome)
}
