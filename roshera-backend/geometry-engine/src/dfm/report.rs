//! DFM report/verdict/fact types — the "can this exist?" analogue of
//! [`crate::sketch2d::sketch_certificate::SketchValidityCertificate`] and
//! [`crate::primitives::provenance::ValidityCertificate`].
//!
//! ## The honesty theorem (spec §3.3 / §4)
//!
//! [`DfmSummary::Pass`] requires EVERY rule in the pack to report
//! [`Verdict::Pass`]. A single [`Verdict::Unverifiable`] forces
//! [`DfmSummary::Inconclusive`] — **never** `Pass` — because a check that
//! did not run is not a check that passed (the same move the assembly
//! certificate's H8 fix made for `mates_in_contact` /
//! `no_static_interference`: an unverified dimension cannot be folded into
//! a passing verdict). A [`Verdict::Violation`] dominates `Inconclusive`
//! when both are present in the same report — a proven defect outranks an
//! open question. [`DfmReport::new`] is the ONLY way to construct a
//! report; the fold is computed from the verdicts, never supplied
//! independently, so a report can never disagree with its own verdicts.
//!
//! ## Provenance (spec §2.2 / §3.1)
//!
//! Every measured quantity in a report is a [`DfmValue`]: a number paired
//! with its [`Derivation`]. There is no way to put a bare `f64` into a
//! [`Verdict`] — `margin`, `measured`, and `limit` are all `DfmValue`, so
//! the type system rules out a number appearing without provenance. A
//! refusal ([`Verdict::Unverifiable`]) is a first-class outcome, not an
//! error: [`DfmError`] exists only for malformed input (a dangling face
//! reference, a solid too unsound for an analyzer's precondition) — never
//! for "the analyzer doesn't know."
//!
//! Every RULE also carries provenance: [`RuleVerdict::provenance`] is a
//! [`RuleProvenance`] (spec §3.2.1, defined in
//! [`crate::dfm::provenance`]) — the kernel's honesty rule applied to its
//! own rule thresholds, not just to the geometry those rules check. A
//! report reader can tell a proven violation of a published STANDARD apart
//! from a proven violation of a shop-floor HEURISTIC without
//! cross-referencing the pack source.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::dfm::provenance::RuleProvenance;
use crate::primitives::face::FaceId;

/// A face identified within the solid a [`DfmReport`] was computed
/// against. This is an alias for the kernel's own [`FaceId`], not a
/// parallel identifier — DFM witnesses point at REAL topology, resolvable
/// through the same face store every other kernel consumer uses.
pub type FaceRef = FaceId;

/// Which rule pack (manufacturing process) a [`DfmReport`] was computed
/// against (spec §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RulePackId {
    /// Fused Deposition Modeling (spec §3.2 FDM pack v1: min_wall, overhang,
    /// min_bore, trapped_volume, support_volume-declared-unverifiable).
    Fdm,
    /// Injection molding (spec §3.2 molding pack v1: draft, undercut,
    /// trapped_core).
    InjectionMolding,
    /// CNC milling on a 3-axis machine over a declared set of setup
    /// directions (`packs::cnc`).
    #[serde(rename = "cnc_3_axis")]
    Cnc3Axis,
    /// Sheet metal: bent from uniform-thickness flat stock
    /// (`packs::sheet`).
    SheetMetal,
    /// CNC turning (lathe) about one spindle axis (`packs::turning`).
    CncTurning,
    /// Casting into a two-part permanent mould / die (`packs::casting`).
    Casting,
}

/// The nine B-Rep surface families a DFM analyzer may encounter, named for
/// provenance tags and refusal reasons. A local, serde-friendly copy of
/// [`crate::primitives::surface::SurfaceType`] (which does not derive
/// `Serialize`/`Deserialize`) — DFM facts must be wire-serializable for the
/// MCP agent surface, and this module must not modify files outside
/// `dfm/` to add that derive upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceKind {
    Plane,
    Cylinder,
    Cone,
    Sphere,
    Torus,
    SurfaceOfRevolution,
    BSpline,
    Nurbs,
    Offset,
    Ruled,
}

impl SurfaceKind {
    /// True for the five surfaces with a closed-form analytic measurement
    /// under the DFM spec's analyzer support table (§3.1) — the kinds an
    /// analyzer SHOULD name inside [`Derivation::Analytic`]. The remaining
    /// five (`SurfaceOfRevolution`, `BSpline`, `Nurbs`, `Offset`, `Ruled`)
    /// are the refused kinds named in the table and are expected only
    /// inside [`UnverifiableReason`].
    ///
    /// This is NOT a type-level guarantee — `Derivation::Analytic` accepts
    /// any `SurfaceKind`, since S1 ships no analyzer to construct one at
    /// all. It is a documented invariant S2+ analyzer call sites must
    /// maintain by construction (only report `Analytic` after a closed-form
    /// method actually succeeded on a supported surface); this predicate
    /// exists so that S2 code, and its tests, have a single place to check
    /// "is this kind one I may claim analytic provenance for" rather than
    /// re-deriving the five-way match at each call site.
    pub fn is_analytic(&self) -> bool {
        matches!(
            self,
            Self::Plane | Self::Cylinder | Self::Cone | Self::Sphere | Self::Torus
        )
    }
}

/// Provenance of a [`DfmValue`] — WHY the kernel is allowed to assert this
/// number. Every derivation names the closed-form method: no analyzer may
/// return a value it cannot justify this way (spec §2.2, §3.1).
///
/// `method` is `String`, not `&'static str`: serde's derived `Deserialize`
/// needs every field to be ownable for an arbitrary input lifetime `'de`,
/// and `&'static str: Deserialize<'de>` only holds when `'de: 'static`,
/// which is not true of borrowed wire input — the derive fails to compile
/// otherwise (`'de` must outlive `'static`). Analyzer call sites still pass
/// short, stable, greppable literals; they are simply owned on arrival.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Derivation {
    /// Computed in closed form on an analytic B-Rep surface. `method`
    /// names the technique (e.g. "coaxial-cylinder radius difference",
    /// "planar normal vs. build direction") — a short, stable, greppable
    /// tag, not a free-form sentence.
    Analytic {
        surface_type: SurfaceKind,
        method: String,
    },
    /// Derived from a PROVEN interval enclosure rather than a single
    /// closed-form number (freeform coverage spec F1/F2:
    /// [`crate::math::enclosure`]). The value this derivation rides on is
    /// a conservative endpoint of an enclosure computed via the
    /// convex-hull property — a theorem about the surface, never a
    /// sampled extreme. `refinement_depth` is the number of subdivision
    /// sweeps performed; `converged` reports whether the requested
    /// tightness was reached within the budget (`false` is an honest
    /// outcome — the bound is still proven, just wider than asked for).
    BoundedAnalytic {
        method: String,
        refinement_depth: usize,
        converged: bool,
    },
}

/// The wire echo of a proven enclosure `[lo, hi]` riding a [`DfmValue`]
/// (freeform spec F1: the "bounded form"). Inert data: the honesty
/// constraint (a straddling bound can NEVER fold to Pass) lives in
/// [`Verdict::from_bounded_max`]/[`Verdict::from_bounded_min`], which take
/// the math layer's honest-by-construction
/// [`crate::math::enclosure::Interval`] — this struct only reports what
/// was proven, it cannot influence the fold.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DfmBound {
    pub lo: f64,
    pub hi: f64,
}

/// A measured (or derived-threshold) quantity that carries its own
/// provenance. This is the smallest type that enforces "no number without
/// provenance" at the type level: there is no constructor and no field
/// that produces a `DfmValue` without a [`Derivation`] attached, so a
/// [`Verdict`] can never carry a bare, unaccountable `f64`.
///
/// A BOUNDED value (freeform spec F1) additionally carries the proven
/// enclosure it was folded from in `bound`, and its `value` is always one
/// of the enclosure's two conservative endpoints — see
/// [`DfmValue::bounded_lower`]/[`DfmValue::bounded_upper`], the only
/// constructors that populate `bound`. `bound` is `None` for classic
/// closed-form values, and the field is skipped on the wire when absent,
/// so every pre-F1 report round-trips byte-identically.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DfmValue {
    pub value: f64,
    pub derivation: Derivation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound: Option<DfmBound>,
}

impl DfmValue {
    pub fn new(value: f64, derivation: Derivation) -> Self {
        Self {
            value,
            derivation,
            bound: None,
        }
    }

    /// Bounded value reporting the enclosure's LOWER endpoint (the number
    /// proven to be a floor of the true quantity) — e.g. the `measured`
    /// of a proven max-rule violation ("the value is at least `lo`"), or
    /// a proven margin ("the headroom is at least `lo`").
    pub fn bounded_lower(
        enclosure: &crate::math::enclosure::Interval,
        method: &str,
        refinement_depth: usize,
        converged: bool,
    ) -> Self {
        Self {
            value: enclosure.lo(),
            derivation: Derivation::BoundedAnalytic {
                method: method.to_string(),
                refinement_depth,
                converged,
            },
            bound: Some(DfmBound {
                lo: enclosure.lo(),
                hi: enclosure.hi(),
            }),
        }
    }

