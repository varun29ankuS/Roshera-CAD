//! Proven enclosures for tensor-product B-spline patches — intervals and
//! normal cones that are THEOREMS, never sampled guesses (freeform-coverage
//! spec F2, 2026-07-30).
//!
//! ## The thesis — never sample, ENCLOSE
//!
//! A sampled extreme over a patch is a lower bound on the true range with
//! no error term: it can silently miss the worst point (the
//! `ray_surface_numerical` 10×10-grid defect class this codebase already
//! deleted). The convex-hull property gives a PROVEN enclosure instead:
//! every quantity derived here comes with a mathematical proof that the
//! true value set lies inside the reported bound. Sampling appears in this
//! module only inside `#[cfg(test)]`, as a TEST ORACLE (a dense sample
//! must fall INSIDE the enclosure) — legitimate as a falsifier, and
//! illegitimate as an implementation.
//!
//! ## Theorems this module rests on (each verified, not assumed)
//!
//! **T1 — convex-hull property.** A tensor-product B-spline patch
//! `S(u,v) = Σ_i Σ_j N_{i,p}(u) N_{j,q}(v) P_ij` has non-negative basis
//! functions forming a partition of unity over the active parameter
//! domain `[u_p, u_{m-p-1}] × [v_q, v_{n-q-1}]`, so every surface point is
//! a convex combination of the control points and lies in their convex
//! hull. Per coordinate axis, `[min_ij P, max_ij P]` is therefore a proven
//! enclosure of the surface's coordinate range (Piegl & Tiller §3.2;
//! Farin ch. 8).
//!
//! **T2 — derivative control net.** The u-partial of a NON-RATIONAL patch
//! is itself a tensor-product B-spline of degree `(p-1, q)`:
//!
//! ```text
//! S_u(u,v) = Σ_{i=0}^{nu-2} Σ_j N_{i,p-1}(u) N_{j,q}(v) Q_ij,
//! Q_ij = p · (P_{i+1,j} − P_{i,j}) / (u_{i+p+1} − u_{i+1})
//! ```
//!
//! over the knot vector `U' = U` with its first and last knot dropped
//! (Piegl & Tiller eq. 3.8 / §3.3; derived from
//! `N'_{i,p} = p/(u_{i+p}−u_i)·N_{i,p-1} − p/(u_{i+p+1}−u_{i+1})·N_{i+1,p-1}`
//! and summation by parts — verified against `NurbsSurface::
//! evaluate_derivatives` in the tests, not assumed). A term whose
//! denominator `u_{i+p+1} − u_{i+1}` is exactly zero has a derivative
//! basis function of empty support (`N_{i,p-1}` over `U'` lives on
//! `[u_{i+1}, u_{i+p+1}]`), contributes nothing anywhere on the domain,
//! and is EXCLUDED from the hull rather than fabricated; the remaining
//! basis functions still sum to 1. By T1 applied to this derivative
//! spline, `S_u` over the patch lies in `conv{Q_ij}`. Same for `S_v`.
//!
//! **T3 — bilinear pairwise-cross enclosure (THE normal cone).** Writing
//! `S_u = Σ λ_ij Q^u_ij` and `S_v = Σ μ_kl Q^v_kl` with convex weights
//! `λ, μ ≥ 0, Σλ = Σμ = 1` (T1/T2), bilinearity of the cross product
//! gives
//!
//! ```text
//! n = S_u × S_v = Σ_ij Σ_kl (λ_ij μ_kl) · (Q^u_ij × Q^v_kl),
//! ```
//!
//! and `Σ_ijkl λ_ij μ_kl = 1` with all terms non-negative — so the
//! unnormalized surface normal is a CONVEX COMBINATION of the pairwise
//! cross products of derivative control vectors. Enclosing the directions
//! of all pairwise crosses encloses the direction of the normal.
//!
//! **T4 — cone closure under convex combination.** The set
//! `K(a,t) = { x : x·a ≥ |x|·cos t }` (a Lorentz / second-order cone) is
//! convex for `t < π/2`. If every pairwise cross lies in `K(axis, t)` with
//! strictly positive dot against `axis`, then so does every convex
//! combination, hence `n ∈ K(axis, t)` and — because
//! `n·axis ≥ Σ λμ (C_pair·axis) ≥ min_pair(C_pair·axis) > 0` — the normal
//! provably never vanishes on the patch. Both facts are needed: the cone
//! bounds the direction, the positive dot proves non-degeneracy.
//!
//! ## The tangent-cone composition bound (derived, documented, NOT used)
//!
//! The alternative construction — enclose `S_u` in a cone `(a_u, t_u)`,
//! `S_v` in `(a_v, t_v)`, and bound the cone of `S_u × S_v` about
//! `a_u × a_v` — was derived rigorously as follows. For unit `û, v̂` with
//! `∠(û,a_u) = α ≤ t_u`, `∠(v̂,a_v) = β ≤ t_v`, write
//! `û = cos α·a_u + sin α·e`, `v̂ = cos β·a_v + sin β·f` (`e ⊥ a_u`,
//! `f ⊥ a_v`, unit). Then
//!
//! ```text
//! û × v̂ = cos α cos β · (a_u × a_v) + d,
//! |d| ≤ cos α sin β + sin α cos β + sin α sin β ≤ D,
//!   D = sin t_u + sin t_v + sin t_u · sin t_v,
//! ```
//!
//! and with `C = cos t_u cos t_v`, `γ = ∠(a_u, a_v)`, the component of
//! `û × v̂` along `n̂_0 = (a_u×a_v)/sin γ` is at least `C·sin γ − D`. When
//! `C·sin γ > D` this is strictly positive, so the cross product cannot
//! vanish or flip, and
//!
//! ```text
//! tan θ ≤ D / (C·sin γ − D),   θ = ∠(û×v̂, a_u×a_v),
//! ```
//!
//! giving half-angle `t_n = atan(D / (C·sin γ − D)) < π/2`. When
//! `C·sin γ ≤ D` the direction of the cross product is UNBOUNDED by this
//! argument (the near-parallel degeneracy, `sin γ → 0`) and the only
//! honest outcome is refusal. Every inequality above is loose-direction
//! only — the bound is sound. It is, however, strictly weaker than T3:
//! for the bilinear patch `S(u,v) = (u, v, uv)` the tangent cones have
//! `t_u = t_v ≈ 26.57°`, giving `C·sin γ = 0.784 < D = 1.094` — the
//! composition bound REFUSES FOREVER (even at the theoretical optimum
//! `t = 22.5°`, `γ = 90°`: `C = 0.854 < D = 0.912`), while the pairwise
//! hull of T3 yields a valid ≈ 31° cone that provably contains every
//! normal. Since both are theorems and T3 is uniformly available and
//! tighter, T3 is the implementation; this derivation is retained because
//! its refusal condition NAMES the degeneracy (near-parallel tangents)
//! that T3 detects as vanishing/opposed pairwise crosses.
//!
//! ## Rational NURBS are refused by name
//!
//! For weighted patches the derivative obeys the quotient rule
//! `S_u = (A_u − W_u·S)/W` — it is NOT a control-point difference, and a
//! sound tangent enclosure would additionally need proven bounds on the
//! weight function `W` and its derivative over the patch. This slice does
//! not fake that bound: a patch whose weights are not all EXACTLY equal
//! is refused as [`EnclosureError::RationalUnsupported`]. (A constant
//! weight `w ≠ 0` cancels between numerator and denominator — the surface
//! IS the non-rational B-spline of its control points — so constant-weight
//! patches are handled exactly. Exact equality, not a tolerance: weights
//! merely close to equal do not PROVE the cancellation.) An honest
//! refusal here is a correct F2 outcome per the spec; the homogeneous-
//! coordinate treatment is future work and is not approximated.
//!
//! ## Floating-point honesty
//!
//! - **Interval arithmetic is outward-rounded**: each finite-precision
//!   `+`, `−`, `×` result is nudged one ulp outward (`next_down`/
//!   `next_up`), so the true real-arithmetic value is always inside;
//!   composition can only widen, never illegitimately narrow. Endpoint
//!   saturation to ±∞ is permitted as the honest widening limit;
//!   constructors reject non-finite INPUT (one cannot claim to enclose a
//!   measurement that is not a number).
//! - **Depth-0 position bounds involve no arithmetic at all** — pure
//!   min/max over stored coordinates — and are exact.
//! - **Refined nets carry an explicit error budget.** Knot insertion is
//!   surface-invariant in exact arithmetic (Boehm); in floating point each
//!   sweep perturbs control points, tracked conservatively as
//!   `err += 8·ε·M·(insertions)` (`M` = max |coordinate|, ε = machine
//!   epsilon; a generous over-count — each inserted knot recombines each
//!   affected point once with ≤ 8 roundings). Position intervals are
//!   padded outward by the accumulated `err`.
//! - **Directions carry error balls.** A derivative control vector
//!   `Q = p·ΔP/h` computed from coordinates uncertain by `err` is treated
//!   as a ball of radius `r_q = 2p·(err + ε·M)/h + 4ε|Q|`; a pairwise
//!   cross of two balls is a ball of radius
//!   `|Q^u|·r_v + |Q^v|·r_u + r_u r_v + 4ε|Q^u||Q^v|`. A ball of radius
//!   `r` around a vector of length `L` widens its direction spread by at
//!   most `asin(r/L) ≤ 2·(r/L)` for `r/L ≤ 1/2` (since
//!   `asin x ≤ (π/2)x ≤ 2x` on `[0,1]`); if `r/L > 1/2` the direction is
//!   too uncertain and the cone REFUSES rather than guesses.
//! - **Transcendental slack.** `acos`/`atan2`-derived angles are padded by
//!   [`ANGLE_SLACK_RAD`] = 1e-9 rad, exceeding the ≲ 1e-15 rounding of
//!   std trig on `[0, π]` by six orders of magnitude, always in the
//!   widening direction.
//!
//! ## Conservative refusal lines (documented, like
//! `orientation.rs::RIM_PERPENDICULAR_TOL`)
//!
//! - [`MAX_HALF_ANGLE_RAD`] = 1.45 rad (≈ 83°): a direction cone this wide
//!   cannot be usefully composed and sits dangerously close to the π/2
//!   limit of T4's convexity argument — refuse rather than return a cone
//!   that is technically a cone and practically a lie.
//! - Ball ratio `r/L > 1/2`: direction uncertainty refusal (above).
//! - Exactly-zero derivative control vector: the tangent hull touches the
//!   origin, so the tangent cannot be proven nonvanishing — refuse.
//! - Exactly-zero or opposed pairwise crosses: the normal direction set
//!   touches the origin or spans a half-turn — the near-parallel-tangent
//!   degeneracy — refuse.
//! - [`PAIR_CAP`]: the T3 enclosure is O(|Q^u|·|Q^v|); beyond the cap the
//!   computation is refused (a budget-shaped refusal, not an approximation).
//!
//! ## Refinement tightens; the budget is honest
//!
//! [`refine`] inserts the midpoint of every non-degenerate knot span
//! (dyadic Oslo/Boehm single-knot insertion, Piegl & Tiller A5.1 — the
//! same algorithm as `NurbsSurface::insert_single_knot_u`, specialized to
//! the constant-weight case this module admits), recomputes bounds, and
//! INTERSECTS with the previous bounds — legitimate because both are
//! proven enclosures of the same surface, so their intersection is too;
//! this makes monotone narrowing a construction, not an aspiration.
//! Refinement stops when the caller's predicate is satisfied, or when the
//! depth/size budget is exhausted — an exhausted budget returns the
//! achieved bound with `converged: false`, NEVER a midpoint fallback.
//!
//! ## References
//! - Piegl & Tiller, *The NURBS Book*, 2nd ed. — §3.2 hull, eq. 3.8
//!   derivative net, A5.1 knot insertion.
//! - Patrikalakis & Maekawa, *Shape Interrogation for CAD/CAM* — interval
//!   methods for surface interrogation (the subject of this module).
//! - Farin, *Curves and Surfaces for CAGD*, 5th ed. — subdivision and
//!   hull convergence.
//!
//! Indexed access below is bounds-guaranteed by the enclosing loop
//! structure (net dimensions validated once in [`net_from_surface`]),
//! matching the `bspline.rs`/`nurbs.rs` idiom.
#![allow(clippy::indexing_slicing)]

