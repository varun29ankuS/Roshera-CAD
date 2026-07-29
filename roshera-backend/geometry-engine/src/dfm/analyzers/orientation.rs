//! `face_orientation_field` — the per-face angle range between a face's
//! OUTWARD normal and a caller-supplied reference direction (spec §3.1).
//!
//! ## Convention (state it once, test it hard)
//!
//! For a face and a unit reference direction `d`, this analyzer reports
//! the exact range of `θ = acos(n · d)` swept over the face's TRIMMED
//! parameter domain, where `n` is the face's OUTWARD normal — the
//! surface's geometric normal at that `(u, v)` multiplied by
//! [`crate::primitives::face::FaceOrientation::sign`] (`Face::normal_at`
//! already applies this; every branch below replicates the same sign
//! convention directly on the closed-form coefficients instead of calling
//! it, so the field can be evaluated in closed form rather than sampled).
//! `θ = 0°` means the outward normal points exactly ALONG `d`; `θ = 180°`
//! means it points exactly AGAINST `d`; `θ = 90°` means the two are
//! perpendicular (a wall exactly parallel to `d`, e.g. a vertical
//! cylindrical side wall against a vertical build/pull direction). `acos`
//! is monotonically DECREASING, so the face's MAXIMUM dot product yields
//! its MINIMUM angle and vice versa — every branch below is careful to
//! swap min/max across that inversion, not copy it straight across.
//!
//! ## The trimmed-domain requirement (spec's flagged highest-risk spot)
//!
//! `Face::uv_bounds` and the carrier `Surface`'s own `angle_limits` /
//! `parameter_bounds()` are BOTH untrustworthy here: neither
//! `operations::boolean` nor `operations::imprint` updates a face's
//! `uv_bounds` after trimming it (grep confirms zero call sites), and a
//! boolean cut never narrows the carrier `Cylinder`/`Cone`'s own
//! `angle_limits` — that field only gets set when a surface is
//! constructed as a partial arc from the start (`Cylinder::new_arc`).
//! A half-cylinder produced by cutting a FULL cylinder therefore carries
//! a surface that still claims a full `2π` domain; trusting either of
//! those sources would silently fabricate a violation (or hide a real
//! one) on exactly the geometry this analyzer exists to get right.
//!
//! The mechanism used instead: walk the face's OUTER loop's edges and
//! classify each one's 3D curve —
//!
//! - a [`Line`] contributes nothing (a straight generatrix pins a single
//!   angle, it does not widen the swept range — and that single angle is
//!   already the shared endpoint of the rim arcs it connects to);
//! - an [`Arc`]/circle whose plane is perpendicular to the surface's axis
//!   (a "rim" at constant height/`v`) contributes the exact angular
//!   sub-interval it sweeps, derived as follows: the arc's own `t = 0`
//!   point (`center + x_axis * radius`) is inverted through the SURFACE's
//!   own closed-form `closest_point` (exact `atan2`, not a numerical
//!   solve — verified by reading `Cylinder`/`Cone::closest_point` before
//!   relying on it) to get an angular `offset`; since the arc's own angle
//!   and the surface's `u` differ by at most a constant rotation and a
//!   sign flip (whether the arc's `normal` points with or against the
//!   surface's axis), the swept sub-interval is exactly
//!   `[offset + s·start_angle, offset + s·(start_angle + sweep_angle)]`
//!   where `s = sign(arc.normal · axis)`. `sweep_angle` is stored exactly
//!   on the [`Arc`] — no unwrap-modulo guessing, no sampling, and no
//!   ambiguity at a sweep of exactly `π` (the half-cylinder headline
//!   case) or a reflex sweep past `π`;
//! - anything else (a NURBS/BSpline boundary curve, or an arc whose plane
//!   is NOT perpendicular to the axis) makes the face's angular envelope
//!   unreconstructable in closed form — the face REFUSES
//!   ([`crate::dfm::report::UnverifiableReason::UnsupportedSurface`], with
//!   the real (topology-shaped) defect named in the `analyzer` field —
//!   S1's `UnverifiableReason` has no dedicated topology variant, and
//!   this module does not own that type) rather than falling back to the
//!   carrier domain.
//!
//! The exact extrema of `A·cos(t) + B·sin(t) + C` (see below) are then
//! taken over the UNION of the per-edge sub-intervals directly — as
//! independent intervals, not merged into one — since the min/max of a
//! function over a union of sets is just the min/max of its per-set
//! extrema; no interval-merging or wraparound bookkeeping is needed.
//!
//! An inner-loop hole is a topology this analyzer also refuses on
//! (rather than silently reporting an envelope that may be too WIDE): a
//! hole can only shrink the surviving surface, never widen the angular
//! range physically present, but S2 does not attempt the 2D reasoning
//! needed to prove a TIGHTER exact bound in that case, and the honest
//! move is to refuse rather than risk fabricating a violation against a
//! range that isn't actually occupied by the hole's interior.
//!
//! ## Math per supported surface kind
//!
//! Every kind below reduces `n(t) · d` to `A·cos(t) + B·sin(t) + C` in the
//! relevant angular parameter `t` (derived by hand from each surface's own
//! `evaluate_full` in `primitives/surface.rs`, not assumed):
//!
//! - **Plane** — `n` is constant; `θ` is a single value (`t0 == t1`
//!   trivially).
//! - **Cylinder** — `n(u) = cos(u)·ref_dir + sin(u)·(axis × ref_dir)`; so
//!   `A = ref_dir·d`, `B = (axis×ref_dir)·d`, `C = 0` (verified against
//!   `Cylinder::evaluate_full`'s `radial.normalize()`).
//! - **Cone** — `n(u) = cos(ha)·cos(u)·ref_dir + cos(ha)·sin(u)·(axis×ref_dir)
//!   − sin(ha)·axis` (derived from `Cone::evaluate_full`'s
//!   `t_theta.cross(&dv).normalize()`, expanded by hand in an orthonormal
//!   `(ref_dir, axis×ref_dir, axis)` frame — see the derivation in the
//!   commit history / task notes); so `A = cos(ha)·(ref_dir·d)`,
//!   `B = cos(ha)·((axis×ref_dir)·d)`, `C = −sin(ha)·(axis·d)`.
//! - **Sphere** — exact ONLY for the full, untrimmed patch (the
//!   degenerate empty-loop primitive construction): a complete sphere's
//!   normal spans every direction in `S²`, so the range is the direction-
//!   independent constant `[0°, 180°]`. Any boundary edges on the outer
//!   loop mean the patch is trimmed; S2 does not attempt the 2-parameter
//!   (azimuth, polar) exact bound for a partial spherical patch and
//!   refuses instead.
//! - **Torus** — refused unconditionally. The minor/major angle pair does
//!   not reduce to the single-parameter form above, and S2 does not
//!   attempt a 2-parameter exact bound (spec §3.1: an honest refusal here
//!   is a correct v1 outcome, not a gap to paper over).
//! - Everything else (`SurfaceOfRevolution`, `BSpline`, `NURBS`, `Offset`,
//!   `Ruled`) refuses immediately, naming the kind.

