//! Parameter-space curves (p-curves) for edges that live on parametric
//! surfaces.
//!
//! A B-Rep edge is a 1-manifold shared by (typically) two faces. The
//! kernel has always carried the edge's *3D* curve in
//! [`CurveStore`](crate::primitives::curve::CurveStore); every operation
//! that needs the (u, v) image of that curve on one of the adjacent
//! faces must inverse-project from 3D back to surface parameter space.
//! That round-trip burns CPU and, for NURBS-NURBS intersections, loses
//! precision: the analytic (u, v) result that the intersector computed
//! gets discarded, and a numerical solver has to recover it.
//!
//! A `PCurve` stores the 2D curve directly in the parameter space of
//! the face it belongs to. Each [`Edge`](crate::primitives::edge::Edge)
//! carries up to two `PCurveId`s — one per adjacent face — populated
//! by the operations that *know* the (u, v) image at construction
//! time:
//!
//! - `imprint::imprint_curves_on_face` cuts a face with a 2D curve
//!   it already has; it can persist that curve as a `PCurve` instead
//!   of throwing it away.
//! - `boolean::compute_intersection_curve` solves surface–surface
//!   intersection and produces (u, v) on both sides; the same data
//!   feeds two `PCurve`s.
//! - Fillet rail construction projects the spine onto the adjacent
//!   face's surface; that projection is the rail's parameter-space
//!   curve.
//!
//! Consumers that walk an edge's pcurves can skip 3D-to-(u, v)
//! inverse projection entirely. The first beneficiary is
//! `tessellation::tessellate_face`: walking pcurves yields the
//! parameter-space boundary directly, which is what the constrained
//! Delaunay triangulator wants.
//!
//! ## Backwards compatibility
//!
//! `Edge::pcurves` defaults to an empty `Vec`. Consumers must check
//! and fall back to the 3D inverse-projection path when no pcurve is
//! attached. There are no implicit pcurves and no placeholder
//! `PCurveId`s — an edge that lacks a pcurve on a given face is
//! semantically distinct from an edge whose pcurve is empty.
//!
//! ## Why not reuse `NurbsCurve2d`?
//!
//! [`crate::sketch2d::spline2d::NurbsCurve2d`] is UUID-keyed and lives
//! inside the sketcher's persistence layer; it carries history /
//! constraint metadata the kernel doesn't need. `PCurve` is a flat
//! value type with a `u32` id parallel to `CurveId`, owned by
//! `BRepModel`. The two representations serve different layers and
//! intentionally do not share a backing store.

use crate::math::Point2;
use crate::primitives::curve::ParameterRange;
use crate::primitives::face::FaceId;

/// Identifier for a [`PCurve`] inside [`PCurveStore`].
///
/// Parallel to [`CurveId`](crate::primitives::curve::CurveId); the two
/// id spaces are independent.
pub type PCurveId = u32;

/// Sentinel for "no p-curve". Constructors that produce an `Edge`
/// without a known parameter-space image leave `Edge::pcurves` empty
/// rather than inserting this value.
pub const INVALID_PCURVE_ID: PCurveId = u32::MAX;

/// 2D curve kinds usable as a parameter-space image.
///
/// The two analytical forms used by the population sites today are
/// straight lines (e.g. imprinted edges on planar faces, rail
/// segments on developable strips) and rational B-splines (NURBS-on-
/// NURBS intersection curves, fillet rails projected onto curved
/// faces). Additional analytic kinds (circular arcs in (u, v),
/// composite poly-pcurves) will land in follow-up slices when the
/// producing operations need them; the variant set is not exhaustive
/// to leave room for that growth.
#[derive(Debug, Clone, PartialEq)]
pub enum PCurve2dKind {
    /// Straight segment in (u, v).
    ///
    /// Used by imprint when a planar face is cut by a chord, and by
    /// fillet/chamfer rails on planar adjacent faces. Evaluation is
    /// the standard linear interpolation `start + t * (end - start)`
    /// over `t ∈ [0, 1]`.
    Line {
        /// Parameter-space start point.
        start: Point2,
        /// Parameter-space end point.
        end: Point2,
    },