use thiserror::Error;

use crate::math::nurbs::NurbsSurface;
use crate::math::{Point3, Vector3};

/// Absolute widening applied to every derived angle, in radians. See the
/// module docs ("Floating-point honesty"): this exceeds the accumulated
/// transcendental rounding by ~6 orders of magnitude, always in the
/// conservative (widening) direction.
pub const ANGLE_SLACK_RAD: f64 = 1e-9;

/// Documented conservative-refusal line: a direction cone whose half-angle
/// would exceed this is refused rather than returned (module docs).
pub const MAX_HALF_ANGLE_RAD: f64 = 1.45;

/// Documented conservative-refusal line: maximum number of pairwise cross
/// products the T3 normal-cone enclosure will compute before refusing on
/// size (module docs).
pub const PAIR_CAP: usize = 4_000_000;

/// Per-sweep rounding budget multiplier for refined control nets
/// (module docs, "Floating-point honesty").
const SWEEP_ROUND_ULPS: f64 = 8.0;

/// Maximum tolerated direction-uncertainty ratio `r/L` for an error ball
/// of radius `r` around a vector of length `L` (module docs).
const MAX_BALL_RATIO: f64 = 0.5;

const EPS: f64 = f64::EPSILON;

/// Parametric direction of a tensor-product patch, for refusal messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamDirection {
    /// The u (row) direction.
    U,
    /// The v (column) direction.
    V,
}

impl std::fmt::Display for ParamDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParamDirection::U => write!(f, "u"),
            ParamDirection::V => write!(f, "v"),
        }
    }
}

/// Typed refusals of this module. Every variant is an honest "cannot
/// prove", never an approximation labeled as exact (kernel honesty rule).
#[derive(Debug, Clone, PartialEq, Error)]
pub enum EnclosureError {
    /// An [`Interval`] constructor was handed endpoints that cannot form a
    /// proven enclosure (non-finite input, or `lo > hi`).
    #[error("interval endpoints must be finite with lo <= hi: {detail}")]
    InvalidInterval {
        /// What was wrong.
        detail: String,
    },
    /// The patch is genuinely rational (weights not all exactly equal):
    /// the derivative is quotient-rule-shaped and this slice does not
    /// fake a bound for it (module docs, "Rational NURBS").
    #[error(
        "rational patch refused by name: weights vary in [{min_weight}, {max_weight}]; \
         this slice proves bounds only for non-rational (constant-weight) patches"
    )]
    RationalUnsupported {
        /// Smallest weight found on the net.
        min_weight: f64,
        /// Largest weight found on the net.
        max_weight: f64,
    },
    /// The patch is malformed or degenerate in a way that defeats the
    /// hull argument before it starts (non-finite coordinates, ragged
    /// control grid, invalid knot vector, empty active domain, …).
    #[error("degenerate or malformed patch: {detail}")]
    DegeneratePatch {
        /// The specific defect.
        detail: String,
    },
    /// A derivative control vector is exactly zero: the tangent hull
    /// touches the origin, so the tangent cannot be proven nonvanishing
    /// and no direction cone exists.
    #[error(
        "zero derivative control vector Q[{i}][{j}] in {direction}: the tangent hull \
         touches the origin, so the {direction}-tangent cannot be proven nonvanishing"
    )]
    ZeroTangentVector {
        /// Which parametric direction's derivative net.
        direction: ParamDirection,
        /// Row index into the derivative net.
        i: usize,
        /// Column index into the derivative net.
        j: usize,
    },
    /// A derivative control vector's floating-point error ball is too
    /// large relative to its magnitude to bound a direction (module docs,
    /// ball-ratio refusal line).
    #[error("tangent vector too uncertain to bound a direction: {detail}")]
    IllConditionedTangent {
        /// The offending ratio and location.
        detail: String,
    },
    /// The normal direction set cannot be enclosed in a proper cone —
    /// vanishing or opposed pairwise cross products (the near-parallel-
    /// tangent degeneracy) or a spread beyond [`MAX_HALF_ANGLE_RAD`].
    #[error("normal direction cannot be enclosed in a proper cone: {detail}")]
    NormalUnbounded {
        /// The specific degeneracy.
        detail: String,
    },
    /// A caller-supplied reference direction could not be normalized.
    #[error("reference direction invalid: {detail}")]
    InvalidReference {
        /// The specific defect.
        detail: String,
    },
    /// The pairwise cross enclosure would exceed [`PAIR_CAP`] — a
    /// budget-shaped refusal, not an approximation.
    #[error(
        "pairwise cross-product enclosure over {pairs} pairs exceeds cap {cap}; \
         bound a smaller net"
    )]
    NetTooLarge {
        /// Pairs the computation would need.
        pairs: usize,
        /// The documented cap.
        cap: usize,
    },
    /// Two enclosures of the same value set came out disjoint — impossible
    /// if both are sound, so this is a loud internal soundness alarm, not
    /// a routine refusal.
    #[error("internal soundness alarm — two enclosures of the same set are disjoint: {detail}")]
    InconsistentBounds {
        /// The two disjoint ranges.
        detail: String,
    },
}