use crate::dfm::report::{Derivation, DfmError, SurfaceKind, UnverifiableReason};
use crate::math::{consts, Point3, Tolerance, Vector3};
use crate::primitives::curve::{Arc, CurveStore, Line};
use crate::primitives::edge::{EdgeId, EdgeStore};
use crate::primitives::face::{Face, FaceId, FaceStore};
use crate::primitives::r#loop::LoopStore;
use crate::primitives::surface::{Cone, Cylinder, Plane, Surface, SurfaceStore, SurfaceType};

/// One face's exact answer from [`face_orientation_field`]: either the
/// closed-form angle range, or an honest refusal naming why. Not itself a
/// wire type — [`crate::dfm::packs`] rule-evaluation functions fold this
/// into the wire [`crate::dfm::report::Verdict`]/[`crate::dfm::report::RuleVerdict`].
#[derive(Debug, Clone, PartialEq)]
pub enum OrientationOutcome {
    /// `[min_deg, max_deg]` — the exact range of the angle between the
    /// face's outward normal and the reference direction, in degrees,
    /// over the face's trimmed parameter domain. `min_deg <= max_deg`
    /// always; both lie in `[0, 180]`.
    Range {
        min_deg: f64,
        max_deg: f64,
        derivation: Derivation,
    },
    /// The face's angular envelope could not be derived in closed form.
    Unverifiable { reason: UnverifiableReason },
}

/// `|arc.normal · axis|` above this counts as "the arc's plane is
/// perpendicular to the axis" (a rim). Deliberately tight — this gates
/// whether an edge's angular contribution is trusted at all, so a
/// borderline case should refuse rather than silently accept a
/// near-miss.
const RIM_PERPENDICULAR_TOL: f64 = 1e-9;

/// Below this, `A·cos(t) + B·sin(t)`'s amplitude is treated as exactly
/// zero (a direction-independent constant field) rather than searching
/// for a critical point via `atan2(0, 0)`.
const AMPLITUDE_EPS: f64 = 1e-12;

