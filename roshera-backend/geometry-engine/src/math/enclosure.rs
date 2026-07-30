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

/// Absolute convergence floor for the regional engine's RELATIVE mode: an
/// enclosure narrower than this is accepted as converged regardless of
/// its relative width (an interval like `[0, 1e−15]` has relative width
/// ~1 forever — its width is rounding noise, and splitting further only
/// inflates the per-region error balls).
const RELATIVE_MODE_ABS_FLOOR: f64 = 1e-9;

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

    /// Exact absolute-value enclosure: encloses `|x|` for every
    /// `x ∈ self`. Endpoint negation and comparison only — no rounding.
    pub fn abs(&self) -> Self {
        if self.lo >= 0.0 {
            *self
        } else if self.hi <= 0.0 {
            self.neg()
        } else {
            Self {
                lo: 0.0,
                hi: self.hi.max(-self.lo),
            }
        }
    }

    /// Outward enclosure of `sqrt(max(x, 0))` over the interval. The
    /// clamp-to-nonnegative is sound exactly when the TRUE value set is
    /// nonnegative and only the outward-rounded enclosure dips below zero
    /// (the curvature discriminant `(κ₁−κ₂)²/4 = H² − K ≥ 0` is the
    /// intended caller); the clamp then discards only impossible values.
    pub fn sqrt_nonneg(&self) -> Self {
        let lo = self.lo.max(0.0);
        let hi = self.hi.max(0.0);
        Self {
            lo: next_down(lo.sqrt()).max(0.0),
            hi: next_up(hi.sqrt()),
        }
    }

    /// Outward-rounded quotient, defined only when the divisor PROVABLY
    /// excludes zero — a divisor interval containing zero cannot bound a
    /// quotient and is refused, never widened to ±∞ silently. With a
    /// sign-definite divisor the quotient is monotone in each endpoint, so
    /// the four endpoint quotients cover the extreme cases (same argument
    /// as [`Interval::mul`]); a NaN candidate (only possible from a
    /// saturated ±∞ endpoint over a ±∞ divisor endpoint) widens to the
    /// whole line — honest, never narrow.
    pub fn div(&self, other: &Self) -> Result<Self, EnclosureError> {
        if other.lo <= 0.0 && other.hi >= 0.0 {
            return Err(EnclosureError::InvalidInterval {
                detail: format!(
                    "division by an interval containing zero: [{}, {}]",
                    other.lo, other.hi
                ),
            });
        }
        let candidates = [
            self.lo / other.lo,
            self.lo / other.hi,
            self.hi / other.lo,
            self.hi / other.hi,
        ];
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for c in candidates {
            if c.is_nan() {
                return Ok(Self {
                    lo: f64::NEG_INFINITY,
                    hi: f64::INFINITY,
                });
            }
            lo = lo.min(c);
            hi = hi.max(c);
        }
        Ok(Self {
            lo: next_down(lo),
            hi: next_up(hi),
        })
    }

    /// Outward-rounded reciprocal of a PROVABLY POSITIVE interval — the
    /// radius-from-curvature conversion (`r = 1/κ`). A lower endpoint at
    /// or below zero means the value is not proven bounded away from zero
    /// and no finite reciprocal enclosure exists: refused by name, never
    /// approximated.
    pub fn recip_positive(&self) -> Result<Self, EnclosureError> {
        if self.lo <= 0.0 {
            return Err(EnclosureError::InvalidInterval {
                detail: format!(
                    "reciprocal of an interval not provably positive: [{}, {}]",
                    self.lo, self.hi
                ),
            });
        }
        Ok(Self {
            lo: next_down(1.0 / self.hi),
            hi: next_up(1.0 / self.lo),
        })
    }

    /// Enclosure of `cos` over an ANGLE interval whose true values lie in
    /// `[0, π]` (angles between directions — every caller in this module).
    /// Endpoints are clamped into `[0, π]` first (sound: the true angle
    /// set lies there, so clamping discards only impossible values), where
    /// `cos` is monotone decreasing: the enclosure is `[cos hi, cos lo]`,
    /// padded outward by one ulp step plus [`ANGLE_SLACK_RAD`]-scale
    /// transcendental slack, then clamped to `[−1, 1]`.
    fn cos_on_0_pi(&self) -> Self {
        let a = self.lo.clamp(0.0, std::f64::consts::PI);
        let b = self.hi.clamp(0.0, std::f64::consts::PI);
        Self {
            lo: (next_down(b.cos()) - ANGLE_SLACK_RAD).max(-1.0),
            hi: (next_up(a.cos()) + ANGLE_SLACK_RAD).min(1.0),
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

// ---------------------------------------------------------------------------
// Regional subdivision (freeform spec F3–F5) — the gap the F2 report named:
// [`refine`] tightens WHOLE-patch bounds and can therefore never bound a
// patch whose normals genuinely spread past [`MAX_HALF_ANGLE_RAD`]. The
// machinery below subdivides the parameter DOMAIN into regions with
// per-region enclosures, and folds them with a proven extremum theorem:
//
// **T5 — masked-support restriction.** Over a parameter sub-rectangle
// `R = [a,b]×[c,d]`, a tensor-product spline (or any of its derivative
// splines) is a convex combination of ONLY those control coefficients
// whose basis support meets `R` (`N_{i,p}` over `U` lives on
// `[u_i, u_{i+p+1}]`; the k-th derivative basis on `[u_{i+k}, u_{i+p+1}]`);
// the excluded coefficients carry weight exactly 0 on `R` while the
// remaining weights still sum to 1. Every T1–T4 hull/cone argument
// therefore holds verbatim on the masked index set — no sub-net
// extraction, no new rounding. Knot insertion (the same Boehm machinery
// [`refine`] uses) shrinks supports, so masks tighten as knots are added.
//
// **T6 — regional extremum fold.** Let regions `R_1..R_n` PARTITION the
// active domain, each with nonempty interior, and let `[lo_i, hi_i]`
// enclose a continuous quantity `q` over `R_i`. Then
// `sup q ∈ [max_i lo_i, max_i hi_i]` and `inf q ∈ [min_i lo_i, min_i hi_i]`:
// the true per-region suprema `s_i` each lie in `[lo_i, hi_i]` (regions
// are nonempty, so each supremum is attained over actual surface points),
// and `sup q = max_i s_i` — a max of values each confined to its own
// interval. This is how a proven TWO-SIDED enclosure of an extremum is
// obtained WITHOUT sampling: the lower end of the sup-enclosure comes
// from a region's own proven lower bound, never from an evaluated point.
// (The fold is what the F3/F5 analyzers consume; it REQUIRES the face's
// trimmed domain to equal the full active domain — the analyzer proves
// that before calling, else a region might contain no face points and
// `max_i lo_i` would fabricate a violation.)
// ---------------------------------------------------------------------------

/// A closed parameter sub-rectangle of a patch's active domain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SubRegion {
    pub u0: f64,
    pub u1: f64,
    pub v0: f64,
    pub v1: f64,
}

/// One coefficient of a derivative control grid.
#[derive(Debug, Clone, Copy)]
enum DerivCell {
    /// The knot denominator is exactly zero: the coefficient's basis has
    /// empty support and contributes nothing anywhere (T2's exclusion).
    VanishingSupport,
    /// A finite coefficient vector with its proven error-ball radius.
    Value { vec: Vector3, ball: f64 },
}

/// A derivative control grid (first or second order, either direction).
#[derive(Debug, Clone)]
struct DerivGrid {
    /// `cells[i][j]` — `i` indexes u, `j` indexes v, in the coefficient
    /// numbering of the corresponding derivative spline.
    cells: Vec<Vec<DerivCell>>,
}

impl DerivGrid {
    fn rows(&self) -> usize {
        self.cells.len()
    }
    fn cols(&self) -> usize {
        if self.cells.is_empty() {
            0
        } else {
            self.cells[0].len()
        }
    }
}

/// First-derivative grid in u (T2): `Q_ij = pu·(P_{i+1,j} − P_{i,j})/h_i`,
/// `h_i = ku[i+pu+1] − ku[i+1]`, with the same error-ball accounting as
/// the F2 `derivative_net` (module docs, "Directions carry error balls").
fn grid_first_u(net: &Net, coord_err: f64) -> Result<DerivGrid, EnclosureError> {
    let p = net.pu;
    let pf = p as f64;
    let (n_u, n_v) = (net.n_u(), net.n_v());
    let mut cells = Vec::with_capacity(n_u - 1);
    for i in 0..(n_u - 1) {
        let h = net.ku[i + p + 1] - net.ku[i + 1];
        if h == 0.0 {
            cells.push(vec![DerivCell::VanishingSupport; n_v]);
            continue;
        }
        if !(h > 0.0) || !h.is_finite() {
            return Err(EnclosureError::DegeneratePatch {
                detail: format!("invalid knot span width {h} in u derivative"),
            });
        }
        let mut row = Vec::with_capacity(n_v);
        for j in 0..n_v {
            let a = net.pts[i][j];
            let b = net.pts[i + 1][j];
            let q = Vector3::new(
                pf * (b.x - a.x) / h,
                pf * (b.y - a.y) / h,
                pf * (b.z - a.z) / h,
            );
            let r = 2.0 * pf * coord_err / h + 4.0 * EPS * q.magnitude();
            row.push(DerivCell::Value { vec: q, ball: r });
        }
        cells.push(row);
    }
    Ok(DerivGrid { cells })
}

/// First-derivative grid in v — the exact mirror of [`grid_first_u`].
fn grid_first_v(net: &Net, coord_err: f64) -> Result<DerivGrid, EnclosureError> {
    let p = net.pv;
    let pf = p as f64;
    let (n_u, n_v) = (net.n_u(), net.n_v());
    let mut cells = Vec::with_capacity(n_u);
    for i in 0..n_u {
        let mut row = Vec::with_capacity(n_v - 1);
        for j in 0..(n_v - 1) {
            let h = net.kv[j + p + 1] - net.kv[j + 1];
            if h == 0.0 {
                row.push(DerivCell::VanishingSupport);
                continue;
            }
            if !(h > 0.0) || !h.is_finite() {
                return Err(EnclosureError::DegeneratePatch {
                    detail: format!("invalid knot span width {h} in v derivative"),
                });
            }
            let a = net.pts[i][j];
            let b = net.pts[i][j + 1];
            let q = Vector3::new(
                pf * (b.x - a.x) / h,
                pf * (b.y - a.y) / h,
                pf * (b.z - a.z) / h,
            );
            let r = 2.0 * pf * coord_err / h + 4.0 * EPS * q.magnitude();
            row.push(DerivCell::Value { vec: q, ball: r });
        }
        cells.push(row);
    }
    Ok(DerivGrid { cells })
}

/// Difference an existing derivative grid ONCE MORE along u (T2 applied a
/// second time): coefficient `c·(G_{i+1,j} − G_{i,j})/h`, `c` the current
/// u-degree of `grid`'s spline, `h = ku[i+pu+1] − ku[i+2]` (the degree-
/// `pu−1` spline's own T2 denominator over the once-shortened knot
/// vector). A zero `h` implies (by knot monotonicity) that any vanishing
/// neighbour rows are unreachable, so reaching a `VanishingSupport`
/// neighbour with `h > 0` is a defensive impossibility, refused loudly.
fn grid_second_from_u(grid: &DerivGrid, net: &Net) -> Result<DerivGrid, EnclosureError> {
    let p = net.pu;
    let c = (p - 1) as f64;
    let rows = grid.rows();
    let cols = grid.cols();
    let mut cells = Vec::with_capacity(rows - 1);
    for i in 0..(rows - 1) {
        let h = net.ku[i + p + 1] - net.ku[i + 2];
        if h == 0.0 {
            cells.push(vec![DerivCell::VanishingSupport; cols]);
            continue;
        }
        let mut row = Vec::with_capacity(cols);
        for j in 0..cols {
            match (grid.cells[i][j], grid.cells[i + 1][j]) {
                (DerivCell::Value { vec: a, ball: ra }, DerivCell::Value { vec: b, ball: rb }) => {
                    let q = Vector3::new(
                        c * (b.x - a.x) / h,
                        c * (b.y - a.y) / h,
                        c * (b.z - a.z) / h,
                    );
                    let r = c * (ra + rb) / h + 4.0 * EPS * q.magnitude();
                    row.push(DerivCell::Value { vec: q, ball: r });
                }
                _ => {
                    return Err(EnclosureError::DegeneratePatch {
                        detail: "second-derivative differencing reached a vanishing-support \
                                 first-derivative row with a nonzero span — knot vector \
                                 inconsistency"
                            .to_string(),
                    })
                }
            }
        }
        cells.push(row);
    }
    Ok(DerivGrid { cells })
}

/// Difference an existing derivative grid along v with v-degree
/// coefficient `pv` — used for the mixed `S_uv` (differencing the u-grid,
/// whose v-structure is still the ORIGINAL degree-`pv` spline) and for
/// `S_vv` (differencing the v-grid once more, coefficient `pv−1`,
/// `h = kv[j+pv+1] − kv[j+2]`). `coeff`/`lead` select between the two.
fn grid_difference_v(
    grid: &DerivGrid,
    net: &Net,
    coeff: f64,
    lead: usize,
) -> Result<DerivGrid, EnclosureError> {
    let p = net.pv;
    let rows = grid.rows();
    let cols = grid.cols();
    let mut cells = Vec::with_capacity(rows);
    for i in 0..rows {
        let mut row = Vec::with_capacity(cols - 1);
        for j in 0..(cols - 1) {
            let h = net.kv[j + p + 1] - net.kv[j + lead];
            if h == 0.0 {
                row.push(DerivCell::VanishingSupport);
                continue;
            }
            match (grid.cells[i][j], grid.cells[i][j + 1]) {
                (DerivCell::Value { vec: a, ball: ra }, DerivCell::Value { vec: b, ball: rb }) => {
                    let q = Vector3::new(
                        coeff * (b.x - a.x) / h,
                        coeff * (b.y - a.y) / h,
                        coeff * (b.z - a.z) / h,
                    );
                    let r = coeff * (ra + rb) / h + 4.0 * EPS * q.magnitude();
                    row.push(DerivCell::Value { vec: q, ball: r });
                }
                (DerivCell::VanishingSupport, DerivCell::VanishingSupport) => {
                    // The whole u-row vanished (a vanishing u-support row
                    // of the source grid): its v-difference vanishes too.
                    row.push(DerivCell::VanishingSupport);
                }
                _ => {
                    return Err(EnclosureError::DegeneratePatch {
                        detail: "v-differencing mixed a vanishing-support cell with a live \
                                 one under a nonzero span — knot vector inconsistency"
                            .to_string(),
                    })
                }
            }
        }
        cells.push(row);
    }
    Ok(DerivGrid { cells })
}

/// Contiguous index range whose degree-`p` basis (derivative order
/// `order`: support `[knots[i+order], knots[i+p+1]]`) meets the OPEN
/// interval `(a, b)` — T5's mask. Monotone knots make the qualifying set
/// contiguous; `None` when it is empty.
fn support_mask(
    knots: &[f64],
    n_items: usize,
    p: usize,
    order: usize,
    a: f64,
    b: f64,
) -> Option<(usize, usize)> {
    let mut first = None;
    let mut last = None;
    for i in 0..n_items {
        if knots[i + order] < b && knots[i + p + 1] > a {
            if first.is_none() {
                first = Some(i);
            }
            last = Some(i);
        }
    }
    match (first, last) {
        (Some(f), Some(l)) => Some((f, l)),
        _ => None,
    }
}

/// Collect the live (non-vanishing) coefficients of `grid` inside the
/// row/column masks.
fn collect_masked(
    grid: &DerivGrid,
    rows: Option<(usize, usize)>,
    cols: Option<(usize, usize)>,
) -> Vec<(Vector3, f64)> {
    let mut out = Vec::new();
    let (Some((r0, r1)), Some((c0, c1))) = (rows, cols) else {
        return out;
    };
    for i in r0..=r1 {
        for j in c0..=c1 {
            if let DerivCell::Value { vec, ball } = grid.cells[i][j] {
                out.push((vec, ball));
            }
        }
    }
    out
}

/// Proven direction cone of a SINGLE coefficient set (the tangent-cone
/// analogue of [`pairwise_normal_cone`]'s two-pass construction, without
/// the cross products): axis = normalized sum of unit directions,
/// half-angle = max padded angle to that axis. Same refusal lines: an
/// exactly-zero vector, an oversized error-ball ratio, cancelling
/// directions, or a spread beyond [`MAX_HALF_ANGLE_RAD`].
fn direction_cone(values: &[(Vector3, f64)], what: &str) -> Result<NormalCone, EnclosureError> {
    if values.is_empty() {
        return Err(EnclosureError::DegeneratePatch {
            detail: format!("{what}: empty coefficient set — no direction to bound"),
        });
    }
    let unit_dir = |(v, r): &(Vector3, f64)| -> Result<(Vector3, f64), EnclosureError> {
        let mag = v.magnitude();
        if mag == 0.0 {
            return Err(EnclosureError::IllConditionedTangent {
                detail: format!(
                    "{what}: exactly-zero coefficient vector — the hull touches the origin, \
                     so the direction cannot be proven nonvanishing"
                ),
            });
        }
        let ratio = r / mag;
        if ratio > MAX_BALL_RATIO {
            return Err(EnclosureError::IllConditionedTangent {
                detail: format!(
                    "{what}: coefficient of magnitude {mag} carries error ball {r} \
                     (ratio > {MAX_BALL_RATIO})"
                ),
            });
        }
        let inv = 1.0 / mag;
        Ok((Vector3::new(v.x * inv, v.y * inv, v.z * inv), 2.0 * ratio))
    };

    let mut sum = Vector3::new(0.0, 0.0, 0.0);
    for value in values {
        let (dir, _) = unit_dir(value)?;
        sum = sum + dir;
    }
    let axis = sum
        .normalize()
        .map_err(|_| EnclosureError::NormalUnbounded {
            detail: format!(
                "{what}: coefficient directions cancel — the direction set spans \
                             opposing directions"
            ),
        })?;

    let mut half_angle = 0.0f64;
    for value in values {
        let (dir, pad) = unit_dir(value)?;
        let ang = dir.dot(&axis).clamp(-1.0, 1.0).acos() + pad + ANGLE_SLACK_RAD;
        half_angle = half_angle.max(ang);
    }
    if half_angle >= MAX_HALF_ANGLE_RAD {
        return Err(EnclosureError::NormalUnbounded {
            detail: format!(
                "{what}: direction spread requires half-angle {half_angle} rad, beyond the \
                 documented conservative-refusal line {MAX_HALF_ANGLE_RAD} rad"
            ),
        });
    }
    Ok(NormalCone { axis, half_angle })
}

/// Proven enclosure of `|Σ λ_k V_k|` for convex weights λ over the true
/// (ball-inflated) coefficient set, given a proven direction cone:
/// UPPER `max_k(|v_k| + r_k)` (triangle inequality on a convex
/// combination); LOWER `(min_k(|v_k| − r_k)) · cos t` since
/// `|S| ≥ S·axis = Σ λ_k |V_k| cos ∠(V_k, axis) ≥ min_k |V_k| · cos t`
/// (every true vector lies in the cone by its construction, and
/// `r/|v| ≤ 1/2` was enforced there so `|v|−r > 0`). Outward-rounded.
fn magnitude_interval(values: &[(Vector3, f64)], cone: &NormalCone) -> Interval {
    let mut min_m = f64::INFINITY;
    let mut max_m = 0.0f64;
    for (v, r) in values {
        let m = v.magnitude();
        min_m = min_m.min(m - r);
        max_m = max_m.max(m + r);
    }
    let cos_t = next_down(cone.half_angle.cos());
    Interval {
        lo: next_down(min_m * cos_t).max(0.0),
        hi: next_up(max_m),
    }
}

/// Snapshot of the derivative grids for the CURRENT net state, shared by
/// every region evaluation until the next knot insertion.
struct EvalCtx {
    ku: Vec<f64>,
    kv: Vec<f64>,
    pu: usize,
    pv: usize,
    qu: DerivGrid,
    qv: DerivGrid,
    /// `None` unless the engine was asked for second derivatives. Inner
    /// `Option`s are `None` when the corresponding degree is 1 (the second
    /// derivative is identically zero — exact, not approximated).
    second: Option<(Option<DerivGrid>, DerivGrid, Option<DerivGrid>)>,
}

impl EvalCtx {
    fn build(net: &Net, err_net: f64, need_second: bool) -> Result<Self, EnclosureError> {
        let coord_err = err_net + EPS * net.max_abs_coord();
        let qu = grid_first_u(net, coord_err)?;
        let qv = grid_first_v(net, coord_err)?;
        let second = if need_second {
            let quu = if net.pu >= 2 {
                Some(grid_second_from_u(&qu, net)?)
            } else {
                None
            };
            let quv = grid_difference_v(&qu, net, net.pv as f64, 1)?;
            let qvv = if net.pv >= 2 {
                Some(grid_difference_v(&qv, net, (net.pv - 1) as f64, 2)?)
            } else {
                None
            };
            Some((quu, quv, qvv))
        } else {
            None
        };
        Ok(Self {
            ku: net.ku.clone(),
            kv: net.kv.clone(),
            pu: net.pu,
            pv: net.pv,
            qu,
            qv,
            second,
        })
    }

    /// Masked first-derivative coefficients in u over `region` (u-mask at
    /// derivative order 1, v-mask at order 0).
    fn masked_first_u(&self, region: &SubRegion) -> Result<Vec<(Vector3, f64)>, EnclosureError> {
        let rows = support_mask(&self.ku, self.qu.rows(), self.pu, 1, region.u0, region.u1);
        let cols = support_mask(&self.kv, self.qu.cols(), self.pv, 0, region.v0, region.v1);
        let out = collect_masked(&self.qu, rows, cols);
        if out.is_empty() {
            return Err(EnclosureError::IllConditionedTangent {
                detail: format!(
                    "no nonvanishing u-derivative support on region u∈[{}, {}], v∈[{}, {}] — \
                     the u-tangent is identically degenerate there",
                    region.u0, region.u1, region.v0, region.v1
                ),
            });
        }
        Ok(out)
    }

    /// Masked first-derivative coefficients in v over `region`.
    fn masked_first_v(&self, region: &SubRegion) -> Result<Vec<(Vector3, f64)>, EnclosureError> {
        let rows = support_mask(&self.ku, self.qv.rows(), self.pu, 0, region.u0, region.u1);
        let cols = support_mask(&self.kv, self.qv.cols(), self.pv, 1, region.v0, region.v1);
        let out = collect_masked(&self.qv, rows, cols);
        if out.is_empty() {
            return Err(EnclosureError::IllConditionedTangent {
                detail: format!(
                    "no nonvanishing v-derivative support on region u∈[{}, {}], v∈[{}, {}] — \
                     the v-tangent is identically degenerate there",
                    region.u0, region.u1, region.v0, region.v1
                ),
            });
        }
        Ok(out)
    }

    /// Proven enclosure of one second-fundamental-form coefficient
    /// (`L = S_uu·n̂`, `M = S_uv·n̂`, or `N = S_vv·n̂`) over `region`, with
    /// the unit normal ranging over `ncone`. The second-derivative spline
    /// is a convex combination of its masked coefficients (T5), and each
    /// coefficient's dot with any cone direction lies in
    /// `|Q|·cos([α − t, α + t] ∩ [0, π])` padded by the coefficient's
    /// error ball — so the hull of the per-coefficient intervals encloses
    /// the form. An empty mask (or a degree below 2) means the derivative
    /// is identically zero on the region: the exact `[0, 0]`.
    fn second_form_interval(
        &self,
        which: SecondForm,
        region: &SubRegion,
        ncone: &NormalCone,
    ) -> Result<Interval, EnclosureError> {
        let Some((quu, quv, qvv)) = &self.second else {
            return Err(EnclosureError::DegeneratePatch {
                detail: "second-derivative grids were not built for this engine run".to_string(),
            });
        };
        let zero = Interval { lo: 0.0, hi: 0.0 };
        let (grid, u_order, v_order) = match which {
            SecondForm::Uu => match quu {
                None => return Ok(zero),
                Some(g) => (g, 2usize, 0usize),
            },
            SecondForm::Uv => (quv, 1, 1),
            SecondForm::Vv => match qvv {
                None => return Ok(zero),
                Some(g) => (g, 0, 2),
            },
        };
        let rows = support_mask(
            &self.ku,
            grid.rows(),
            self.pu,
            u_order,
            region.u0,
            region.u1,
        );
        let cols = support_mask(
            &self.kv,
            grid.cols(),
            self.pv,
            v_order,
            region.v0,
            region.v1,
        );
        let values = collect_masked(grid, rows, cols);
        if values.is_empty() {
            return Ok(zero);
        }
        let t = ncone.half_angle;
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for (v, r) in values {
            let mag = v.magnitude();
            if mag == 0.0 {
                lo = lo.min(-r);
                hi = hi.max(r);
                continue;
            }
            let alpha = (v.dot(&ncone.axis) / mag).clamp(-1.0, 1.0).acos();
            let ang = Interval {
                lo: (alpha - t - ANGLE_SLACK_RAD).max(0.0),
                hi: (alpha + t + ANGLE_SLACK_RAD).min(std::f64::consts::PI),
            };
            let d = Interval { lo: mag, hi: mag }
                .mul(&ang.cos_on_0_pi())
                .padded(r);
            lo = lo.min(d.lo);
            hi = hi.max(d.hi);
        }
        Ok(Interval { lo, hi })
    }
}

/// Which second-fundamental-form coefficient to enclose.
#[derive(Debug, Clone, Copy)]
enum SecondForm {
    Uu,
    Uv,
    Vv,
}

/// Budget for the regional engine. Exhaustion is an honest outcome
/// (`converged: false` with the achieved fold) when every region is
/// bounded, and a typed refusal when one is not.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionalBudget {
    /// Maximum number of leaf regions the adaptive subdivision may reach.
    pub max_regions: usize,
    /// Maximum control points the refined net may reach (each split may
    /// insert one knot row/column to tighten T5 masks).
    pub max_control_points: usize,
    /// Convergence target on the fold widths — absolute in the measured
    /// quantity's unit, or a fraction of the fold magnitude when
    /// `relative` is set.
    pub target_width: f64,
    /// Interpret `target_width` relative to the fold's magnitude.
    pub relative: bool,
}