/// Next representable `f64` above `x` (total, never panics; ±∞/NaN map to
/// themselves). Used for outward rounding.
fn next_up(x: f64) -> f64 {
    if x.is_nan() || x == f64::INFINITY {
        return x;
    }
    if x == 0.0 {
        return f64::from_bits(1);
    }
    if x > 0.0 {
        f64::from_bits(x.to_bits() + 1)
    } else {
        f64::from_bits(x.to_bits() - 1)
    }
}

/// Next representable `f64` below `x` (total, never panics).
fn next_down(x: f64) -> f64 {
    -next_up(-x)
}

/// A PROVEN enclosure `[lo, hi]` of some real value set — a theorem, not a
/// tolerance band (freeform spec F1).
///
/// Honest by construction:
/// - no `Default`, no way to build one from non-finite input or with
///   `lo > hi`;
/// - fields are private — the only reads are [`Interval::lo`]/
///   [`Interval::hi`], the only writes are the validated constructors and
///   the outward-rounded arithmetic;
/// - arithmetic results may saturate an endpoint to ±∞ (the honest
///   widening limit) but are never NaN and never narrower than the true
///   real-arithmetic result;
/// - deliberately NOT serde-serializable: a wire peer must not be able to
///   forge an "enclosure" the kernel never proved. The DFM report layer
///   copies endpoints into its own inert wire struct.
///
/// There is deliberately no `midpoint()` — the spec's budget-exhaustion
/// rule is "report the bound, never fall back to the midpoint", and this
/// type does not offer the temptation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Interval {
    lo: f64,
    hi: f64,
}

impl Interval {
    /// Enclosure of the single real value `v` (every fp literal denotes an
    /// exact real number, so `[v, v]` is a proven enclosure of it).
    pub fn point(v: f64) -> Result<Self, EnclosureError> {
        if !v.is_finite() {
            return Err(EnclosureError::InvalidInterval {
                detail: format!("point value {v} is not finite"),
            });
        }
        Ok(Self { lo: v, hi: v })
    }

    /// Enclosure with explicit endpoints. The caller asserts (and remains
    /// responsible for) `[lo, hi]` actually enclosing the value set it
    /// will be used for; this constructor rules out the states that can
    /// never be an enclosure of anything (`NaN`, ±∞ input, `lo > hi`).
    pub fn enclosing(lo: f64, hi: f64) -> Result<Self, EnclosureError> {
        if !lo.is_finite() || !hi.is_finite() {
            return Err(EnclosureError::InvalidInterval {
                detail: format!("endpoints [{lo}, {hi}] are not finite"),
            });
        }
        if lo > hi {
            return Err(EnclosureError::InvalidInterval {
                detail: format!("lo {lo} exceeds hi {hi}"),
            });
        }
        Ok(Self { lo, hi })
    }

    /// Exact hull `[min, max]` of a non-empty set of finite values —
    /// pure comparisons, no arithmetic, no rounding.
    pub fn hull_of<I: IntoIterator<Item = f64>>(values: I) -> Result<Self, EnclosureError> {
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        let mut any = false;
        for v in values {
            if !v.is_finite() {
                return Err(EnclosureError::InvalidInterval {
                    detail: format!("hull input {v} is not finite"),
                });
            }
            lo = lo.min(v);
            hi = hi.max(v);
            any = true;
        }
        if !any {
            return Err(EnclosureError::InvalidInterval {
                detail: "hull of an empty value set".to_string(),
            });
        }
        Ok(Self { lo, hi })
    }

    /// Lower endpoint.
    pub fn lo(&self) -> f64 {
        self.lo
    }

    /// Upper endpoint.
    pub fn hi(&self) -> f64 {
        self.hi
    }

    /// Outward-rounded width (an upper bound on the true width).
    pub fn width(&self) -> f64 {
        next_up(self.hi - self.lo)
    }

    /// Whether `v` lies inside the enclosure (endpoints included).
    pub fn contains(&self, v: f64) -> bool {
        self.lo <= v && v <= self.hi
    }

    /// Outward-rounded sum: encloses `x + y` for every `x ∈ self`,
    /// `y ∈ other`.
    pub fn add(&self, other: &Self) -> Self {
        Self {
            lo: next_down(self.lo + other.lo),
            hi: next_up(self.hi + other.hi),
        }
    }

    /// Outward-rounded difference: encloses `x − y` for every `x ∈ self`,
    /// `y ∈ other`.
    pub fn sub(&self, other: &Self) -> Self {
        Self {
            lo: next_down(self.lo - other.hi),
            hi: next_up(self.hi - other.lo),
        }
    }

    /// Exact negation (sign flip introduces no rounding).
    pub fn neg(&self) -> Self {
        Self {
            lo: -self.hi,
            hi: -self.lo,
        }
    }

    /// Outward-rounded product: encloses `x · y` over both operands. The
    /// four endpoint products cover all monotonicity cases; a NaN product
    /// (only possible from a saturated ±∞ endpoint times zero) widens to
    /// the whole line — honest, never narrow.
    pub fn mul(&self, other: &Self) -> Self {
        let candidates = [
            self.lo * other.lo,
            self.lo * other.hi,
            self.hi * other.lo,
            self.hi * other.hi,
        ];
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for c in candidates {
            if c.is_nan() {
                return Self {
                    lo: f64::NEG_INFINITY,
                    hi: f64::INFINITY,
                };
            }
            lo = lo.min(c);
            hi = hi.max(c);
        }
        Self {
            lo: next_down(lo),
            hi: next_up(hi),
        }
    }

    /// Exact union hull (encloses both operands' value sets).
    pub fn union_hull(&self, other: &Self) -> Self {
        Self {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
        }
    }

    /// Intersection of two enclosures OF THE SAME value set — itself a
    /// proven enclosure of that set (both bounds hold simultaneously), and
    /// the legitimate mechanism by which [`refine`] guarantees monotone
    /// narrowing. Disjoint inputs mean at least one claimed enclosure was
    /// wrong: a loud [`EnclosureError::InconsistentBounds`] alarm, never a
    /// silent pick.
    pub fn intersect(&self, other: &Self) -> Result<Self, EnclosureError> {
        let lo = self.lo.max(other.lo);
        let hi = self.hi.min(other.hi);
        if lo > hi {
            return Err(EnclosureError::InconsistentBounds {
                detail: format!(
                    "[{}, {}] and [{}, {}] are disjoint",
                    self.lo, self.hi, other.lo, other.hi
                ),
            });
        }
        Ok(Self { lo, hi })
    }

    /// Outward pad by `r ≥ 0` (used to absorb a proven absolute-error
    /// budget, e.g. refined-net rounding). `r = 0` is exact.
    fn padded(&self, r: f64) -> Self {
        if r == 0.0 {
            return *self;
        }
        Self {
            lo: next_down(self.lo - r),
            hi: next_up(self.hi + r),
        }
    }
}

/// A proven enclosure of a set of DIRECTIONS: every direction in the set
/// lies within `half_angle` of `axis`. Constructed only by this module's
/// bound machinery (there is no public constructor that could assert an
/// unproven cone); `half_angle < π/2` always, so the T4 convexity argument
/// applies and the enclosed vector set provably excludes the origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalCone {
    axis: Vector3,
    half_angle: f64,
}

impl NormalCone {
    /// The (unit) cone axis.
    pub fn axis(&self) -> Vector3 {
        self.axis
    }

    /// The proven half-angle, radians, in `[0, π/2)`.
    pub fn half_angle(&self) -> f64 {
        self.half_angle
    }

    /// Whether `v`'s direction lies inside the cone. A zero-length `v`
    /// has no direction and is never contained.
    pub fn contains_direction(&self, v: &Vector3) -> bool {
        match v.normalize() {
            Ok(unit) => {
                let cos = unit.dot(&self.axis).clamp(-1.0, 1.0);
                cos.acos() <= self.half_angle
            }
            Err(_) => false,
        }
    }

    /// Proven enclosure of the angle (radians) between any direction in
    /// this cone and the fixed direction `d`: by the spherical triangle
    /// inequality, `∠(n, d) ∈ [∠(axis,d) − t, ∠(axis,d) + t]` for every
    /// `n` with `∠(n, axis) ≤ t`, clamped to `[0, π]` and padded by
    /// [`ANGLE_SLACK_RAD`]. This is the primitive the F3 orientation arm
    /// will consume.
    pub fn angle_to(&self, d: &Vector3) -> Result<Interval, EnclosureError> {
        let unit = d
            .normalize()
            .map_err(|_| EnclosureError::InvalidReference {
                detail: "direction has zero length".to_string(),
            })?;
        let gamma = unit.dot(&self.axis).clamp(-1.0, 1.0).acos();
        let lo = (gamma - self.half_angle - ANGLE_SLACK_RAD).max(0.0);
        let hi = (gamma + self.half_angle + ANGLE_SLACK_RAD).min(std::f64::consts::PI);
        Interval::enclosing(lo, hi)
    }
}