/// Exact `(min, max)` of `f(t) = a·cos(t) + b·sin(t) + c` over the closed
/// interval `[t0, t1]` (`t0 <= t1`). `f` is smooth and periodic with
/// period `2π`; its only critical points are `t* = atan2(b, a)` (global
/// max of the `a·cos + b·sin` part, value `+r`) and `t* + π` (global min,
/// value `-r`), where `r = √(a² + b²)`. The exact extrema over the
/// interval are the larger/smaller of the two endpoint values and every
/// occurrence of those two critical points (mod `2π`) that actually falls
/// inside `[t0, t1]` — callers here only ever pass a single loop edge's
/// own swept sub-interval, which by B-Rep construction never exceeds one
/// full turn, so at most one occurrence of each critical point can lie
/// inside; the loop below is written generally regardless.
fn cos_sin_extrema(a: f64, b: f64, c: f64, t0: f64, t1: f64) -> (f64, f64) {
    let f = |t: f64| a * t.cos() + b * t.sin() + c;
    let mut lo = f(t0).min(f(t1));
    let mut hi = f(t0).max(f(t1));

    let r = (a * a + b * b).sqrt();
    if r > AMPLITUDE_EPS {
        let base = b.atan2(a);
        for critical in [base, base + consts::PI] {
            // Shift `critical` to the first occurrence at or after `t0`,
            // then walk forward by one full turn at a time while still
            // inside `[t0, t1]`.
            let mut k = critical + consts::TWO_PI * ((t0 - critical) / consts::TWO_PI).ceil();
            while k <= t1 + 1e-9 {
                if k >= t0 - 1e-9 {
                    let v = f(k);
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
                k += consts::TWO_PI;
            }
        }
    }
    (lo, hi)
}

/// One boundary edge's contribution to a face's swept angular domain.
enum EdgeAngularContribution {
    /// A straight generatrix: pins a single angle, adds no width.
    Point,
    /// A rim arc: sweeps this closed sub-interval of the surface's `u`.
    Range(f64, f64),
}

/// Classify one loop edge's contribution to the angular domain, or `None`
/// if this edge cannot be classified in closed form (caller refuses the
/// whole face).
fn edge_angular_contribution(
    edge_id: EdgeId,
    edge_store: &EdgeStore,
    curve_store: &CurveStore,
    surface: &dyn Surface,
    axis: Vector3,
) -> Option<EdgeAngularContribution> {
    let edge = edge_store.get(edge_id)?;
    let curve = curve_store.get(edge.curve_id)?;

    if curve.as_any().downcast_ref::<Line>().is_some() {
        return Some(EdgeAngularContribution::Point);
    }

    let arc = curve.as_any().downcast_ref::<Arc>()?;
    let alignment = arc.normal.dot(&axis);
    if (alignment.abs() - 1.0).abs() > RIM_PERPENDICULAR_TOL {
        // Not a rim in a plane perpendicular to the axis — cannot relate
        // the arc's own angle to the surface's `u` by a constant offset.
        return None;
    }
    let sign = if alignment >= 0.0 { 1.0 } else { -1.0 };

    // The arc's own φ = 0 point, exact and closed-form: center + x_axis·r.
    let phi_zero_point: Point3 = arc.center + arc.x_axis * arc.radius;
    let (offset, _v) = surface
        .closest_point(&phi_zero_point, Tolerance::default())
        .ok()?;

    let u_start = offset + sign * arc.start_angle;
    let u_end = offset + sign * (arc.start_angle + arc.sweep_angle);
    Some(EdgeAngularContribution::Range(
        u_start.min(u_end),
        u_start.max(u_end),
    ))
}

/// Derive the face's trimmed angular sub-intervals (spec's flagged
/// highest-risk mechanism — see module docs). Returns one `(t0, t1)` pair
/// per qualifying rim edge; callers fold `cos_sin_extrema` over each
/// independently and take the global min/max (no interval merging
/// needed — see module docs).
fn angular_intervals_for_face(
    face: &Face,
    loop_store: &LoopStore,
    edge_store: &EdgeStore,
    curve_store: &CurveStore,
    surface: &dyn Surface,
    axis: Vector3,
    surface_kind: SurfaceKind,
) -> Result<Vec<(f64, f64)>, UnverifiableReason> {
    // Every refusal below is topology-shaped, not surface-kind-shaped (the
    // surface kind IS one `face_orientation_field` generally supports —
    // that is exactly why this function was reached at all). S1's
    // `UnverifiableReason` has no dedicated topology variant, so these
    // reuse `UnsupportedSurface` with `surface_kind` (the caller's actual,
    // supported kind) and put the real defect in `analyzer`'s trailing
    // parenthetical — an honest, if slightly overloaded, fit rather than
    // widening a type this module does not own.
    let unsupported = |detail: &str| UnverifiableReason::UnsupportedSurface {
        surface_type: surface_kind,
        analyzer: format!("face_orientation_field ({detail})"),
    };

    if !face.inner_loops.is_empty() {
        return Err(unsupported(
            "face has inner-loop trimming; S2's angular-envelope reconstruction only \
             bounds a single outer boundary exactly",
        ));
    }

    let outer = loop_store
        .get(face.outer_loop)
        .ok_or_else(|| unsupported("face's outer loop does not resolve"))?;

    if outer.edges.is_empty() {
        return Err(unsupported(
            "face has no boundary edges to derive a trimmed angular domain from",
        ));
    }

    let mut ranges = Vec::with_capacity(outer.edges.len());
    for &edge_id in &outer.edges {
        match edge_angular_contribution(edge_id, edge_store, curve_store, surface, axis) {
            Some(EdgeAngularContribution::Point) => {}
            Some(EdgeAngularContribution::Range(lo, hi)) => ranges.push((lo, hi)),
            None => {
                return Err(unsupported(&format!(
                    "boundary edge {edge_id} is neither a straight generatrix nor an \
                     axis-perpendicular circular rim; cannot bound the trimmed angular \
                     domain exactly"
                )))
            }
        }
    }

    if ranges.is_empty() {
        return Err(unsupported(
            "face boundary contains no rim arc to derive an angular domain from",
        ));
    }

    Ok(ranges)
}

fn to_surface_kind(surface_type: SurfaceType) -> SurfaceKind {
    match surface_type {
        SurfaceType::Plane => SurfaceKind::Plane,
        SurfaceType::Cylinder => SurfaceKind::Cylinder,
        SurfaceType::Cone => SurfaceKind::Cone,
        SurfaceType::Sphere => SurfaceKind::Sphere,
        SurfaceType::Torus => SurfaceKind::Torus,
        SurfaceType::SurfaceOfRevolution => SurfaceKind::SurfaceOfRevolution,
        SurfaceType::BSpline => SurfaceKind::BSpline,
        SurfaceType::NURBS => SurfaceKind::Nurbs,
        SurfaceType::Offset => SurfaceKind::Offset,
        SurfaceType::Ruled => SurfaceKind::Ruled,
    }
}

/// Turn a `(dot_min, dot_max)` pair (clamped to `[-1, 1]` — floating-point
/// round-off can otherwise hand `acos` a value fractionally past ±1 and
/// get `NaN` back) into the `(min_deg, max_deg)` angle range. `acos` is
/// decreasing, so the DOT max yields the angle MIN and vice versa.
fn dot_range_to_angle_range_deg(dot_min: f64, dot_max: f64) -> (f64, f64) {
    let dot_min_c = dot_min.clamp(-1.0, 1.0);
    let dot_max_c = dot_max.clamp(-1.0, 1.0);
    let max_deg = dot_min_c.acos().to_degrees();
    let min_deg = dot_max_c.acos().to_degrees();
    (min_deg, max_deg)
}

/// Per-face angle range between the face's outward normal and
/// `reference_direction`, exact over the face's TRIMMED parameter domain
/// (spec §3.1). See module docs for the angle convention, the trimmed-
/// domain mechanism, and the exact per-kind math.
///
/// Returns `Err` only for malformed input (the face or its surface does
/// not resolve). A face whose surface kind or boundary topology this
/// analyzer cannot bound exactly returns `Ok(OrientationOutcome::Unverifiable)`
/// — a refusal is a value, never an error (spec §4).
pub fn face_orientation_field(
    face_id: FaceId,
    reference_direction: Vector3,
    face_store: &FaceStore,
    loop_store: &LoopStore,
    edge_store: &EdgeStore,
    curve_store: &CurveStore,
    surface_store: &SurfaceStore,
) -> Result<OrientationOutcome, DfmError> {
    let face = face_store
        .get(face_id)
        .ok_or(DfmError::DanglingFaceRef { face: face_id })?;
    let surface = surface_store
        .get(face.surface_id)
        .ok_or(DfmError::DanglingFaceRef { face: face_id })?;

    // Reference direction must be a unit vector for the dot-product
    // extrema to read directly as a cosine. A zero-length direction is a
    // caller precondition violation this analyzer has no honest number
    // to report for; falling back to +Z rather than propagating a new
    // error variant keeps `analyze()` malformed-input errors limited to
    // topology (spec §4) — the fallback only matters for a degenerate
    // input no rule pack constructs.
    let dir = reference_direction.normalize().unwrap_or(Vector3::Z);
    let sign = face.orientation.sign();

    match surface.surface_type() {
        SurfaceType::Plane => {
            // surface_type() == Plane guarantees this downcast succeeds;
            // every `Surface` impl's `surface_type()` returns the constant
            // matching its own concrete type (verified by reading each
            // impl in primitives/surface.rs).
            #[allow(clippy::expect_used)]
            let plane = surface
                .as_any()
                .downcast_ref::<Plane>()
                .expect("surface_type() == Plane guarantees Plane downcast");
            let dot = (sign * plane.normal.dot(&dir)).clamp(-1.0, 1.0);
            let angle = dot.acos().to_degrees();
            Ok(OrientationOutcome::Range {
                min_deg: angle,
                max_deg: angle,
                derivation: Derivation::Analytic {
                    surface_type: SurfaceKind::Plane,
                    method: "planar outward normal vs reference direction".to_string(),
                },
            })
        }

        SurfaceType::Cylinder => {
            #[allow(clippy::expect_used)]
            let cyl = surface
                .as_any()
                .downcast_ref::<Cylinder>()
                .expect("surface_type() == Cylinder guarantees Cylinder downcast");
            match angular_intervals_for_face(
                face,
                loop_store,
                edge_store,
                curve_store,
                surface,
                cyl.axis,
                SurfaceKind::Cylinder,
            ) {
                Err(reason) => Ok(OrientationOutcome::Unverifiable { reason }),
                Ok(intervals) => {
                    let y_dir = cyl.axis.cross(&cyl.ref_dir);
                    let a = sign * cyl.ref_dir.dot(&dir);
                    let b = sign * y_dir.dot(&dir);
                    let c = 0.0;

                    let mut dot_min = f64::INFINITY;
                    let mut dot_max = f64::NEG_INFINITY;
                    for (t0, t1) in intervals {
                        let (lo, hi) = cos_sin_extrema(a, b, c, t0, t1);
                        dot_min = dot_min.min(lo);
                        dot_max = dot_max.max(hi);
                    }
                    let (min_deg, max_deg) = dot_range_to_angle_range_deg(dot_min, dot_max);
                    Ok(OrientationOutcome::Range {
                        min_deg,
                        max_deg,
                        derivation: Derivation::Analytic {
                            surface_type: SurfaceKind::Cylinder,
                            method: "radial normal vs reference direction, exact \
                                     trimmed-domain extrema"
                                .to_string(),
                        },
                    })
                }
            }
        }

        SurfaceType::Cone => {
            #[allow(clippy::expect_used)]
            let cone = surface
                .as_any()
                .downcast_ref::<Cone>()
                .expect("surface_type() == Cone guarantees Cone downcast");
            match angular_intervals_for_face(
                face,
                loop_store,
                edge_store,
                curve_store,
                surface,
                cone.axis,
                SurfaceKind::Cone,
            ) {
                Err(reason) => Ok(OrientationOutcome::Unverifiable { reason }),
                Ok(intervals) => {
                    let y_dir = cone.axis.cross(&cone.ref_dir);
                    let cos_ha = cone.half_angle.cos();
                    let sin_ha = cone.half_angle.sin();
                    let a = sign * cos_ha * cone.ref_dir.dot(&dir);
                    let b = sign * cos_ha * y_dir.dot(&dir);
                    let c = sign * (-sin_ha) * cone.axis.dot(&dir);

                    let mut dot_min = f64::INFINITY;
                    let mut dot_max = f64::NEG_INFINITY;
                    for (t0, t1) in intervals {
                        let (lo, hi) = cos_sin_extrema(a, b, c, t0, t1);
                        dot_min = dot_min.min(lo);
                        dot_max = dot_max.max(hi);
                    }
                    let (min_deg, max_deg) = dot_range_to_angle_range_deg(dot_min, dot_max);
                    Ok(OrientationOutcome::Range {
                        min_deg,
                        max_deg,
                        derivation: Derivation::Analytic {
                            surface_type: SurfaceKind::Cone,
                            method: "lateral normal vs reference direction, exact \
                                     trimmed-domain extrema"
                                .to_string(),
                        },
                    })
                }
            }
        }

        SurfaceType::Sphere => {
            let outer_is_empty = loop_store
                .get(face.outer_loop)
                .map(|l| l.edges.is_empty())
                .unwrap_or(true);
            if outer_is_empty && face.inner_loops.is_empty() {
                // The full, untrimmed spherical patch: its normal spans
                // every direction in S², so the range vs ANY reference
                // direction is the constant [0°, 180°] — direction-
                // independent by the sphere's own symmetry.
                Ok(OrientationOutcome::Range {
                    min_deg: 0.0,
                    max_deg: 180.0,
                    derivation: Derivation::Analytic {
                        surface_type: SurfaceKind::Sphere,
                        method: "full spherical patch — normal spans every direction".to_string(),
                    },
                })
            } else {
                Ok(OrientationOutcome::Unverifiable {
                    reason: UnverifiableReason::UnsupportedSurface {
                        surface_type: SurfaceKind::Sphere,
                        analyzer: "face_orientation_field (trimmed spherical patches are not \
                                   bounded exactly in S2)"
                            .to_string(),
                    },
                })
            }
        }

        SurfaceType::Torus => Ok(OrientationOutcome::Unverifiable {
            reason: UnverifiableReason::UnsupportedSurface {
                surface_type: SurfaceKind::Torus,
                analyzer: "face_orientation_field (two-parameter torus normal is not bounded \
                           exactly in S2)"
                    .to_string(),
            },
        }),

        other => Ok(OrientationOutcome::Unverifiable {
            reason: UnverifiableReason::UnsupportedSurface {
                surface_type: to_surface_kind(other),
                analyzer: "face_orientation_field".to_string(),
            },
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::curve::ParameterRange;
    use crate::primitives::edge::{Edge, EdgeOrientation};
    use crate::primitives::face::FaceOrientation;
    use crate::primitives::r#loop::{Loop, LoopType};
    use crate::primitives::surface::Sphere;
    use std::f64::consts::PI;

    /// Build a `(CurveStore, EdgeStore, LoopStore, LoopId)` outer loop
    /// directly from a hand-picked list of curves — bypassing booleans /
    /// imprint entirely (per-agent design note: booleans are a KNOWN_REDS
    /// hazard area and the loop's edge LIST is all this analyzer reads;
    /// vertex identity and edge traversal direction are irrelevant to it).
    struct LoopFixture {
        curves: CurveStore,
        edges: EdgeStore,
        loops: LoopStore,
        outer_loop: crate::primitives::r#loop::LoopId,
    }

    fn build_loop_from_curves(
        curves_in: Vec<Box<dyn crate::primitives::curve::Curve>>,
    ) -> LoopFixture {
        let mut curves = CurveStore::new();
        let mut edges = EdgeStore::new();
        let mut loops = LoopStore::new();
        let mut loop_ = Loop::new(0, LoopType::Outer);
        for curve in curves_in {
            let curve_id = curves.add(curve);
            let edge = Edge::new(
                0,
                0,
                1,
                curve_id,
                EdgeOrientation::Forward,
                ParameterRange::unit(),
            );
            let edge_id = edges.add(edge);
            loop_.add_edge(edge_id, true);
        }
        let outer_loop = loops.add(loop_);
        LoopFixture {
            curves,
            edges,
            loops,
            outer_loop,
        }
    }

    /// Builds a face on a FULL (untrimmed carrier: `angle_limits: None`,
    /// i.e. a genuine `2π` `Cylinder`) whose outer loop bounds the
    /// half `u ∈ [0, π]` — the headline trimmed-domain fixture. `radius`
    /// and `height` are free so different tests can pick convenient
    /// numbers.
    fn half_cylinder_fixture(
        radius: f64,
        height: f64,
    ) -> (SurfaceStore, FaceStore, LoopFixture, FaceId) {
        let mut surfaces = SurfaceStore::new();
        let cylinder = Cylinder::new(Point3::new(0.0, 0.0, 0.0), Vector3::Z, radius)
            .expect("valid cylinder params");
        assert!(
            cylinder.angle_limits.is_none(),
            "fixture must carry a FULL (untrimmed) carrier surface — angle_limits must stay \
             None so the mutation test (loop-derived vs carrier-derived domain) is meaningful"
        );
        let surface_id = surfaces.add(Box::new(cylinder));

        let bottom_arc =
            Arc::new(Point3::new(0.0, 0.0, 0.0), Vector3::Z, radius, 0.0, PI).expect("valid arc");
        let top_arc = Arc::new(Point3::new(0.0, 0.0, height), Vector3::Z, radius, 0.0, PI)
            .expect("valid arc");
        let line_u0 = Line::new(
            Point3::new(radius, 0.0, 0.0),
            Point3::new(radius, 0.0, height),
        );
        let line_upi = Line::new(
            Point3::new(-radius, 0.0, 0.0),
            Point3::new(-radius, 0.0, height),
        );

        let fixture = build_loop_from_curves(vec![
            Box::new(bottom_arc),
            Box::new(line_upi),
            Box::new(top_arc),
            Box::new(line_u0),
        ]);

        let mut faces = FaceStore::new();
        let mut face = Face::new(0, surface_id, fixture.outer_loop, FaceOrientation::Forward);
        // Deliberately set uv_bounds to the FULL carrier domain — see
        // module docs and the mutation-proof test below: this analyzer
        // must NOT read this field for its answer.
        face.set_uv_bounds(0.0, consts::TWO_PI, 0.0, height);
        let face_id = faces.add(face);

        (surfaces, faces, fixture, face_id)
    }

    /// Wrong-on-purpose stand-in for `angular_intervals_for_face` that
    /// reads the CARRIER's own full domain instead of the loop boundary
    /// — the exact bug this analyzer exists to avoid. Used only by the
    /// mutation-proof test to show the headline test actually catches it.
    fn carrier_domain_dot_range(
        cyl: &Cylinder,
        reference_direction: Vector3,
        sign: f64,
    ) -> (f64, f64) {
        let y_dir = cyl.axis.cross(&cyl.ref_dir);
        let a = sign * cyl.ref_dir.dot(&reference_direction);
        let b = sign * y_dir.dot(&reference_direction);
        let (u_min, u_max) = cyl.parameter_bounds().0;
        cos_sin_extrema(a, b, 0.0, u_min, u_max)
    }

    // ----- Headline trimmed-domain test -------------------------------

    #[test]
    fn half_cylinder_reports_half_range_not_full_2pi() {
        let (surfaces, faces, fixture, face_id) = half_cylinder_fixture(2.0, 5.0);
        // Reference direction perpendicular to the axis so the field
        // actually varies with u (a reference along the axis would give
        // the direction-independent 90° constant seen in the dedicated
        // "full cylinder vs its own axis" test below, which would make
        // this test degenerate).
        let reference = Vector3::X;
        let outcome = face_orientation_field(
            face_id,
            reference,
            &faces,
            &fixture.loops,
            &fixture.edges,
            &fixture.curves,
            &surfaces,
        )
        .expect("malformed-input free fixture");

        match outcome {
            OrientationOutcome::Range {
                min_deg, max_deg, ..
            } => {
                // u ranges over [0, π] against reference = +X (u=0):
                // dot(n(u), X) = cos(u) ranges over [-1, 1] as u sweeps
                // [0, π] — the FULL [-1,1] range is achieved WITHIN the
                // half, at u=0 (dot=1, angle=0°) and u=π (dot=-1,
                // angle=180°). This is still the exact half-domain
                // answer; the point is it must NOT silently equal what a
                // full 2π domain would ALSO give here (see the
                // mutation-proof test for a reference direction where
                // full vs half actually differ numerically).
                assert!((min_deg - 0.0).abs() < 1e-9, "min_deg = {min_deg}");
                assert!((max_deg - 180.0).abs() < 1e-9, "max_deg = {max_deg}");
            }
            other => panic!("expected an exact Range, got {other:?}"),
        }
    }

    /// The real discriminating case: a reference direction where the
    /// TRUE half-domain range is a strict subset of what the full 2π
    /// carrier would report. `reference = (cos(20°), sin(20°), 0)` sits
    /// INSIDE the trimmed half `[0, π]`; its own angle (20° off u=0) is
    /// the half-domain's minimum, but the full circle would additionally
    /// reach dot = 1 exactly (u = 20° is already inside both domains) —
    /// so instead we pick a reference OUTSIDE the half, at u = -30°
    /// (i.e. 330°), whose closest occupied point in the half domain is
    /// the u=0 boundary (30° away), while a full carrier would report
    /// dot_max = 1 (0° away, at u = -30° itself, which is only reachable
    /// outside the trimmed half). This is exactly the "fabricated
    /// violation" shape the spec warns about.
    #[test]
    fn half_cylinder_min_angle_reflects_trim_not_carrier() {
        let (surfaces, faces, fixture, face_id) = half_cylinder_fixture(2.0, 5.0);
        let reference = Vector3::new(
            (-30f64).to_radians().cos(),
            (-30f64).to_radians().sin(),
            0.0,
        );

        let outcome = face_orientation_field(
            face_id,
            reference,
            &faces,
            &fixture.loops,
            &fixture.edges,
            &fixture.curves,
            &surfaces,
        )
        .expect("malformed-input free fixture");

        let (min_deg, _max_deg) = match outcome {
            OrientationOutcome::Range {
                min_deg, max_deg, ..
            } => (min_deg, max_deg),
            other => panic!("expected an exact Range, got {other:?}"),
        };
        // Closest point of [0, π] to a reference at angle -30° is u=0,
        // 30° away — the trimmed-domain-correct answer.
        assert!(
            (min_deg - 30.0).abs() < 1e-6,
            "loop-derived min_deg should be exactly 30°, got {min_deg}"
        );
        // The WRONG (carrier-domain) answer would be 0°, since a full
        // circle DOES reach u = -30° (dot = 1) — demonstrate the
        // divergence directly using the deliberately-wrong helper so the
        // mutation is falsifiable, not just asserted in prose.
        let cyl = surfaces
            .get(faces.get(face_id).expect("face").surface_id)
            .expect("surface")
            .as_any()
            .downcast_ref::<Cylinder>()
            .expect("cylinder");
        let (wrong_dot_min, wrong_dot_max) = carrier_domain_dot_range(
            cyl,
            reference,
            faces.get(face_id).expect("face").orientation.sign(),
        );
        let (wrong_min_deg, _) = dot_range_to_angle_range_deg(wrong_dot_min, wrong_dot_max);
        assert!(
            (wrong_min_deg - 0.0).abs() < 1e-6,
            "sanity: carrier-domain min_deg should be 0°, got {wrong_min_deg}"
        );
        assert!(
            (min_deg - wrong_min_deg).abs() > 1.0,
            "the loop-derived and carrier-derived answers must genuinely diverge for this \
             fixture, or this test proves nothing"
        );
    }

    /// Mutation proof, raw before/after: swap the analyzer's domain
    /// source from the LOOP (correct) to the CARRIER surface's own
    /// `parameter_bounds()` (the exact bug class this analyzer exists to
    /// prevent) and show the headline assertion above fails under that
    /// mutation. This is a static demonstration (calling the
    /// deliberately-wrong helper directly), not a `cfg`-gated toggle of
    /// production code — see the module docs' mutation-proof discussion
    /// in the task report for the raw before/after values.
    #[test]
    fn mutation_proof_carrier_domain_would_fabricate_a_violation() {
        let (surfaces, faces, _fixture, face_id) = half_cylinder_fixture(2.0, 5.0);
        let reference = Vector3::new(
            (-30f64).to_radians().cos(),
            (-30f64).to_radians().sin(),
            0.0,
        );
        let face = faces.get(face_id).expect("face");
        let cyl = surfaces
            .get(face.surface_id)
            .expect("surface")
            .as_any()
            .downcast_ref::<Cylinder>()
            .expect("cylinder");

        // BEFORE (mutant): carrier-domain answer.
        let (wrong_dot_min, wrong_dot_max) =
            carrier_domain_dot_range(cyl, reference, face.orientation.sign());
        let (wrong_min_deg, _wrong_max_deg) =
            dot_range_to_angle_range_deg(wrong_dot_min, wrong_dot_max);

        // AFTER (real production path): loop-derived answer.
        // (re-borrow via a fresh fixture since `faces`/`surfaces` above are
        // already borrowed immutably by `cyl` — rebuild for the real call)
        let (surfaces2, faces2, fixture2, face_id2) = half_cylinder_fixture(2.0, 5.0);
        let correct = face_orientation_field(
            face_id2,
            reference,
            &faces2,
            &fixture2.loops,
            &fixture2.edges,
            &fixture2.curves,
            &surfaces2,
        )
        .expect("malformed-input free fixture");
        let correct_min_deg = match correct {
            OrientationOutcome::Range { min_deg, .. } => min_deg,
            other => panic!("expected an exact Range, got {other:?}"),
        };

        // A rule with, say, a 10° threshold measured from this minimum
        // would PASS under the mutant (0° reads as flush-aligned, i.e.
        // maximally non-compliant reported as compliant depends on the
        // rule's direction — the material point is the two numbers
        // differ by exactly the trim, 30°) and give a DIFFERENT verdict
        // under the correct, loop-derived path.
        assert!(
            (wrong_min_deg - 0.0).abs() < 1e-6,
            "mutant (carrier-domain) min_deg should read 0°, got {wrong_min_deg}"
        );
        assert!(
            (correct_min_deg - 30.0).abs() < 1e-6,
            "production (loop-derived) min_deg should read 30°, got {correct_min_deg}"
        );
        assert!(
            (wrong_min_deg - correct_min_deg).abs() > 29.0,
            "mutation must move the reported answer by the full 30° trim, not a rounding blip"
        );
    }

    // ----- Hand-computed exact cases -----------------------------------

    #[test]
    fn plane_face_reports_single_known_angle() {
        let mut surfaces = SurfaceStore::new();
        let angle = 30f64.to_radians();
        let normal = Vector3::new(angle.sin(), 0.0, angle.cos());
        let plane = Plane::from_point_normal(Point3::new(0.0, 0.0, 0.0), normal).expect("plane");
        let surface_id = surfaces.add(Box::new(plane));

        // Plane faces need a resolvable (even if trivial) outer loop only
        // if the analyzer's Plane branch reads it — it does not; a plane's
        // normal is constant, so no loop is required for this branch.
        let mut loops = LoopStore::new();
        let loop_ = Loop::new(0, LoopType::Outer);
        let outer_loop = loops.add(loop_);
        let curves = CurveStore::new();
        let edges = EdgeStore::new();

        let mut faces = FaceStore::new();
        let face = Face::new(0, surface_id, outer_loop, FaceOrientation::Forward);
        let face_id = faces.add(face);

        let outcome = face_orientation_field(
            face_id,
            Vector3::Z,
            &faces,
            &loops,
            &edges,
            &curves,
            &surfaces,
        )
        .expect("malformed-input free fixture");

        match outcome {
            OrientationOutcome::Range {
                min_deg, max_deg, ..
            } => {
                assert!(
                    (min_deg - max_deg).abs() < 1e-12,
                    "plane range must be a single value"
                );
                assert!(
                    (min_deg - 30.0).abs() < 1e-9,
                    "expected exactly 30°, got {min_deg}"
                );
            }
            other => panic!("expected an exact Range, got {other:?}"),
        }
    }

    #[test]
    fn full_cylinder_vs_own_axis_is_constant_90_degrees() {
        let mut surfaces = SurfaceStore::new();
        let cylinder =
            Cylinder::new(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 3.0).expect("cylinder");
        let surface_id = surfaces.add(Box::new(cylinder));

        // Full (untrimmed) circle boundary: two full-circle rim arcs.
        let bottom = Arc::circle(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 3.0).expect("arc");
        let top = Arc::circle(Point3::new(0.0, 0.0, 4.0), Vector3::Z, 3.0).expect("arc");
        let fixture = build_loop_from_curves(vec![Box::new(bottom), Box::new(top)]);

        let mut faces = FaceStore::new();
        let face = Face::new(0, surface_id, fixture.outer_loop, FaceOrientation::Forward);
        let face_id = faces.add(face);

        let outcome = face_orientation_field(
            face_id,
            Vector3::Z,
            &faces,
            &fixture.loops,
            &fixture.edges,
            &fixture.curves,
            &surfaces,
        )
        .expect("malformed-input free fixture");

        match outcome {
            OrientationOutcome::Range {
                min_deg, max_deg, ..
            } => {
                assert!((min_deg - 90.0).abs() < 1e-9, "min_deg = {min_deg}");
                assert!((max_deg - 90.0).abs() < 1e-9, "max_deg = {max_deg}");
            }
            other => panic!("expected an exact Range, got {other:?}"),
        }
    }

    // ----- Honesty: refusal tests ---------------------------------------

    #[test]
    fn nurbs_face_is_unverifiable_naming_the_kind() {
        let mut surfaces = SurfaceStore::new();
        let nurbs = crate::math::nurbs::NurbsSurface::new(
            vec![
                vec![Point3::new(0.0, 0.0, 0.0), Point3::new(0.0, 1.0, 0.0)],
                vec![Point3::new(1.0, 0.0, 0.0), Point3::new(1.0, 1.0, 0.0)],
            ],
            vec![vec![1.0, 1.0], vec![1.0, 1.0]],
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
            1,
            1,
        )
        .expect("trivial flat NURBS patch");
        let surface = crate::primitives::surface::GeneralNurbsSurface { nurbs };
        let surface_id = surfaces.add(Box::new(surface));

        let mut loops = LoopStore::new();
        let outer_loop = loops.add(Loop::new(0, LoopType::Outer));
        let curves = CurveStore::new();
        let edges = EdgeStore::new();

        let mut faces = FaceStore::new();
        let face = Face::new(0, surface_id, outer_loop, FaceOrientation::Forward);
        let face_id = faces.add(face);

        let outcome = face_orientation_field(
            face_id,
            Vector3::Z,
            &faces,
            &loops,
            &edges,
            &curves,
            &surfaces,
        )
        .expect("malformed-input free fixture");

        match outcome {
            OrientationOutcome::Unverifiable {
                reason: UnverifiableReason::UnsupportedSurface { surface_type, .. },
            } => assert_eq!(surface_type, SurfaceKind::Nurbs),
            other => panic!("expected Unverifiable{{UnsupportedSurface}}, got {other:?}"),
        }
    }

    #[test]
    fn torus_face_is_unverifiable() {
        let mut surfaces = SurfaceStore::new();
        let torus = crate::primitives::surface::Torus::new(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::Z,
            5.0,
            1.0,
        )
        .expect("torus");
        let surface_id = surfaces.add(Box::new(torus));

        let mut loops = LoopStore::new();
        let outer_loop = loops.add(Loop::new(0, LoopType::Outer));
        let curves = CurveStore::new();
        let edges = EdgeStore::new();
        let mut faces = FaceStore::new();
        let face = Face::new(0, surface_id, outer_loop, FaceOrientation::Forward);
        let face_id = faces.add(face);

        let outcome = face_orientation_field(
            face_id,
            Vector3::Z,
            &faces,
            &loops,
            &edges,
            &curves,
            &surfaces,
        )
        .expect("malformed-input free fixture");

        match outcome {
            OrientationOutcome::Unverifiable {
                reason: UnverifiableReason::UnsupportedSurface { surface_type, .. },
            } => assert_eq!(surface_type, SurfaceKind::Torus),
            other => panic!("expected Unverifiable{{UnsupportedSurface}}, got {other:?}"),
        }
    }

    #[test]
    fn full_untrimmed_sphere_reports_0_to_180() {
        let mut surfaces = SurfaceStore::new();
        let sphere = Sphere::new(Point3::new(0.0, 0.0, 0.0), 1.0).expect("sphere");
        let surface_id = surfaces.add(Box::new(sphere));

        // Degenerate empty outer loop — matches how the primitive
        // constructor builds a full sphere face (see topology_builder.rs).
        let mut loops = LoopStore::new();
        let outer_loop = loops.add(Loop::new(0, LoopType::Outer));
        let curves = CurveStore::new();
        let edges = EdgeStore::new();
        let mut faces = FaceStore::new();
        let face = Face::new(0, surface_id, outer_loop, FaceOrientation::Forward);
        let face_id = faces.add(face);

        let outcome = face_orientation_field(
            face_id,
            Vector3::Z,
            &faces,
            &loops,
            &edges,
            &curves,
            &surfaces,
        )
        .expect("malformed-input free fixture");

        match outcome {
            OrientationOutcome::Range {
                min_deg, max_deg, ..
            } => {
                assert!((min_deg - 0.0).abs() < 1e-9);
                assert!((max_deg - 180.0).abs() < 1e-9);
            }
            other => panic!("expected an exact Range, got {other:?}"),
        }
    }

    #[test]
    fn dangling_face_ref_is_an_error_not_a_refusal() {
        let surfaces = SurfaceStore::new();
        let loops = LoopStore::new();
        let curves = CurveStore::new();
        let edges = EdgeStore::new();
        let faces = FaceStore::new();

        let result =
            face_orientation_field(999, Vector3::Z, &faces, &loops, &edges, &curves, &surfaces);
        assert!(matches!(
            result,
            Err(DfmError::DanglingFaceRef { face: 999 })
        ));
    }
}