impl RegionalBudget {
    /// Documented standard budget for angle enclosures: fold widths to
    /// 0.25° absolute, ≤ 192 regions, ≤ 4096 control points.
    pub const STANDARD_ANGLE: RegionalBudget = RegionalBudget {
        max_regions: 192,
        max_control_points: 4096,
        target_width: 0.25 * std::f64::consts::PI / 180.0,
        relative: false,
    };
    /// Documented standard budget for curvature enclosures: fold widths
    /// to 5% relative, ≤ 192 regions, ≤ 4096 control points.
    pub const STANDARD_CURVATURE: RegionalBudget = RegionalBudget {
        max_regions: 192,
        max_control_points: 4096,
        target_width: 0.05,
        relative: true,
    };
}

/// The regional engine's result: proven enclosures of the quantity's true
/// infimum and supremum over the whole active domain (T6), with the
/// budget actually spent. `converged: false` reports the achieved fold —
/// never a fallback value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionalEnclosure {
    /// Enclosure of the true infimum over the domain.
    pub min_value: Interval,
    /// Enclosure of the true supremum over the domain.
    pub max_value: Interval,
    /// Leaf regions at termination.
    pub regions: usize,
    /// Splits performed (the analyzer-facing refinement depth).
    pub splits: usize,
    /// Whether the fold widths met the budget's target.
    pub converged: bool,
}