/// The proven bounds [`control_net_bounds`] computes for a patch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlNetBounds {
    /// Per-axis (x, y, z) enclosure of every surface point over the
    /// patch's active parameter domain (theorem T1).
    pub position: [Interval; 3],
    /// Enclosure of the direction of the parametric normal
    /// `S_u × S_v` over the patch (theorems T2–T4). Orientation follows
    /// the `u`-then-`v` cross order; mapping to a face's OUTWARD normal
    /// (the `FaceOrientation::sign` flip) is the analyzer's job, exactly
    /// as in `orientation.rs`.
    pub normal: NormalCone,
}

/// Refinement budget for [`refine`]. Exhaustion is an honest outcome
/// reported with the achieved bound, never a fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefineBudget {
    /// Maximum number of full midpoint-insertion sweeps.
    pub max_depth: usize,
    /// Maximum total control points the refined net may reach (each sweep
    /// roughly doubles the span count in each direction).
    pub max_control_points: usize,
}

impl RefineBudget {
    /// A documented standard budget: depth 16, ≤ 4096 control points.
    pub const STANDARD: RefineBudget = RefineBudget {
        max_depth: 16,
        max_control_points: 4096,
    };
}

/// Outcome of budgeted refinement: the tightest proven bounds achieved,
/// the depth actually reached, and whether the caller's convergence
/// predicate was satisfied. `converged: false` is an honest report of the
/// achieved bound — the load-bearing third row of the spec's verdict
/// table, never a midpoint guess.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RefinedBounds {
    /// Tightest proven bounds achieved within the budget.
    pub bounds: ControlNetBounds,
    /// Number of completed midpoint sweeps whose bounds were incorporated.
    pub refinement_depth: usize,
    /// Whether the caller's predicate held on the final bounds.
    pub converged: bool,
}

// ---------------------------------------------------------------------------
// Internal non-rational net representation
// ---------------------------------------------------------------------------

/// A validated non-rational tensor-product B-spline control net. The
/// constant weight of the source patch has been PROVEN and cancelled
/// (module docs, "Rational NURBS"), so points alone determine the surface.
#[derive(Debug, Clone)]
struct Net {
    /// `pts[i][j]`: row index `i` runs along u, column `j` along v.
    pts: Vec<Vec<Point3>>,
    ku: Vec<f64>,
    kv: Vec<f64>,
    pu: usize,
    pv: usize,
}

impl Net {
    fn n_u(&self) -> usize {
        self.pts.len()
    }

    fn n_v(&self) -> usize {
        if self.pts.is_empty() {
            0
        } else {
            self.pts[0].len()
        }
    }

    fn max_abs_coord(&self) -> f64 {
        let mut m = 0.0f64;
        for row in &self.pts {
            for p in row {
                m = m.max(p.x.abs()).max(p.y.abs()).max(p.z.abs());
            }
        }
        m
    }

    /// Swap the u and v roles (points transposed, knots and degrees
    /// swapped) — lets one Boehm insertion routine serve both directions,
    /// mirroring `NurbsSurface::transpose`.
    fn transpose(&mut self) {
        let n_u = self.n_u();
        let n_v = self.n_v();
        let mut new_pts = vec![vec![Point3::new(0.0, 0.0, 0.0); n_u]; n_v];
        for i in 0..n_u {
            for j in 0..n_v {
                new_pts[j][i] = self.pts[i][j];
            }
        }
        self.pts = new_pts;
        std::mem::swap(&mut self.ku, &mut self.kv);
        std::mem::swap(&mut self.pu, &mut self.pv);
    }

    /// Boehm single-knot insertion in u (Piegl & Tiller A5.1, non-rational
    /// specialization of `NurbsSurface::insert_single_knot_u` — the same
    /// Oslo machinery, with the proven-constant weight cancelled).
    /// Precondition (guaranteed by the caller, which only ever inserts the
    /// strict interior midpoint of a non-empty active span): `u` lies
    /// strictly inside a span of the active domain, so the located span
    /// index satisfies `s ≥ pu` and every blend denominator is strictly
    /// positive.
    fn insert_knot_u(&mut self, u: f64) {
        // Last index with ku[s] <= u; u strictly above ku[pu] guarantees
        // s >= pu >= 1, so the subtraction cannot underflow.
        let s = self.ku.partition_point(|&k| k <= u).saturating_sub(1);
        let p = self.pu;
        let n_u = self.n_u();
        let n_v = self.n_v();

        let mut new_pts: Vec<Vec<Point3>> = Vec::with_capacity(n_u + 1);
        // Unaffected prefix: Q_i = P_i for i in 0..=s-p.
        for i in 0..=s.saturating_sub(p) {
            new_pts.push(self.pts[i].clone());
        }
        // Blended band: Q_i = (1-α_i)·P_{i-1} + α_i·P_i, α_i in [0, 1].
        for i in (s + 1 - p)..=s {
            let denom = self.ku[i + p] - self.ku[i];
            // denom >= (span width) > 0 by the precondition; the clamp
            // keeps the combination convex under fp rounding, preserving
            // the hull argument exactly.
            let alpha = if denom > 0.0 {
                ((u - self.ku[i]) / denom).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let mut row = Vec::with_capacity(n_v);
            for j in 0..n_v {
                let a = self.pts[i - 1][j];
                let b = self.pts[i][j];
                row.push(Point3::new(
                    (1.0 - alpha) * a.x + alpha * b.x,
                    (1.0 - alpha) * a.y + alpha * b.y,
                    (1.0 - alpha) * a.z + alpha * b.z,
                ));
            }
            new_pts.push(row);
        }
        // Shifted suffix: Q_i = P_{i-1}.
        for i in (s + 1)..=n_u {
            new_pts.push(self.pts[i - 1].clone());
        }
        self.pts = new_pts;
        self.ku.insert(s + 1, u);
    }

    /// Midpoints of every non-degenerate active span of `knots` (degree
    /// `p`) whose midpoint is strictly interior in fp — spans too narrow
    /// to split are skipped (natural refinement floor).
    fn span_midpoints(knots: &[f64], p: usize) -> Vec<f64> {
        let mut mids = Vec::new();
        if knots.len() < 2 * (p + 1) {
            return mids;
        }
        for s in p..(knots.len() - p - 1) {
            let a = knots[s];
            let b = knots[s + 1];
            if b > a {
                let m = 0.5 * (a + b);
                if m > a && m < b {
                    mids.push(m);
                }
            }
        }
        mids
    }
}

/// Validate a [`NurbsSurface`] (whose fields are public and therefore not
/// trusted post-construction) and extract the non-rational net, PROVING
/// the constant-weight cancellation on the way (module docs).
fn net_from_surface(surface: &NurbsSurface) -> Result<Net, EnclosureError> {
    let n_u = surface.control_points.len();
    if n_u == 0 {
        return Err(EnclosureError::DegeneratePatch {
            detail: "empty control grid".to_string(),
        });
    }
    let n_v = surface.control_points[0].len();
    if n_v == 0 {
        return Err(EnclosureError::DegeneratePatch {
            detail: "empty control grid row".to_string(),
        });
    }
    for (i, row) in surface.control_points.iter().enumerate() {
        if row.len() != n_v {
            return Err(EnclosureError::DegeneratePatch {
                detail: format!(
                    "ragged control grid: row {i} has {} columns, expected {n_v}",
                    row.len()
                ),
            });
        }
        for (j, p) in row.iter().enumerate() {
            if !p.x.is_finite() || !p.y.is_finite() || !p.z.is_finite() {
                return Err(EnclosureError::DegeneratePatch {
                    detail: format!("non-finite control point at [{i}][{j}]"),
                });
            }
        }
    }
    if surface.weights.len() != n_u || surface.weights.iter().any(|row| row.len() != n_v) {
        return Err(EnclosureError::DegeneratePatch {
            detail: "weight grid dimensions do not match the control grid".to_string(),
        });
    }
    let w00 = surface.weights[0][0];
    if !w00.is_finite() || w00 == 0.0 {
        return Err(EnclosureError::DegeneratePatch {
            detail: format!("reference weight w[0][0] = {w00} is zero or non-finite"),
        });
    }
    let mut min_w = f64::INFINITY;
    let mut max_w = f64::NEG_INFINITY;
    let mut constant = true;
    for row in &surface.weights {
        for &w in row {
            if !w.is_finite() {
                return Err(EnclosureError::DegeneratePatch {
                    detail: format!("non-finite weight {w}"),
                });
            }
            min_w = min_w.min(w);
            max_w = max_w.max(w);
            if w != w00 {
                constant = false;
            }
        }
    }
    if !constant {
        return Err(EnclosureError::RationalUnsupported {
            min_weight: min_w,
            max_weight: max_w,
        });
    }

    let pu = surface.degree_u;
    let pv = surface.degree_v;
    if pu == 0 || pv == 0 {
        return Err(EnclosureError::DegeneratePatch {
            detail: "degree must be at least 1 in each direction".to_string(),
        });
    }
    let ku = surface.knots_u.values().to_vec();
    let kv = surface.knots_v.values().to_vec();
    if ku.len() != n_u + pu + 1 || kv.len() != n_v + pv + 1 {
        return Err(EnclosureError::DegeneratePatch {
            detail: format!(
                "knot counts ({}, {}) do not match net ({n_u}+{pu}+1, {n_v}+{pv}+1)",
                ku.len(),
                kv.len()
            ),
        });
    }
    for (name, knots) in [("u", &ku), ("v", &kv)] {
        for w in knots.windows(2) {
            if !(w[0].is_finite() && w[1].is_finite()) || w[1] < w[0] {
                return Err(EnclosureError::DegeneratePatch {
                    detail: format!("{name} knot vector is not finite and non-decreasing"),
                });
            }
        }
    }
    if !(ku[pu] < ku[ku.len() - pu - 1]) || !(kv[pv] < kv[kv.len() - pv - 1]) {
        return Err(EnclosureError::DegeneratePatch {
            detail: "empty active parameter domain".to_string(),
        });
    }

    Ok(Net {
        pts: surface.control_points.clone(),
        ku,
        kv,
        pu,
        pv,
    })
}

/// Position enclosure (T1): exact per-axis hull of the control net, padded
/// outward by the proven coordinate-error budget `coord_pad` (zero for the
/// original, unrefined net — then this is pure min/max with no rounding).
fn position_bounds(net: &Net, coord_pad: f64) -> [Interval; 3] {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for row in &net.pts {
        for p in row {
            let c = [p.x, p.y, p.z];
            for k in 0..3 {
                lo[k] = lo[k].min(c[k]);
                hi[k] = hi[k].max(c[k]);
            }
        }
    }
    // Coordinates were validated finite and the net is non-empty, so
    // lo <= hi per axis; construct directly and pad.
    [
        Interval {
            lo: lo[0],
            hi: hi[0],
        }
        .padded(coord_pad),
        Interval {
            lo: lo[1],
            hi: hi[1],
        }
        .padded(coord_pad),
        Interval {
            lo: lo[2],
            hi: hi[2],
        }
        .padded(coord_pad),
    ]
}

/// Derivative control net in the given direction (T2), with per-vector
/// error-ball radii. `coord_err` is the proven absolute uncertainty of the
/// net's coordinates (refined-net rounding budget + one ε·M for the
/// subtraction itself). Rows with an exactly-zero knot denominator have
/// identically-zero derivative basis support and are excluded (T2's
/// documented exclusion, not a fabrication).
fn derivative_net(
    net: &Net,
    direction: ParamDirection,
    coord_err: f64,
) -> Result<Vec<(Vector3, f64)>, EnclosureError> {
    // Work on a u-oriented view: for V, operate on the transposed net.
    let view: std::borrow::Cow<'_, Net> = match direction {
        ParamDirection::U => std::borrow::Cow::Borrowed(net),
        ParamDirection::V => {
            let mut t = net.clone();
            t.transpose();
            std::borrow::Cow::Owned(t)
        }
    };
    let net = view.as_ref();
    let p = net.pu;
    let pf = p as f64;
    let n_u = net.n_u();
    let n_v = net.n_v();