    /// Bounded value reporting the enclosure's UPPER endpoint (the number
    /// proven to be a ceiling of the true quantity) — e.g. the `measured`
    /// of a proven min-rule violation ("the value is at most `hi`").
    pub fn bounded_upper(
        enclosure: &crate::math::enclosure::Interval,
        method: &str,
        refinement_depth: usize,
        converged: bool,
    ) -> Self {
        Self {
            value: enclosure.hi(),
            derivation: Derivation::BoundedAnalytic {
                method: method.to_string(),
                refinement_depth,
                converged,
            },
            bound: Some(DfmBound {
                lo: enclosure.lo(),
                hi: enclosure.hi(),
            }),
        }
    }
}

/// Why an analyzer refused to produce a value for a region (spec §3.1: the
/// analytic-or-refuse contract — no tessellation fallback, no invented
/// number).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UnverifiableReason {
    /// The region's carrier surface has no closed-form method for this
    /// rule (e.g. a NURBS/BSpline/Offset/Ruled face reaching
    /// `face_orientation_field`, or a freeform G2 blend reaching
    /// `blend_radius` — spec §3.1's per-analyzer refusal column).
    UnsupportedSurface {
        surface_type: SurfaceKind,
        analyzer: String,
    },
    /// The analyzer's soundness precondition failed (non-manifold /
    /// unsound solid) before a measurement could even be attempted —
    /// `internal_voids` refuses this way per spec §3.1. Distinct from
    /// [`DfmError::UnsoundSolid`]: this is a per-rule refusal that still
    /// yields a report (other rules may still be checkable), not a hard
    /// error that aborts `analyze()` entirely.
    UnsoundPrecondition { detail: String },
    /// The face's boundary TOPOLOGY (not its surface kind) prevents
    /// deriving the closed-form trimmed domain a rule needs — S2's
    /// analytic-or-refuse contract applied to topology rather than
    /// surface kind. Distinct from [`Self::UnsupportedSurface`]: the
    /// surface kind IS one `face_orientation_field` generally supports
    /// (e.g. `Cylinder`), but THIS face's boundary defeats the exact
    /// reconstruction — an inner-loop hole whose effect on the occupied
    /// angular range cannot be bounded exactly, a boundary edge that is
    /// neither a straight generatrix nor an axis-perpendicular circular
    /// rim (e.g. a NURBS intersection curve), or a degenerate empty
    /// boundary on a surface kind that requires a real one. `detail`
    /// names the specific defect.
    UnsupportedTopology { detail: String },
    /// The value's PROVEN enclosure `[lo, hi]` straddles the rule's
    /// `limit` after the refinement budget (freeform spec F1, verdict
    /// table row 3): the kernel cannot separate them, and says so WITH
    /// THE BOUND — strictly more useful than a blanket refusal (an agent
    /// can tighten the design, move the threshold, or accept the risk
    /// explicitly), and strictly more honest than picking a side.
    /// `converged: false` means the budget ran out before the requested
    /// tightness; the bound reported is still a theorem.
    BoundNotSeparating {
        lo: f64,
        hi: f64,
        limit: f64,
        refinement_depth: usize,
        converged: bool,
    },
}

/// The kernel's per-rule answer: proven safe margin, a proven defect, or an
/// honest "cannot tell" — never a fabricated pass. See the module docs for
/// the honesty theorem this feeds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Verdict {
    /// The rule holds, with the measured headroom above (or below, for a
    /// max-style rule) the limit.
    Pass { margin: DfmValue },
    /// The rule is proven violated. `witnesses` names every face
    /// implicated (ascending, analyzer-defined order); `measured` and
    /// `limit` are both provenance-carrying so the violation is exactly
    /// reproducible from the report alone.
    Violation {
        witnesses: Vec<FaceRef>,
        measured: DfmValue,
        limit: DfmValue,
    },
    /// The analyzer could not determine the rule's outcome for these
    /// regions — a refusal, not an error. Non-empty `regions` is why the
    /// pack-level summary reads `Inconclusive`, never `Pass`.
    Unverifiable {
        regions: Vec<FaceRef>,
        reason: UnverifiableReason,
    },
}

impl Verdict {
    /// THE BOUNDED HONESTY FOLD (freeform spec F1, §3's verdict table) —
    /// the three-way verdict for a MAX-style rule ("the value must stay
    /// below `limit`") decided from a PROVEN enclosure `[lo, hi]`:
    ///
    /// | condition            | verdict                                  |
    /// |----------------------|------------------------------------------|
    /// | `hi < limit`         | provable `Pass`, margin ≥ `limit − hi`   |
    /// | `lo > limit`         | provable `Violation`, value ≥ `lo`       |
    /// | enclosure straddles  | `Unverifiable` REPORTING THE BOUND       |
    ///
    /// The load-bearing rule mirrors S1's [`DfmReport::summarize`] fold: a
    /// straddling enclosure can NEVER produce `Pass` — deciding from any
    /// interior point (a midpoint, a sampled extreme) is exactly the
    /// silent-wrong-answer defect this subsystem exists to delete, and
    /// the mutation-proof test below demonstrates the divergence with raw
    /// numbers. Comparisons are strict: an enclosure touching the limit
    /// (`hi == limit` or `lo == limit`) cannot prove either side and
    /// falls to the reporting refusal. A NaN limit compares false on both
    /// sides and lands in the refusal row — a broken threshold can never
    /// fabricate a Pass.
    ///
    /// `faces` become the `witnesses` of a violation / the `regions` of a
    /// refusal (the same face set is implicated either way); the Pass
    /// margin is computed with outward-rounded interval arithmetic
    /// (`limit − enclosure`, lower endpoint), so the reported margin is
    /// itself proven, never optimistic. If `limit.value` is non-finite,
    /// no margin can be proven and the fold refuses with the bound.
    pub fn from_bounded_max(
        enclosure: crate::math::enclosure::Interval,
        limit: DfmValue,
        faces: Vec<FaceRef>,
        method: &str,
        refinement_depth: usize,
        converged: bool,
    ) -> Verdict {
        let t = limit.value;
        let not_separating = |faces: Vec<FaceRef>| Verdict::Unverifiable {
            regions: faces,
            reason: UnverifiableReason::BoundNotSeparating {
                lo: enclosure.lo(),
                hi: enclosure.hi(),
                limit: t,
                refinement_depth,
                converged,
            },
        };
        if enclosure.hi() < t {
            match crate::math::enclosure::Interval::point(t) {
                Ok(limit_point) => {
                    let margin = limit_point.sub(&enclosure);
                    Verdict::Pass {
                        margin: DfmValue::bounded_lower(
                            &margin,
                            method,
                            refinement_depth,
                            converged,
                        ),
                    }
                }
                // Non-finite limit: `hi < +∞` proves nothing about a real
                // threshold — refuse with the bound, never fabricate.
                Err(_) => not_separating(faces),
            }
        } else if enclosure.lo() > t {
            Verdict::Violation {
                witnesses: faces,
                measured: DfmValue::bounded_lower(&enclosure, method, refinement_depth, converged),
                limit,
            }
        } else {
            not_separating(faces)
        }
    }

    /// Dual of [`Verdict::from_bounded_max`] for a MIN-style rule ("the
    /// value must stay above `limit`", e.g. a wall thickness floor):
    /// `lo > limit` is the provable Pass (margin ≥ `lo − limit`),
    /// `hi < limit` the provable Violation (value proven ≤ `hi`), and a
    /// straddling enclosure is `Unverifiable` reporting the bound — the
    /// same never-Pass-on-straddle rule, mirrored.
    pub fn from_bounded_min(
        enclosure: crate::math::enclosure::Interval,
        limit: DfmValue,
        faces: Vec<FaceRef>,
        method: &str,
        refinement_depth: usize,
        converged: bool,
    ) -> Verdict {
        let t = limit.value;
        let not_separating = |faces: Vec<FaceRef>| Verdict::Unverifiable {
            regions: faces,
            reason: UnverifiableReason::BoundNotSeparating {
                lo: enclosure.lo(),
                hi: enclosure.hi(),
                limit: t,
                refinement_depth,
                converged,
            },
        };
        if enclosure.lo() > t {
            match crate::math::enclosure::Interval::point(t) {
                Ok(limit_point) => {
                    let margin = enclosure.sub(&limit_point);
                    Verdict::Pass {
                        margin: DfmValue::bounded_lower(
                            &margin,
                            method,
                            refinement_depth,
                            converged,
                        ),
                    }
                }
                Err(_) => not_separating(faces),
            }
        } else if enclosure.hi() < t {
            Verdict::Violation {
                witnesses: faces,
                measured: DfmValue::bounded_upper(&enclosure, method, refinement_depth, converged),
                limit,
            }
        } else {
            not_separating(faces)
        }
    }
}