/// One leaf of the adaptive subdivision.
struct Leaf {
    region: SubRegion,
    state: Result<Interval, EnclosureError>,
    splittable: bool,
}

/// The adaptive regional engine (T5 + T6): maintains a partition of the
/// active domain, evaluates `eval` per region against the current
/// derivative grids, splits the BINDING region (a refused one first, else
/// the one whose own bound pins the unconverged fold endpoint — splitting
/// any other region provably cannot tighten that endpoint), and inserts a
/// knot at each split point so T5 masks genuinely shrink. Refinement is
/// therefore REGIONAL: a patch whose normals spread past
/// [`MAX_HALF_ANGLE_RAD`] as a whole (which [`refine`] refuses at ANY
/// depth) is bounded piecewise. Termination: every iteration either
/// returns or grows the leaf count toward `max_regions`.
fn regional_enclosure_engine<F>(
    surface: &NurbsSurface,
    budget: RegionalBudget,
    need_second: bool,
    eval: F,
) -> Result<RegionalEnclosure, EnclosureError>
where
    F: Fn(&EvalCtx, &SubRegion) -> Result<Interval, EnclosureError>,
{
    let mut net = net_from_surface(surface)?;
    let mut err_net = 0.0f64;
    let mut ctx = EvalCtx::build(&net, err_net, need_second)?;
    let domain = SubRegion {
        u0: net.ku[net.pu],
        u1: net.ku[net.ku.len() - net.pu - 1],
        v0: net.kv[net.pv],
        v1: net.kv[net.kv.len() - net.pv - 1],
    };
    let first_state = eval(&ctx, &domain);
    let mut leaves = vec![Leaf {
        region: domain,
        state: first_state,
        splittable: true,
    }];
    let mut splits = 0usize;

    // Refusal for a region that can be neither bounded nor split further.
    let stuck = |leaf: &Leaf, splits: usize| -> EnclosureError {
        let inner = match &leaf.state {
            Err(e) => e.to_string(),
            Ok(_) => "internal: stuck leaf with a bound".to_string(),
        };
        EnclosureError::NormalUnbounded {
            detail: format!(
                "regional refinement exhausted on parameter region u∈[{}, {}], v∈[{}, {}] \
                 after {} splits: {}",
                leaf.region.u0, leaf.region.u1, leaf.region.v0, leaf.region.v1, splits, inner
            ),
        }
    };

    loop {
        // ---- fold over the current partition ----
        let mut blocked_splittable: Option<usize> = None;
        let mut blocked_stuck: Option<usize> = None;
        let mut min_lo = f64::INFINITY;
        let mut min_hi = f64::INFINITY;
        let mut max_lo = f64::NEG_INFINITY;
        let mut max_hi = f64::NEG_INFINITY;
        let mut idx_min_lo = usize::MAX;
        let mut idx_max_hi = usize::MAX;
        for (i, leaf) in leaves.iter().enumerate() {
            match &leaf.state {
                Ok(iv) => {
                    if iv.lo < min_lo {
                        min_lo = iv.lo;
                        idx_min_lo = i;
                    }
                    min_hi = min_hi.min(iv.hi);
                    max_lo = max_lo.max(iv.lo);
                    if iv.hi > max_hi {
                        max_hi = iv.hi;
                        idx_max_hi = i;
                    }
                }
                Err(_) => {
                    if leaf.splittable {
                        if blocked_splittable.is_none() {
                            blocked_splittable = Some(i);
                        }
                    } else if blocked_stuck.is_none() {
                        blocked_stuck = Some(i);
                    }
                }
            }
        }
        if let Some(i) = blocked_stuck {
            return Err(stuck(&leaves[i], splits));
        }
        let all_ok = blocked_splittable.is_none();

        let fold = |min_lo: f64, min_hi: f64, max_lo: f64, max_hi: f64| {
            (
                Interval {
                    lo: min_lo,
                    hi: min_hi,
                },
                Interval {
                    lo: max_lo,
                    hi: max_hi,
                },
            )
        };

        let mut needed: Option<usize> = None;
        if all_ok {
            let (min_fold, max_fold) = fold(min_lo, min_hi, max_lo, max_hi);
            // Relative mode carries an ABSOLUTE noise floor: an interval
            // like `[0, 1e−15]` has relative width ~1 no matter how far
            // it is refined (the flat-patch curvature case) — its width is
            // already rounding noise, and further splitting only inflates
            // the per-region error balls. Below the floor, the enclosure
            // is accepted as converged; the floor is far below any
            // physically meaningful curvature/quantity this engine serves.
            let tol = if budget.relative {
                let scale = max_fold
                    .hi
                    .abs()
                    .max(min_fold.lo.abs())
                    .max(f64::MIN_POSITIVE);
                (budget.target_width * scale).max(RELATIVE_MODE_ABS_FLOOR)
            } else {
                budget.target_width
            };
            let max_wide = max_fold.width() > tol;
            let min_wide = min_fold.width() > tol;
            if !max_wide && !min_wide {
                return Ok(RegionalEnclosure {
                    min_value: min_fold,
                    max_value: max_fold,
                    regions: leaves.len(),
                    splits,
                    converged: true,
                });
            }
            // Splitting only the BINDING leaf can tighten a fold endpoint:
            // the max-fold's width is bounded by the width of the leaf
            // attaining max hi (its lo is ≤ the fold's max-lo), and dually.
            needed = if max_wide && leaves[idx_max_hi].splittable {
                Some(idx_max_hi)
            } else if min_wide && leaves[idx_min_lo].splittable {
                Some(idx_min_lo)
            } else {
                // Binding leaves cannot be split further — honest,
                // unconverged fold.
                return Ok(RegionalEnclosure {
                    min_value: min_fold,
                    max_value: max_fold,
                    regions: leaves.len(),
                    splits,
                    converged: false,
                });
            };
        }

        let target_idx = match blocked_splittable.or(needed) {
            Some(i) => i,
            None => {
                // Unreachable by construction (all_ok picked or returned);
                // refuse loudly rather than loop.
                return Err(EnclosureError::DegeneratePatch {
                    detail: "regional engine reached an inconsistent scheduling state".to_string(),
                });
            }
        };

        if leaves.len() >= budget.max_regions {
            if let Some(i) = blocked_splittable {
                return Err(stuck(&leaves[i], splits));
            }
            let (min_fold, max_fold) = fold(min_lo, min_hi, max_lo, max_hi);
            return Ok(RegionalEnclosure {
                min_value: min_fold,
                max_value: max_fold,
                regions: leaves.len(),
                splits,
                converged: false,
            });
        }

        // ---- split the target region ----
        let region = leaves[target_idx].region;
        let interior_knots = |knots: &[f64], a: f64, b: f64| -> usize {
            knots.iter().filter(|&&k| k > a && k < b).count()
        };
        let su = interior_knots(&ctx.ku, region.u0, region.u1);
        let sv = interior_knots(&ctx.kv, region.v0, region.v1);
        let mid_u = 0.5 * (region.u0 + region.u1);
        let mid_v = 0.5 * (region.v0 + region.v1);
        let u_ok = mid_u > region.u0 && mid_u < region.u1;
        let v_ok = mid_v > region.v0 && mid_v < region.v1;
        // Split-direction viability: fp-representable midpoint AND either
        // the net can grow (insertion tightens masks) or existing interior
        // knots already separate the children's masks.
        let can_insert_u = (net.n_u() + 1) * net.n_v() <= budget.max_control_points;
        let can_insert_v = net.n_u() * (net.n_v() + 1) <= budget.max_control_points;
        let u_viable = u_ok && (can_insert_u || su > 0);
        let v_viable = v_ok && (can_insert_v || sv > 0);
        let split_u = if u_viable && v_viable {
            // Prefer the direction with more knot structure to shrink;
            // tie-break on wider parameter extent.
            if su != sv {
                su > sv
            } else {
                (region.u1 - region.u0) >= (region.v1 - region.v0)
            }
        } else if u_viable {
            true
        } else if v_viable {
            false
        } else {
            leaves[target_idx].splittable = false;
            continue;
        };

        let (mid, along_u) = if split_u {
            (mid_u, true)
        } else {
            (mid_v, false)
        };
        let already_knot = if along_u {
            ctx.ku.contains(&mid)
        } else {
            ctx.kv.contains(&mid)
        };
        let can_insert = if along_u { can_insert_u } else { can_insert_v };
        if !already_knot && can_insert {
            if along_u {
                net.insert_knot_u(mid);
            } else {
                net.transpose();
                net.insert_knot_u(mid);
                net.transpose();
            }
            err_net += SWEEP_ROUND_ULPS * EPS * net.max_abs_coord();
            ctx = EvalCtx::build(&net, err_net, need_second)?;
        }

        let (r1, r2) = if along_u {
            (
                SubRegion { u1: mid, ..region },
                SubRegion { u0: mid, ..region },
            )
        } else {
            (
                SubRegion { v1: mid, ..region },
                SubRegion { v0: mid, ..region },
            )
        };
        splits += 1;
        let s1 = eval(&ctx, &r1);
        let s2 = eval(&ctx, &r2);
        leaves[target_idx] = Leaf {
            region: r1,
            state: s1,
            splittable: true,
        };
        leaves.push(Leaf {
            region: r2,
            state: s2,
            splittable: true,
        });
    }
}