    let mut out = Vec::new();
    for i in 0..(n_u - 1) {
        let h = net.ku[i + p + 1] - net.ku[i + 1];
        if h == 0.0 {
            // Zero-support derivative basis: the term vanishes identically
            // on the domain (module docs, T2) — excluded, not invented.
            continue;
        }
        if !(h > 0.0) || !h.is_finite() {
            return Err(EnclosureError::DegeneratePatch {
                detail: format!("invalid knot span width {h} in {direction} derivative"),
            });
        }
        for j in 0..n_v {
            let a = net.pts[i][j];
            let b = net.pts[i + 1][j];
            let q = Vector3::new(
                pf * (b.x - a.x) / h,
                pf * (b.y - a.y) / h,
                pf * (b.z - a.z) / h,
            );
            let mag = q.magnitude();
            if mag == 0.0 {
                return Err(EnclosureError::ZeroTangentVector { direction, i, j });
            }
            // Error ball: coordinate uncertainty propagated through the
            // difference quotient, plus the operation's own rounding
            // (module docs, "Directions carry error balls").
            let r = 2.0 * pf * coord_err / h + 4.0 * EPS * mag;
            if r > MAX_BALL_RATIO * mag {
                return Err(EnclosureError::IllConditionedTangent {
                    detail: format!(
                        "|Q[{i}][{j}]| = {mag} in {direction} carries error ball {r} \
                         (ratio > {MAX_BALL_RATIO})"
                    ),
                });
            }
            out.push((q, r));
        }
    }
    if out.is_empty() {
        return Err(EnclosureError::DegeneratePatch {
            detail: format!("no nonvanishing derivative terms in {direction}"),
        });
    }
    Ok(out)
}

/// T3 + T4: enclose the direction of `S_u × S_v` from the pairwise cross
/// products of the two derivative nets, inflating each cross by its
/// propagated error ball. Two passes (axis, then max angle) so nothing
/// pair-sized is stored.
fn pairwise_normal_cone(
    qu: &[(Vector3, f64)],
    qv: &[(Vector3, f64)],
) -> Result<NormalCone, EnclosureError> {
    let pairs = qu.len().saturating_mul(qv.len());
    if pairs > PAIR_CAP {
        return Err(EnclosureError::NetTooLarge {
            pairs,
            cap: PAIR_CAP,
        });
    }

    // Per-pair cross, magnitude, and direction-ball padding. Returns the
    // (unit direction, angle padding) or a refusal.
    let cross_dir = |(u, ru): &(Vector3, f64),
                     (v, rv): &(Vector3, f64)|
     -> Result<(Vector3, f64), EnclosureError> {
        let c = u.cross(v);
        let mag = c.magnitude();
        let mu = u.magnitude();
        let mv = v.magnitude();
        if mag == 0.0 {
            return Err(EnclosureError::NormalUnbounded {
                detail: "a pairwise tangent cross product vanishes exactly \
                         (parallel tangent directions — the near-parallel degeneracy)"
                    .to_string(),
            });
        }
        let r = mu * rv + mv * ru + ru * rv + 4.0 * EPS * mu * mv;
        let ratio = r / mag;
        if ratio > MAX_BALL_RATIO {
            return Err(EnclosureError::NormalUnbounded {
                detail: format!(
                    "a pairwise tangent cross product of magnitude {mag} carries error \
                     ball {r} (ratio > {MAX_BALL_RATIO}): tangents too close to parallel \
                     to bound the normal direction"
                ),
            });
        }
        let inv = 1.0 / mag;
        // asin(x) <= 2x for x in [0, 1/2] (module docs) — conservative
        // direction padding for the error ball.
        Ok((Vector3::new(c.x * inv, c.y * inv, c.z * inv), 2.0 * ratio))
    };

    // Pass 1: axis = normalized sum of unit cross directions.
    let mut sum = Vector3::new(0.0, 0.0, 0.0);
    for a in qu {
        for b in qv {
            let (dir, _) = cross_dir(a, b)?;
            sum = sum + dir;
        }
    }
    let axis = sum
        .normalize()
        .map_err(|_| EnclosureError::NormalUnbounded {
            detail: "pairwise cross directions cancel — the normal direction set spans \
                 opposing directions"
                .to_string(),
        })?;

    // Pass 2: proven half-angle = max padded angle to the axis.
    let mut half_angle = 0.0f64;
    for a in qu {
        for b in qv {
            let (dir, pad) = cross_dir(a, b)?;
            let ang = dir.dot(&axis).clamp(-1.0, 1.0).acos() + pad + ANGLE_SLACK_RAD;
            half_angle = half_angle.max(ang);
        }
    }
    if half_angle >= MAX_HALF_ANGLE_RAD {
        return Err(EnclosureError::NormalUnbounded {
            detail: format!(
                "normal direction spread requires half-angle {half_angle} rad, beyond \
                 the documented conservative-refusal line {MAX_HALF_ANGLE_RAD} rad"
            ),
        });
    }
    Ok(NormalCone { axis, half_angle })
}

