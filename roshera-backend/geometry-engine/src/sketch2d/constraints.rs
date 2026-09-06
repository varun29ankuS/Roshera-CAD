//! 2D Constraint system for sketching
//!
//! This module implements geometric and dimensional constraints for 2D sketches.
//! Constraints define relationships between sketch entities that must be maintained.
//!
//! # Constraint Types
//!
//! ## Geometric Constraints
//! - Coincident: Two points occupy the same location
//! - Parallel: Two lines have the same direction
//! - Perpendicular: Two lines are at 90 degrees
//! - Tangent: A line is tangent to a curve
//! - Concentric: Two circles/arcs share the same center
//! - Equal: Two entities have the same dimension
//! - Horizontal/Vertical: A line is aligned with an axis
//! - Symmetric: Entities are symmetric about a line
//!
//! ## Dimensional Constraints
//! - Distance: Fixed distance between points or parallel lines
//! - Angle: Fixed angle between lines
//! - Radius: Fixed radius for circles/arcs
//! - Length: Fixed length for line segments

use super::{
    Arc2dId, Circle2dId, Ellipse2dId, Line2dId, Point2dId, Polyline2dId, Rectangle2dId, Spline2dId,
};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use uuid::Uuid;

/// Unique identifier for a constraint
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConstraintId(pub Uuid);

impl ConstraintId {
    /// Create a new unique constraint ID
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for ConstraintId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Constraint_{}", &self.0.to_string()[..8])
    }
}

/// Entity reference for constraints
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub enum EntityRef {
    Point(Point2dId),
    Line(Line2dId),
    Arc(Arc2dId),
    Circle(Circle2dId),
    Rectangle(Rectangle2dId),
    Ellipse(Ellipse2dId),
    Spline(Spline2dId),
    Polyline(Polyline2dId),
}

impl fmt::Display for EntityRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EntityRef::Point(id) => write!(f, "{}", id),
            EntityRef::Line(id) => write!(f, "{}", id),
            EntityRef::Arc(id) => write!(f, "{}", id),
            EntityRef::Circle(id) => write!(f, "{}", id),
            EntityRef::Rectangle(id) => write!(f, "{}", id),
            EntityRef::Ellipse(id) => write!(f, "{}", id),
            EntityRef::Spline(id) => write!(f, "{}", id),
            EntityRef::Polyline(id) => write!(f, "{}", id),
        }
    }
}

impl EntityRef {
    /// The wire/diagnostic name of this entity's KIND — used by the
    /// constraint-shape refusals so a rejection can say what it was
    /// handed, not just what it wanted.
    pub fn kind_name(&self) -> &'static str {
        match self {
            EntityRef::Point(_) => "point",
            EntityRef::Line(_) => "line",
            EntityRef::Arc(_) => "arc",
            EntityRef::Circle(_) => "circle",
            EntityRef::Rectangle(_) => "rectangle",
            EntityRef::Ellipse(_) => "ellipse",
            EntityRef::Spline(_) => "spline",
            EntityRef::Polyline(_) => "polyline",
        }
    }

    /// Does this kind carry a defined POINT-LIKE position?
    ///
    /// A point IS its position; a circle, arc, rectangle or ellipse
    /// contributes its CENTRE (the legacy semantic the solver's
    /// `get_point_position` implements and `sketch_ops::pattern_anchor`
    /// depends on when it ties a pattern anchor to a legacy circle's
    /// centre with `Coincident`). A line, spline or polyline has NO
    /// such position — its leading parameters are an anchor point, a
    /// first control point or a first vertex, and reading them as
    /// "the entity's position" is the fabrication this predicate
    /// exists to stop.
    pub fn has_point_position(&self) -> bool {
        matches!(
            self,
            EntityRef::Point(_)
                | EntityRef::Circle(_)
                | EntityRef::Arc(_)
                | EntityRef::Rectangle(_)
                | EntityRef::Ellipse(_)
        )
    }

    /// Kinds `Coincident` accepts: a point, or a circle/arc standing
    /// for its CENTRE. Rectangles and ellipses are excluded even
    /// though they have a centre -- see the `Coincident` arm of
    /// [`Constraint::shape_is_defined`].
    pub fn is_coincidence_anchor(&self) -> bool {
        matches!(
            self,
            EntityRef::Point(_) | EntityRef::Circle(_) | EntityRef::Arc(_)
        )
    }

    /// Does this kind carry a defined CENTRE? (Everything
    /// point-like except a bare point.)
    pub fn has_centre(&self) -> bool {
        matches!(
            self,
            EntityRef::Circle(_)
                | EntityRef::Arc(_)
                | EntityRef::Rectangle(_)
                | EntityRef::Ellipse(_)
        )
    }

    /// Circular kinds: a centre plus a radius.
    pub fn is_round(&self) -> bool {
        matches!(self, EntityRef::Circle(_) | EntityRef::Arc(_))
    }

    /// Kinds that carry a CARRIER a point can be projected onto —
    /// the `PointOnCurve` / continuity carriers.
    pub fn is_curve(&self) -> bool {
        matches!(
            self,
            EntityRef::Line(_)
                | EntityRef::Circle(_)
                | EntityRef::Arc(_)
                | EntityRef::Spline(_)
                | EntityRef::Polyline(_)
        )
    }

    /// Kinds that carry a TANGENT and a CURVATURE the solver can
    /// actually read: `curve_join_frame` (G1/G2 continuity) and
    /// `curvature_at_point_foot` (curvature at a point's foot) both
    /// match exactly `Line | Circle | Arc | Spline`.
    ///
    /// This is deliberately NARROWER than [`Self::is_curve`], which
    /// includes POLYLINE. A polyline is a legitimate `PointOnCurve`
    /// carrier (the evaluator projects onto its closest segment) but
    /// has no tangent frame here, so blessing it for continuity or
    /// curvature would re-open the exact drift this shape table
    /// closes: the door accepts, the DOF tally debits, and the
    /// evaluator refuses.
    pub fn has_tangent_frame(&self) -> bool {
        matches!(
            self,
            EntityRef::Line(_) | EntityRef::Circle(_) | EntityRef::Arc(_) | EntityRef::Spline(_)
        )
    }

    /// Kinds the solver can integrate an AREA and a PERIMETER over
    /// (`entity_area` / `entity_perimeter`).
    pub fn has_area(&self) -> bool {
        matches!(
            self,
            EntityRef::Circle(_) | EntityRef::Rectangle(_) | EntityRef::Ellipse(_)
        )
    }
}

/// Geometric constraint types
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum GeometricConstraint {
    /// Two entities occupy the same POINT.
    ///
    /// At least one must be a point; the other may be a point, a
    /// circle or an arc, and for a circle or arc the constrained
    /// position is its CENTRE - not a point on its curve. To put a
    /// point ON a curve use [`GeometricConstraint::PointOnCurve`]; to
    /// make two circles or arcs share a centre use
    /// [`GeometricConstraint::Concentric`]. Both of those pairings are
    /// REFUSED here rather than quietly reinterpreted -- see
    /// [`Constraint::shape_is_defined`].
    Coincident,
    /// Two lines are parallel
    Parallel,
    /// Two lines are perpendicular
    Perpendicular,
    /// A line is tangent to a curve
    Tangent,
    /// Two circles/arcs are concentric
    Concentric,
    /// Two entities have equal dimension
    Equal,
    /// A line is horizontal
    Horizontal,
    /// A line is vertical
    Vertical,
    /// Entities are symmetric about a line
    Symmetric,
    /// A point lies on a curve
    PointOnCurve,
    /// A point is at the midpoint of a line
    Midpoint,
    /// Lines or curves are collinear
    Collinear,

    // Advanced constraint types
    /// Smooth tangent continuity between curves (G1 continuity)
    SmoothTangent,
    /// Curvature continuity between curves (G2 continuity)
    CurvatureContinuity,
    /// Two entities are offset by a fixed distance
    Offset,
    /// A curve is tangent to multiple entities (multi-tangent)
    MultiTangent,
    /// Entities maintain a fixed area relationship
    EqualArea,
    /// Entities maintain a fixed perimeter relationship
    EqualPerimeter,
    /// Point lies at the center of mass of a closed curve
    Centroid,
    /// Curve has minimum or maximum curvature at a point
    CurvatureExtremum,
    /// Two curves intersect at a specific angle
    IntersectionAngle(f64),
    /// Entity maintains contact with a boundary
    ContactConstraint,
}