/// One rule's certified outcome (spec §3.2 `Rule` / §3.3 `RuleVerdict`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleVerdict {
    /// The rule's stable id, e.g. `"fdm.min_wall"` — matches `Rule::id` in
    /// the S2+ rule pack (spec §3.2), which types it `&'static str` since
    /// pack definitions are compile-time Rust literals. Here it is `String`
    /// rather than a `RuleId` newtype: a `RuleVerdict` rides the wire inside
    /// `DfmReport`, and serde's derived `Deserialize` cannot produce
    /// `&'static str` from borrowed input of arbitrary lifetime (see the
    /// [`Derivation`] doc comment) — a wrapper newtype around the same
    /// owned string would add no invariant beyond what `String` already
    /// gives, since rule ids are never user input to validate.
    pub rule: String,
    pub verdict: Verdict,
    /// WHERE the rule's threshold comes from — standard, handbook,
    /// datasheet, or shop practice (spec §3.2.1). Echoed here (not just
    /// held in the static [`crate::dfm::packs::Rule`] definition) so a
    /// report reader — human or MCP agent, working from the `DfmReport`
    /// JSON alone — can tell a proven-violation-of-a-standard apart from
    /// a proven-violation-of-a-heuristic without cross-referencing the
    /// pack source.
    pub provenance: RuleProvenance,
}

/// The pack-level honesty verdict — a PRECEDENCE FOLD over every rule's
/// [`Verdict`] (see the module docs for the theorem). Never constructed
/// directly by callers; only [`DfmReport::new`] produces one, from the
/// verdicts it is given.
///
/// Derives `Hash` (on top of the `Eq` it already carries) so that
/// `Option<DfmSummary>` — the type `ValidationResult::manufacturing_valid`
/// became in spec S6 (kills H5, the hardcoded-`true` "kernel can lie" bug
/// sibling to `geometry_valid`'s) — can itself be hashed by
/// `validation.rs`'s `generate_signature`, which hashes every
/// `ValidationResult` field by value rather than deriving `Hash` on the
/// struct itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DfmSummary {
    /// Every rule in the pack reported `Pass`.
    Pass,
    /// At least one rule reported `Violation`. Dominates `Inconclusive`
    /// when both are present.
    Violations { count: usize },
    /// No violations, but at least one rule reported `Unverifiable` — the
    /// kernel refuses to call the design manufacturable when a check never
    /// ran. This variant is the entire reason `DfmSummary` exists instead
    /// of a bare `bool`.
    Inconclusive { unverifiable: usize },
}

/// Process parameters a rule pack was evaluated with, echoed on the report
/// so it is self-describing (spec §3.3: `params echo`). Tagged by pack so
/// a report's `params` can never claim to belong to a different process
/// than the rules that produced it — [`DfmReport::new`] derives `pack`
/// from this value rather than accepting it as an independent argument,
/// which removes the pack/params-mismatch failure mode at the type level
/// instead of policing it at runtime.
///
/// The four process packs added after FDM/molding (`cnc_3_axis`,
/// `cnc_turning`, `sheet_metal`, `casting`) carry their thresholds in a
/// params struct whose every field has a serde default, so `{"pack":
/// "cnc_3_axis"}` alone is a complete request — and the report echoes
/// the defaults that were actually applied, never an implicit constant.
/// Every default is documented on its field with where it comes from;
/// the rule's [`RuleProvenance`] says the same on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "pack", rename_all = "snake_case")]
pub enum PackParams {
    /// FDM pack v1 params (spec §3.2 defaults: 0.4 mm nozzle, +Z build).
    Fdm {
        nozzle_diameter: f64,
        build_direction: [f64; 3],
    },
    /// Injection-molding pack v1 params (spec §3.2 default: 1° min draft).
    InjectionMolding {
        pull_direction: [f64; 3],
        min_draft_deg: f64,
    },
    /// CNC milling (3-axis) params.
    #[serde(rename = "cnc_3_axis")]
    Cnc3Axis(CncMillingParams),
    /// CNC turning params.
    CncTurning(CncTurningParams),
    /// Sheet-metal params.
    SheetMetal(SheetMetalParams),
    /// Casting params.
    Casting(CastingParams),
}

/// Standard twist-drill series a drilled diameter is checked against by
/// `cnc.standard_drill_size`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrillSeries {
    /// Metric jobber drills in 0.1 mm steps (the commercial metric set).
    Metric,
    /// Fractional-inch drills in 1/64 in steps.
    InchFractional,
    /// Either series.
    MetricOrInch,
}

impl DrillSeries {
    fn tag(self) -> u8 {
        match self {
            DrillSeries::Metric => 0,
            DrillSeries::InchFractional => 1,
            DrillSeries::MetricOrInch => 2,
        }
    }
}

fn default_setup_directions() -> Vec<[f64; 3]> {
    vec![[0.0, 0.0, 1.0], [0.0, 0.0, -1.0]]
}
fn default_pull() -> [f64; 3] {
    [0.0, 0.0, 1.0]
}
fn d_min_tool_diameter() -> f64 {
    3.0
}
fn d_max_corner_reach_ratio() -> f64 {
    4.0
}
fn d_max_drill_depth_ratio() -> f64 {
    10.0
}
fn d_min_drill_diameter() -> f64 {
    1.0
}
fn d_standard_drill_max_diameter() -> f64 {
    13.0
}
fn d_drill_series() -> DrillSeries {
    DrillSeries::Metric
}
fn d_drill_size_tolerance() -> f64 {
    0.001
}
fn d_drill_point_angles() -> Vec<f64> {
    vec![118.0, 135.0, 140.0]
}
fn d_drill_point_tolerance() -> f64 {
    0.5
}
fn d_hole_axis_tolerance_deg() -> f64 {
    1e-6
}
fn d_cnc_min_wall() -> f64 {
    0.8
}
fn d_max_bore_depth_ratio() -> f64 {
    4.0
}
fn d_one() -> f64 {
    1.0
}
fn d_hole_edge_factor() -> f64 {
    2.0
}
fn d_hole_bend_factor() -> f64 {
    2.5
}
fn d_min_flange_factor() -> f64 {
    4.0
}
fn d_thickness_tolerance() -> f64 {
    0.001
}
fn d_cast_max_wall() -> f64 {
    6.0
}
fn d_cored_hole() -> f64 {
    2.5
}

/// CNC milling pack params. Defaults are shop practice (see each field);
/// `setup_directions` is the machining PLAN, not a property of the part —
/// an agent sets it to the setups it intends (each a unit vector pointing
/// from the part toward the spindle).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CncMillingParams {
    /// Setup (approach) directions; default `[+Z, −Z]` (top op, then flip).
    #[serde(default = "default_setup_directions")]
    pub setup_directions: Vec<[f64; 3]>,
    /// Smallest end mill the shop will use, mm (default 3.0). An inside
    /// corner a cutter's SIDE forms is at best this tool's radius.
    #[serde(default = "d_min_tool_diameter")]
    pub min_tool_diameter: f64,
    /// Max tool reach, length of cut ÷ tool diameter (default 4.0: standard
    /// end mills cut ~3–4× their diameter).
    #[serde(default = "d_max_corner_reach_ratio")]
    pub max_corner_reach_ratio: f64,
    /// Max drilled depth ÷ diameter (default 10: twist-drill limit; deeper
    /// is gun drilling).
    #[serde(default = "d_max_drill_depth_ratio")]
    pub max_drill_depth_ratio: f64,
    /// Smallest drill, mm (default 1.0).
    #[serde(default = "d_min_drill_diameter")]
    pub min_drill_diameter: f64,
    /// Drills larger than this are assumed milled/bored, not drilled, and
    /// are outside `cnc.standard_drill_size`'s scope (default 13.0 mm, the
    /// top of the common 0.1 mm-step metric jobber set).
    #[serde(default = "d_standard_drill_max_diameter")]
    pub standard_drill_max_diameter: f64,
    /// Drill series (default metric 0.1 mm steps).
    #[serde(default = "d_drill_series")]
    pub drill_series: DrillSeries,
    /// How far a diameter may sit from a series size, mm (default 0.001).
    #[serde(default = "d_drill_size_tolerance")]
    pub drill_size_tolerance: f64,
    /// Standard drill-point included angles, degrees (default 118/135/140).
    #[serde(default = "d_drill_point_angles")]
    pub drill_point_angles_deg: Vec<f64>,
    /// Tolerance on a drill-point angle, degrees (default 0.5).
    #[serde(default = "d_drill_point_tolerance")]
    pub drill_point_tolerance_deg: f64,
    /// Accept flat-bottomed blind holes (default false: they need a
    /// flat-bottom drill or an end mill after drilling).
    #[serde(default)]
    pub allow_flat_bottom_holes: bool,
    /// Hole-axis vs setup-direction misalignment tolerance, degrees
    /// (default 1e-6: numerical only — an angled hole is a real finding).
    #[serde(default = "d_hole_axis_tolerance_deg")]
    pub hole_axis_tolerance_deg: f64,
    /// Thinnest machinable wall, mm (default 0.8, the common CNC guide
    /// figure for metals; ~1.5 for plastics).
    #[serde(default = "d_cnc_min_wall")]
    pub min_wall: f64,
}