/// Bounds of one (possibly refined) net. `err_net` is the accumulated
/// proven coordinate-error budget (0 for the original net).
fn bounds_of_net(net: &Net, err_net: f64) -> Result<ControlNetBounds, EnclosureError> {
    let coord_err = err_net + EPS * net.max_abs_coord();
    let qu = derivative_net(net, ParamDirection::U, coord_err)?;
    let qv = derivative_net(net, ParamDirection::V, coord_err)?;
    let normal = pairwise_normal_cone(&qu, &qv)?;
    Ok(ControlNetBounds {
        position: position_bounds(net, err_net),
        normal,
    })
}

/// Tighten `prev` with `next` — both proven enclosures of the same patch,
/// so per-axis interval intersection and the narrower of the two cones are
/// proven enclosures too. This is what makes refinement monotone BY
/// CONSTRUCTION (module docs).
fn tighten(
    prev: &ControlNetBounds,
    next: &ControlNetBounds,
) -> Result<ControlNetBounds, EnclosureError> {
    let position = [
        prev.position[0].intersect(&next.position[0])?,
        prev.position[1].intersect(&next.position[1])?,
        prev.position[2].intersect(&next.position[2])?,
    ];
    let normal = if next.normal.half_angle < prev.normal.half_angle {
        next.normal
    } else {
        prev.normal
    };
    Ok(ControlNetBounds { position, normal })
}

/// Proven position and normal-direction bounds for a non-rational
/// tensor-product B-spline patch, straight from its control net (theorems
/// T1–T4 in the module docs). Refuses — by name — rational patches,
/// degenerate nets, vanishing tangents, and unboundable normal
/// directions; never approximates.
pub fn control_net_bounds(surface: &NurbsSurface) -> Result<ControlNetBounds, EnclosureError> {
    let net = net_from_surface(surface)?;
    bounds_of_net(&net, 0.0)
}