/// Dimensional constraint types
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum DimensionalConstraint {
    /// Fixed distance between points
    Distance(f64),
    /// Fixed angle between lines (in radians)
    Angle(f64),
    /// Fixed radius for circle/arc
    Radius(f64),
    /// Fixed diameter for circle
    Diameter(f64),
    /// Fixed length for line segment
    Length(f64),
    /// Fixed X coordinate
    XCoordinate(f64),
    /// Fixed Y coordinate
    YCoordinate(f64),

    // Advanced dimensional constraints
    /// Fixed area for closed curves
    Area(f64),
    /// Fixed perimeter for closed curves
    Perimeter(f64),
    /// Fixed arc length for curves
    ArcLength(f64),
    /// Fixed curvature at a point
    Curvature(f64),
    /// Fixed slope (dy/dx) at a point
    Slope(f64),
    /// Fixed offset distance from a curve
    OffsetDistance(f64),
    /// Fixed aspect ratio (width/height)
    AspectRatio(f64),
    /// Fixed minimum distance between entities
    MinDistance(f64),
    /// Fixed maximum distance between entities
    MaxDistance(f64),
    /// Fixed moment of inertia
    MomentOfInertia(f64),
    /// Fixed center of mass position
    CenterOfMass { x: f64, y: f64 },
}

impl DimensionalConstraint {
    /// Replace the scalar value carried by a single-value dimensional
    /// constraint. Returns `Err(UnsupportedVariant)` for the two-field
    /// `CenterOfMass` variant — that constraint carries `{x, y}` and
    /// can't be re-targeted with a single scalar; updating it requires
    /// a richer API surface that the editable-measurements UX doesn't
    /// need today. Length-like values (distance, radius, perimeter,
    /// etc.) reject non-positive inputs; signed-valued constraints
    /// (XCoordinate, YCoordinate, Slope) accept any finite value.
    pub fn set_scalar(&mut self, value: f64) -> Result<(), DimensionalUpdateError> {
        if !value.is_finite() {
            return Err(DimensionalUpdateError::InvalidValue {
                value,
                reason: "value must be a finite real number",
            });
        }
        let require_positive = |v: f64, kind: &'static str| -> Result<(), DimensionalUpdateError> {
            if v <= 0.0 {
                Err(DimensionalUpdateError::InvalidValue {
                    value: v,
                    reason: kind,
                })
            } else {
                Ok(())
            }
        };
        match self {
            DimensionalConstraint::Distance(v) => {
                require_positive(value, "distance must be > 0")?;
                *v = value;
            }
            DimensionalConstraint::Radius(v) => {
                require_positive(value, "radius must be > 0")?;
                *v = value;
            }
            DimensionalConstraint::Diameter(v) => {
                require_positive(value, "diameter must be > 0")?;
                *v = value;
            }
            DimensionalConstraint::Length(v) => {
                require_positive(value, "length must be > 0")?;
                *v = value;
            }
            DimensionalConstraint::Perimeter(v) => {
                require_positive(value, "perimeter must be > 0")?;
                *v = value;
            }
            DimensionalConstraint::Area(v) => {
                require_positive(value, "area must be > 0")?;
                *v = value;
            }
            DimensionalConstraint::ArcLength(v) => {
                require_positive(value, "arc length must be > 0")?;
                *v = value;
            }
            DimensionalConstraint::OffsetDistance(v) => {
                require_positive(value, "offset distance must be > 0")?;
                *v = value;
            }
            DimensionalConstraint::AspectRatio(v) => {
                require_positive(value, "aspect ratio must be > 0")?;
                *v = value;
            }
            DimensionalConstraint::MinDistance(v) => {
                require_positive(value, "min distance must be > 0")?;
                *v = value;
            }
            DimensionalConstraint::MaxDistance(v) => {
                require_positive(value, "max distance must be > 0")?;
                *v = value;
            }
            DimensionalConstraint::MomentOfInertia(v) => {
                require_positive(value, "moment of inertia must be > 0")?;
                *v = value;
            }
            // Signed values: any finite scalar is admissible.
            DimensionalConstraint::Angle(v) => *v = value,
            DimensionalConstraint::XCoordinate(v) => *v = value,
            DimensionalConstraint::YCoordinate(v) => *v = value,
            DimensionalConstraint::Curvature(v) => *v = value,
            DimensionalConstraint::Slope(v) => *v = value,
            DimensionalConstraint::CenterOfMass { .. } => {
                return Err(DimensionalUpdateError::UnsupportedVariant {
                    variant: "CenterOfMass",
                });
            }
        }
        Ok(())
    }
}