    /// Rational B-spline in (u, v).
    ///
    /// Stored in homogeneous-weight form: `control_points[i]` is the
    /// 2D control point and `weights[i]` is its rational weight.
    /// `knots` is the full open knot vector
    /// (`control_points.len() + degree + 1` entries).
    Nurbs {
        /// Polynomial degree (NOT order).
        degree: u32,
        /// Open knot vector of length `control_points.len() + degree + 1`.
        knots: Vec<f64>,
        /// 2D control polygon.
        control_points: Vec<Point2>,
        /// Rational weights, one per control point.
        weights: Vec<f64>,
    },
}

impl PCurve2dKind {
    /// Cheap validity check — used by `PCurveStore::add` so a corrupt
    /// pcurve cannot enter the store. Returns the first defect found
    /// or `Ok(())` if all invariants hold.
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            PCurve2dKind::Line { start, end } => {
                if !start.x.is_finite()
                    || !start.y.is_finite()
                    || !end.x.is_finite()
                    || !end.y.is_finite()
                {
                    return Err("Line pcurve has non-finite endpoint");
                }
                Ok(())
            }
            PCurve2dKind::Nurbs {
                degree,
                knots,
                control_points,
                weights,
            } => {
                if control_points.is_empty() {
                    return Err("NURBS pcurve has no control points");
                }
                if control_points.len() != weights.len() {
                    return Err("NURBS pcurve control-point / weight count mismatch");
                }
                let expected_knots = control_points.len() + (*degree as usize) + 1;
                if knots.len() != expected_knots {
                    return Err("NURBS pcurve knot vector length does not match degree + n + 1");
                }
                if knots.windows(2).any(|w| w[0] > w[1]) {
                    return Err("NURBS pcurve knot vector is not monotonic");
                }
                if weights.iter().any(|w| !w.is_finite() || *w <= 0.0) {
                    return Err("NURBS pcurve weight is non-positive or non-finite");
                }
                if control_points
                    .iter()
                    .any(|p| !p.x.is_finite() || !p.y.is_finite())
                {
                    return Err("NURBS pcurve control point is non-finite");
                }
                Ok(())
            }
        }
    }

    /// Evaluate the curve at a normalized parameter `t ∈ [0, 1]`.
    ///
    /// For `Line` this is straightforward linear interpolation. For
    /// `Nurbs` the input `t` is mapped onto the knot domain
    /// `[knots[degree], knots[n]]` (where `n = control_points.len()`)
    /// and the rational B-spline is evaluated by Cox-de Boor with the
    /// homogeneous-coordinate trick to preserve rational weights.
    pub fn evaluate(&self, t: f64) -> Point2 {
        match self {
            PCurve2dKind::Line { start, end } => {
                let s = t.clamp(0.0, 1.0);
                Point2::new(
                    start.x + s * (end.x - start.x),
                    start.y + s * (end.y - start.y),
                )
            }
            PCurve2dKind::Nurbs {
                degree,
                knots,
                control_points,
                weights,
            } => {
                let n = control_points.len();
                let p = *degree as usize;
                // Map t ∈ [0, 1] to the clamped knot domain
                // [knots[p], knots[n]]. Open knot vectors give this
                // span as the curve's full parameter range.
                let u_min = knots[p];
                let u_max = knots[n];
                let u = u_min + t.clamp(0.0, 1.0) * (u_max - u_min);

                // Find the knot span k such that knots[k] <= u < knots[k+1].
                // The right endpoint is handled by clamping to n - 1.
                let mut k = p;
                while k + 1 < n && u >= knots[k + 1] {
                    k += 1;
                }

                // Build the windowed homogeneous control points
                // (w_i * P_i, w_i) for indices k-p..=k. Cox-de Boor
                // operates on this window in place.
                let mut wx = vec![0.0f64; p + 1];
                let mut wy = vec![0.0f64; p + 1];
                let mut ww = vec![0.0f64; p + 1];
                for j in 0..=p {
                    let idx = k - p + j;
                    let w = weights[idx];
                    wx[j] = control_points[idx].x * w;
                    wy[j] = control_points[idx].y * w;
                    ww[j] = w;
                }

                // de Boor recursion. For r = 1..=p, blend each level.
                for r in 1..=p {
                    for j in (r..=p).rev() {
                        let i = k - p + j;
                        let denom = knots[i + p + 1 - r] - knots[i];
                        let alpha = if denom.abs() > 0.0 {
                            (u - knots[i]) / denom
                        } else {
                            0.0
                        };
                        let one_minus = 1.0 - alpha;
                        wx[j] = one_minus * wx[j - 1] + alpha * wx[j];
                        wy[j] = one_minus * wy[j - 1] + alpha * wy[j];
                        ww[j] = one_minus * ww[j - 1] + alpha * ww[j];
                    }
                }

                let w_final = ww[p];
                if w_final.abs() > 0.0 {
                    Point2::new(wx[p] / w_final, wy[p] / w_final)
                } else {
                    // Degenerate weight collapse — fall back to the
                    // first control point. validate() rejects zero
                    // weights at construction, so this is unreachable
                    // for stored pcurves, but the evaluator must not
                    // produce NaN if a caller hands in an invalid
                    // value mid-construction.
                    control_points[k - p]
                }
            }
        }
    }
}