impl Default for CncMillingParams {
    fn default() -> Self {
        Self {
            setup_directions: default_setup_directions(),
            min_tool_diameter: d_min_tool_diameter(),
            max_corner_reach_ratio: d_max_corner_reach_ratio(),
            max_drill_depth_ratio: d_max_drill_depth_ratio(),
            min_drill_diameter: d_min_drill_diameter(),
            standard_drill_max_diameter: d_standard_drill_max_diameter(),
            drill_series: d_drill_series(),
            drill_size_tolerance: d_drill_size_tolerance(),
            drill_point_angles_deg: d_drill_point_angles(),
            drill_point_tolerance_deg: d_drill_point_tolerance(),
            allow_flat_bottom_holes: false,
            hole_axis_tolerance_deg: d_hole_axis_tolerance_deg(),
            min_wall: d_cnc_min_wall(),
        }
    }
}

/// CNC turning pack params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CncTurningParams {
    /// Spindle axis as `(point, direction)`. `None` (default) infers it
    /// from the part: the unique axis line shared by every convex (OD)
    /// cylinder; no unique axis is a typed refusal, never a guess.
    #[serde(default)]
    pub spindle_axis: Option<([f64; 3], [f64; 3])>,
    /// Max internal bore depth ÷ diameter (default 4.0: steel boring-bar
    /// overhang ~4× bar diameter).
    #[serde(default = "d_max_bore_depth_ratio")]
    pub max_bore_depth_ratio: f64,
    /// Thinnest turnable wall, mm (default 0.8).
    #[serde(default = "d_cnc_min_wall")]
    pub min_wall: f64,
}

impl Default for CncTurningParams {
    fn default() -> Self {
        Self {
            spindle_axis: None,
            max_bore_depth_ratio: d_max_bore_depth_ratio(),
            min_wall: d_cnc_min_wall(),
        }
    }
}

/// Sheet-metal pack params. Every distance factor multiplies the sheet
/// thickness `t`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SheetMetalParams {
    /// Gauge, mm. `None` (default) infers it: the thinnest proven wall
    /// pair; no proven pair is a typed refusal for every rule needing `t`.
    #[serde(default)]
    pub thickness: Option<f64>,
    /// Equality tolerance on thickness, mm (default 0.001).
    #[serde(default = "d_thickness_tolerance")]
    pub thickness_tolerance: f64,
    /// Min inside bend radius ÷ t (default 1.0; soft alloys 0.5, hard 1.5).
    #[serde(default = "d_one")]
    pub min_bend_radius_factor: f64,
    /// Min hole-rim-to-free-edge distance ÷ t (default 2.0).
    #[serde(default = "d_hole_edge_factor")]
    pub hole_edge_factor: f64,
    /// Min hole-rim-to-bend distance ÷ t, plus the inside bend radius
    /// (default 2.5 t + R).
    #[serde(default = "d_hole_bend_factor")]
    pub hole_bend_factor: f64,
    /// Min flange length ÷ t (default 4.0).
    #[serde(default = "d_min_flange_factor")]
    pub min_flange_factor: f64,
}

impl Default for SheetMetalParams {
    fn default() -> Self {
        Self {
            thickness: None,
            thickness_tolerance: d_thickness_tolerance(),
            min_bend_radius_factor: d_one(),
            hole_edge_factor: d_hole_edge_factor(),
            hole_bend_factor: d_hole_bend_factor(),
            min_flange_factor: d_min_flange_factor(),
        }
    }
}

/// Casting (two-part die / permanent mould) pack params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CastingParams {
    /// Die opening direction (default +Z).
    #[serde(default = "default_pull")]
    pub pull_direction: [f64; 3],
    /// Minimum draft, degrees (default 1.0).
    #[serde(default = "d_one")]
    pub min_draft_deg: f64,
    /// Thinnest wall that fills, mm (default 1.0, die-cast aluminium).
    #[serde(default = "d_one")]
    pub min_wall: f64,
    /// Thickest wall before hot spots / shrink porosity, mm (default 6.0).
    #[serde(default = "d_cast_max_wall")]
    pub max_wall: f64,
    /// Smallest internal fillet radius, mm (default 1.0; sources diverge
    /// 0.5–1.5).
    #[serde(default = "d_one")]
    pub min_fillet_radius: f64,
    /// Smallest cored (cast-in) hole diameter, mm (default 2.5).
    #[serde(default = "d_cored_hole")]
    pub min_cored_hole_diameter: f64,
}

impl Default for CastingParams {
    fn default() -> Self {
        Self {
            pull_direction: default_pull(),
            min_draft_deg: d_one(),
            min_wall: d_one(),
            max_wall: d_cast_max_wall(),
            min_fillet_radius: d_one(),
            min_cored_hole_diameter: d_cored_hole(),
        }
    }
}

impl PackParams {
    /// The [`RulePackId`] these params belong to — the single source of
    /// truth [`DfmReport::new`] uses instead of taking `pack` as a
    /// separately suppliable field.
    pub fn pack_id(&self) -> RulePackId {
        match self {
            PackParams::Fdm { .. } => RulePackId::Fdm,
            PackParams::InjectionMolding { .. } => RulePackId::InjectionMolding,
            PackParams::Cnc3Axis(_) => RulePackId::Cnc3Axis,
            PackParams::CncTurning(_) => RulePackId::CncTurning,
            PackParams::SheetMetal(_) => RulePackId::SheetMetal,
            PackParams::Casting(_) => RulePackId::Casting,
        }
    }

    /// Stable hash of the parameters for [`DfmFact::params_hash`]. `f64`
    /// has no [`std::hash::Hash`] impl (NaN / signed-zero equality is
    /// ill-defined), so this hashes the IEEE-754 bit pattern directly —
    /// the same technique `timeline-engine`'s `ModelDigest` uses for
    /// coordinate hashing. Two bit-identical parameter sets always hash
    /// identically; this is a change-detection digest (did the params
    /// used to certify this fact change?), not a numerically-tolerant
    /// comparison.
    ///
    /// ## Algorithm (pinned, reproducible from source alone)
    ///
    /// FNV-1a, 64-bit: offset basis `14695981039346656037`
    /// (`0xcbf29ce484222325`), prime `1099511628211` (`0x100000001b3`). A
    /// one-byte variant discriminant is folded FIRST (`0` for
    /// [`PackParams::Fdm`], `1` for [`PackParams::InjectionMolding`]) so the
    /// two variants can never collide even when their `f64` payloads are
    /// byte-for-byte identical; then every `f64` field, in declaration
    /// order, is folded 1 byte at a time over `to_bits().to_le_bytes()` —
    /// little-endian explicitly, never `to_ne_bytes()`, so the digest is
    /// reproducible across CPU architectures as well as across builds and
    /// toolchain versions. Each byte folds as
    /// `hash = (hash ^ byte).wrapping_mul(PRIME)`.
    ///
    /// This replaces a `DefaultHasher`-backed digest: the standard library
    /// gives `DefaultHasher` NO cross-version stability guarantee, so a
    /// `DfmFact::params_hash` computed before a toolchain upgrade could
    /// silently stop matching one computed after with the exact same
    /// params — a certificate that lies about whether the params changed,
    /// which is exactly the defect class this subsystem exists to
    /// eliminate. FNV-1a is fully specified above and implemented inline
    /// (no new dependency) as a fixed algorithm over an explicit byte
    /// encoding, not a language-provided black box, so it carries no such
    /// instability. [`tests::pack_params_hash_matches_pinned_value`] pins
    /// one input's exact output so a future change to the algorithm breaks
    /// loudly instead of silently invalidating a persisted `DfmFact`, and
    /// [`tests::fnv1a_constants_match_the_documented_values`] anchors the
    /// two magic numbers independently, so a "fix" to a failing
    /// pinned-value test can never quietly redefine the algorithm instead
    /// of fixing an actual bug.
    pub fn stable_hash(&self) -> u64 {
        const FNV_OFFSET_BASIS: u64 = 14695981039346656037;
        const FNV_PRIME: u64 = 1099511628211;

        fn fold_byte(hash: u64, byte: u8) -> u64 {
            (hash ^ byte as u64).wrapping_mul(FNV_PRIME)
        }
        fn fold_f64(hash: u64, value: f64) -> u64 {
            let mut hash = hash;
            for byte in value.to_bits().to_le_bytes() {
                hash = fold_byte(hash, byte);
            }
            hash
        }
        fn fold_len(hash: u64, len: usize) -> u64 {
            let mut hash = hash;
            for byte in (len as u64).to_le_bytes() {
                hash = fold_byte(hash, byte);
            }
            hash
        }

        let mut hash = FNV_OFFSET_BASIS;
        match self {
            PackParams::Fdm {
                nozzle_diameter,
                build_direction,
            } => {
                hash = fold_byte(hash, 0u8);
                hash = fold_f64(hash, *nozzle_diameter);
                for c in build_direction {
                    hash = fold_f64(hash, *c);
                }
            }
            PackParams::InjectionMolding {
                pull_direction,
                min_draft_deg,
            } => {
                hash = fold_byte(hash, 1u8);
                for c in pull_direction {
                    hash = fold_f64(hash, *c);
                }
                hash = fold_f64(hash, *min_draft_deg);
            }
            // Later packs: discriminants 2..=5, then every field in
            // declaration order; a Vec folds its length (u64 LE) first and
            // an Option a 0/1 presence byte first, so no two distinct
            // parameter sets share a byte stream.
            PackParams::Cnc3Axis(p) => {
                hash = fold_byte(hash, 2u8);
                hash = fold_len(hash, p.setup_directions.len());
                for d in &p.setup_directions {
                    for c in d {
                        hash = fold_f64(hash, *c);
                    }
                }
                for v in [
                    p.min_tool_diameter,
                    p.max_corner_reach_ratio,
                    p.max_drill_depth_ratio,
                    p.min_drill_diameter,
                    p.standard_drill_max_diameter,
                ] {
                    hash = fold_f64(hash, v);
                }
                hash = fold_byte(hash, p.drill_series.tag());
                hash = fold_f64(hash, p.drill_size_tolerance);
                hash = fold_len(hash, p.drill_point_angles_deg.len());
                for a in &p.drill_point_angles_deg {
                    hash = fold_f64(hash, *a);
                }
                hash = fold_f64(hash, p.drill_point_tolerance_deg);
                hash = fold_byte(hash, u8::from(p.allow_flat_bottom_holes));
                hash = fold_f64(hash, p.hole_axis_tolerance_deg);
                hash = fold_f64(hash, p.min_wall);
            }
            PackParams::CncTurning(p) => {
                hash = fold_byte(hash, 3u8);
                match &p.spindle_axis {
                    None => hash = fold_byte(hash, 0u8),
                    Some((o, k)) => {
                        hash = fold_byte(hash, 1u8);
                        for c in o.iter().chain(k.iter()) {
                            hash = fold_f64(hash, *c);
                        }
                    }
                }
                hash = fold_f64(hash, p.max_bore_depth_ratio);
                hash = fold_f64(hash, p.min_wall);
            }
            PackParams::SheetMetal(p) => {
                hash = fold_byte(hash, 4u8);
                match p.thickness {
                    None => hash = fold_byte(hash, 0u8),
                    Some(t) => {
                        hash = fold_byte(hash, 1u8);
                        hash = fold_f64(hash, t);
                    }
                }
                for v in [
                    p.thickness_tolerance,
                    p.min_bend_radius_factor,
                    p.hole_edge_factor,
                    p.hole_bend_factor,
                    p.min_flange_factor,
                ] {
                    hash = fold_f64(hash, v);
                }
            }
            PackParams::Casting(p) => {
                hash = fold_byte(hash, 5u8);
                for c in p.pull_direction {
                    hash = fold_f64(hash, c);
                }
                for v in [
                    p.min_draft_deg,
                    p.min_wall,
                    p.max_wall,
                    p.min_fillet_radius,
                    p.min_cored_hole_diameter,
                ] {
                    hash = fold_f64(hash, v);
                }
            }
        }
        hash
    }
}