/// Failure modes for `ConstraintStore::update_dimensional_value` and
/// the upstream REST endpoint. Geometric-constraint updates aren't
/// covered: they carry no editable scalar (the
/// `GeometricConstraint::IntersectionAngle(f64)` variant has a value
/// but it's a *property* of the relationship, not a dimension the
/// user types — leave that to a follow-up if it ever ships through
/// the UI).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DimensionalUpdateError {
    /// No constraint with that id exists in the store.
    #[error("constraint {0} not found")]
    NotFound(ConstraintId),
    /// The constraint exists but is geometric, not dimensional.
    #[error("constraint {0} is geometric and has no scalar value to edit")]
    NotDimensional(ConstraintId),
    /// The dimensional variant does not carry a single scalar value
    /// (e.g. `CenterOfMass { x, y }`).
    #[error("constraint variant {variant} is not editable via a single scalar")]
    UnsupportedVariant { variant: &'static str },
    /// The input fails domain validation (non-finite, sign, etc.).
    #[error("invalid value {value}: {reason}")]
    InvalidValue { value: f64, reason: &'static str },
}

/// Combined constraint type
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ConstraintType {
    Geometric(GeometricConstraint),
    Dimensional(DimensionalConstraint),
}

/// Constraint status
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ConstraintStatus {
    /// Constraint is satisfied
    Satisfied,
    /// Constraint is violated
    Violated {
        /// Current error/deviation
        error: f64,
        /// Suggested correction
        suggestion: Option<f64>,
    },
    /// Constraint is temporarily disabled
    Disabled,
    /// Constraint conflicts with others
    Conflicting,
}

/// Constraint priority for solver
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ConstraintPriority {
    /// Cannot be violated (e.g., user-fixed points)
    Required = 0,
    /// High priority (most constraints)
    High = 1,
    /// Medium priority
    Medium = 2,
    /// Low priority (can be relaxed if needed)
    Low = 3,
}

/// A constraint between sketch entities
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Constraint {
    /// Unique identifier
    pub id: ConstraintId,
    /// Type of constraint
    pub constraint_type: ConstraintType,
    /// Entities involved in the constraint
    pub entities: Vec<EntityRef>,
    /// Priority for solving
    pub priority: ConstraintPriority,
    /// Current status
    pub status: ConstraintStatus,
    /// User-defined name (optional)
    pub name: Option<String>,
}

impl Constraint {
    /// Create a new geometric constraint
    pub fn new_geometric(
        constraint_type: GeometricConstraint,
        entities: Vec<EntityRef>,
        priority: ConstraintPriority,
    ) -> Self {
        Self {
            id: ConstraintId::new(),
            constraint_type: ConstraintType::Geometric(constraint_type),
            entities,
            priority,
            status: ConstraintStatus::Satisfied,
            name: None,
        }
    }

    /// Create a new dimensional constraint
    pub fn new_dimensional(
        constraint_type: DimensionalConstraint,
        entities: Vec<EntityRef>,
        priority: ConstraintPriority,
    ) -> Self {
        Self {
            id: ConstraintId::new(),
            constraint_type: ConstraintType::Dimensional(constraint_type),
            entities,
            priority,
            status: ConstraintStatus::Satisfied,
            name: None,
        }
    }

    /// Check if constraint involves a specific entity
    pub fn involves_entity(&self, entity: &EntityRef) -> bool {
        self.entities.contains(entity)
    }

    /// Is this constraint's ENTITY SHAPE — its arity AND its entity
    /// kinds — one the kernel actually DEFINES?
    ///
    /// This is the ONE definition of "shape", used at every seam:
    ///
    /// * [`Constraint::degrees_of_freedom_removed`] debits ZERO for an
    ///   undefined shape, so a mismatched constraint can never buy a
    ///   `FullyConstrained` verdict;
    /// * the solver's `evaluate_constraint_error` emits the irreducible
    ///   refusal residual (`UNSUPPORTED_CONSTRAINT_RESIDUAL`) in every
    ///   budgeted row for an undefined shape, so a solve can never
    ///   report one satisfied;
    /// * `Sketch::try_add_constraint` and the
    ///   `POST /api/csketch/{id}/constraint` route reject one at the
    ///   door with a typed error naming the expected arity and kinds.
    ///
    /// Before this existed, ~12 classic arms answered a kind or arity
    /// mismatch with `vec![0.0]` — a residual that reads EXACTLY like
    /// "satisfied" — while the DOF tally still debited. `Parallel` on
    /// two circles read solved and removed a DOF nothing was pinning.
    ///
    /// Two kinds are deliberately shape-AGNOSTIC here:
    /// `ContactConstraint` and `MomentOfInertia` are recognised but
    /// carry no residual at all
    /// ([`ConstraintType::is_numerically_enforced`] is false for them),
    /// so there is no shape that would make them enforceable and none
    /// worth rejecting at the door.
    pub fn shape_is_defined(&self) -> bool {
        let e = self.entities.as_slice();
        match &self.constraint_type {
            ConstraintType::Geometric(g) => match g {
                // A POINT plus a point-like entity, and at least one
                // of the two must be an actual POINT.
                //
                // Coincidence against a circle or arc means its
                // CENTRE, which is what `sketch_ops::pattern_anchor`
                // mints to tie a pattern anchor to a legacy circle.
                // Two centre-bearing entities is NOT that relation:
                // `Coincident(circle, circle)` is `Concentric` said
                // badly, and a rectangle or ellipse has no
                // point-and-centre reading anybody asked for. Both are
                // refused, and `expected_shape` names the constraint
                // the caller wanted.
                GeometricConstraint::Coincident => match e {
                    [a, b] => {
                        a.is_coincidence_anchor()
                            && b.is_coincidence_anchor()
                            && (matches!(a, EntityRef::Point(_))
                                || matches!(b, EntityRef::Point(_)))
                    }
                    _ => false,
                },
                GeometricConstraint::Parallel | GeometricConstraint::Perpendicular => {
                    matches!(e, [EntityRef::Line(_), EntityRef::Line(_)])
                }
                GeometricConstraint::Horizontal | GeometricConstraint::Vertical => {
                    matches!(e, [EntityRef::Line(_)])
                }
                // Line-to-curve tangency only: the residual is the
                // distance from a centre to a line, compared against a
                // radius, so it needs exactly one line and one round
                // curve. Curve-to-curve tangency has no residual here.
                GeometricConstraint::Tangent => {
                    matches!(e, [EntityRef::Line(_), b] if b.is_round())
                        || matches!(e, [a, EntityRef::Line(_)] if a.is_round())
                }
                GeometricConstraint::Concentric => {
                    matches!(e, [a, b] if a.has_centre() && b.has_centre())
                }
                // Per-PAIRING: equal lengths (line pair), equal radii
                // (round pair, mixed circle/arc included), equal width
                // AND height (rectangle pair), equal semi-axes (ellipse
                // pair). Anything else has no comparable dimension.
                // Kept in lock-step with the solver's
                // `evaluate_equal_constraint` and its row budget.
                GeometricConstraint::Equal => match e {
                    [EntityRef::Line(_), EntityRef::Line(_)] => true,
                    [EntityRef::Rectangle(_), EntityRef::Rectangle(_)] => true,
                    [EntityRef::Ellipse(_), EntityRef::Ellipse(_)] => true,
                    [a, b] => a.is_round() && b.is_round(),
                    _ => false,
                },
                // Reflection needs a LINE axis; the arc pair takes the
                // 4-row arm, every other point-like pair the 2-row one.
                GeometricConstraint::Symmetric => {
                    matches!(e, [a, b, EntityRef::Line(_)]
                        if a.has_point_position() && b.has_point_position())
                }
                GeometricConstraint::PointOnCurve => {
                    matches!(e, [a, b] if a.has_point_position() && b.is_curve())
                }
                GeometricConstraint::Midpoint => {
                    matches!(e, [a, EntityRef::Line(_)] if a.has_point_position())
                }
                GeometricConstraint::Collinear => {
                    matches!(e, [a, b, c]
                        if a.has_point_position()
                            && b.has_point_position()
                            && c.has_point_position())
                }
                // G1/G2 continuity: two curves, optionally with the
                // join point named explicitly. Whether the two curves
                // actually MEET is state, not shape — `continuity_pair`
                // refuses that at evaluation time.
                GeometricConstraint::SmoothTangent | GeometricConstraint::CurvatureContinuity => {
                    matches!(e, [a, b] if a.has_tangent_frame() && b.has_tangent_frame())
                        || matches!(e, [a, b, EntityRef::Point(_)]
                            if a.has_tangent_frame() && b.has_tangent_frame())
                }
                GeometricConstraint::EqualArea | GeometricConstraint::EqualPerimeter => {
                    matches!(e, [a, b] if a.has_area() && b.has_area())
                }
                GeometricConstraint::Centroid => {
                    matches!(e, [a, b] if a.has_point_position() && b.has_centre())
                }
                // `[line1, line2]`, optionally with the intersection
                // point that locates where they meet.
                GeometricConstraint::IntersectionAngle(_) => {
                    matches!(e, [EntityRef::Line(_), EntityRef::Line(_)])
                        || matches!(
                            e,
                            [EntityRef::Line(_), EntityRef::Line(_), EntityRef::Point(_)]
                        )
                }
                GeometricConstraint::Offset => {
                    matches!(e, [EntityRef::Line(_), EntityRef::Line(_)])
                        || matches!(e, [a, b] if a.is_round() && b.is_round())
                }
                GeometricConstraint::MultiTangent => {
                    matches!(e, [EntityRef::Line(_), rest @ ..]
                        if !rest.is_empty() && rest.iter().all(|c| c.is_round()))
                }
                GeometricConstraint::CurvatureExtremum => {
                    matches!(e, [EntityRef::Spline(_), EntityRef::Point(_)])
                }
                // No residual for any shape — see the doc above.
                GeometricConstraint::ContactConstraint => true,
            },
            ConstraintType::Dimensional(d) => match d {
                DimensionalConstraint::Distance(_)
                | DimensionalConstraint::MinDistance(_)
                | DimensionalConstraint::MaxDistance(_) => {
                    matches!(e, [a, b] if a.has_point_position() && b.has_point_position())
                }
                DimensionalConstraint::Angle(_) => {
                    matches!(e, [EntityRef::Line(_), EntityRef::Line(_)])
                        || matches!(
                            e,
                            [EntityRef::Line(_), EntityRef::Line(_), EntityRef::Point(_)]
                        )
                }
                DimensionalConstraint::Radius(_) | DimensionalConstraint::Diameter(_) => {
                    matches!(e, [a] if a.is_round())
                }
                DimensionalConstraint::Length(_) | DimensionalConstraint::Slope(_) => {
                    matches!(e, [EntityRef::Line(_)])
                }
                DimensionalConstraint::XCoordinate(_) | DimensionalConstraint::YCoordinate(_) => {
                    matches!(e, [a] if a.has_point_position())
                }
                DimensionalConstraint::Area(_) | DimensionalConstraint::Perimeter(_) => {
                    matches!(e, [a] if a.has_area())
                }
                // `[curve]` = the whole swept length (arc or circle);
                // `[arc, p, q]` = the length between two points' feet.
                DimensionalConstraint::ArcLength(_) => {
                    matches!(e, [a] if a.is_round())
                        || matches!(
                            e,
                            [EntityRef::Arc(_), EntityRef::Point(_), EntityRef::Point(_)]
                        )
                }
                // `[curve]` = constant curvature (round curve, or a
                // line's zero); `[curve, point]` = curvature at the
                // point's foot, which is also the spline path.
                // `[curve]` = constant curvature (round curve, or a
                // line's zero); `[curve, point]` = curvature at the
                // point's foot. The second arm is `has_tangent_frame`,
                // NOT `is_curve`: `curvature_at_point_foot` has no
                // POLYLINE branch, and blessing one here would let the
                // door accept and debit a DOF for a residual the
                // evaluator can only refuse.
                DimensionalConstraint::Curvature(_) => {
                    matches!(e, [a] if a.is_round() || matches!(a, EntityRef::Line(_)))
                        || matches!(e, [a, EntityRef::Point(_)] if a.has_tangent_frame())
                }
                DimensionalConstraint::AspectRatio(_) => {
                    matches!(e, [EntityRef::Rectangle(_)] | [EntityRef::Ellipse(_)])
                }
                // NOTE: `entity_centroid` IS `get_circle_center`, so
                // for an ARC this pins the arc's CENTRE, not the
                // centroid of a lamina bounded by the arc. That is the
                // kernel's standing definition (a circle, rectangle
                // and ellipse agree with the true centroid; an open
                // arc does not have one), recorded here so the shape
                // table is not read as a stronger claim than the
                // evaluator makes.
                DimensionalConstraint::CenterOfMass { .. } => {
                    matches!(e, [a] if a.has_centre())
                }
                DimensionalConstraint::OffsetDistance(_) => {
                    matches!(e, [EntityRef::Line(_), EntityRef::Line(_)])
                        || matches!(e, [a, b] if a.is_round() && b.is_round())
                }
                // No residual for any shape — see the doc above.
                DimensionalConstraint::MomentOfInertia(_) => true,
            },
        }
    }

    /// The arity and kinds [`Constraint::shape_is_defined`] accepts,
    /// phrased for the human (or agent) whose constraint was just
    /// refused. One line, no runs of spaces — it is quoted verbatim
    /// into the REST error body.
    pub fn expected_shape(&self) -> &'static str {
        match &self.constraint_type {
            ConstraintType::Geometric(g) => match g {
                GeometricConstraint::Coincident => concat!(
                    "exactly 2 entities, at least one a point, the other a point, circle or arc ",
                    "(a circle or arc means its CENTRE); for two circles or arcs use Concentric, ",
                    "for a point ON a curve use PointOnCurve"
                ),
                GeometricConstraint::Parallel | GeometricConstraint::Perpendicular => {
                    "exactly 2 lines"
                }
                GeometricConstraint::Horizontal | GeometricConstraint::Vertical => "exactly 1 line",
                GeometricConstraint::Tangent => "exactly 2 entities: 1 line and 1 circle or arc",
                GeometricConstraint::Concentric => {
                    "exactly 2 entities with a centre (circle, arc, rectangle, ellipse)"
                }
                GeometricConstraint::Equal => concat!(
                    "exactly 2 entities of comparable dimension: 2 lines, ",
                    "2 circles or arcs, 2 rectangles, or 2 ellipses"
                ),
                GeometricConstraint::Symmetric => {
                    "exactly 3 entities: 2 with a position and a line axis"
                }
                GeometricConstraint::PointOnCurve => concat!(
                    "exactly 2 entities: 1 with a position and 1 curve ",
                    "(line, circle, arc, spline, polyline)"
                ),
                GeometricConstraint::Midpoint => "exactly 2 entities: 1 with a position and 1 line",
                GeometricConstraint::Collinear => "exactly 3 entities, each with a position",
                GeometricConstraint::SmoothTangent | GeometricConstraint::CurvatureContinuity => {
                    "2 lines, circles, arcs or splines, optionally followed by the join point"
                }
                GeometricConstraint::EqualArea | GeometricConstraint::EqualPerimeter => {
                    "exactly 2 entities enclosing an area (circle, rectangle, ellipse)"
                }
                GeometricConstraint::Centroid => {
                    "exactly 2 entities: 1 with a position and 1 with a centre"
                }
                GeometricConstraint::IntersectionAngle(_) => {
                    "2 lines, optionally followed by the intersection point"
                }
                GeometricConstraint::Offset => "exactly 2 lines, or 2 circles or arcs",
                GeometricConstraint::MultiTangent => "1 line followed by 1 or more circles or arcs",
                GeometricConstraint::CurvatureExtremum => "exactly 1 spline and 1 point",
                GeometricConstraint::ContactConstraint => "any entities",
            },
            ConstraintType::Dimensional(d) => match d {
                DimensionalConstraint::Distance(_)
                | DimensionalConstraint::MinDistance(_)
                | DimensionalConstraint::MaxDistance(_) => {
                    "exactly 2 entities with a position (point, circle, arc, rectangle, ellipse)"
                }
                DimensionalConstraint::Angle(_) => {
                    "2 lines, optionally followed by the intersection point"
                }
                DimensionalConstraint::Radius(_) | DimensionalConstraint::Diameter(_) => {
                    "exactly 1 circle or arc"
                }
                DimensionalConstraint::Length(_) | DimensionalConstraint::Slope(_) => {
                    "exactly 1 line"
                }
                DimensionalConstraint::XCoordinate(_) | DimensionalConstraint::YCoordinate(_) => {
                    "exactly 1 entity with a position"
                }
                DimensionalConstraint::Area(_) | DimensionalConstraint::Perimeter(_) => {
                    "exactly 1 entity enclosing an area (circle, rectangle, ellipse)"
                }
                DimensionalConstraint::ArcLength(_) => {
                    "1 circle or arc, or 1 arc followed by 2 points"
                }
                DimensionalConstraint::Curvature(_) => concat!(
                    "1 line, circle or arc, or 1 line, circle, arc or spline ",
                    "followed by 1 point"
                ),
                DimensionalConstraint::AspectRatio(_) => "exactly 1 rectangle or ellipse",
                DimensionalConstraint::CenterOfMass { .. } => "exactly 1 entity with a centre",
                DimensionalConstraint::OffsetDistance(_) => "exactly 2 lines, or 2 circles or arcs",
                DimensionalConstraint::MomentOfInertia(_) => "any entities",
            },
        }
    }

    /// The kinds this constraint was actually handed, in wire order.
    pub fn entity_kind_names(&self) -> Vec<&'static str> {
        self.entities.iter().map(EntityRef::kind_name).collect()
    }

    /// Refuse a constraint whose shape the kernel does not define,
    /// naming the expected arity and kinds alongside the kinds
    /// supplied.
    ///
    /// Called at the doors — `Sketch::try_add_constraint` and the
    /// csketch route — so a constraint the solver could only ever
    /// refuse never enters the store in the first place.
    pub fn validate_shape(&self) -> super::Sketch2dResult<()> {
        if self.shape_is_defined() {
            return Ok(());
        }
        Err(super::Sketch2dError::UndefinedConstraintShape {
            constraint: format!("{:?}", self.constraint_type),
            expected: self.expected_shape().to_string(),
            got: self.entity_kind_names().join(", "),
        })
    }

    /// Get the number of degrees of freedom this constraint removes
    pub fn degrees_of_freedom_removed(&self) -> usize {
        // A shape the kernel does not define removes NOTHING. The
        // solver refuses it with an irreducible residual
        // (`UNSUPPORTED_CONSTRAINT_RESIDUAL`) rather than a zero row,
        // so debiting a DOF here would hand a mismatched constraint —
        // `Parallel` on two circles, `Coincident` on three points —
        // the freedom it never pins, and let a per-component tally
        // fold to `FullyConstrained` on the strength of it. This is
        // the same per-shape rule `Offset`, `MultiTangent` and
        // `CurvatureExtremum` already carried in their own arms,
        // lifted to every kind and driven from ONE definition of
        // shape ([`Constraint::shape_is_defined`]).
        if !self.shape_is_defined() {
            return 0;
        }
        match &self.constraint_type {
            ConstraintType::Geometric(g) => match g {
                GeometricConstraint::Coincident => 2,    // Removes X and Y
                GeometricConstraint::Parallel => 1,      // Removes angle
                GeometricConstraint::Perpendicular => 1, // Removes angle
                GeometricConstraint::Tangent => 1,       // Removes one DOF
                GeometricConstraint::Concentric => 2,    // Removes center position
                // PER-PAIRING, in lock-step with the solver's
                // `evaluate_equal_constraint` and `constraint_error_count`:
                // a rectangle pair equates width AND height, an
                // ellipse pair both semi-axes — two independent rows,
                // two DOF. A line pair (length) and a round pair
                // (radius) equate one scalar. Reading 1 for the
                // rectangle/ellipse pairings left every sketch that
                // used one permanently 1 DOF under-constrained.
                GeometricConstraint::Equal => match self.entities.as_slice() {
                    [EntityRef::Rectangle(_), EntityRef::Rectangle(_)]
                    | [EntityRef::Ellipse(_), EntityRef::Ellipse(_)] => 2,
                    _ => 1,
                },
                GeometricConstraint::Horizontal => 1, // Removes Y variation
                GeometricConstraint::Vertical => 1,   // Removes X variation
                // Symmetric: 2 (reflected position) generically; an
                // ARC PAIR removes 4 (reflected center + reflected
                // traversal-normalized angles — SKETCH-DCM #45
                // follow-ups A; with the mirror op's Equal radius this
                // pins all 5 legacy-arc parameters). Keep in lock-step
                // with the solver's `evaluate_symmetric_arc_pair` /
                // `constraint_error_count`.
                GeometricConstraint::Symmetric => match self.entities.as_slice() {
                    [EntityRef::Arc(_), EntityRef::Arc(_), _] => 4,
                    _ => 2,
                },
                GeometricConstraint::PointOnCurve => 1, // One parameter
                GeometricConstraint::Midpoint => 2,     // X and Y
                GeometricConstraint::Collinear => 1,    // One DOF per entity

                // Advanced constraint types
                GeometricConstraint::SmoothTangent => 1,
                GeometricConstraint::CurvatureContinuity => 2,
                GeometricConstraint::EqualArea => 1,
                GeometricConstraint::EqualPerimeter => 1,
                GeometricConstraint::Centroid => 2,
                GeometricConstraint::IntersectionAngle(_) => 1,

                // Offset-pair correspondence (SKETCH-DCM #45 Slice 6).
                // Line pair: the offset line's endpoints correspond
                // perpendicular to the source and share one common gap —
                // 3 DOF (the gap magnitude stays free; `OffsetDistance`
                // pins it). Circle/arc pair: concentric — 2 DOF (the
                // radial gap stays free). Any other pairing has no
                // defined correspondence: 0 DOF + the solver's
                // irreducible refuse residual.
                GeometricConstraint::Offset => match self.entities.as_slice() {
                    [EntityRef::Line(_), EntityRef::Line(_)] => 3,
                    [EntityRef::Circle(_) | EntityRef::Arc(_), EntityRef::Circle(_) | EntityRef::Arc(_)] => {
                        2
                    }
                    _ => 0,
                },
                // One line tangent to each of N trailing circles/arcs
                // (SKETCH-DCM #45 Slice 6): one tangency DOF per curve.
                // Malformed shapes (no leading line, non-circular
                // curves) remove 0 DOF and refuse in the solver.
                GeometricConstraint::MultiTangent => match self.entities.as_slice() {
                    [EntityRef::Line(_), rest @ ..]
                        if !rest.is_empty()
                            && rest
                                .iter()
                                .all(|e| matches!(e, EntityRef::Circle(_) | EntityRef::Arc(_))) =>
                    {
                        rest.len()
                    }
                    _ => 0,
                },

                // Stationary curvature at a point's foot (SKETCH-DCM
                // #45 Slice 7): one row / one DOF for the supported
                // `[spline, point]` shape (∂κ/∂u = 0 at the foot);
                // every other shape removes ZERO DOF and refuses in
                // the solver — the per-shape refusal pattern Offset /
                // MultiTangent established in Slice 6.
                GeometricConstraint::CurvatureExtremum => match self.entities.as_slice() {
                    [EntityRef::Spline(_), EntityRef::Point(_)] => 1,
                    _ => 0,
                },

                // Recognised but not yet enforceable by the numerical
                // solver (no real residual equation). Removes ZERO
                // DOF so it can never fake a "fully constrained"
                // verdict; the solver additionally emits an irreducible
                // residual (see `UNSUPPORTED_CONSTRAINT_RESIDUAL`) so a
                // sketch carrying one is never reported solved. Keep this
                // set in lock-step with `ConstraintType::is_numerically_enforced`.
                GeometricConstraint::ContactConstraint => 0,
            },
            ConstraintType::Dimensional(d) => match d {
                DimensionalConstraint::Distance(_) => 1,
                DimensionalConstraint::Angle(_) => 1,
                DimensionalConstraint::Radius(_) => 1,
                DimensionalConstraint::Diameter(_) => 1,
                DimensionalConstraint::Length(_) => 1,
                DimensionalConstraint::XCoordinate(_) => 1,
                DimensionalConstraint::YCoordinate(_) => 1,

                // Advanced dimensional constraints
                DimensionalConstraint::Area(_) => 1,
                DimensionalConstraint::Perimeter(_) => 1,
                // ArcLength: `[curve]` = total swept length;
                // `[arc, p, q]` (SKETCH-DCM #45 follow-ups A) = arc
                // length between the points' angular feet along the
                // rail. Other shapes remove ZERO DOF and refuse in the
                // solver (per-shape convention).
                DimensionalConstraint::ArcLength(_) => match self.entities.as_slice() {
                    [_] => 1,
                    [EntityRef::Arc(_), EntityRef::Point(_), EntityRef::Point(_)] => 1,
                    _ => 0,
                },
                DimensionalConstraint::Curvature(_) => 1,
                DimensionalConstraint::Slope(_) => 1,
                DimensionalConstraint::AspectRatio(_) => 1,
                DimensionalConstraint::CenterOfMass { .. } => 2,

                // Offset-gap magnitude (SKETCH-DCM #45 Slice 6): pins
                // the one scalar `Offset` leaves free. Only defined for
                // the pairings `Offset` defines correspondence for;
                // anything else removes 0 DOF and refuses in the solver.
                DimensionalConstraint::OffsetDistance(_) => match self.entities.as_slice() {
                    [EntityRef::Line(_), EntityRef::Line(_)]
                    | [EntityRef::Circle(_) | EntityRef::Arc(_), EntityRef::Circle(_) | EntityRef::Arc(_)] => {
                        1
                    }
                    _ => 0,
                },

                // One-sided inequalities (SKETCH-DCM #45 Slice 6):
                // numerically ENFORCED via an active-set-style residual
                // (`max(0, bound - d)` / `max(0, d - bound)`) but they
                // remove ZERO DOF by design — the D-Cubed convention: a
                // satisfied inequality is inactive and consumes no
                // freedom, and a one-sided bound never pins a parameter
                // to a point value.
                DimensionalConstraint::MinDistance(_) | DimensionalConstraint::MaxDistance(_) => 0,

                // Not yet enforceable — removes ZERO DOF (see the
                // geometric note above); needs region mass-property
                // residuals w.r.t. sketch DOFs (exact-mass-properties
                // campaign, OCCT-parity roadmap #3).
                DimensionalConstraint::MomentOfInertia(_) => 0,
            },
        }
    }
}