/// Proven regional enclosure of the angle between the patch's PARAMETRIC
/// normal direction (`S_u × S_v` order — mapping to a face's OUTWARD
/// normal is the analyzer's job, as with [`ControlNetBounds::normal`])
/// and the fixed `direction`, over the patch's full active domain:
/// `min_value`/`max_value` enclose the true infimum/supremum of the angle
/// (T6). This is the F3 entry point, and the fix for the F2 gap: each
/// region gets its OWN normal cone, so wide-normal-spread patches are
/// bounded piecewise instead of refused forever.
pub fn regional_angle_enclosure(
    surface: &NurbsSurface,
    direction: &Vector3,
    budget: RegionalBudget,
) -> Result<RegionalEnclosure, EnclosureError> {
    let unit = direction
        .normalize()
        .map_err(|_| EnclosureError::InvalidReference {
            detail: "direction has zero length".to_string(),
        })?;
    regional_enclosure_engine(surface, budget, false, |ctx, region| {
        let qu = ctx.masked_first_u(region)?;
        let qv = ctx.masked_first_v(region)?;
        let cone = pairwise_normal_cone(&qu, &qv)?;
        cone.angle_to(&unit)
    })
}

/// Proven regional enclosure of the patch's maximum absolute normal
/// curvature `max(|κ₁|, |κ₂|)` over the full active domain (F5). Per
/// region: first-derivative cones and magnitude intervals bound the first
/// fundamental form `E, F, G`; T2-applied-twice second-derivative nets
/// with the region's normal cone bound `L, M, N` (each form coefficient
/// is a convex combination of masked net coefficients — T5); then the
/// closed-form principal-curvature eigenvalues
/// `κ = H ± √(H² − K)`, `H = (EN − 2FM + GL)/(2(EG − F²))`,
/// `K = (LN − M²)/(EG − F²)` are evaluated in outward-rounded interval
/// arithmetic, giving `max|κ| ∈ |H| + √(max(H² − K, 0))`. A region whose
/// `EG − F²` cannot be proven positive refuses (near-degenerate
/// parameterization) and is subdivided; consume `max_value` for the
/// minimum-radius question (`r_min = 1/κ_max`).
pub fn regional_max_abs_curvature(
    surface: &NurbsSurface,
    budget: RegionalBudget,
) -> Result<RegionalEnclosure, EnclosureError> {
    regional_enclosure_engine(surface, budget, true, |ctx, region| {
        let qu = ctx.masked_first_u(region)?;
        let qv = ctx.masked_first_v(region)?;
        let cone_u = direction_cone(&qu, "u-tangent")?;
        let cone_v = direction_cone(&qv, "v-tangent")?;
        let ncone = pairwise_normal_cone(&qu, &qv)?;
        let mu = magnitude_interval(&qu, &cone_u);
        let mv = magnitude_interval(&qv, &cone_v);
        let e = mu.mul(&mu);
        let g = mv.mul(&mv);
        let gamma = cone_u.axis.dot(&cone_v.axis).clamp(-1.0, 1.0).acos();
        let ang = Interval {
            lo: (gamma - cone_u.half_angle - cone_v.half_angle - ANGLE_SLACK_RAD).max(0.0),
            hi: (gamma + cone_u.half_angle + cone_v.half_angle + ANGLE_SLACK_RAD)
                .min(std::f64::consts::PI),
        };
        let f = mu.mul(&mv).mul(&ang.cos_on_0_pi());
        let l = ctx.second_form_interval(SecondForm::Uu, region, &ncone)?;
        let m = ctx.second_form_interval(SecondForm::Uv, region, &ncone)?;
        let n = ctx.second_form_interval(SecondForm::Vv, region, &ncone)?;
        let w = e.mul(&g).sub(&f.mul(&f));
        if w.lo <= 0.0 {
            return Err(EnclosureError::IllConditionedTangent {
                detail: format!(
                    "first fundamental form not provably nondegenerate on region \
                     u∈[{}, {}], v∈[{}, {}] (EG−F² ∈ [{}, {}])",
                    region.u0, region.u1, region.v0, region.v1, w.lo, w.hi
                ),
            });
        }
        let two = Interval { lo: 2.0, hi: 2.0 };
        let h_num = e.mul(&n).sub(&two.mul(&f).mul(&m)).add(&g.mul(&l));
        let h = h_num.div(&two.mul(&w))?;
        let k = l.mul(&n).sub(&m.mul(&m)).div(&w)?;
        let disc = h.mul(&h).sub(&k);
        let kappa = h.abs().add(&disc.sqrt_nonneg());
        // The true max|κ| is nonnegative; clamping the outward-rounded
        // lower endpoint discards only impossible values.
        Ok(Interval {
            lo: kappa.lo.max(0.0),
            hi: kappa.hi,
        })
    })
}