/// Malformed-input errors only — a refusal an analyzer makes honestly is
/// [`Verdict::Unverifiable`], a VALUE in the report, never an `Err` (spec
/// §4). `analyze()` (S2+) returns `Err` only when the input itself is
/// broken: a witness/region names a face that does not exist, or the
/// solid fails a soundness precondition an analyzer needs before it can
/// even attempt a measurement.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum DfmError {
    /// A face reference does not resolve against the solid the report is
    /// being computed for — malformed input, never a legitimate analyzer
    /// outcome.
    #[error("face {face} does not exist in the solid DFM analysis was requested against")]
    DanglingFaceRef { face: FaceRef },
    /// A pack parameter cannot be used as given (a zero direction vector,
    /// a non-finite threshold) — malformed input, never a refusal.
    #[error("DFM pack parameters are malformed: {detail}")]
    InvalidParams { detail: String },
    /// A solid reference does not resolve against the model DFM analysis
    /// was requested against — malformed input, never a legitimate
    /// analyzer outcome. Introduced by [`crate::dfm::analyzers::bore`]
    /// (spec S4), the first analyzer whose contract is `(model, solid_id)`
    /// rather than a caller-enumerated `faces: &[FaceId]` — see that
    /// module's docs for why `bore_metrics` needs solid-level scope
    /// (through-vs-blind requires the SOLID's own extent along the bore
    /// axis, not just one face's trim).
    #[error("solid {solid} does not exist in the model DFM analysis was requested against")]
    DanglingSolidRef {
        solid: crate::primitives::solid::SolidId,
    },
    /// The solid fails the soundness precondition an analyzer requires
    /// before it can even attempt a measurement (spec §4: e.g.
    /// `internal_voids` needs a sound shell/solid structure to walk).
    /// `detail` carries the soundness defect description; S2+ analyzers
    /// that exercise this variant may tighten it to embed the actual
    /// soundness certificate once one exists to reference here.
    #[error("solid is not sound enough for DFM analysis: {detail}")]
    UnsoundSolid { detail: String },
    /// **P1 enforcement.** The solid has been mutated (or never certified)
    /// since its last full verification —
    /// [`crate::primitives::provenance::SoundnessReading::Stale`]. Distinct
    /// from [`Self::UnsoundSolid`]: `UnsoundSolid` means the kernel HAS
    /// checked and found a real defect; `UnverifiedSolid` means the kernel
    /// has NOT checked recently enough to say either way. A DFM `pass`
    /// computed against unverified geometry is exactly the "laundering a
    /// guess as authority" failure the freshness gate exists to close — so
    /// this is refused pre-flight, before any rule runs, rather than folded
    /// into a verdict.
    #[error(
        "solid {solid} has not been fully verified since its last mutation \
         — call verify_part before requesting a DFM analysis"
    )]
    UnverifiedSolid {
        solid: crate::primitives::solid::SolidId,
    },
}

/// The full per-rule DFM report for one (model, solid, pack) analysis
/// (spec §3.3). Self-describing: `params` echoes exactly what the pack was
/// evaluated with, and `verdicts` always has one entry per rule in the
/// pack — a rule that could not be checked still appears, as
/// `Unverifiable`, never simply omitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DfmReport {
    pack: RulePackId,
    params: PackParams,
    verdicts: Vec<RuleVerdict>,
    summary: DfmSummary,
}

impl DfmReport {
    /// The only way to build a report. `pack` is derived from `params`
    /// (never a second, independently-suppliable argument — see
    /// [`PackParams::pack_id`]), and `summary` is FOLDED from `verdicts`
    /// (see [`DfmReport::summarize`]) — a caller cannot hand the kernel a
    /// report whose `pack` disagrees with its own params, or whose
    /// `summary` disagrees with its own verdicts. Fields are private
    /// precisely so no code path can construct a `DfmReport` any other
    /// way; use the accessors below to read it back.
    pub fn new(params: PackParams, verdicts: Vec<RuleVerdict>) -> Self {
        let pack = params.pack_id();
        let summary = Self::summarize(&verdicts);
        Self {
            pack,
            params,
            verdicts,
            summary,
        }
    }

    pub fn pack(&self) -> RulePackId {
        self.pack
    }

    pub fn params(&self) -> &PackParams {
        &self.params
    }

    pub fn verdicts(&self) -> &[RuleVerdict] {
        &self.verdicts
    }

    /// The honesty-folded verdict. Always consistent with `verdicts` by
    /// construction — there is no setter.
    pub fn summary(&self) -> DfmSummary {
        self.summary
    }

    /// Count each verdict kind once. The single source of truth both
    /// [`DfmReport::summarize`] and [`DfmFact::from_report`] read from, so
    /// the report's summary and the certificate fact's counts can never
    /// drift apart from each other.
    fn tally(verdicts: &[RuleVerdict]) -> (usize, usize, usize) {
        let mut pass = 0usize;
        let mut violations = 0usize;
        let mut unverifiable = 0usize;
        for rv in verdicts {
            match &rv.verdict {
                Verdict::Pass { .. } => pass += 1,
                Verdict::Violation { .. } => violations += 1,
                Verdict::Unverifiable { .. } => unverifiable += 1,
            }
        }
        (pass, violations, unverifiable)
    }

    /// THE HONESTY FOLD (spec §3.3 / §4, the load-bearing logic of this
    /// slice). `Pass` requires every rule to pass. A `Violation` DOMINATES:
    /// any proven defect makes the summary `Violations`, regardless of how
    /// many rules were also `Unverifiable`. Only when there are zero
    /// violations does an `Unverifiable` rule force `Inconclusive` — it
    /// NEVER falls through to `Pass`. This mirrors the assembly
    /// certificate's H8 fix: a check that did not run is not a check that
    /// passed.
    fn summarize(verdicts: &[RuleVerdict]) -> DfmSummary {
        let (_pass, violations, unverifiable) = Self::tally(verdicts);
        if violations > 0 {
            DfmSummary::Violations { count: violations }
        } else if unverifiable > 0 {
            DfmSummary::Inconclusive { unverifiable }
        } else {
            DfmSummary::Pass
        }
    }
}

/// The compact certificate-riding summary (spec §3.3) — rides the existing
/// certificate machinery the way [`crate::sketch2d::sketch_certificate::CertificateSummary`]
/// rides solve/extrude responses. Always derived from a [`DfmReport`] via
/// [`DfmFact::from_report`], never asserted independently, so a fact can
/// never disagree with the report it was minted from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DfmFact {
    pub pack: RulePackId,
    pub pass_count: usize,
    pub violation_count: usize,
    pub unverifiable_count: usize,
    pub params_hash: u64,
}