impl ConstraintType {
    /// Whether the numerical constraint solver enforces this constraint
    /// with a real residual equation.
    ///
    /// `false` for constraints that are recognised by the type system
    /// but not yet backed by a residual — they remove zero degrees of
    /// freedom and the solver emits an irreducible residual for them so
    /// they can never contribute to a false "solved" verdict. The set
    /// here MUST match the zero-DOF / refusal arms in
    /// [`Constraint::degrees_of_freedom_removed`] and the solver's
    /// `evaluate_geometric_constraint` / `evaluate_dimensional_constraint`.
    ///
    /// SKETCH-DCM #45 Slice 6 shrank the refuse set from eight to three:
    /// `Offset`, `MultiTangent`, `OffsetDistance`, `MinDistance` and
    /// `MaxDistance` now carry real residuals; Slice 7 lifted
    /// `CurvatureExtremum` (∂κ/∂u = 0 on `[spline, point]`), leaving
    /// {`ContactConstraint`, `MomentOfInertia`}. Note the distinction:
    /// this method is TYPE-level. `Offset`/`OffsetDistance`/
    /// `MultiTangent`/`CurvatureExtremum` still refuse per-SHAPE (an
    /// unsupported entity pairing emits the irreducible residual and
    /// removes 0 DOF), which is why
    /// [`analyze_dofs`](super::sketch_solver::analyze_dofs)
    /// forces the numeric diagnosis whenever a constraint removes zero
    /// structural DOF.
    pub fn is_numerically_enforced(&self) -> bool {
        !matches!(
            self,
            ConstraintType::Geometric(GeometricConstraint::ContactConstraint)
                | ConstraintType::Dimensional(DimensionalConstraint::MomentOfInertia(_))
        )
    }