/// A parameter-space curve attached to a specific face.
///
/// The `face` field anchors the (u, v) values to a particular
/// face's surface parameterization; the same edge can carry
/// multiple `PCurve`s — typically one for each adjacent face — and
/// each lives in its own parameter space.
#[derive(Debug, Clone)]
pub struct PCurve {
    /// Face whose (u, v) parameterization this curve lives in.
    pub face: FaceId,
    /// Curve geometry in parameter space.
    pub kind: PCurve2dKind,
    /// Parameter range along the pcurve (analogous to
    /// `Edge::param_range` for the 3D curve).
    pub parameter_range: ParameterRange,
    /// Maximum lift error: distance between the 3D point produced by
    /// the host edge's 3D curve at `t` and the 3D point produced by
    /// evaluating the surface at `pcurve.evaluate(t)`. Populated by
    /// the producing operation; downstream code may use this to
    /// decide whether to trust the pcurve or fall back to inverse
    /// projection.
    pub tolerance: f64,
}

impl PCurve {
    /// Construct a new pcurve. Does NOT call
    /// [`PCurve2dKind::validate`]; validation runs once on insertion
    /// into the store.
    #[inline]
    pub fn new(
        face: FaceId,
        kind: PCurve2dKind,
        parameter_range: ParameterRange,
        tolerance: f64,
    ) -> Self {
        Self {
            face,
            kind,
            parameter_range,
            tolerance,
        }
    }

    /// Evaluate at normalized parameter `t ∈ [0, 1]`.
    #[inline]
    pub fn evaluate(&self, t: f64) -> Point2 {
        self.kind.evaluate(t)
    }
}

/// Storage for [`PCurve`]s, parallel to
/// [`CurveStore`](crate::primitives::curve::CurveStore).
///
/// Lives on [`BRepModel`](crate::primitives::topology_builder::BRepModel)
/// alongside the 3D curve store. Ids are dense `u32` and not reused
/// after removal in the current slice — no removal API exists yet
/// because every population site is append-only.
#[derive(Debug)]
pub struct PCurveStore {
    pcurves: Vec<PCurve>,
    next_id: PCurveId,
}

impl PCurveStore {
    /// Create an empty store.
    #[inline]
    pub fn new() -> Self {
        Self {
            pcurves: Vec::new(),
            next_id: 0,
        }
    }