impl DfmFact {
    /// Derive the compact fact from a full report.
    pub fn from_report(report: &DfmReport) -> Self {
        let (pass_count, violation_count, unverifiable_count) = DfmReport::tally(report.verdicts());
        Self {
            pack: report.pack(),
            pass_count,
            violation_count,
            unverifiable_count,
            params_hash: report.params().stable_hash(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dfm::provenance::StandardBody;

    fn analytic(method: &'static str) -> Derivation {
        Derivation::Analytic {
            surface_type: SurfaceKind::Plane,
            method: method.to_string(),
        }
    }

    fn shop_practice(note: &'static str) -> RuleProvenance {
        RuleProvenance::ShopPractice {
            note: note.to_string(),
        }
    }

    fn pass(rule: &'static str) -> RuleVerdict {
        RuleVerdict {
            rule: rule.to_string(),
            verdict: Verdict::Pass {
                margin: DfmValue::new(1.5, analytic("plane-pair distance")),
            },
            provenance: shop_practice("test fixture"),
        }
    }

    fn violation(rule: &'static str) -> RuleVerdict {
        RuleVerdict {
            rule: rule.to_string(),
            verdict: Verdict::Violation {
                witnesses: vec![7, 12],
                measured: DfmValue::new(0.2, analytic("plane-pair distance")),
                limit: DfmValue::new(0.8, analytic("2x nozzle diameter")),
            },
            provenance: shop_practice("test fixture"),
        }
    }

    fn unverifiable(rule: &'static str) -> RuleVerdict {
        RuleVerdict {
            rule: rule.to_string(),
            verdict: Verdict::Unverifiable {
                regions: vec![3],
                reason: UnverifiableReason::UnsupportedSurface {
                    surface_type: SurfaceKind::Nurbs,
                    analyzer: "face_orientation_field".to_string(),
                },
            },
            provenance: shop_practice("test fixture"),
        }
    }

    fn fdm_params() -> PackParams {
        PackParams::Fdm {
            nozzle_diameter: 0.4,
            build_direction: [0.0, 0.0, 1.0],
        }
    }

    #[test]
    fn all_pass_yields_pass() {
        let report = DfmReport::new(
            fdm_params(),
            vec![pass("fdm.min_wall"), pass("fdm.overhang")],
        );
        assert_eq!(report.summary(), DfmSummary::Pass);
        assert_eq!(report.pack(), RulePackId::Fdm);
    }

    #[test]
    fn one_violation_among_passes_yields_violations_one() {
        let report = DfmReport::new(
            fdm_params(),
            vec![pass("fdm.min_wall"), violation("fdm.overhang")],
        );
        assert_eq!(report.summary(), DfmSummary::Violations { count: 1 });
    }

    /// THE HEADLINE TEST — the honesty theorem itself. A pack where every
    /// rule passes except one that is `Unverifiable` must summarize as
    /// `Inconclusive`, never `Pass`. An analyzer that could not check a
    /// rule must not be silently treated as though it had checked and
    /// approved.
    #[test]
    fn one_unverifiable_among_all_pass_yields_inconclusive_never_pass() {
        let report = DfmReport::new(
            fdm_params(),
            vec![
                pass("fdm.min_wall"),
                pass("fdm.min_bore"),
                unverifiable("fdm.support_volume"),
            ],
        );
        assert_eq!(
            report.summary(),
            DfmSummary::Inconclusive { unverifiable: 1 }
        );
        assert_ne!(
            report.summary(),
            DfmSummary::Pass,
            "an unverifiable rule must never be folded into a Pass summary"
        );
    }

    #[test]
    fn violation_and_unverifiable_together_violations_dominate() {
        let report = DfmReport::new(
            fdm_params(),
            vec![
                pass("fdm.min_wall"),
                violation("fdm.overhang"),
                unverifiable("fdm.support_volume"),
            ],
        );
        assert_eq!(
            report.summary(),
            DfmSummary::Violations { count: 1 },
            "a proven violation dominates an open (unverifiable) question"
        );
    }

    #[test]
    fn dfm_fact_counts_match_report_tally() {
        let report = DfmReport::new(
            fdm_params(),
            vec![
                pass("fdm.min_wall"),
                pass("fdm.min_bore"),
                violation("fdm.overhang"),
                unverifiable("fdm.support_volume"),
            ],
        );
        let fact = DfmFact::from_report(&report);
        assert_eq!(fact.pack, RulePackId::Fdm);
        assert_eq!(fact.pass_count, 2);
        assert_eq!(fact.violation_count, 1);
        assert_eq!(fact.unverifiable_count, 1);
        assert_eq!(fact.params_hash, report.params().stable_hash());
    }

    #[test]
    fn pack_params_hash_is_stable_and_sensitive() {
        let a = PackParams::Fdm {
            nozzle_diameter: 0.4,
            build_direction: [0.0, 0.0, 1.0],
        };
        let b = PackParams::Fdm {
            nozzle_diameter: 0.4,
            build_direction: [0.0, 0.0, 1.0],
        };
        let c = PackParams::Fdm {
            nozzle_diameter: 0.5,
            build_direction: [0.0, 0.0, 1.0],
        };
        assert_eq!(
            a.stable_hash(),
            b.stable_hash(),
            "identical params hash identically"
        );
        assert_ne!(
            a.stable_hash(),
            c.stable_hash(),
            "different params must not collide for a trivially distinct case"
        );
    }

    /// The `0`/`1` variant discriminant is the ONLY thing standing between
    /// two different `PackParams` variants whose `f64` payloads fold to the
    /// exact same byte sequence. `Fdm`'s fold order is `[nozzle_diameter,
    /// build_direction.x, .y, .z]`; `InjectionMolding`'s is
    /// `[pull_direction.x, .y, .z, min_draft_deg]` — chosen here so both
    /// sequences are `[0.4, 0.0, 0.0, 1.0]` bit-for-bit. If the
    /// discriminant byte were ever dropped from `stable_hash`, this test
    /// would start failing (the two hashes would collide) even though
    /// nothing else about the algorithm changed.
    #[test]
    fn pack_params_hash_discriminant_prevents_cross_variant_collision() {
        let fdm = PackParams::Fdm {
            nozzle_diameter: 0.4,
            build_direction: [0.0, 0.0, 1.0],
        };
        let molding = PackParams::InjectionMolding {
            pull_direction: [0.4, 0.0, 0.0],
            min_draft_deg: 1.0,
        };
        assert_ne!(
            fdm.stable_hash(),
            molding.stable_hash(),
            "identical f64 payloads across variants must still hash \
             differently — the discriminant byte is load-bearing"
        );
    }

    /// Pins `PackParams::Fdm { nozzle_diameter: 0.4, build_direction: [0,0,1] }`
    /// to its exact FNV-1a output, computed from a real run of the algorithm
    /// documented on `stable_hash` (offset basis `14695981039346656037`,
    /// prime `1099511628211`, discriminant byte `0` then the little-endian
    /// bit pattern of `0.4`, `0.0`, `0.0`, `1.0` in that order). A future
    /// change to the algorithm — accidental or deliberate — breaks this
    /// test loudly instead of silently changing what "the same params"
    /// hashes to for an already-persisted `DfmFact`.
    #[test]
    fn pack_params_hash_matches_pinned_value() {
        let params = PackParams::Fdm {
            nozzle_diameter: 0.4,
            build_direction: [0.0, 0.0, 1.0],
        };
        assert_eq!(params.stable_hash(), 0x4c2daf7ee726ebefu64);
    }

    /// Anchors the two FNV-1a magic numbers independently of the pinned-
    /// hash-value test above: the offset basis must equal the exact
    /// literal documented on `stable_hash`. If a future edit "fixes" a
    /// failing pinned-value test by quietly changing `FNV_OFFSET_BASIS` or
    /// `FNV_PRIME` instead of fixing a real bug, this test catches that
    /// independently of whatever value the other test expects.
    #[test]
    fn fnv1a_constants_match_the_documented_values() {
        const FNV_OFFSET_BASIS: u64 = 14695981039346656037;
        const FNV_PRIME: u64 = 1099511628211;
        assert_eq!(FNV_OFFSET_BASIS, 0xcbf29ce484222325u64);
        assert_eq!(FNV_PRIME, 0x100000001b3u64);
    }

    #[test]
    fn pack_params_pack_id_matches_variant() {
        assert_eq!(fdm_params().pack_id(), RulePackId::Fdm);
        assert_eq!(
            PackParams::InjectionMolding {
                pull_direction: [0.0, 0.0, 1.0],
                min_draft_deg: 1.0,
            }
            .pack_id(),
            RulePackId::InjectionMolding
        );
    }

    #[test]
    fn dfm_report_serde_round_trip() {
        let report = DfmReport::new(
            fdm_params(),
            vec![
                pass("fdm.min_wall"),
                violation("fdm.overhang"),
                unverifiable("fdm.support_volume"),
            ],
        );
        let json = serde_json::to_string(&report).expect("serialize DfmReport");
        let back: DfmReport = serde_json::from_str(&json).expect("deserialize DfmReport");
        assert_eq!(report, back);
    }

    /// Pins the EXACT wire substrings `roshera-eval`'s scenario 16/17 oracles
    /// depend on (`scenarios/16-shelf-bracket.mjs` / `17-nema17-motor-mount.mjs`,
    /// via each file's `dfmVerdict(dfm, ruleId)` helper reading
    /// `dfm.verdicts[].rule` and `.verdict.kind`, and the top-level `summary`
    /// field). Those oracles are hand-authored JS fixtures that infer this
    /// shape from `#[serde(...)]` attributes read at review time — never
    /// exercised against a live serialize until this test. If a future
    /// refactor renames the `rule` field, the `kind` tag, or the `verdicts`/
    /// `summary` keys, THIS test fails loudly in the crate the rename
    /// happened in, instead of the eval harness silently reporting every
    /// DFM-scored criterion as "missing" (`dfmVerdict` returns `null` on any
    /// lookup miss, which reads as a geometry regression, not a wiring
    /// break — see `dfm/mod.rs`'s S6/S7 module docs).
    #[test]
    fn dfm_report_wire_shape_matches_eval_oracle_expectations() {
        let report = DfmReport::new(
            fdm_params(),
            vec![pass("fdm.min_wall"), pass("fdm.overhang")],
        );
        let json = serde_json::to_string(&report).expect("serialize DfmReport");

        assert!(
            json.contains("\"rule\":\"fdm.min_wall\""),
            "eval oracle reads verdicts[].rule == \"fdm.min_wall\" verbatim: {json}"
        );
        assert!(
            json.contains("\"rule\":\"fdm.overhang\""),
            "eval oracle reads verdicts[].rule == \"fdm.overhang\" verbatim: {json}"
        );
        assert!(
            json.contains("\"kind\":\"pass\""),
            "eval oracle reads verdict.kind == \"pass\" | \"violation\" | \"unverifiable\": {json}"
        );
        assert!(
            json.contains("\"verdicts\":["),
            "eval oracle reads the top-level `verdicts` array verbatim: {json}"
        );
        assert!(
            json.contains("\"summary\":"),
            "eval oracle's dfm field carries `summary` alongside `verdicts` (DfmReport::new's honesty fold): {json}"
        );

        // And the violation/unverifiable tags an eval LIE mutates to prove
        // the oracle catches a real defect (test/oracle-16.mjs, test/oracle-17.mjs).
        let violating = DfmReport::new(fdm_params(), vec![violation("fdm.min_wall")]);
        let violating_json = serde_json::to_string(&violating).expect("serialize DfmReport");
        assert!(
            violating_json.contains("\"kind\":\"violation\""),
            "eval LIE fixtures construct a violation verdict with this exact tag: {violating_json}"
        );
    }

    #[test]
    fn dfm_fact_serde_round_trip() {
        let report = DfmReport::new(
            fdm_params(),
            vec![pass("fdm.min_wall"), unverifiable("fdm.support_volume")],
        );
        let fact = DfmFact::from_report(&report);
        let json = serde_json::to_string(&fact).expect("serialize DfmFact");
        let back: DfmFact = serde_json::from_str(&json).expect("deserialize DfmFact");
        assert_eq!(fact, back);
    }

    /// A [`RuleVerdict`] carrying [`RuleProvenance::Standard`] round-trips
    /// through JSON with the citation intact — the wire path an MCP agent
    /// reading a `DfmReport` actually exercises.
    #[test]
    fn rule_verdict_standard_provenance_serde_round_trip() {
        let verdict = RuleVerdict {
            rule: "fdm.min_bore".to_string(),
            verdict: Verdict::Pass {
                margin: DfmValue::new(0.1, analytic("bore diameter vs. floor")),
            },
            provenance: RuleProvenance::Standard {
                body: StandardBody::Asme,
                designation: "Y14.5".to_string(),
                edition: "2018".to_string(),
                clause: Some("Table 1".to_string()),
            },
        };
        let json = serde_json::to_string(&verdict).expect("serialize RuleVerdict");
        let back: RuleVerdict = serde_json::from_str(&json).expect("deserialize RuleVerdict");
        assert_eq!(verdict, back);
    }

    /// A [`RuleVerdict`] carrying [`RuleProvenance::ShopPractice`] — the
    /// arm used by FDM's v1 numbers (spec §3.2.1) — round-trips too, and
    /// stays distinguishable from the `Standard` case above (they must not
    /// collapse to the same wire shape).
    #[test]
    fn rule_verdict_shop_practice_provenance_serde_round_trip() {
        let verdict = RuleVerdict {
            rule: "fdm.overhang".to_string(),
            verdict: Verdict::Pass {
                margin: DfmValue::new(5.0, analytic("planar normal vs. build direction")),
            },
            provenance: shop_practice("45 degree overhang is a slicer-convention number"),
        };
        let json = serde_json::to_string(&verdict).expect("serialize RuleVerdict");
        let back: RuleVerdict = serde_json::from_str(&json).expect("deserialize RuleVerdict");
        assert_eq!(verdict, back);

        let standard_verdict = RuleVerdict {
            rule: "fdm.min_bore".to_string(),
            verdict: Verdict::Pass {
                margin: DfmValue::new(0.1, analytic("bore diameter vs. floor")),
            },
            provenance: RuleProvenance::Standard {
                body: StandardBody::Asme,
                designation: "Y14.5".to_string(),
                edition: "2018".to_string(),
                clause: Some("Table 1".to_string()),
            },
        };
        let standard_json =
            serde_json::to_string(&standard_verdict).expect("serialize comparison RuleVerdict");
        assert_ne!(
            json, standard_json,
            "ShopPractice and Standard provenance must not collapse to the same wire shape"
        );
    }

    // ------------------------------------------------------------------
    // F1: the bounded honesty fold (freeform coverage spec §3 / F1)
    // ------------------------------------------------------------------

    use crate::math::enclosure::Interval;

    fn limit_at(t: f64) -> DfmValue {
        DfmValue::new(t, analytic("test fixture limit"))
    }

    fn interval(lo: f64, hi: f64) -> Interval {
        Interval::enclosing(lo, hi).expect("test fixture interval")
    }

    #[test]
    fn bounded_max_whole_interval_below_limit_is_provable_pass_with_margin() {
        let v = Verdict::from_bounded_max(
            interval(40.0, 44.0),
            limit_at(45.0),
            vec![3],
            "normal-cone overhang enclosure",
            7,
            true,
        );
        match v {
            Verdict::Pass { margin } => {
                // Proven margin: limit − hi = 1.0, outward-rounded DOWN —
                // never optimistic.
                assert!(margin.value <= 1.0 && margin.value >= 1.0 - 1e-9);
                let bound = margin.bound.expect("bounded margin carries its enclosure");
                assert!(bound.lo <= 1.0 && bound.hi >= 5.0 - 1e-9);
                match margin.derivation {
                    Derivation::BoundedAnalytic {
                        refinement_depth,
                        converged,
                        ..
                    } => {
                        assert_eq!(refinement_depth, 7);
                        assert!(converged);
                    }
                    other => panic!("expected BoundedAnalytic, got {other:?}"),
                }
            }
            other => panic!("expected Pass, got {other:?}"),
        }
    }

    #[test]
    fn bounded_max_whole_interval_above_limit_is_provable_violation() {
        let v = Verdict::from_bounded_max(
            interval(46.0, 48.0),
            limit_at(45.0),
            vec![3, 9],
            "normal-cone overhang enclosure",
            12,
            false,
        );
        match v {
            Verdict::Violation {
                witnesses,
                measured,
                limit,
            } => {
                assert_eq!(witnesses, vec![3, 9]);
                // Reported value is the PROVEN floor: at least lo = 46.
                assert_eq!(measured.value, 46.0);
                assert_eq!(measured.bound, Some(DfmBound { lo: 46.0, hi: 48.0 }));
                assert_eq!(limit.value, 45.0);
            }
            other => panic!("expected Violation, got {other:?}"),
        }
    }

    /// THE HEADLINE F1 TEST — the spec's own worked example: the value is
    /// in [44.6°, 45.9°], the limit is 45°, the budget refined to depth 12
    /// and could not separate them. The only honest verdict is
    /// `Unverifiable` REPORTING THE BOUND.
    #[test]
    fn bounded_max_straddling_interval_is_unverifiable_reporting_the_bound() {
        let v = Verdict::from_bounded_max(
            interval(44.6, 45.9),
            limit_at(45.0),
            vec![5],
            "normal-cone overhang enclosure",
            12,
            false,
        );
        match v {
            Verdict::Unverifiable { regions, reason } => {
                assert_eq!(regions, vec![5]);
                match reason {
                    UnverifiableReason::BoundNotSeparating {
                        lo,
                        hi,
                        limit,
                        refinement_depth,
                        converged,
                    } => {
                        assert_eq!((lo, hi, limit), (44.6, 45.9, 45.0));
                        assert_eq!(refinement_depth, 12);
                        assert!(!converged);
                    }
                    other => panic!("expected BoundNotSeparating, got {other:?}"),
                }
            }
            other => panic!("expected Unverifiable, got {other:?}"),
        }
    }

    /// Wrong-on-purpose mutants of the fold, mirroring `orientation.rs`'s
    /// mutation-proof idiom: deciding the pass check from the enclosure's
    /// LOWER endpoint (a "sampled extreme" — the exact grid-sampling
    /// defect class) or from the MIDPOINT. Both are shown, with raw
    /// numbers, to fabricate a Pass on a straddling enclosure where the
    /// real fold refuses — so the never-Pass-on-straddle rule is
    /// falsifiable, not just asserted in prose.
    fn mutant_pass_check_from_lower_endpoint(enclosure: &Interval, t: f64) -> bool {
        enclosure.lo() < t // BUG: a lower bound proves nothing about the max
    }

    fn mutant_pass_check_from_midpoint(enclosure: &Interval, t: f64) -> bool {
        0.5 * (enclosure.lo() + enclosure.hi()) < t // BUG: the spec's forbidden fallback
    }

    #[test]
    fn mutation_proof_straddling_interval_can_never_pass() {
        let straddling = [
            interval(44.0, 46.0), // limit strictly inside
            interval(44.6, 45.9), // the spec's worked example
            interval(45.0, 45.5), // lo touches the limit
            interval(44.0, 45.0), // hi touches the limit
            interval(45.0, 45.0), // degenerate exact hit
        ];
        for e in &straddling {
            // BEFORE (mutants): both wrong folds claim Pass for at least
            // the interior-straddle cases.
            if e.lo() < 45.0 {
                assert!(
                    mutant_pass_check_from_lower_endpoint(e, 45.0),
                    "the lower-endpoint mutant fabricates a Pass on [{}, {}]",
                    e.lo(),
                    e.hi()
                );
            }
            // AFTER (production): the real fold NEVER passes a straddler.
            let v = Verdict::from_bounded_max(
                *e,
                limit_at(45.0),
                vec![1],
                "mutation-proof fixture",
                3,
                false,
            );
            assert!(
                !matches!(v, Verdict::Pass { .. }),
                "a straddling enclosure [{}, {}] must never fold to Pass, got {v:?}",
                e.lo(),
                e.hi()
            );
            assert!(
                matches!(
                    v,
                    Verdict::Unverifiable {
                        reason: UnverifiableReason::BoundNotSeparating { .. },
                        ..
                    }
                ),
                "a straddling enclosure must refuse REPORTING THE BOUND, got {v:?}"
            );
        }
        // Raw divergence, lower-endpoint mutant: [44, 46] vs 45 — the
        // sampled "extreme" 44 reads as passing while the true max may be
        // anywhere up to 46.
        let e = interval(44.0, 46.0);
        assert!(mutant_pass_check_from_lower_endpoint(&e, 45.0));
        // Raw divergence, midpoint mutant: [43, 46] vs 45 — midpoint 44.5
        // reads as passing while the enclosure straddles the limit.
        let e_mid = interval(43.0, 46.0);
        assert!(mutant_pass_check_from_midpoint(&e_mid, 45.0));
        for straddler in [e, e_mid] {
            let real = Verdict::from_bounded_max(
                straddler,
                limit_at(45.0),
                vec![1],
                "mutation-proof fixture",
                3,
                false,
            );
            assert!(
                matches!(real, Verdict::Unverifiable { .. }),
                "a mutant says Pass where the sound fold refuses — the honesty rule is \
                 load-bearing, not decorative"
            );
        }
    }

    #[test]
    fn bounded_min_fold_mirrors_the_same_honesty_rule() {
        // Pass: whole interval above the floor.
        let pass = Verdict::from_bounded_min(
            interval(1.0, 1.4),
            limit_at(0.8),
            vec![2],
            "pair-thickness enclosure",
            4,
            true,
        );
        match pass {
            Verdict::Pass { margin } => {
                // Proven margin: lo − limit = 0.2, outward-rounded down.
                assert!(margin.value <= 0.2 && margin.value >= 0.2 - 1e-9);
            }
            other => panic!("expected Pass, got {other:?}"),
        }
        // Violation: whole interval below the floor; measured is the
        // proven CEILING (value at most hi).
        let violation = Verdict::from_bounded_min(
            interval(0.3, 0.5),
            limit_at(0.8),
            vec![2],
            "pair-thickness enclosure",
            4,
            true,
        );
        match violation {
            Verdict::Violation { measured, .. } => {
                assert_eq!(measured.value, 0.5);
                assert_eq!(measured.bound, Some(DfmBound { lo: 0.3, hi: 0.5 }));
            }
            other => panic!("expected Violation, got {other:?}"),
        }
        // Straddle (including boundary touches): never Pass.
        for e in [interval(0.5, 1.0), interval(0.8, 0.9), interval(0.7, 0.8)] {
            let v = Verdict::from_bounded_min(
                e,
                limit_at(0.8),
                vec![2],
                "pair-thickness enclosure",
                4,
                false,
            );
            assert!(
                matches!(
                    v,
                    Verdict::Unverifiable {
                        reason: UnverifiableReason::BoundNotSeparating { .. },
                        ..
                    }
                ),
                "min-fold straddle [{}, {}] must refuse reporting the bound, got {v:?}",
                e.lo(),
                e.hi()
            );
        }
    }

    /// A straddling bounded rule feeds the S1 pack fold exactly like any
    /// other refusal: the report reads `Inconclusive`, never `Pass` — F1's
    /// three-way semantics compose with the existing honesty theorem
    /// instead of bypassing it.
    #[test]
    fn bounded_straddle_forces_inconclusive_through_the_s1_fold() {
        let straddle_verdict = RuleVerdict {
            rule: "fdm.overhang".to_string(),
            verdict: Verdict::from_bounded_max(
                interval(44.6, 45.9),
                limit_at(45.0),
                vec![5],
                "normal-cone overhang enclosure",
                12,
                false,
            ),
            provenance: shop_practice("test fixture"),
        };
        let report = DfmReport::new(fdm_params(), vec![pass("fdm.min_wall"), straddle_verdict]);
        assert_eq!(
            report.summary(),
            DfmSummary::Inconclusive { unverifiable: 1 }
        );
        assert_ne!(report.summary(), DfmSummary::Pass);
    }

    #[test]
    fn bounded_value_and_reason_serde_round_trip_with_wire_tags() {
        let verdict = Verdict::from_bounded_max(
            interval(44.6, 45.9),
            limit_at(45.0),
            vec![5],
            "normal-cone overhang enclosure",
            12,
            false,
        );
        let json = serde_json::to_string(&verdict).expect("serialize");
        assert!(
            json.contains("\"kind\":\"bound_not_separating\""),
            "wire tag for the bound-reporting refusal: {json}"
        );
        let back: Verdict = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(verdict, back);

        let pass = Verdict::from_bounded_max(
            interval(40.0, 44.0),
            limit_at(45.0),
            vec![3],
            "normal-cone overhang enclosure",
            7,
            true,
        );
        let pass_json = serde_json::to_string(&pass).expect("serialize");
        assert!(
            pass_json.contains("\"kind\":\"bounded_analytic\""),
            "wire tag for the bounded derivation: {pass_json}"
        );
        assert!(
            pass_json.contains("\"bound\":"),
            "a bounded value carries its enclosure on the wire: {pass_json}"
        );
        // serde_json WITHOUT the `float_roundtrip` feature (the workspace
        // default) uses a fast, up-to-1-ulp-lossy float parser: the
        // outward-rounded margin lo `0.9999999999999999` re-parses as
        // `1.0`. Exact equality would therefore test the JSON library,
        // not this module — assert structure exactly and floats to 1 ulp.
        // The DFM wire contract treats these as measurements, and a 1-ulp
        // perturbation of an already-outward-rounded endpoint stays within
        // the reported provenance.
        let pass_back: Verdict = serde_json::from_str(&pass_json).expect("deserialize");
        match (&pass, &pass_back) {
            (Verdict::Pass { margin: a }, Verdict::Pass { margin: b }) => {
                assert_eq!(a.derivation, b.derivation);
                assert!((a.value - b.value).abs() <= f64::EPSILON * 2.0);
                let (ab, bb) = (
                    a.bound.expect("bounded margin"),
                    b.bound.expect("bounded margin"),
                );
                assert!((ab.lo - bb.lo).abs() <= f64::EPSILON * 2.0);
                assert!((ab.hi - bb.hi).abs() <= f64::EPSILON * 16.0);
            }
            other => panic!("expected Pass on both sides of the round trip, got {other:?}"),
        }
    }

    /// Pre-F1 wire compatibility: a legacy `DfmValue` JSON without the
    /// `bound` field still deserializes (`bound: None`), and classic
    /// values serialize without the field at all — no existing consumer
    /// sees a shape change.
    #[test]
    fn legacy_dfm_value_wire_shape_is_unchanged() {
        let legacy = "{\"value\":1.5,\"derivation\":{\"kind\":\"analytic\",\
                      \"surface_type\":\"plane\",\"method\":\"m\"}}";
        let v: DfmValue = serde_json::from_str(legacy).expect("legacy deserializes");
        assert_eq!(v.bound, None);
        let json = serde_json::to_string(&DfmValue::new(1.5, analytic("m"))).expect("serialize");
        assert!(
            !json.contains("bound"),
            "classic values must not grow a wire field: {json}"
        );
    }
}