    /// One-sided inequality kinds (`MinDistance` / `MaxDistance`).
    ///
    /// Their residual is `max(0, …)` — ZERO with a zero gradient
    /// whenever the bound is satisfied — so rank analysis must not
    /// classify a satisfied (inactive) inequality as a redundant
    /// duplicate: it carries standing information (the bound) even
    /// while inactive.
    pub fn is_inequality(&self) -> bool {
        matches!(
            self,
            ConstraintType::Dimensional(
                DimensionalConstraint::MinDistance(_) | DimensionalConstraint::MaxDistance(_)
            )
        )
    }
}

/// Constraint storage using DashMap for concurrent access
pub struct ConstraintStore {
    /// All constraints indexed by ID
    constraints: Arc<DashMap<ConstraintId, Constraint>>,
    /// Constraints indexed by entity
    entity_constraints: Arc<DashMap<EntityRef, Vec<ConstraintId>>>,
    /// Constraint groups for related constraints
    constraint_groups: Arc<DashMap<String, Vec<ConstraintId>>>,
}

impl ConstraintStore {
    /// Create a new constraint store
    pub fn new() -> Self {
        Self {
            constraints: Arc::new(DashMap::new()),
            entity_constraints: Arc::new(DashMap::new()),
            constraint_groups: Arc::new(DashMap::new()),
        }
    }