// ---------------------------------------------------------------------------
// Footprint bounds (freeform spec F4) — projections of a patch onto a
// wall frame, with a PROVEN inner rectangle:
//
// **T7 — winding footprint theorem.** Let `F = π ∘ S` be the patch
// composed with the orthogonal projection onto a plane spanned by
// orthonormal `(e1, e2)`. Suppose both knot vectors are CLAMPED (so the
// four boundary isolines are the boundary control rows/columns and the
// patch interpolates its corner control points), and the four projected
// boundary-row hulls ("bands") satisfy: one opposite pair is strictly
// separated along `e1`, the other pair strictly separated along `e2`.
// Let `R` be the open rectangle between them. Then every `p ∈ R` lies in
// the projected footprint `F([0,1]²)`:
//
// - `p` is not in the projected boundary (each boundary curve lies in its
//   band by the hull property, and every band avoids `R`);
// - the ray from `p` along `+e1` crosses only the right band's curve,
//   whose endpoints are the two right-side corner points (interpolated,
//   hence inside the bottom/top bands, i.e. strictly below/above `p`), so
//   its crossing parity — and hence the boundary loop's winding parity
//   around `p` — is odd;
// - if `p` were NOT in `F([0,1]²)`, `F` restricted to shrinking boundary
//   loops would null-homotope `F|∂` inside `R² \ {p}`, forcing winding 0 —
//   contradiction. Hence `p ∈ F([0,1]²)`.
//
// The theorem is stated for the FULL patch domain: a caller pairing
// trimmed faces must first prove the face's trim covers the full domain.
// ---------------------------------------------------------------------------