    /// Create an empty store with reserved capacity.
    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            pcurves: Vec::with_capacity(capacity),
            next_id: 0,
        }
    }

    /// Deep copy of this store for [`ModelSnapshot`]. The result owns
    /// its own backing `Vec`.
    pub(crate) fn deep_copy(&self) -> Self {
        Self {
            pcurves: self.pcurves.clone(),
            next_id: self.next_id,
        }
    }

    /// Insert a pcurve. Validates the underlying kind before
    /// allocation; returns the assigned id on success or an error
    /// string identifying the first defect.
    pub fn add(&mut self, pcurve: PCurve) -> Result<PCurveId, &'static str> {
        pcurve.kind.validate()?;
        if !pcurve.tolerance.is_finite() || pcurve.tolerance < 0.0 {
            return Err("PCurve tolerance must be a finite, non-negative value");
        }
        let id = self.next_id;
        self.pcurves.push(pcurve);
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or("PCurveStore id space exhausted")?;
        Ok(id)
    }

    /// Borrow a pcurve by id.
    #[inline]
    pub fn get(&self, id: PCurveId) -> Option<&PCurve> {
        self.pcurves.get(id as usize)
    }

    /// Borrow a pcurve by id, mutably.
    #[inline]
    pub fn get_mut(&mut self, id: PCurveId) -> Option<&mut PCurve> {
        self.pcurves.get_mut(id as usize)
    }

    /// Number of pcurves currently in the store.
    #[inline]
    pub fn len(&self) -> usize {
        self.pcurves.len()
    }

    /// Whether the store is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.pcurves.is_empty()
    }

    /// Iterate `(id, &pcurve)` pairs in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (PCurveId, &PCurve)> {
        self.pcurves
            .iter()
            .enumerate()
            .map(|(i, p)| (i as PCurveId, p))
    }
}

impl Default for PCurveStore {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_range() -> ParameterRange {
        ParameterRange::unit()
    }

    #[test]
    fn line_pcurve_validates() {
        let line = PCurve2dKind::Line {
            start: Point2::new(0.0, 0.0),
            end: Point2::new(1.0, 2.0),
        };
        assert!(line.validate().is_ok());
    }

    #[test]
    fn line_pcurve_rejects_nan_endpoint() {
        let line = PCurve2dKind::Line {
            start: Point2::new(0.0, f64::NAN),
            end: Point2::new(1.0, 0.0),
        };
        assert!(line.validate().is_err());
    }

    #[test]
    fn line_evaluates_at_endpoints_and_midpoint() {
        let line = PCurve2dKind::Line {
            start: Point2::new(0.0, 0.0),
            end: Point2::new(2.0, 4.0),
        };
        assert_eq!(line.evaluate(0.0), Point2::new(0.0, 0.0));
        assert_eq!(line.evaluate(1.0), Point2::new(2.0, 4.0));
        assert_eq!(line.evaluate(0.5), Point2::new(1.0, 2.0));
    }