    /// Add a constraint
    pub fn add_constraint(&self, constraint: Constraint) -> ConstraintId {
        let id = constraint.id;

        // Update entity index
        for entity in &constraint.entities {
            self.entity_constraints.entry(*entity).or_default().push(id);
        }

        // Store constraint
        self.constraints.insert(id, constraint);

        id
    }

    /// Remove a constraint
    pub fn remove_constraint(&self, id: &ConstraintId) -> Option<Constraint> {
        if let Some((_, constraint)) = self.constraints.remove(id) {
            // Remove from entity index
            for entity in &constraint.entities {
                if let Some(mut entity_constraints) = self.entity_constraints.get_mut(entity) {
                    entity_constraints.retain(|&c| c != *id);
                }
            }

            Some(constraint)
        } else {
            None
        }
    }

    /// Get a constraint by ID
    pub fn get(&self, id: &ConstraintId) -> Option<Constraint> {
        self.constraints.get(id).map(|entry| entry.clone())
    }

    /// Get all constraints for an entity
    pub fn get_entity_constraints(&self, entity: &EntityRef) -> Vec<Constraint> {
        self.entity_constraints
            .get(entity)
            .map(|ids| ids.iter().filter_map(|id| self.get(id)).collect())
            .unwrap_or_default()
    }

    /// Update constraint status
    pub fn update_status(&self, id: &ConstraintId, status: ConstraintStatus) {
        if let Some(mut constraint) = self.constraints.get_mut(id) {
            constraint.status = status;
        }
    }

    /// Edit the scalar value of a dimensional constraint in place.
    ///
    /// On success the constraint's `constraint_type` carries the new
    /// value and its `status` is reset to `Satisfied` so a subsequent
    /// solve gets a clean slate — the previous violation report (if
    /// any) belonged to the old target and would be misleading next to
    /// a freshly-typed dimension.
    ///
    /// On failure the store is left untouched, including the existing
    /// status and value, so callers can safely propagate the error to
    /// the user without rolling anything back.
    pub fn update_dimensional_value(
        &self,
        id: &ConstraintId,
        value: f64,
    ) -> Result<Constraint, DimensionalUpdateError> {
        let mut entry = self
            .constraints
            .get_mut(id)
            .ok_or(DimensionalUpdateError::NotFound(*id))?;
        let dim = match &mut entry.constraint_type {
            ConstraintType::Dimensional(d) => d,
            ConstraintType::Geometric(_) => {
                return Err(DimensionalUpdateError::NotDimensional(*id));
            }
        };
        dim.set_scalar(value)?;
        entry.status = ConstraintStatus::Satisfied;
        Ok(entry.clone())
    }