/// Conservative fp padding factor for projection hulls (documented
/// generous over-count of the dot-product rounding, scaled by the point
/// magnitudes involved).
const PROJECTION_PAD_ULPS: f64 = 16.0;

/// Outward-rounded enclosure of `(S(u,v) − origin) · axis` over the WHOLE
/// patch (T1 + linearity of projection): the control-point projections'
/// hull, padded by the documented fp budget. `axis` is normalized here;
/// a zero axis refuses.
pub fn patch_projection_interval(
    surface: &NurbsSurface,
    origin: &Point3,
    axis: &Vector3,
) -> Result<Interval, EnclosureError> {
    let unit = axis
        .normalize()
        .map_err(|_| EnclosureError::InvalidReference {
            detail: "projection axis has zero length".to_string(),
        })?;
    let net = net_from_surface(surface)?;
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let mut scale = 1.0f64;
    for row in &net.pts {
        for p in row {
            let rel = *p - *origin;
            let d = rel.dot(&unit);
            lo = lo.min(d);
            hi = hi.max(d);
            scale = scale.max(rel.magnitude());
        }
    }
    Ok(Interval { lo, hi }.padded(PROJECTION_PAD_ULPS * EPS * scale))
}

/// The projected OUTER hull rectangle of the patch on the `(e1, e2)`
/// frame — a NECESSARY bound (footprint ⊆ rectangle): can prove absence
/// of overlap, never presence. Returned as `(s, t)` enclosures.
pub fn footprint_outer_rectangle(
    surface: &NurbsSurface,
    origin: &Point3,
    e1: &Vector3,
    e2: &Vector3,
) -> Result<(Interval, Interval), EnclosureError> {
    Ok((
        patch_projection_interval(surface, origin, e1)?,
        patch_projection_interval(surface, origin, e2)?,
    ))
}