    #[test]
    fn nurbs_pcurve_validates_open_knot_vector() {
        // Degree-2 NURBS with 4 control points => 7 knots.
        let nurbs = PCurve2dKind::Nurbs {
            degree: 2,
            knots: vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0],
            control_points: vec![
                Point2::new(0.0, 0.0),
                Point2::new(1.0, 1.0),
                Point2::new(2.0, 1.0),
                Point2::new(3.0, 0.0),
            ],
            weights: vec![1.0, 1.0, 1.0, 1.0],
        };
        assert!(nurbs.validate().is_ok());
    }

    #[test]
    fn nurbs_pcurve_rejects_mismatched_knots() {
        let nurbs = PCurve2dKind::Nurbs {
            degree: 2,
            knots: vec![0.0, 0.0, 1.0, 1.0],
            control_points: vec![
                Point2::new(0.0, 0.0),
                Point2::new(1.0, 1.0),
                Point2::new(2.0, 1.0),
                Point2::new(3.0, 0.0),
            ],
            weights: vec![1.0, 1.0, 1.0, 1.0],
        };
        assert!(nurbs.validate().is_err());
    }

    #[test]
    fn nurbs_pcurve_rejects_non_positive_weight() {
        let nurbs = PCurve2dKind::Nurbs {
            degree: 1,
            knots: vec![0.0, 0.0, 1.0, 1.0],
            control_points: vec![Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)],
            weights: vec![1.0, 0.0],
        };
        assert!(nurbs.validate().is_err());
    }

    #[test]
    fn nurbs_pcurve_rejects_non_monotonic_knots() {
        let nurbs = PCurve2dKind::Nurbs {
            degree: 1,
            knots: vec![0.0, 1.0, 0.5, 1.0],
            control_points: vec![Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)],
            weights: vec![1.0, 1.0],
        };
        assert!(nurbs.validate().is_err());
    }

    #[test]
    fn linear_nurbs_matches_line_evaluation() {
        // A degree-1 NURBS with two equal-weight control points is a
        // straight line; evaluate both forms and compare.
        let cps = [Point2::new(1.0, 2.0), Point2::new(4.0, 6.0)];
        let nurbs = PCurve2dKind::Nurbs {
            degree: 1,
            knots: vec![0.0, 0.0, 1.0, 1.0],
            control_points: cps.to_vec(),
            weights: vec![1.0, 1.0],
        };
        let line = PCurve2dKind::Line {
            start: cps[0],
            end: cps[1],
        };
        for &t in &[0.0, 0.25, 0.5, 0.75, 1.0] {
            let n = nurbs.evaluate(t);
            let l = line.evaluate(t);
            assert!((n.x - l.x).abs() < 1e-12, "x mismatch at t={}", t);
            assert!((n.y - l.y).abs() < 1e-12, "y mismatch at t={}", t);
        }
    }

    #[test]
    fn quadratic_rational_nurbs_at_endpoints_hits_control_polygon() {
        // Standard quadratic rational Bézier circle quadrant
        // (0,1) → (1,1) → (1,0) with weights {1, 1/sqrt(2), 1};
        // evaluated as an open-knot NURBS the endpoints must
        // coincide with the first / last control point.
        let nurbs = PCurve2dKind::Nurbs {
            degree: 2,
            knots: vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            control_points: vec![
                Point2::new(0.0, 1.0),
                Point2::new(1.0, 1.0),
                Point2::new(1.0, 0.0),
            ],
            weights: vec![1.0, std::f64::consts::FRAC_1_SQRT_2, 1.0],
        };
        let p0 = nurbs.evaluate(0.0);
        let p1 = nurbs.evaluate(1.0);
        assert!((p0.x - 0.0).abs() < 1e-12 && (p0.y - 1.0).abs() < 1e-12);
        assert!((p1.x - 1.0).abs() < 1e-12 && (p1.y - 0.0).abs() < 1e-12);

        // Midpoint should lie on the unit circle to within
        // floating-point precision: x² + y² ≈ 1.
        let pm = nurbs.evaluate(0.5);
        let r2 = pm.x * pm.x + pm.y * pm.y;
        assert!(
            (r2 - 1.0).abs() < 1e-12,
            "rational midpoint not on unit circle: r² = {}",
            r2
        );
    }

    #[test]
    fn store_add_assigns_dense_ids() {
        let mut store = PCurveStore::new();
        let pc = |face: FaceId| {
            PCurve::new(
                face,
                PCurve2dKind::Line {
                    start: Point2::new(0.0, 0.0),
                    end: Point2::new(1.0, 0.0),
                },
                unit_range(),
                1e-6,
            )
        };
        let a = store.add(pc(7)).expect("first add");
        let b = store.add(pc(7)).expect("second add");
        let c = store.add(pc(9)).expect("third add");
        assert_eq!(a, 0);
        assert_eq!(b, 1);
        assert_eq!(c, 2);
        assert_eq!(store.len(), 3);
    }

    #[test]
    fn store_add_rejects_invalid_kind() {
        let mut store = PCurveStore::new();
        let bad = PCurve::new(
            0,
            PCurve2dKind::Line {
                start: Point2::new(f64::INFINITY, 0.0),
                end: Point2::new(0.0, 0.0),
            },
            unit_range(),
            1e-6,
        );
        assert!(store.add(bad).is_err());
        assert!(store.is_empty());
    }

    #[test]
    fn store_add_rejects_negative_tolerance() {
        let mut store = PCurveStore::new();
        let pc = PCurve::new(
            0,
            PCurve2dKind::Line {
                start: Point2::new(0.0, 0.0),
                end: Point2::new(1.0, 0.0),
            },
            unit_range(),
            -1.0,
        );
        assert!(store.add(pc).is_err());
    }

    #[test]
    fn store_get_returns_inserted_pcurve() {
        let mut store = PCurveStore::new();
        let id = store
            .add(PCurve::new(
                42,
                PCurve2dKind::Line {
                    start: Point2::new(0.5, 0.5),
                    end: Point2::new(1.5, 2.5),
                },
                unit_range(),
                1e-9,
            ))
            .expect("add");
        let got = store.get(id).expect("retrievable");
        assert_eq!(got.face, 42);
        assert!((got.tolerance - 1e-9).abs() < 1e-18);
        match &got.kind {
            PCurve2dKind::Line { start, end } => {
                assert_eq!(*start, Point2::new(0.5, 0.5));
                assert_eq!(*end, Point2::new(1.5, 2.5));
            }
            _ => panic!("expected line variant"),
        }
    }

    #[test]
    fn store_get_returns_none_for_unknown_id() {
        let store = PCurveStore::new();
        assert!(store.get(0).is_none());
        assert!(store.get(INVALID_PCURVE_ID).is_none());
    }

    #[test]
    fn store_iter_yields_all_in_insertion_order() {
        let mut store = PCurveStore::new();
        for face in [3, 5, 7] {
            store
                .add(PCurve::new(
                    face,
                    PCurve2dKind::Line {
                        start: Point2::ZERO,
                        end: Point2::new(1.0, 0.0),
                    },
                    unit_range(),
                    1e-6,
                ))
                .expect("add");
        }
        let collected: Vec<_> = store.iter().map(|(_, p)| p.face).collect();
        assert_eq!(collected, vec![3, 5, 7]);
    }

    #[test]
    fn store_deep_copy_is_independent() {
        let mut original = PCurveStore::new();
        let _ = original
            .add(PCurve::new(
                1,
                PCurve2dKind::Line {
                    start: Point2::ZERO,
                    end: Point2::new(1.0, 1.0),
                },
                unit_range(),
                1e-6,
            ))
            .expect("seed");

        let mut copy = original.deep_copy();
        let _ = copy
            .add(PCurve::new(
                2,
                PCurve2dKind::Line {
                    start: Point2::ZERO,
                    end: Point2::new(2.0, 0.0),
                },
                unit_range(),
                1e-6,
            ))
            .expect("mutate copy");

        assert_eq!(original.len(), 1);
        assert_eq!(copy.len(), 2);
        assert_eq!(original.get(0).unwrap().face, 1);
        assert_eq!(copy.get(0).unwrap().face, 1);
        assert_eq!(copy.get(1).unwrap().face, 2);
    }

    #[test]
    fn pcurve_evaluate_delegates_to_kind() {
        let pc = PCurve::new(
            0,
            PCurve2dKind::Line {
                start: Point2::new(0.0, 0.0),
                end: Point2::new(10.0, -10.0),
            },
            unit_range(),
            1e-6,
        );
        let mid = pc.evaluate(0.5);
        assert!((mid.x - 5.0).abs() < 1e-12);
        assert!((mid.y + 5.0).abs() < 1e-12);
    }
}