    /// Get all constraints
    pub fn all_constraints(&self) -> Vec<Constraint> {
        self.constraints
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get constraints by type
    pub fn get_by_type(&self, constraint_type: ConstraintType) -> Vec<Constraint> {
        self.constraints
            .iter()
            .filter(|entry| entry.constraint_type == constraint_type)
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Check for conflicts between constraints
    pub fn find_conflicts(&self) -> Vec<(ConstraintId, ConstraintId)> {
        let mut conflicts = Vec::new();
        let all_constraints: Vec<_> = self.constraints.iter().collect();

        // Check for conflicts between pairs of constraints
        for i in 0..all_constraints.len() {
            for j in (i + 1)..all_constraints.len() {
                let constraint1 = all_constraints[i].value();
                let constraint2 = all_constraints[j].value();

                if self.constraints_conflict(constraint1, constraint2) {
                    conflicts.push((constraint1.id, constraint2.id));
                }
            }
        }

        // Transitive coincidence vs separation (the DCM-grade tier): points
        // joined by a CHAIN of Coincident constraints share one location, so any
        // Distance > 0 between two of them is contradictory even though no single
        // Coincident names that pair (e.g. a≡b, b≡c, distance(a,c)=10). Pairwise
        // comparison cannot see it; a union-find over the Coincident graph does.
        let mut parent: std::collections::HashMap<Point2dId, Point2dId> =
            std::collections::HashMap::new();
        let mut witness: std::collections::HashMap<Point2dId, ConstraintId> =
            std::collections::HashMap::new();
        for entry in self.constraints.iter() {
            let con = entry.value();
            if let ConstraintType::Geometric(GeometricConstraint::Coincident) = &con.constraint_type
            {
                if let [EntityRef::Point(a), EntityRef::Point(b)] = con.entities.as_slice() {
                    let (a, b) = (*a, *b);
                    let ra = Self::uf_find(&mut parent, a);
                    let rb = Self::uf_find(&mut parent, b);
                    if ra != rb {
                        parent.insert(ra, rb);
                    }
                    witness.entry(a).or_insert(con.id);
                    witness.entry(b).or_insert(con.id);
                }
            }
        }
        if !parent.is_empty() {
            for entry in self.constraints.iter() {
                let con = entry.value();
                if let ConstraintType::Dimensional(DimensionalConstraint::Distance(v)) =
                    &con.constraint_type
                {
                    if v.abs() > 1e-9 {
                        if let [EntityRef::Point(p), EntityRef::Point(q)] = con.entities.as_slice()
                        {
                            let (p, q) = (*p, *q);
                            if Self::uf_find(&mut parent, p) == Self::uf_find(&mut parent, q) {
                                if let Some(w) =
                                    witness.get(&p).or_else(|| witness.get(&q)).copied()
                                {
                                    conflicts.push((con.id, w));
                                }
                            }
                        }
                    }
                }
            }

            // Coordinate coherence within a coincident group: points forced
            // coincident cannot carry different fixed X (or Y) coordinates, even
            // when the two coordinate constraints name DIFFERENT points in the
            // same chain (x(a)=0, x(c)=5 with a≡b≡c). The same-point case is
            // already caught pairwise; here we only consider coincidence-touched
            // points so we don't double-report it.
            let mut x_by_root: std::collections::HashMap<Point2dId, (f64, ConstraintId)> =
                std::collections::HashMap::new();
            let mut y_by_root: std::collections::HashMap<Point2dId, (f64, ConstraintId)> =
                std::collections::HashMap::new();
            for entry in self.constraints.iter() {
                let con = entry.value();
                let coord = match &con.constraint_type {
                    ConstraintType::Dimensional(DimensionalConstraint::XCoordinate(v)) => {
                        Some((true, *v))
                    }
                    ConstraintType::Dimensional(DimensionalConstraint::YCoordinate(v)) => {
                        Some((false, *v))
                    }
                    _ => None,
                };
                if let Some((is_x, v)) = coord {
                    if let [EntityRef::Point(p)] = con.entities.as_slice() {
                        let p = *p;
                        // `witness` holds every coincidence-touched point;
                        // `parent` does not (a group's final root is only ever a
                        // value, never a key), so gate on `witness`.
                        if !witness.contains_key(&p) {
                            continue; // not in any coincident group
                        }
                        let root = Self::uf_find(&mut parent, p);
                        let map = if is_x { &mut x_by_root } else { &mut y_by_root };
                        match map.get(&root) {
                            Some(&(prev_v, prev_id)) => {
                                if (prev_v - v).abs() > 1e-9 {
                                    conflicts.push((con.id, prev_id));
                                }
                            }
                            None => {
                                map.insert(root, (v, con.id));
                            }
                        }
                    }
                }
            }
        }

        conflicts
    }

    /// Union-find root with path compression over point ids. Used by
    /// `find_conflicts` to take the transitive closure of Coincident
    /// constraints so a chained coincidence is treated as one location.
    fn uf_find(
        parent: &mut std::collections::HashMap<Point2dId, Point2dId>,
        x: Point2dId,
    ) -> Point2dId {
        let p = match parent.get(&x) {
            Some(&p) => p,
            None => return x,
        };
        if p == x {
            return x;
        }
        let root = Self::uf_find(parent, p);
        parent.insert(x, root);
        root
    }

    /// Check if two constraints conflict with each other
    fn constraints_conflict(&self, c1: &Constraint, c2: &Constraint) -> bool {
        // If constraints don't share any entities, they can't conflict
        if !self.constraints_share_entities(c1, c2) {
            return false;
        }

        match (&c1.constraint_type, &c2.constraint_type) {
            // Dimensional conflicts: two different fixed values for same property
            (ConstraintType::Dimensional(d1), ConstraintType::Dimensional(d2)) => {
                self.dimensional_constraints_conflict(c1, c2, d1, d2)
            }

            // Geometric conflicts: contradictory geometric relationships
            (ConstraintType::Geometric(g1), ConstraintType::Geometric(g2)) => {
                self.geometric_constraints_conflict(c1, c2, g1, g2)
            }

            // Mixed conflicts: dimensional constraint contradicts geometric
            (ConstraintType::Dimensional(d), ConstraintType::Geometric(g))
            | (ConstraintType::Geometric(g), ConstraintType::Dimensional(d)) => {
                self.mixed_constraints_conflict(c1, c2, d, g)
            }
        }
    }

    /// Check if two constraints share any entities
    fn constraints_share_entities(&self, c1: &Constraint, c2: &Constraint) -> bool {
        for entity1 in &c1.entities {
            for entity2 in &c2.entities {
                if std::mem::discriminant(entity1) == std::mem::discriminant(entity2) {
                    match (entity1, entity2) {
                        (EntityRef::Point(id1), EntityRef::Point(id2)) => {
                            if id1 == id2 {
                                return true;
                            }
                        }
                        (EntityRef::Line(id1), EntityRef::Line(id2)) => {
                            if id1 == id2 {
                                return true;
                            }
                        }
                        (EntityRef::Arc(id1), EntityRef::Arc(id2)) => {
                            if id1 == id2 {
                                return true;
                            }
                        }
                        (EntityRef::Circle(id1), EntityRef::Circle(id2)) => {
                            if id1 == id2 {
                                return true;
                            }
                        }
                        (EntityRef::Rectangle(id1), EntityRef::Rectangle(id2)) => {
                            if id1 == id2 {
                                return true;
                            }
                        }
                        (EntityRef::Ellipse(id1), EntityRef::Ellipse(id2)) => {
                            if id1 == id2 {
                                return true;
                            }
                        }
                        (EntityRef::Spline(id1), EntityRef::Spline(id2)) => {
                            if id1 == id2 {
                                return true;
                            }
                        }
                        (EntityRef::Polyline(id1), EntityRef::Polyline(id2)) => {
                            if id1 == id2 {
                                return true;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        false
    }

    /// Check conflicts between dimensional constraints
    fn dimensional_constraints_conflict(
        &self,
        c1: &Constraint,
        c2: &Constraint,
        d1: &DimensionalConstraint,
        d2: &DimensionalConstraint,
    ) -> bool {
        use DimensionalConstraint::*;

        // Check for conflicting fixed values on same entities
        match (d1, d2) {
            // Two different distances for same point pair
            (Distance(v1), Distance(v2)) => {
                if self.same_entity_pairs(c1, c2) && (v1 - v2).abs() > 1e-10 {
                    return true;
                }
            }

            // Two different lengths for same line
            (Length(v1), Length(v2)) => {
                if self.constraints_share_entities(c1, c2) && (v1 - v2).abs() > 1e-10 {
                    return true;
                }
            }

            // Two different radii for same circle/arc
            (Radius(v1), Radius(v2)) => {
                if self.constraints_share_entities(c1, c2) && (v1 - v2).abs() > 1e-10 {
                    return true;
                }
            }

            // Radius vs diameter conflict
            (Radius(r), Diameter(d)) | (Diameter(d), Radius(r)) => {
                if self.constraints_share_entities(c1, c2) && (2.0 * r - d).abs() > 1e-10 {
                    return true;
                }
            }

            // Different coordinates for same point
            (XCoordinate(v1), XCoordinate(v2)) | (YCoordinate(v1), YCoordinate(v2)) => {
                if self.constraints_share_entities(c1, c2) && (v1 - v2).abs() > 1e-10 {
                    return true;
                }
            }

            _ => {}
        }

        false
    }

    /// Check conflicts between geometric constraints
    fn geometric_constraints_conflict(
        &self,
        c1: &Constraint,
        c2: &Constraint,
        g1: &GeometricConstraint,
        g2: &GeometricConstraint,
    ) -> bool {
        use GeometricConstraint::*;

        if !self.constraints_share_entities(c1, c2) {
            return false;
        }

        match (g1, g2) {
            // Parallel and perpendicular are contradictory
            (Parallel, Perpendicular) | (Perpendicular, Parallel) => true,

            // Collinear lines lie on the same infinite line, hence are parallel;
            // a perpendicular requirement contradicts that. (Collinear+Parallel,
            // by contrast, AGREE and must not be flagged.)
            (Collinear, Perpendicular) | (Perpendicular, Collinear) => true,

            // Horizontal and vertical are contradictory (for same line)
            (Horizontal, Vertical) | (Vertical, Horizontal) => true,

            // A line can't be both horizontal and have an angle constraint
            (Horizontal, _) | (_, Horizontal) | (Vertical, _) | (_, Vertical) => {
                // This would need more context to determine conflict
                false
            }

            _ => false,
        }
    }

    /// Check conflicts between a dimensional and a geometric constraint on
    /// the same entity pair. Specific syntactic incompatibilities (distance
    /// = 0 with non-coincident, angle = 0/180 with non-parallel/non-collinear,
    /// angle = 90 with non-perpendicular) require unifying the two constraint
    /// kinds in a satisfiability check, which is performed by the constraint
    /// solver during `solve()` rather than pre-flight conflict detection.
    /// We therefore report no static conflict here — the solver surfaces the
    /// incompatibility as an unsatisfied residual.
    fn mixed_constraints_conflict(
        &self,
        c1: &Constraint,
        c2: &Constraint,
        d: &DimensionalConstraint,
        g: &GeometricConstraint,
    ) -> bool {
        // Only constraints on the same ordered entity tuple can contradict.
        if !self.same_entity_pairs(c1, c2) {
            return false;
        }
        // Coincident forces ZERO separation between the two points; any non-zero
        // Distance demands a POSITIVE separation — a direct contradiction. This
        // is precisely the case the numerical (Jacobian) diagnosis cannot see
        // reliably: at coincident points the distance gradient is 0/0, so the
        // solver may report a spurious degenerate "solution". Configuration-
        // independent static detection closes that hole.
        // Angle between two lines is defined modulo π (a line has no head/tail),
        // so compare the requested angle on [0, π) against the orientation the
        // geometric constraint pins.
        use std::f64::consts::{FRAC_PI_2, PI};
        const ANGLE_TOL: f64 = 1e-9;
        match (g, d) {
            (GeometricConstraint::Coincident, DimensionalConstraint::Distance(v)) => v.abs() > 1e-9,
            // Parallel pins the inter-line angle to 0 (mod π). Any Angle that is
            // not aligned contradicts it.
            (GeometricConstraint::Parallel, DimensionalConstraint::Angle(theta)) => {
                let a = theta.rem_euclid(PI);
                a.min(PI - a) > ANGLE_TOL
            }
            // Perpendicular pins the inter-line angle to π/2. Any Angle that is
            // not a right angle contradicts it.
            (GeometricConstraint::Perpendicular, DimensionalConstraint::Angle(theta)) => {
                let a = theta.rem_euclid(PI);
                (a - FRAC_PI_2).abs() > ANGLE_TOL
            }
            // Room to grow: Concentric + a positive centre Distance, Equal + two
            // fixed-but-different dimensions, etc. — added as the harness demands.
            _ => false,
        }
    }

    /// Check if two constraints apply to the same entity tuple. Compares the
    /// entity vectors element-wise and in order; constraints whose entity
    /// lists are permutations of each other are not considered equivalent.
    fn same_entity_pairs(&self, c1: &Constraint, c2: &Constraint) -> bool {
        if c1.entities.len() != c2.entities.len() {
            return false;
        }
        c1.entities == c2.entities
    }

    /// Clear all constraints
    pub fn clear(&self) {
        self.constraints.clear();
        self.entity_constraints.clear();
        self.constraint_groups.clear();
    }

    /// Get the total number of constraints
    pub fn constraint_count(&self) -> usize {
        self.constraints.len()
    }

    /// Create symmetry constraint between two entities about a line
    pub fn add_symmetry_constraint(
        &self,
        entity1: EntityRef,
        entity2: EntityRef,
        symmetry_line: EntityRef,
        priority: ConstraintPriority,
    ) -> ConstraintId {
        let constraint = Constraint {
            id: ConstraintId::new(),
            constraint_type: ConstraintType::Geometric(GeometricConstraint::Symmetric),
            entities: vec![entity1, entity2, symmetry_line],
            priority,
            status: ConstraintStatus::Satisfied,
            name: Some("Symmetric about line".to_string()),
        };

        let constraint_id = constraint.id;
        self.add_constraint(constraint);
        constraint_id
    }

    /// Create smooth tangent continuity constraint between curves
    pub fn add_smooth_tangent_constraint(
        &self,
        curve1: EntityRef,
        curve2: EntityRef,
        connection_point: EntityRef,
        priority: ConstraintPriority,
    ) -> ConstraintId {
        let constraint = Constraint {
            id: ConstraintId::new(),
            constraint_type: ConstraintType::Geometric(GeometricConstraint::SmoothTangent),
            entities: vec![curve1, curve2, connection_point],
            priority,
            status: ConstraintStatus::Satisfied,
            name: Some("G1 continuity between curves".to_string()),
        };

        let constraint_id = constraint.id;
        self.add_constraint(constraint);
        constraint_id
    }

    /// Create curvature continuity constraint between curves
    pub fn add_curvature_continuity_constraint(
        &self,
        curve1: EntityRef,
        curve2: EntityRef,
        connection_point: EntityRef,
        priority: ConstraintPriority,
    ) -> ConstraintId {
        let constraint = Constraint {
            id: ConstraintId::new(),
            constraint_type: ConstraintType::Geometric(GeometricConstraint::CurvatureContinuity),
            entities: vec![curve1, curve2, connection_point],
            priority,
            status: ConstraintStatus::Satisfied,
            name: Some("G2 continuity between curves".to_string()),
        };

        let constraint_id = constraint.id;
        self.add_constraint(constraint);
        constraint_id
    }

    /// Create multi-tangent constraint (curve tangent to multiple entities)
    pub fn add_multi_tangent_constraint(
        &self,
        curve: EntityRef,
        tangent_entities: Vec<EntityRef>,
        priority: ConstraintPriority,
    ) -> ConstraintId {
        let mut entities = vec![curve];
        entities.extend(tangent_entities);

        let constraint = Constraint {
            id: ConstraintId::new(),
            constraint_type: ConstraintType::Geometric(GeometricConstraint::MultiTangent),
            entities,
            priority,
            status: ConstraintStatus::Satisfied,
            name: Some("Multi-tangent constraint".to_string()),
        };

        let constraint_id = constraint.id;
        self.add_constraint(constraint);
        constraint_id
    }

    /// Create area constraint for closed curves
    pub fn add_area_constraint(
        &self,
        entity: EntityRef,
        target_area: f64,
        priority: ConstraintPriority,
    ) -> ConstraintId {
        let constraint = Constraint {
            id: ConstraintId::new(),
            constraint_type: ConstraintType::Dimensional(DimensionalConstraint::Area(target_area)),
            entities: vec![entity],
            priority,
            status: ConstraintStatus::Satisfied,
            name: Some(format!("Area = {}", target_area)),
        };

        let constraint_id = constraint.id;
        self.add_constraint(constraint);
        constraint_id
    }

    /// Create perimeter constraint for closed curves
    pub fn add_perimeter_constraint(
        &self,
        entity: EntityRef,
        target_perimeter: f64,
        priority: ConstraintPriority,
    ) -> ConstraintId {
        let constraint = Constraint {
            id: ConstraintId::new(),
            constraint_type: ConstraintType::Dimensional(DimensionalConstraint::Perimeter(
                target_perimeter,
            )),
            entities: vec![entity],
            priority,
            status: ConstraintStatus::Satisfied,
            name: Some(format!("Perimeter = {}", target_perimeter)),
        };

        let constraint_id = constraint.id;
        self.add_constraint(constraint);
        constraint_id
    }

    /// Create aspect ratio constraint for rectangular entities
    pub fn add_aspect_ratio_constraint(
        &self,
        entity: EntityRef,
        aspect_ratio: f64,
        priority: ConstraintPriority,
    ) -> ConstraintId {
        let constraint = Constraint {
            id: ConstraintId::new(),
            constraint_type: ConstraintType::Dimensional(DimensionalConstraint::AspectRatio(
                aspect_ratio,
            )),
            entities: vec![entity],
            priority,
            status: ConstraintStatus::Satisfied,
            name: Some(format!("Aspect ratio = {}", aspect_ratio)),
        };

        let constraint_id = constraint.id;
        self.add_constraint(constraint);
        constraint_id
    }

    /// Create offset constraint between two entities
    pub fn add_offset_constraint(
        &self,
        entity1: EntityRef,
        entity2: EntityRef,
        offset_distance: f64,
        priority: ConstraintPriority,
    ) -> ConstraintId {
        let constraint = Constraint {
            id: ConstraintId::new(),
            constraint_type: ConstraintType::Dimensional(DimensionalConstraint::OffsetDistance(
                offset_distance,
            )),
            entities: vec![entity1, entity2],
            priority,
            status: ConstraintStatus::Satisfied,
            name: Some(format!("Offset distance = {}", offset_distance)),
        };

        let constraint_id = constraint.id;
        self.add_constraint(constraint);
        constraint_id
    }

    /// Create intersection angle constraint between two curves
    pub fn add_intersection_angle_constraint(
        &self,
        curve1: EntityRef,
        curve2: EntityRef,
        intersection_point: EntityRef,
        angle: f64,
        priority: ConstraintPriority,
    ) -> ConstraintId {
        let constraint = Constraint {
            id: ConstraintId::new(),
            constraint_type: ConstraintType::Geometric(GeometricConstraint::IntersectionAngle(
                angle,
            )),
            entities: vec![curve1, curve2, intersection_point],
            priority,
            status: ConstraintStatus::Satisfied,
            name: Some(format!("Intersection angle = {} rad", angle)),
        };

        let constraint_id = constraint.id;
        self.add_constraint(constraint);
        constraint_id
    }
}