/// Budgeted refinement (module docs, "Refinement tightens"): midpoint knot
/// insertion (Oslo/Boehm) tightens the control net toward the surface;
/// bounds are recomputed each sweep and intersected with the running
/// bounds, so the reported enclosure narrows monotonically. Stops when
/// `converged_when` holds, when the budget is exhausted, when no span can
/// be split further at fp resolution, or when a sweep's cone computation
/// refuses (the bounds already proven remain valid and are returned).
/// Budget exhaustion reports the achieved bound with `converged: false` —
/// never a midpoint fallback.
pub fn refine<F>(
    surface: &NurbsSurface,
    budget: RefineBudget,
    mut converged_when: F,
) -> Result<RefinedBounds, EnclosureError>
where
    F: FnMut(&ControlNetBounds) -> bool,
{
    let mut net = net_from_surface(surface)?;
    let mut err_net = 0.0f64;
    let mut best = bounds_of_net(&net, err_net)?;
    let mut depth = 0usize;
    let mut converged = converged_when(&best);

    while !converged && depth < budget.max_depth {
        let mids_u = Net::span_midpoints(&net.ku, net.pu);
        let mids_v = Net::span_midpoints(&net.kv, net.pv);
        if mids_u.is_empty() && mids_v.is_empty() {
            break; // fp resolution floor — nothing left to split
        }
        let projected = (net.n_u() + mids_u.len()) * (net.n_v() + mids_v.len());
        if projected > budget.max_control_points {
            break; // size budget exhausted — honest stop
        }
        let inserted = mids_u.len() + mids_v.len();
        for m in &mids_u {
            net.insert_knot_u(*m);
        }
        if !mids_v.is_empty() {
            net.transpose();
            for m in &mids_v {
                net.insert_knot_u(*m);
            }
            net.transpose();
        }
        err_net += SWEEP_ROUND_ULPS * EPS * net.max_abs_coord() * inserted as f64;

        match bounds_of_net(&net, err_net) {
            Ok(next) => {
                best = tighten(&best, &next)?;
                depth += 1;
                converged = converged_when(&best);
            }
            Err(EnclosureError::InconsistentBounds { detail }) => {
                return Err(EnclosureError::InconsistentBounds { detail });
            }
            Err(_) => {
                // A refined sweep refused (e.g. conditioning) — the bounds
                // proven so far remain valid theorems; stop honestly.
                break;
            }
        }
    }

    Ok(RefinedBounds {
        bounds: best,
        refinement_depth: depth,
        converged,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::nurbs::NurbsSurface;

    // -------------------------------------------------------------------
    // Fixtures
    // -------------------------------------------------------------------

    /// Bilinear hyperbolic paraboloid S(u,v) = (u, v, u·v) on [0,1]².
    fn hyperbolic_paraboloid() -> NurbsSurface {
        NurbsSurface::new(
            vec![
                vec![Point3::new(0.0, 0.0, 0.0), Point3::new(0.0, 1.0, 0.0)],
                vec![Point3::new(1.0, 0.0, 0.0), Point3::new(1.0, 1.0, 1.0)],
            ],
            vec![vec![1.0, 1.0], vec![1.0, 1.0]],
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
            1,
            1,
        )
        .expect("valid bilinear patch")
    }

    /// Degree (2,1) "bump": S(u,v) = (u, v, 2u(1−u)) — quadratic Bézier in
    /// u with z-control {0, 1, 0}, ruled in v. True z-range is [0, 0.5]
    /// (max at u = 1/2); the depth-0 control hull reports [0, 1].
    fn bump_patch() -> NurbsSurface {
        let col = |x: f64, y: f64, z: f64| Point3::new(x, y, z);
        NurbsSurface::new(
            vec![
                vec![col(0.0, 0.0, 0.0), col(0.0, 1.0, 0.0)],
                vec![col(0.5, 0.0, 1.0), col(0.5, 1.0, 1.0)],
                vec![col(1.0, 0.0, 0.0), col(1.0, 1.0, 0.0)],
            ],
            vec![vec![1.0, 1.0], vec![1.0, 1.0], vec![1.0, 1.0]],
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
            2,
            1,
        )
        .expect("valid bump patch")
    }

    /// Gently wavy degree (2,2) patch, 4×4 net, interior knot 0.5 in each
    /// direction — a non-Bézier multi-span exercise for the property test.
    fn wavy_patch() -> NurbsSurface {
        let z = [
            [0.0, 0.1, 0.2, 0.1],
            [0.1, 0.3, 0.2, 0.0],
            [0.2, 0.2, 0.1, 0.1],
            [0.0, 0.1, 0.3, 0.2],
        ];
        let mut cps = Vec::new();
        for (i, zr) in z.iter().enumerate() {
            let mut row = Vec::new();
            for (j, &zz) in zr.iter().enumerate() {
                row.push(Point3::new(i as f64, j as f64, zz));
            }
            cps.push(row);
        }
        NurbsSurface::new(
            cps,
            vec![vec![1.0; 4]; 4],
            vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0],
            vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0],
            2,
            2,
        )
        .expect("valid wavy patch")
    }

    fn assert_close(a: f64, b: f64, tol: f64, what: &str) {
        assert!((a - b).abs() <= tol, "{what}: {a} vs {b} (tol {tol})");
    }

    // -------------------------------------------------------------------
    // Interval: hand-computed outward arithmetic + honest construction
    // -------------------------------------------------------------------

    #[test]
    fn interval_constructors_reject_non_enclosures() {
        assert!(Interval::point(f64::NAN).is_err());
        assert!(Interval::point(f64::INFINITY).is_err());
        assert!(Interval::enclosing(2.0, 1.0).is_err());
        assert!(Interval::enclosing(f64::NAN, 1.0).is_err());
        assert!(Interval::enclosing(0.0, f64::INFINITY).is_err());
        assert!(Interval::hull_of(std::iter::empty()).is_err());
        assert!(Interval::hull_of([1.0, f64::NAN]).is_err());
        let ok = Interval::enclosing(1.0, 2.0).expect("valid");
        assert_eq!(ok.lo(), 1.0);
        assert_eq!(ok.hi(), 2.0);
    }

    #[test]
    fn interval_add_sub_hand_cases_outward() {
        let a = Interval::enclosing(1.0, 2.0).expect("a");
        let b = Interval::enclosing(3.0, 5.0).expect("b");
        let s = a.add(&b);
        // Encloses the exact real result [4, 7], within one ulp outward.
        assert!(s.lo() <= 4.0 && s.lo() >= 4.0 - 1e-12);
        assert!(s.hi() >= 7.0 && s.hi() <= 7.0 + 1e-12);
        let d = a.sub(&b);
        assert!(d.lo() <= -4.0 && d.hi() >= -1.0);
        assert!(d.lo() >= -4.0 - 1e-12 && d.hi() <= -1.0 + 1e-12);
    }

    /// The classic 0.1 + 0.2 case: the outward-rounded sum must contain
    /// BOTH the true real number 0.30000000000000001665… (witnessed by the
    /// closest double 0.30000000000000004) and the fp literal 0.3 — a
    /// naive equality would fail exactly here.
    #[test]
    fn interval_outward_rounding_covers_the_true_real_sum() {
        let a = Interval::point(0.1).expect("a");
        let b = Interval::point(0.2).expect("b");
        let s = a.add(&b);
        assert!(s.contains(0.1 + 0.2));
        assert!(s.contains(0.3));
    }

    #[test]
    fn interval_mul_crossing_zero_hand_case() {
        let a = Interval::enclosing(-2.0, 3.0).expect("a");
        let b = Interval::enclosing(4.0, 5.0).expect("b");
        let m = a.mul(&b);
        // Exact real result [-10, 15].
        assert!(m.lo() <= -10.0 && m.lo() >= -10.0 - 1e-9);
        assert!(m.hi() >= 15.0 && m.hi() <= 15.0 + 1e-9);
    }

    #[test]
    fn interval_neg_union_intersect() {
        let a = Interval::enclosing(1.0, 3.0).expect("a");
        let n = a.neg();
        assert_eq!(n.lo(), -3.0);
        assert_eq!(n.hi(), -1.0);
        let b = Interval::enclosing(2.0, 5.0).expect("b");
        let u = a.union_hull(&b);
        assert_eq!((u.lo(), u.hi()), (1.0, 5.0));
        let i = a.intersect(&b).expect("overlap");
        assert_eq!((i.lo(), i.hi()), (2.0, 3.0));
        let far = Interval::enclosing(10.0, 11.0).expect("far");
        assert!(matches!(
            a.intersect(&far),
            Err(EnclosureError::InconsistentBounds { .. })
        ));
    }

    // -------------------------------------------------------------------
    // Hand-computed enclosure on a known patch
    // -------------------------------------------------------------------

    /// Every number here is hand-derived from T2/T3, independent of the
    /// production code path. For S = (u, v, uv):
    /// Q^u = {(1,0,0), (1,0,1)}, Q^v = {(0,1,0), (0,1,1)}; the four unit
    /// pairwise crosses are (0,0,1), (0,-1,1)/√2, (-1,0,1)/√2,
    /// (-1,-1,1)/√3; the axis is their normalized sum and the half-angle
    /// the max angle to it.
    #[test]
    fn hyperbolic_paraboloid_hand_computed_bounds() {
        let b = control_net_bounds(&hyperbolic_paraboloid()).expect("bounds");

        // Depth-0 position hull: pure min/max, EXACT.
        assert_eq!((b.position[0].lo(), b.position[0].hi()), (0.0, 1.0));
        assert_eq!((b.position[1].lo(), b.position[1].hi()), (0.0, 1.0));
        assert_eq!((b.position[2].lo(), b.position[2].hi()), (0.0, 1.0));

        // Independent hand computation of the expected cone.
        let r2 = std::f64::consts::SQRT_2;
        let r3 = 3.0f64.sqrt();
        let dirs = [
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(0.0, -1.0 / r2, 1.0 / r2),
            Vector3::new(-1.0 / r2, 0.0, 1.0 / r2),
            Vector3::new(-1.0 / r3, -1.0 / r3, 1.0 / r3),
        ];
        let mut sum = Vector3::new(0.0, 0.0, 0.0);
        for d in &dirs {
            sum = sum + *d;
        }
        let expected_axis = sum.normalize().expect("nonzero");
        let mut expected_half = 0.0f64;
        for d in &dirs {
            expected_half = expected_half.max(d.dot(&expected_axis).clamp(-1.0, 1.0).acos());
        }

        let axis = b.normal.axis();
        assert_close(axis.x, expected_axis.x, 1e-9, "axis.x");
        assert_close(axis.y, expected_axis.y, 1e-9, "axis.y");
        assert_close(axis.z, expected_axis.z, 1e-9, "axis.z");
        // Production half-angle = hand value + (tiny, documented) padding.
        assert!(
            b.normal.half_angle() >= expected_half,
            "half-angle must not be tighter than the hand-computed max angle"
        );
        assert!(
            b.normal.half_angle() <= expected_half + 1e-6,
            "half-angle padding must stay tiny: {} vs {}",
            b.normal.half_angle(),
            expected_half
        );
    }

    // -------------------------------------------------------------------
    // THE property test: the enclosure contains a dense sample of the
    // true surface (sampling as TEST ORACLE, never implementation)
    // -------------------------------------------------------------------

    /// The enclosure is a theorem about TRUE real surface values; the
    /// oracle below evaluates in floating point and carries its own ~ulp
    /// rounding (observed: y = 3.0000000000000004 where the true value is
    /// exactly 3). The oracle therefore tolerates ITS OWN error with this
    /// slack — this pads the test's measurement, never the enclosure.
    const ORACLE_EVAL_SLACK: f64 = 1e-9;

    fn oracle_contains(iv: &Interval, v: f64) -> bool {
        v >= iv.lo() - ORACLE_EVAL_SLACK && v <= iv.hi() + ORACLE_EVAL_SLACK
    }

    fn assert_bounds_contain_dense_samples(surface: &NurbsSurface, b: &ControlNetBounds) {
        const N: usize = 41;
        for iu in 0..N {
            for iv in 0..N {
                let u = iu as f64 / (N - 1) as f64;
                let v = iv as f64 / (N - 1) as f64;
                let sp = surface.evaluate_derivatives(u, v, 1, 1);
                let p = sp.point;
                assert!(
                    oracle_contains(&b.position[0], p.x)
                        && oracle_contains(&b.position[1], p.y)
                        && oracle_contains(&b.position[2], p.z),
                    "sampled point ({}, {}, {}) at (u,v)=({u},{v}) escapes the position \
                     enclosure",
                    p.x,
                    p.y,
                    p.z
                );
                let (Some(du), Some(dv)) = (sp.du, sp.dv) else {
                    panic!("oracle needs first derivatives at ({u},{v})");
                };
                let n = du.cross(&dv);
                assert!(
                    b.normal.contains_direction(&n),
                    "sampled normal {:?} at (u,v)=({u},{v}) escapes the normal cone \
                     (axis {:?}, half-angle {})",
                    n,
                    b.normal.axis(),
                    b.normal.half_angle()
                );
            }
        }
    }

    #[test]
    fn enclosure_contains_dense_true_samples_hyperbolic_paraboloid() {
        let s = hyperbolic_paraboloid();
        let b = control_net_bounds(&s).expect("bounds");
        assert_bounds_contain_dense_samples(&s, &b);
    }

    #[test]
    fn enclosure_contains_dense_true_samples_wavy_patch() {
        let s = wavy_patch();
        let b = control_net_bounds(&s).expect("bounds");
        assert_bounds_contain_dense_samples(&s, &b);
    }

    #[test]
    fn refined_enclosure_still_contains_dense_true_samples() {
        let s = bump_patch();
        let r = refine(
            &s,
            RefineBudget {
                max_depth: 4,
                max_control_points: 4096,
            },
            |_| false,
        )
        .expect("refine");
        assert!(r.refinement_depth >= 3, "budget allows several sweeps");
        assert_bounds_contain_dense_samples(&s, &r.bounds);
    }

    // -------------------------------------------------------------------
    // Refinement: monotone narrowing, convergence, honest budgets
    // -------------------------------------------------------------------

    #[test]
    fn refinement_monotonically_narrows_and_converges_toward_truth() {
        let s = bump_patch();
        // Depth 0: control hull reports z in [0, 1].
        let d0 = control_net_bounds(&s).expect("depth 0");
        assert_eq!((d0.position[2].lo(), d0.position[2].hi()), (0.0, 1.0));

        // Increasing depth budgets with a never-true predicate: widths
        // must narrow monotonically (guaranteed by construction — verify
        // it anyway), and z-hi must approach the true max 0.5 from above.
        let mut prev_width = f64::INFINITY;
        let mut last_hi = f64::INFINITY;
        // Depth 5 keeps the pairwise cross count under PAIR_CAP; depth 6
        // would refuse on size (an honest budget refusal, tested elsewhere).
        for depth in 0..=5usize {
            let r = refine(
                &s,
                RefineBudget {
                    max_depth: depth,
                    max_control_points: 100_000,
                },
                |_| false,
            )
            .expect("refine");
            assert_eq!(r.refinement_depth, depth, "budget fully used");
            assert!(!r.converged, "predicate is never satisfied");
            let w = r.bounds.position[2].width();
            assert!(
                w <= prev_width,
                "z-width must narrow monotonically: {w} after depth {depth}, was {prev_width}"
            );
            // Soundness at every depth: the true max 0.5 stays enclosed.
            assert!(r.bounds.position[2].hi() >= 0.5 - 1e-12);
            prev_width = w;
            last_hi = r.bounds.position[2].hi();
        }
        // Depth 1 alone already tightens strictly (Boehm insertion at 0.5
        // gives z-control values {0, 1/2, 1/2, 0}), and by depth 5 the
        // bound is close to the truth.
        let d1 = refine(
            &s,
            RefineBudget {
                max_depth: 1,
                max_control_points: 100_000,
            },
            |_| false,
        )
        .expect("refine");
        assert!(
            d1.bounds.position[2].hi() < 0.75,
            "one sweep must strictly tighten the z bound, got {}",
            d1.bounds.position[2].hi()
        );
        assert!(
            last_hi <= 0.51,
            "five sweeps should bound the true max 0.5 within 0.01, got {last_hi}"
        );
    }

    #[test]
    fn refinement_predicate_convergence_is_reported() {
        let s = bump_patch();
        let r =
            refine(&s, RefineBudget::STANDARD, |b| b.position[2].width() <= 0.6).expect("refine");
        assert!(r.converged);
        assert!(r.bounds.position[2].width() <= 0.6);
    }

    #[test]
    fn budget_exhaustion_is_honest_reports_bound_not_midpoint() {
        let s = bump_patch();
        let r = refine(
            &s,
            RefineBudget {
                max_depth: 2,
                max_control_points: 100_000,
            },
            |b| b.position[2].width() <= 1e-9, // unreachable in 2 sweeps
        )
        .expect("refine");
        assert!(
            !r.converged,
            "budget exhaustion must report converged: false"
        );
        assert_eq!(r.refinement_depth, 2);
        // The achieved bound is still a valid enclosure of the truth —
        // reported as-is, not collapsed to any single number.
        assert!(r.bounds.position[2].contains(0.5));
        assert!(r.bounds.position[2].contains(0.0));
        assert!(r.bounds.position[2].width() > 1e-9);
    }

    // -------------------------------------------------------------------
    // Mutation proof: the sampled-extreme mutant misses what the
    // enclosure provably covers
    // -------------------------------------------------------------------

    /// Wrong-on-purpose stand-in for the enclosure: a 10×10 evaluation
    /// grid reporting the sampled z-max — the exact defect class the spec
    /// forbids (`ray_surface_numerical`'s grid). Used only to demonstrate,
    /// with raw numbers, that the headline property test would fail under
    /// that mutation.
    fn sampled_extreme_mutant_z_max(surface: &NurbsSurface) -> f64 {
        let mut z_max = f64::NEG_INFINITY;
        for iu in 0..10 {
            for iv in 0..10 {
                let u = iu as f64 / 9.0;
                let v = iv as f64 / 9.0;
                z_max = z_max.max(surface.evaluate(u, v).point.z);
            }
        }
        z_max
    }

    #[test]
    fn mutation_proof_grid_sampling_underestimates_what_enclosure_covers() {
        let s = bump_patch();
        // True z-max is exactly 0.5 at u = 1/2 — which a 10-point grid
        // (u = i/9) never hits; its best is u = 4/9: 2·(4/9)(5/9) = 40/81.
        let true_max = 0.5;
        let mutant = sampled_extreme_mutant_z_max(&s);
        assert_close(mutant, 40.0 / 81.0, 1e-12, "mutant sampled max");
        assert!(
            mutant < true_max - 1e-3,
            "BEFORE (mutant): the sampled extreme {mutant} silently misses the true \
             worst point {true_max} with no error term"
        );
        // AFTER (production): every proven bound contains the true max, at
        // every refinement depth.
        for depth in 0..=5usize {
            let r = refine(
                &s,
                RefineBudget {
                    max_depth: depth,
                    max_control_points: 100_000,
                },
                |_| false,
            )
            .expect("refine");
            assert!(
                r.bounds.position[2].hi() >= true_max,
                "the enclosure must never report a z-hi below the true max: {} at depth \
                 {depth}",
                r.bounds.position[2].hi()
            );
        }
    }

    // -------------------------------------------------------------------
    // Honest refusals
    // -------------------------------------------------------------------

    #[test]
    fn rational_patch_refuses_by_name() {
        // A genuine rational patch: cylinder arc weights are cos(θ/2) ≠ 1.
        let cyl = NurbsSurface::cylinder_patch(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::Z,
            2.0,
            5.0,
            0.0,
            std::f64::consts::PI,
        )
        .expect("cylinder patch");
        match control_net_bounds(&cyl) {
            Err(EnclosureError::RationalUnsupported {
                min_weight,
                max_weight,
            }) => {
                assert!(min_weight < max_weight, "weights genuinely vary");
                assert!(min_weight > 0.0);
            }
            other => panic!("expected RationalUnsupported, got {other:?}"),
        }
        // refine must refuse identically — no path may quietly approximate.
        assert!(matches!(
            refine(&cyl, RefineBudget::STANDARD, |_| false),
            Err(EnclosureError::RationalUnsupported { .. })
        ));
    }

    #[test]
    fn near_parallel_twisted_tangents_refuse() {
        // S_u directions ~(1, 0, 0); S_v directions (1, ±1e-10, 0): the
        // pairwise crosses point in OPPOSED directions (±z), so the normal
        // direction genuinely flips across the patch and no proper cone
        // exists. This is the near-parallel-tangent degeneracy the module
        // docs require to refuse.
        let s = NurbsSurface::new(
            vec![
                vec![Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 1e-10, 0.0)],
                vec![Point3::new(1.0, 0.0, 0.0), Point3::new(2.0, -1e-10, 0.0)],
            ],
            vec![vec![1.0, 1.0], vec![1.0, 1.0]],
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
            1,
            1,
        )
        .expect("valid twisted sliver");
        assert!(matches!(
            control_net_bounds(&s),
            Err(EnclosureError::NormalUnbounded { .. })
        ));
    }

    #[test]
    fn exactly_parallel_tangents_refuse() {
        // S_u ∥ S_v exactly: every pairwise cross vanishes.
        let s = NurbsSurface::new(
            vec![
                vec![Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0)],
                vec![Point3::new(1.0, 0.0, 0.0), Point3::new(2.0, 0.0, 0.0)],
            ],
            vec![vec![1.0, 1.0], vec![1.0, 1.0]],
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
            1,
            1,
        )
        .expect("valid degenerate sliver");
        assert!(matches!(
            control_net_bounds(&s),
            Err(EnclosureError::NormalUnbounded { .. })
        ));
    }

    #[test]
    fn zero_tangent_vector_refuses_by_name() {
        // Adjacent control points coincide in u: Q^u contains an exact
        // zero — the tangent hull touches the origin.
        let s = NurbsSurface::new(
            vec![
                vec![Point3::new(0.0, 0.0, 0.0), Point3::new(0.0, 1.0, 0.0)],
                vec![Point3::new(0.0, 0.0, 0.0), Point3::new(0.0, 1.0, 0.0)],
            ],
            vec![vec![1.0, 1.0], vec![1.0, 1.0]],
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
            1,
            1,
        )
        .expect("valid degenerate patch");
        match control_net_bounds(&s) {
            Err(EnclosureError::ZeroTangentVector { direction, .. }) => {
                assert_eq!(direction, ParamDirection::U);
            }
            other => panic!("expected ZeroTangentVector, got {other:?}"),
        }
    }

    // -------------------------------------------------------------------
    // NormalCone::angle_to — the F3 primitive
    // -------------------------------------------------------------------

    #[test]
    fn cone_angle_interval_encloses_sampled_true_angles() {
        let s = hyperbolic_paraboloid();
        let b = control_net_bounds(&s).expect("bounds");
        let d = Vector3::Z;
        let enclosure = b.normal.angle_to(&d).expect("angle interval");

        // True normal is (−v, −u, 1): angle to +Z is
        // acos(1/√(1+u²+v²)) ∈ [0°, acos(1/√3) ≈ 54.7°].
        let mut sampled_max = 0.0f64;
        for iu in 0..41 {
            for iv in 0..41 {
                let u = iu as f64 / 40.0;
                let v = iv as f64 / 40.0;
                let n = Vector3::new(-v, -u, 1.0);
                let ang = (n.dot(&d) / n.magnitude()).clamp(-1.0, 1.0).acos();
                assert!(
                    enclosure.contains(ang),
                    "true angle {ang} at ({u},{v}) escapes [{}, {}]",
                    enclosure.lo(),
                    enclosure.hi()
                );
                sampled_max = sampled_max.max(ang);
            }
        }
        assert!(enclosure.lo() <= 1e-9, "u=v=0 gives angle 0");
        assert!(
            enclosure.hi() >= sampled_max,
            "enclosure hi must cover the sampled max angle"
        );
        assert!(matches!(
            b.normal.angle_to(&Vector3::new(0.0, 0.0, 0.0)),
            Err(EnclosureError::InvalidReference { .. })
        ));
    }
}