/// A PROVEN inner rectangle of the patch's projected footprint (T7):
/// `Some([s_lo, s_hi, t_lo, t_hi])` is an OPEN rectangle in the
/// `(e1, e2)` frame every interior point of which provably lies in the
/// projection of the FULL patch. `None` is an honest "not provable by
/// this construction" (unclamped knots, or bands that do not separate) —
/// never a guess. Requires the caller to have proven the face's trim
/// covers the full patch domain before treating footprint membership as
/// face membership.
pub fn footprint_inner_rectangle(
    surface: &NurbsSurface,
    origin: &Point3,
    e1: &Vector3,
    e2: &Vector3,
) -> Result<Option<[f64; 4]>, EnclosureError> {
    let u1 = e1
        .normalize()
        .map_err(|_| EnclosureError::InvalidReference {
            detail: "frame axis e1 has zero length".to_string(),
        })?;
    let u2 = e2
        .normalize()
        .map_err(|_| EnclosureError::InvalidReference {
            detail: "frame axis e2 has zero length".to_string(),
        })?;
    let net = net_from_surface(surface)?;

    // Clamped knot vectors: boundary isolines are the boundary control
    // rows/columns and corners are interpolated — both load-bearing for
    // T7. Exact equality, as with the constant-weight proof.
    let clamped = |knots: &[f64], p: usize| -> bool {
        let n = knots.len();
        knots[..=p].iter().all(|&k| k == knots[0])
            && knots[n - p - 1..].iter().all(|&k| k == knots[n - 1])
    };
    if !clamped(&net.ku, net.pu) || !clamped(&net.kv, net.pv) {
        return Ok(None);
    }

    // Projected hull rectangle of one boundary band, padded outward.
    let band = |pts: Vec<Point3>| -> [f64; 4] {
        let mut s_lo = f64::INFINITY;
        let mut s_hi = f64::NEG_INFINITY;
        let mut t_lo = f64::INFINITY;
        let mut t_hi = f64::NEG_INFINITY;
        let mut scale = 1.0f64;
        for p in &pts {
            let rel = *p - *origin;
            let s = rel.dot(&u1);
            let t = rel.dot(&u2);
            s_lo = s_lo.min(s);
            s_hi = s_hi.max(s);
            t_lo = t_lo.min(t);
            t_hi = t_hi.max(t);
            scale = scale.max(rel.magnitude());
        }
        let pad = PROJECTION_PAD_ULPS * EPS * scale;
        [s_lo - pad, s_hi + pad, t_lo - pad, t_hi + pad]
    };

    let n_u = net.n_u();
    let n_v = net.n_v();
    let row_first = band(net.pts[0].clone());
    let row_last = band(net.pts[n_u - 1].clone());
    let col_first = band((0..n_u).map(|i| net.pts[i][0]).collect());
    let col_last = band((0..n_u).map(|i| net.pts[i][n_v - 1]).collect());

    // Try both axis assignments: (row pair along s, col pair along t) and
    // the transpose. Band rect layout: [s_lo, s_hi, t_lo, t_hi].
    let try_axes = |x_pair: (&[f64; 4], &[f64; 4]),
                    y_pair: (&[f64; 4], &[f64; 4]),
                    x_idx: (usize, usize),
                    y_idx: (usize, usize)|
     -> Option<[f64; 4]> {
        // Order each pair along its axis; require strict separation.
        let (x_low, x_high) = if x_pair.0[x_idx.1] < x_pair.1[x_idx.0] {
            (x_pair.0, x_pair.1)
        } else if x_pair.1[x_idx.1] < x_pair.0[x_idx.0] {
            (x_pair.1, x_pair.0)
        } else {
            return None;
        };
        let (y_low, y_high) = if y_pair.0[y_idx.1] < y_pair.1[y_idx.0] {
            (y_pair.0, y_pair.1)
        } else if y_pair.1[y_idx.1] < y_pair.0[y_idx.0] {
            (y_pair.1, y_pair.0)
        } else {
            return None;
        };
        let s_lo = x_low[x_idx.1];
        let s_hi = x_high[x_idx.0];
        let t_lo = y_low[y_idx.1];
        let t_hi = y_high[y_idx.0];
        if s_lo < s_hi && t_lo < t_hi {
            Some([s_lo, s_hi, t_lo, t_hi])
        } else {
            None
        }
    };

    // Assignment 1: u-boundary rows separated along s (indices 0/1),
    // v-boundary columns along t (indices 2/3).
    if let Some(r) = try_axes(
        (&row_first, &row_last),
        (&col_first, &col_last),
        (0, 1),
        (2, 3),
    ) {
        return Ok(Some(r));
    }
    // Assignment 2: rows along t, columns along s — returned in the SAME
    // [s_lo, s_hi, t_lo, t_hi] layout.
    if let Some(r) = try_axes(
        (&col_first, &col_last),
        (&row_first, &row_last),
        (0, 1),
        (2, 3),
    ) {
        return Ok(Some(r));
    }
    Ok(None)
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

    // -------------------------------------------------------------------
    // F3–F5 machinery: new Interval ops
    // -------------------------------------------------------------------

    #[test]
    fn interval_abs_sqrt_div_recip_hand_cases() {
        let neg = Interval::enclosing(-3.0, -1.0).expect("neg");
        let cross = Interval::enclosing(-2.0, 5.0).expect("cross");
        let pos = Interval::enclosing(4.0, 9.0).expect("pos");

        assert_eq!((neg.abs().lo(), neg.abs().hi()), (1.0, 3.0));
        assert_eq!((cross.abs().lo(), cross.abs().hi()), (0.0, 5.0));
        assert_eq!((pos.abs().lo(), pos.abs().hi()), (4.0, 9.0));

        let r = pos.sqrt_nonneg();
        assert!(r.contains(2.0) && r.contains(3.0));
        assert!(r.lo() <= 2.0 && r.hi() >= 3.0);
        // Clamp arm: an outward-dipped negative lower endpoint roots to 0.
        let dipped = Interval::enclosing(-1e-12, 4.0).expect("dipped");
        assert_eq!(dipped.sqrt_nonneg().lo(), 0.0);

        // Division by a zero-containing interval is a refusal, never ±∞.
        assert!(pos.div(&cross).is_err());
        let q = Interval::enclosing(2.0, 6.0)
            .expect("num")
            .div(&Interval::enclosing(1.0, 2.0).expect("den"))
            .expect("sign-definite divisor");
        assert!(q.contains(1.0) && q.contains(6.0));
        assert!(q.lo() <= 1.0 && q.hi() >= 6.0);

        // Reciprocal requires PROVEN positivity.
        assert!(cross.recip_positive().is_err());
        assert!(Interval::enclosing(0.0, 1.0)
            .expect("touching zero")
            .recip_positive()
            .is_err());
        let rp = pos.recip_positive().expect("positive");
        assert!(rp.contains(1.0 / 4.0) && rp.contains(1.0 / 9.0));
    }

    // -------------------------------------------------------------------
    // Regional subdivision (T5/T6): THE GAP — global refine refuses
    // forever on a wide-normal-spread patch; regional bounds it.
    // -------------------------------------------------------------------

    /// Degree (2,1) steep bump S(u,v) = (u, v, 40u(1−u)) (z-control
    /// (0, 20, 0)): slope z′ = 40(1−2u) sweeps [−40, 40], so the normal's
    /// angle to +Z sweeps exactly [0°, atan(40) ≈ 88.568°] — a total
    /// spread ≈ 177°, far beyond a single proper cone.
    fn steep_bump() -> NurbsSurface {
        NurbsSurface::new(
            vec![
                vec![Point3::new(0.0, 0.0, 0.0), Point3::new(0.0, 1.0, 0.0)],
                vec![Point3::new(0.5, 0.0, 20.0), Point3::new(0.5, 1.0, 20.0)],
                vec![Point3::new(1.0, 0.0, 0.0), Point3::new(1.0, 1.0, 0.0)],
            ],
            vec![vec![1.0; 2]; 3],
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
            2,
            1,
        )
        .expect("valid steep bump")
    }

    #[test]
    fn regional_engine_bounds_what_global_refine_refuses_forever() {
        let s = steep_bump();

        // BEFORE (the F2 gap, demonstrated live): the WHOLE-patch cone
        // refuses, and global refinement can never help — the whole-patch
        // normal spread does not shrink under knot insertion.
        assert!(
            matches!(
                control_net_bounds(&s),
                Err(EnclosureError::NormalUnbounded { .. })
            ),
            "whole-patch cone must refuse on a ~177° normal spread"
        );
        assert!(
            matches!(
                refine(&s, RefineBudget::STANDARD, |_| false),
                Err(EnclosureError::NormalUnbounded { .. })
            ),
            "global refine must refuse at any depth — this is the gap regional \
             subdivision exists to close"
        );

        // AFTER: the regional engine bounds it, tightly.
        let re = regional_angle_enclosure(&s, &Vector3::Z, RegionalBudget::STANDARD_ANGLE)
            .expect("regional enclosure succeeds where global refuses");
        let true_max = 40.0f64.atan(); // ≈ 1.5458 rad ≈ 88.568°
        assert!(
            re.min_value.contains(0.0),
            "min-angle enclosure must contain the true 0: [{}, {}]",
            re.min_value.lo(),
            re.min_value.hi()
        );
        assert!(
            re.max_value.contains(true_max),
            "max-angle enclosure must contain atan(40): [{}, {}]",
            re.max_value.lo(),
            re.max_value.hi()
        );
        assert!(re.converged, "standard budget should converge: {re:?}");

        // T6's load-bearing endpoint (the mutation target): the sup
        // enclosure's LOWER end is a proven lower bound on the true
        // maximum — the thing a sampler can only guess at and a
        // whole-patch bound (lo = 0) could never prove. A rule with an
        // 80° threshold is PROVABLY violated from this alone.
        assert!(
            re.max_value.lo() > 80.0f64.to_radians(),
            "sup-fold lower endpoint must prove the >80° violation, got {} rad",
            re.max_value.lo()
        );

        // TEST ORACLE (sampling is legitimate here and only here): every
        // densely-sampled true angle lies inside the union of the fold
        // envelopes, the sampled max is inside the sup enclosure, and the
        // sampled min inside the inf enclosure.
        let mut sampled_min = f64::INFINITY;
        let mut sampled_max = f64::NEG_INFINITY;
        for iu in 0..=400 {
            let u = iu as f64 / 400.0;
            // True normal of (u, v, z(u)): (−z′(u), 0, 1).
            let zp = 40.0 * (1.0 - 2.0 * u);
            let n = Vector3::new(-zp, 0.0, 1.0);
            let ang = (n.dot(&Vector3::Z) / n.magnitude()).clamp(-1.0, 1.0).acos();
            assert!(
                ang >= re.min_value.lo() - 1e-9 && ang <= re.max_value.hi() + 1e-9,
                "sampled angle {ang} at u={u} escapes the fold envelope"
            );
            sampled_min = sampled_min.min(ang);
            sampled_max = sampled_max.max(ang);
        }
        assert!(
            re.max_value.contains(sampled_max) || sampled_max <= re.max_value.hi(),
            "sampled max must not exceed the sup enclosure"
        );
        assert!(sampled_max >= re.max_value.lo() - 1e-9);
        assert!(sampled_min <= re.min_value.hi() + 1e-9);
    }

    // -------------------------------------------------------------------
    // F5: curvature enclosure
    // -------------------------------------------------------------------

    /// Degree (2,1) parabolic cylinder S(u,v) = (u, v, u²) — principal
    /// curvatures are κ₁(u) = 2/(1+4u²)^{3/2} (max 2 at u = 0) and
    /// κ₂ = 0, so the true max|κ| over the patch is exactly 2.
    fn parabolic_cylinder() -> NurbsSurface {
        NurbsSurface::new(
            vec![
                vec![Point3::new(0.0, 0.0, 0.0), Point3::new(0.0, 1.0, 0.0)],
                vec![Point3::new(0.5, 0.0, 0.0), Point3::new(0.5, 1.0, 0.0)],
                vec![Point3::new(1.0, 0.0, 1.0), Point3::new(1.0, 1.0, 1.0)],
            ],
            vec![vec![1.0; 2]; 3],
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
            2,
            1,
        )
        .expect("valid parabolic cylinder")
    }

    #[test]
    fn regional_curvature_encloses_parabolic_cylinder_max() {
        let s = parabolic_cylinder();
        let re = regional_max_abs_curvature(&s, RegionalBudget::STANDARD_CURVATURE)
            .expect("curvature enclosure");
        assert!(
            re.max_value.contains(2.0),
            "max|κ| enclosure must contain the true 2.0: [{}, {}]",
            re.max_value.lo(),
            re.max_value.hi()
        );
        assert!(
            re.max_value.lo() > 0.0,
            "curvature must be proven bounded away from zero here"
        );

        // TEST ORACLE: κ(u) = 2/(1+4u²)^{3/2} sampled densely — every
        // value must sit at/below the sup enclosure's ceiling, and the
        // sampled max at/above its floor.
        let mut sampled_max = 0.0f64;
        for iu in 0..=400 {
            let u = iu as f64 / 400.0;
            let kappa = 2.0 / (1.0 + 4.0 * u * u).powf(1.5);
            assert!(
                kappa <= re.max_value.hi() + 1e-9,
                "sampled κ {kappa} at u={u} exceeds the ceiling {}",
                re.max_value.hi()
            );
            sampled_max = sampled_max.max(kappa);
        }
        assert!(
            sampled_max >= re.max_value.lo() - 1e-9,
            "sup-fold floor {} must not exceed the sampled max {sampled_max}",
            re.max_value.lo()
        );
        // The radius view: min radius over the patch is 1/2.
        let radius = re.max_value.recip_positive().expect("κ proven positive");
        assert!(
            radius.contains(0.5),
            "min-radius enclosure must contain 0.5: [{}, {}]",
            radius.lo(),
            radius.hi()
        );
    }

    /// A flat patch's curvature enclosure must include 0 and REFUSE the
    /// reciprocal — the honest "no radius ceiling" outcome, never a
    /// fabricated finite radius.
    #[test]
    fn flat_patch_curvature_contains_zero_and_radius_refuses() {
        let s = NurbsSurface::new(
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
        .expect("flat patch");
        let re = regional_max_abs_curvature(&s, RegionalBudget::STANDARD_CURVATURE)
            .expect("flat patch bounds");
        assert!(
            re.max_value.contains(0.0),
            "flat patch max|κ| must contain 0: [{}, {}]",
            re.max_value.lo(),
            re.max_value.hi()
        );
        assert!(
            re.max_value.recip_positive().is_err(),
            "a curvature not proven nonzero must refuse the radius reciprocal"
        );
    }

    // -------------------------------------------------------------------
    // F4: projection + footprint (T7)
    // -------------------------------------------------------------------

    /// Wavy wall over x ∈ [0,3], y ∈ [0,3] at height ~z0: x = 3u exactly
    /// (control (0, 1.5, 3) is linear), y = 3v, z-control
    /// (z0, z0 + amp, z0) so the true z sweeps [z0, z0 + amp/2].
    fn wavy_wall_patch(z0: f64, amp: f64) -> NurbsSurface {
        NurbsSurface::new(
            vec![
                vec![Point3::new(0.0, 0.0, z0), Point3::new(0.0, 3.0, z0)],
                vec![
                    Point3::new(1.5, 0.0, z0 + amp),
                    Point3::new(1.5, 3.0, z0 + amp),
                ],
                vec![Point3::new(3.0, 0.0, z0), Point3::new(3.0, 3.0, z0)],
            ],
            vec![vec![1.0; 2]; 3],
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
            2,
            1,
        )
        .expect("valid wavy wall")
    }

    #[test]
    fn projection_interval_encloses_true_range_via_hull() {
        let s = wavy_wall_patch(1.0, 0.1);
        let proj = patch_projection_interval(&s, &Point3::new(0.0, 0.0, 0.0), &Vector3::Z)
            .expect("projection");
        // True z-range is [1.0, 1.05]; the control hull proves [1.0, 1.1].
        assert!(proj.contains(1.0) && proj.contains(1.05));
        assert!(proj.lo() <= 1.0 && proj.hi() >= 1.05 && proj.hi() <= 1.11);
        // Zero axis refuses.
        assert!(patch_projection_interval(
            &s,
            &Point3::new(0.0, 0.0, 0.0),
            &Vector3::new(0.0, 0.0, 0.0)
        )
        .is_err());
    }

    #[test]
    fn footprint_inner_rectangle_proves_central_region() {
        let s = wavy_wall_patch(0.0, 0.1);
        let origin = Point3::new(0.0, 0.0, 0.0);
        let inner = footprint_inner_rectangle(&s, &origin, &Vector3::X, &Vector3::Y)
            .expect("well-formed patch")
            .expect("bands separate — inner rectangle must be provable");
        let [s_lo, s_hi, t_lo, t_hi] = inner;
        // The true footprint is exactly [0,3]²; the proven inner rectangle
        // must be a (near-full) subset of it.
        assert!(s_lo >= -1e-9 && s_lo < 0.1, "s_lo = {s_lo}");
        assert!(s_hi <= 3.0 + 1e-9 && s_hi > 2.9, "s_hi = {s_hi}");
        assert!(t_lo >= -1e-9 && t_lo < 0.1, "t_lo = {t_lo}");
        assert!(t_hi <= 3.0 + 1e-9 && t_hi > 2.9, "t_hi = {t_hi}");

        // Inner ⊆ outer (sanity between the two bounds).
        let (os, ot) =
            footprint_outer_rectangle(&s, &origin, &Vector3::X, &Vector3::Y).expect("outer");
        assert!(os.lo() <= s_lo && os.hi() >= s_hi);
        assert!(ot.lo() <= t_lo && ot.hi() >= t_hi);
    }

    /// A patch folded back on itself in u (first and last control rows
    /// coincide) has no separated band pair — the inner rectangle is
    /// honestly unprovable, never guessed.
    #[test]
    fn folded_patch_footprint_inner_rectangle_is_unprovable() {
        let s = NurbsSurface::new(
            vec![
                vec![Point3::new(0.0, 0.0, 0.0), Point3::new(0.0, 3.0, 0.0)],
                vec![Point3::new(3.0, 0.0, 1.0), Point3::new(3.0, 3.0, 1.0)],
                vec![Point3::new(0.0, 0.0, 2.0), Point3::new(0.0, 3.0, 2.0)],
            ],
            vec![vec![1.0; 2]; 3],
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
            2,
            1,
        )
        .expect("folded patch");
        let inner =
            footprint_inner_rectangle(&s, &Point3::new(0.0, 0.0, 0.0), &Vector3::X, &Vector3::Y)
                .expect("well-formed patch");
        assert!(
            inner.is_none(),
            "coincident opposite bands must make the inner rectangle unprovable"
        );
    }
}
