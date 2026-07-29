//! FDM (fused deposition modeling) rule pack v1 (spec §3.2).
//!
//! Ships TWO rules: [`evaluate_overhang`] (`fdm.overhang`, S2) and
//! [`evaluate_min_wall`] (`fdm.min_wall`, S3 — this addition, riding
//! [`pair_thickness`]). The remaining v1 rules (`fdm.min_bore`,
//! `fdm.trapped_volume`, `fdm.support_volume`) need analyzers that do not
//! exist yet (`bore_metrics`, `internal_voids` — spec S4/S5) and are out
//! of scope here.

use std::collections::BTreeSet;

use crate::dfm::analyzers::{face_orientation_field, pair_thickness, OrientationOutcome};
use crate::dfm::packs::{Rule, RulePack};
use crate::dfm::provenance::RuleProvenance;
use crate::dfm::report::{
    Derivation, DfmError, DfmReport, DfmValue, FaceRef, PackParams, RuleVerdict, SurfaceKind,
    UnverifiableReason, Verdict,
};
use crate::math::Vector3;
use crate::primitives::curve::CurveStore;
use crate::primitives::edge::EdgeStore;
use crate::primitives::face::{FaceId, FaceStore};
use crate::primitives::r#loop::LoopStore;
use crate::primitives::surface::SurfaceStore;

/// Stable rule id, matches [`crate::dfm::report::RuleVerdict::rule`] and
/// eval-16/17's criterion name verbatim.
pub const OVERHANG_RULE_ID: &str = "fdm.overhang";

/// Practice-derived provenance for `fdm.overhang` (spec §3.2.1 "known
/// landscape": additive/DfAM's governing lineage is ISO/ASTM 52900
/// series, which covers design *guidelines* generally — the specific 45°
/// figure used here is a slicer/shop convention, not a cited clause of
/// any edition. Per the module's non-negotiable discipline, an
/// unconfirmed citation is never invented; this stays `ShopPractice` and
/// says so.
pub fn overhang_provenance() -> RuleProvenance {
    RuleProvenance::ShopPractice {
        note: "45° overhang; practice-derived, no governing standard".to_string(),
    }
}

/// Stable rule id for `fdm.min_wall` (spec §3.2), matches eval-16/17's
/// "wall thickness" criterion verbatim.
pub const MIN_WALL_RULE_ID: &str = "fdm.min_wall";

/// Practice-derived provenance for `fdm.min_wall` (spec §3.2.1 "known
/// landscape": the additive/DfAM governing lineage — ISO/ASTM 52900
/// series — covers design *guidelines*, not a specific numeric wall-
/// thickness-vs-nozzle-diameter multiplier; "2× nozzle diameter" is a
/// widely-used slicer/shop convention, not a cited clause of any edition.
/// Per the module's non-negotiable discipline, this stays `ShopPractice`.
pub fn min_wall_provenance() -> RuleProvenance {
    RuleProvenance::ShopPractice {
        note: "wall thickness >= 2x nozzle diameter; practice-derived, no governing standard"
            .to_string(),
    }
}

/// The FDM pack's declared rule list (spec §3.2) tied to `params` the same
/// way [`DfmReport`] ties itself to its own params (see
/// [`crate::dfm::packs::RulePack`]).
pub fn rule_pack(params: PackParams) -> RulePack {
    RulePack {
        params,
        rules: vec![
            Rule {
                id: OVERHANG_RULE_ID,
                provenance: overhang_provenance(),
            },
            Rule {
                id: MIN_WALL_RULE_ID,
                provenance: min_wall_provenance(),
            },
        ],
    }
}

/// A downward-facing region is a violation once it is steeper than this
/// many degrees FROM VERTICAL (spec §3.2, eval-16/17 criterion verbatim).
const OVERHANG_THRESHOLD_DEG: f64 = 45.0;

/// Convert [`face_orientation_field`]'s angle convention — `θ` = angle
/// between the face's OUTWARD normal and `build_direction`; `0°` = normal
/// exactly ALONG build, `180°` = normal exactly AGAINST build (see
/// `analyzers/orientation.rs` module docs, "state it once, test it hard")
/// — into "degrees from vertical", the axis `fdm.overhang`'s 45°
/// threshold is actually stated in.
///
/// ## Hand-check (paper, before coding the comparison)
///
/// - A vertical WALL (e.g. a cylinder side wall) has its normal
///   perpendicular to the build axis: `θ = 90°`. It IS vertical, i.e. 0°
///   from vertical, and never needs support: `90° − 90° = 0°`. ✓
/// - A horizontal, downward-facing ceiling (the underside of a flat
///   shelf) has its normal exactly ANTI-parallel to build: `θ = 180°`. It
///   is fully horizontal, i.e. 90° from vertical — the worst case:
///   `180° − 90° = 90°`. ✓
/// - The textbook 45°-overhang borderline (the underside of a 45°
///   chamfer, sloped exactly halfway between a wall and a ceiling) has
///   its normal halfway between "horizontal" (`θ=90°`, a wall) and
///   "straight down" (`θ=180°`): `θ = 135°`. `135° − 90° = 45°`, exactly
///   the threshold, as it must be. ✓
///
/// So `degrees_from_vertical = θ − 90°`, meaningful as an OVERHANG
/// reading only for `θ > 90°` (a normal with any downward component at
/// all). A face with `θ ≤ 90°` (upward- or exactly sideways-facing) is
/// never an overhang candidate; its `degrees_from_vertical` comes out
/// `≤ 0°`, which sorts as "maximally safe" in the aggregation below
/// rather than needing a separate branch.
fn degrees_from_vertical(theta_deg: f64) -> f64 {
    theta_deg - 90.0
}

/// Re-tag an analyzer-produced [`Derivation`] to note the linear
/// degrees-from-vertical conversion applied on top of it, so `measured`/
/// `margin` stay traceable to the exact closed-form method that produced
/// the raw angle (spec §2.2: no number without provenance) without
/// inventing a new `Derivation` variant this module does not own.
fn as_overhang_derivation(inner: Derivation) -> Derivation {
    match inner {
        Derivation::Analytic {
            surface_type,
            method,
        } => Derivation::Analytic {
            surface_type,
            method: format!("{method}; fdm.overhang reads degrees-from-vertical = θ − 90°"),
        },
    }
}

/// Derivation for a fixed pack-configured constant (the 45° threshold
/// itself, or a "no candidate faces" fallback) rather than a value
/// measured off a specific face. `Derivation` has only the `Analytic`
/// variant (no analyzer has needed a second one yet), so this follows the
/// existing in-tree precedent for tagging a non-geometric constant:
/// `report.rs`'s own test fixtures tag a fixed rule threshold (e.g. "2x
/// nozzle diameter") as `Analytic { surface_type: Plane, method: .. } —
/// the `method` string is what actually carries the honest meaning here.
fn constant_derivation(method: &str) -> Derivation {
    Derivation::Analytic {
        surface_type: SurfaceKind::Plane,
        method: method.to_string(),
    }
}

/// One face's contribution to `fdm.overhang`'s aggregate verdict — not
/// itself a Pass/Violation decision, just the exact number (with its
/// provenance) or the refusal.
enum FaceOutcome {
    Measured(f64, Derivation),
    Unverifiable(UnverifiableReason),
}

fn face_outcome(
    face_id: FaceId,
    build_direction: Vector3,
    face_store: &FaceStore,
    loop_store: &LoopStore,
    edge_store: &EdgeStore,
    curve_store: &CurveStore,
    surface_store: &SurfaceStore,
) -> Result<FaceOutcome, DfmError> {
    let outcome = face_orientation_field(
        face_id,
        build_direction,
        face_store,
        loop_store,
        edge_store,
        curve_store,
        surface_store,
    )?;
    Ok(match outcome {
        // `max_deg` is the steepest (most-against-build-direction) point
        // on the face's trimmed domain — the worst-case, exact overhang
        // reading for this face, not an approximation of it.
        OrientationOutcome::Range {
            max_deg,
            derivation,
            ..
        } => FaceOutcome::Measured(degrees_from_vertical(max_deg), derivation),
        OrientationOutcome::Unverifiable { reason } => FaceOutcome::Unverifiable(reason),
    })
}

/// Evaluate `fdm.overhang` over `faces` (every candidate face — typically
/// every face of the solid under test; see [`crate::dfm::packs`] module
/// docs for why enumerating a solid's faces is the caller's job in this
/// slice). See [`degrees_from_vertical`] for the angle-convention
/// conversion and its hand-check.
///
/// ## Multi-face aggregation (this rule's own policy — spec leaves the
/// exact shape open, `report.rs` only mandates the fold ACROSS RULES)
///
/// One rule ⇒ one [`RuleVerdict`], but a solid has many faces. Faces are
/// folded with the SAME precedence spec §3.3 already uses across rules,
/// applied here across faces of one rule: a proven [`Verdict::Violation`]
/// on any face dominates — never silently smoothed over by other faces
/// passing — and only when NO face violates does an
/// [`Verdict::Unverifiable`] face force the rule itself to read
/// `Unverifiable` (never `Pass`, honoring the same honesty theorem
/// `report.rs` states for the whole report). `measured`/`margin` report
/// the SINGLE worst-case face's exact value; `witnesses`/`regions` name
/// EVERY face that actually violates / was refused (ascending `FaceId`
/// order, spec §3.3: "analyzer-defined order").
pub fn evaluate_overhang(
    faces: &[FaceId],
    build_direction: Vector3,
    face_store: &FaceStore,
    loop_store: &LoopStore,
    edge_store: &EdgeStore,
    curve_store: &CurveStore,
    surface_store: &SurfaceStore,
) -> Result<RuleVerdict, DfmError> {
    let mut violations: Vec<(FaceId, f64, Derivation)> = Vec::new();
    let mut unverifiable: Vec<(FaceId, UnverifiableReason)> = Vec::new();
    let mut best_safe: Option<(f64, Derivation)> = None;

    for &face_id in faces {
        match face_outcome(
            face_id,
            build_direction,
            face_store,
            loop_store,
            edge_store,
            curve_store,
            surface_store,
        )? {
            FaceOutcome::Measured(deg, derivation) => {
                // Mutation-proof target (see task report): this is the
                // ONE comparison that decides overhang violation. Flipping
                // it (`<=` instead of `>`) makes every safe face read as a
                // violation and vice versa — the thesis test is built to
                // catch exactly that flip.
                if deg > OVERHANG_THRESHOLD_DEG {
                    violations.push((face_id, deg, derivation));
                } else {
                    let replace = match &best_safe {
                        Some((current, _)) => deg > *current,
                        None => true,
                    };
                    if replace {
                        best_safe = Some((deg, derivation));
                    }
                }
            }
            FaceOutcome::Unverifiable(reason) => unverifiable.push((face_id, reason)),
        }
    }

    if !violations.is_empty() {
        violations.sort_by_key(|(id, _, _)| *id);
        // Non-empty by construction (the `if` above); `unwrap_or` supplies
        // a total, panic-free fallback rather than an `.expect()` on a
        // branch that cannot actually miss.
        let (worst_deg, worst_derivation) = violations
            .iter()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(_, deg, derivation)| (*deg, derivation.clone()))
            .unwrap_or((
                OVERHANG_THRESHOLD_DEG,
                constant_derivation("unreachable: violations checked non-empty above"),
            ));
        let witnesses: Vec<FaceRef> = violations.into_iter().map(|(id, _, _)| id).collect();
        return Ok(RuleVerdict {
            rule: OVERHANG_RULE_ID.to_string(),
            verdict: Verdict::Violation {
                witnesses,
                measured: DfmValue::new(worst_deg, as_overhang_derivation(worst_derivation)),
                limit: DfmValue::new(
                    OVERHANG_THRESHOLD_DEG,
                    constant_derivation(
                        "fdm.overhang threshold: 45° from vertical (shop practice)",
                    ),
                ),
            },
            provenance: overhang_provenance(),
        });
    }

    if !unverifiable.is_empty() {
        unverifiable.sort_by_key(|(id, _)| *id);
        // Deterministic choice of ONE reason for the rule-level refusal
        // (Verdict::Unverifiable carries a single `reason`, not one per
        // region): the lowest-FaceId region's reason, independent of
        // traversal order.
        let reason = unverifiable[0].1.clone();
        let regions: Vec<FaceRef> = unverifiable.into_iter().map(|(id, _)| id).collect();
        return Ok(RuleVerdict {
            rule: OVERHANG_RULE_ID.to_string(),
            verdict: Verdict::Unverifiable { regions, reason },
            provenance: overhang_provenance(),
        });
    }

    let (worst_safe_deg, worst_safe_derivation) = best_safe.unwrap_or((
        -90.0,
        constant_derivation("fdm.overhang: no candidate faces supplied"),
    ));
    Ok(RuleVerdict {
        rule: OVERHANG_RULE_ID.to_string(),
        verdict: Verdict::Pass {
            margin: DfmValue::new(
                OVERHANG_THRESHOLD_DEG - worst_safe_deg,
                as_overhang_derivation(worst_safe_derivation),
            ),
        },
        provenance: overhang_provenance(),
    })
}

/// A sentinel "wall thickness" used ONLY for the vacuous case — the
/// candidate face list contains no proven wall pair AND no unverifiable
/// region at all (e.g. an empty face list, or a solid with no analytic
/// wall-forming faces). Mirrors [`evaluate_overhang`]'s own `-90.0`
/// "no candidate faces" fallback: a Pass with a deliberately huge (but
/// finite — `serde_json` cannot round-trip `f64::INFINITY`) margin, never
/// a fabricated real measurement. Named, not a magic number: 1e300 is far
/// beyond any real-world dimension, so the resulting margin can never be
/// mistaken for an actual measured wall.
const NO_PAIR_SENTINEL_THICKNESS: f64 = 1.0e300;

/// Evaluate `fdm.min_wall` (spec §3.2: wall thickness ≥ 2× nozzle
/// diameter) over `faces`, riding [`pair_thickness`] (spec S3).
///
/// ## Aggregation policy (mirrors [`evaluate_overhang`]'s own multi-face
/// policy, applied here across PAIRS rather than single faces)
///
/// A proven [`Verdict::Violation`] on any pair dominates: `measured` is
/// the single THINNEST violating pair's thickness (the worst case);
/// `witnesses` is the UNION of every face implicated across every
/// violating pair (ascending `FaceId` order, deduplicated — `report.rs`'s
/// own doc on [`Verdict::Violation::witnesses`] states "names every face
/// implicated", not just the worst pair's two). Only when there are ZERO
/// violating pairs does an [`crate::dfm::report::UnverifiableReason`]
/// region force the rule to read `Unverifiable` (never `Pass` — the same
/// honesty theorem [`evaluate_overhang`] and `report.rs` both apply).
pub fn evaluate_min_wall(
    faces: &[FaceId],
    nozzle_diameter: f64,
    face_store: &FaceStore,
    loop_store: &LoopStore,
    edge_store: &EdgeStore,
    curve_store: &CurveStore,
    surface_store: &SurfaceStore,
) -> Result<RuleVerdict, DfmError> {
    let outcome = pair_thickness(
        faces,
        face_store,
        loop_store,
        edge_store,
        curve_store,
        surface_store,
    )?;
    let threshold = 2.0 * nozzle_diameter;

    let mut violations: Vec<(FaceId, FaceId, f64, Derivation)> = Vec::new();
    let mut best_safe: Option<(f64, Derivation)> = None;

    for pair in &outcome.pairs {
        if pair.thickness.value < threshold {
            violations.push((
                pair.face_a,
                pair.face_b,
                pair.thickness.value,
                pair.thickness.derivation.clone(),
            ));
        } else {
            // Mutation-proof target (see task report): this is the ONE
            // comparison deciding min_wall violation. Worst-case SAFE pair
            // is the THINNEST one (closest to the threshold from above),
            // mirroring `evaluate_overhang`'s "closest to violating"
            // worst-safe policy.
            let replace = match &best_safe {
                Some((current, _)) => pair.thickness.value < *current,
                None => true,
            };
            if replace {
                best_safe = Some((pair.thickness.value, pair.thickness.derivation.clone()));
            }
        }
    }

    if !violations.is_empty() {
        let (_, _, worst_thickness, worst_derivation) = violations
            .iter()
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(a, b, t, d)| (*a, *b, *t, d.clone()))
            .unwrap_or((
                0,
                0,
                threshold,
                constant_derivation("unreachable: violations checked non-empty above"),
            ));
        let mut witness_set: BTreeSet<FaceId> = BTreeSet::new();
        for (a, b, _, _) in &violations {
            witness_set.insert(*a);
            witness_set.insert(*b);
        }
        let witnesses: Vec<FaceRef> = witness_set.into_iter().collect();
        return Ok(RuleVerdict {
            rule: MIN_WALL_RULE_ID.to_string(),
            verdict: Verdict::Violation {
                witnesses,
                measured: DfmValue::new(worst_thickness, worst_derivation),
                limit: DfmValue::new(
                    threshold,
                    constant_derivation(
                        "fdm.min_wall threshold: 2x nozzle diameter (shop practice)",
                    ),
                ),
            },
            provenance: min_wall_provenance(),
        });
    }

    if !outcome.unverifiable.is_empty() {
        let mut regions: Vec<FaceRef> = outcome.unverifiable.iter().map(|u| u.face).collect();
        regions.sort_unstable();
        // Deterministic choice of ONE reason (see `evaluate_overhang`'s
        // identical convention): the lowest-FaceId region's reason. The
        // pair_thickness analyzer already returns `unverifiable` sorted
        // ascending by face (it folds a `BTreeMap`), so `[0]` is already
        // the lowest — re-deriving via `regions[0]` after the explicit
        // sort above keeps this independent of that internal ordering
        // detail.
        let reason = outcome
            .unverifiable
            .iter()
            .find(|u| u.face == regions[0])
            .map(|u| u.reason.clone())
            .unwrap_or_else(|| UnverifiableReason::UnsupportedTopology {
                detail: "unreachable: regions derived from outcome.unverifiable above".to_string(),
            });
        return Ok(RuleVerdict {
            rule: MIN_WALL_RULE_ID.to_string(),
            verdict: Verdict::Unverifiable { regions, reason },
            provenance: min_wall_provenance(),
        });
    }

    let (safe_thickness, safe_derivation) = best_safe.unwrap_or((
        NO_PAIR_SENTINEL_THICKNESS,
        constant_derivation("fdm.min_wall: no wall pairs found among candidate faces"),
    ));
    Ok(RuleVerdict {
        rule: MIN_WALL_RULE_ID.to_string(),
        verdict: Verdict::Pass {
            margin: DfmValue::new(safe_thickness - threshold, safe_derivation),
        },
        provenance: min_wall_provenance(),
    })
}

/// The FDM pack's `evaluate()` arm (spec §3.2 params: `nozzle_diameter`,
/// `build_direction`; defaults 0.4 mm, +Z). Runs BOTH FDM rules —
/// `fdm.overhang` (S2) and `fdm.min_wall` (S3) — and folds across them via
/// [`DfmReport::new`]'s honesty fold.
pub fn evaluate(
    faces: &[FaceId],
    nozzle_diameter: f64,
    build_direction: [f64; 3],
    face_store: &FaceStore,
    loop_store: &LoopStore,
    edge_store: &EdgeStore,
    curve_store: &CurveStore,
    surface_store: &SurfaceStore,
) -> Result<DfmReport, DfmError> {
    let dir = Vector3::new(build_direction[0], build_direction[1], build_direction[2]);
    let overhang = evaluate_overhang(
        faces,
        dir,
        face_store,
        loop_store,
        edge_store,
        curve_store,
        surface_store,
    )?;
    let min_wall = evaluate_min_wall(
        faces,
        nozzle_diameter,
        face_store,
        loop_store,
        edge_store,
        curve_store,
        surface_store,
    )?;
    Ok(DfmReport::new(
        PackParams::Fdm {
            nozzle_diameter,
            build_direction,
        },
        vec![overhang, min_wall],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dfm::packs::fixtures::plane_face_at_theta_deg;

    /// Hand-computed VIOLATION: a wedge's downward face tilted 20° off
    /// straight-down (`θ = 160°` from `+Z`) — 70° off vertical, well past
    /// the 45° threshold. `70°` is asserted exactly, not approximated:
    /// the plane's normal is an exact closed-form constant, so
    /// `degrees_from_vertical = 160° − 90° = 70°` to floating-point noise
    /// only.
    #[test]
    fn wedge_70_degrees_from_vertical_is_exact_violation() {
        let (surfaces, faces, loops, edges, curves, face_id) = plane_face_at_theta_deg(160.0);
        let verdict = evaluate_overhang(
            &[face_id],
            Vector3::Z,
            &faces,
            &loops,
            &edges,
            &curves,
            &surfaces,
        )
        .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        match verdict.verdict {
            Verdict::Violation {
                witnesses,
                measured,
                limit,
            } => {
                assert_eq!(witnesses, vec![face_id]);
                assert!(
                    (measured.value - 70.0).abs() < 1e-9,
                    "measured = {}",
                    measured.value
                );
                assert!(
                    (limit.value - OVERHANG_THRESHOLD_DEG).abs() < 1e-9,
                    "limit = {}",
                    limit.value
                );
            }
            other => panic!("expected Violation, got {other:?}"),
        }
    }

    /// Hand-computed PASS: a wedge's downward face tilted 70° off
    /// straight-down (`θ = 110°` from `+Z`) — only 20° off vertical,
    /// comfortably under the 45° threshold. Margin asserted exactly:
    /// `45° − 20° = 25°`.
    #[test]
    fn wedge_20_degrees_from_vertical_passes_with_exact_margin() {
        let (surfaces, faces, loops, edges, curves, face_id) = plane_face_at_theta_deg(110.0);
        let verdict = evaluate_overhang(
            &[face_id],
            Vector3::Z,
            &faces,
            &loops,
            &edges,
            &curves,
            &surfaces,
        )
        .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        match verdict.verdict {
            Verdict::Pass { margin } => {
                assert!(
                    (margin.value - 25.0).abs() < 1e-9,
                    "margin = {}",
                    margin.value
                );
            }
            other => panic!("expected Pass, got {other:?}"),
        }
    }

    /// Two PLANE faces sharing ONE set of stores — `plane_face_at_theta_deg`
    /// (from `packs::fixtures`) builds a fresh store set per call, which
    /// cannot represent "two faces of the same solid"; this local helper
    /// builds both directly against shared stores so a multi-face
    /// aggregation test can address them together, mirroring how
    /// multiple faces of one real solid would actually be enumerated.
    fn two_plane_faces_at(
        theta_a_deg: f64,
        theta_b_deg: f64,
    ) -> (
        crate::primitives::surface::SurfaceStore,
        FaceStore,
        LoopStore,
        EdgeStore,
        CurveStore,
        FaceId,
        FaceId,
    ) {
        use crate::math::Point3;
        use crate::primitives::face::{Face, FaceOrientation};
        use crate::primitives::r#loop::{Loop, LoopType};
        use crate::primitives::surface::{Plane, SurfaceStore};

        let mut surfaces = SurfaceStore::new();
        let mut faces = FaceStore::new();
        let mut loops = LoopStore::new();
        let curves = CurveStore::new();
        let edges = EdgeStore::new();

        let plane_at = |theta_deg: f64| {
            let theta = theta_deg.to_radians();
            let normal = Vector3::new(theta.sin(), 0.0, theta.cos());
            Plane::from_point_normal(Point3::new(0.0, 0.0, 0.0), normal)
                .unwrap_or_else(|e| panic!("valid plane fixture: {e}"))
        };

        let surface_a = surfaces.add(Box::new(plane_at(theta_a_deg)));
        let outer_a = loops.add(Loop::new(0, LoopType::Outer));
        let face_a = faces.add(Face::new(0, surface_a, outer_a, FaceOrientation::Forward));

        let surface_b = surfaces.add(Box::new(plane_at(theta_b_deg)));
        let outer_b = loops.add(Loop::new(1, LoopType::Outer));
        let face_b = faces.add(Face::new(1, surface_b, outer_b, FaceOrientation::Forward));

        (surfaces, faces, loops, edges, curves, face_a, face_b)
    }

    /// Multi-face aggregation: one safe face + one violating face on the
    /// same rule call must report the violation (dominates) and name
    /// ONLY the actually-violating face as a witness — the safe face
    /// must not appear in `witnesses` and must not suppress the
    /// violation either.
    #[test]
    fn one_violating_face_among_safe_faces_is_witnessed_alone() {
        let (surfaces, faces, loops, edges, curves, face_safe, face_violating) =
            two_plane_faces_at(90.0, 170.0); // vertical wall (safe), 80°-off-vertical (violates)

        let verdict = evaluate_overhang(
            &[face_safe, face_violating],
            Vector3::Z,
            &faces,
            &loops,
            &edges,
            &curves,
            &surfaces,
        )
        .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        match verdict.verdict {
            Verdict::Violation {
                witnesses,
                measured,
                ..
            } => {
                assert_eq!(witnesses, vec![face_violating]);
                assert!(
                    (measured.value - 80.0).abs() < 1e-9,
                    "measured = {}",
                    measured.value
                );
            }
            other => panic!("expected Violation, got {other:?}"),
        }
    }

    // ----- fdm.min_wall (S3) -----

    /// Two opposing unit-square rectangle faces separated by `width` along
    /// Z, fully overlapping footprints — the simplest possible exact wall
    /// pair. Built directly (not via boolean/extrude), same convention as
    /// `two_plane_faces_at` above and `thickness.rs`'s own test module.
    fn rectangle_wall_pair(
        width: f64,
    ) -> (
        crate::primitives::surface::SurfaceStore,
        FaceStore,
        LoopStore,
        EdgeStore,
        CurveStore,
        FaceId,
        FaceId,
    ) {
        use crate::math::Point3;
        use crate::primitives::curve::{Line, ParameterRange};
        use crate::primitives::edge::{Edge, EdgeOrientation};
        use crate::primitives::face::{Face, FaceOrientation};
        use crate::primitives::r#loop::{Loop, LoopType};
        use crate::primitives::surface::{Plane, SurfaceStore};

        let mut surfaces = SurfaceStore::new();
        let mut faces = FaceStore::new();
        let mut loops = LoopStore::new();
        let mut edges = EdgeStore::new();
        let mut curves = CurveStore::new();

        fn add_face(
            surfaces: &mut SurfaceStore,
            faces: &mut FaceStore,
            loops: &mut LoopStore,
            edges: &mut EdgeStore,
            curves: &mut CurveStore,
            z: f64,
            normal: Vector3,
        ) -> FaceId {
            let plane = Plane::from_point_normal(Point3::new(0.0, 0.0, z), normal)
                .unwrap_or_else(|e| panic!("valid plane fixture: {e}"));
            let surface_id = surfaces.add(Box::new(plane));

            let corners = [
                Point3::new(0.0, 0.0, z),
                Point3::new(1.0, 0.0, z),
                Point3::new(1.0, 1.0, z),
                Point3::new(0.0, 1.0, z),
            ];
            let mut loop_ = Loop::new(0, LoopType::Outer);
            for i in 0..4 {
                let (start, end) = (corners[i], corners[(i + 1) % 4]);
                let curve_id = curves.add(Box::new(Line::new(start, end)));
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
            let face = Face::new(0, surface_id, outer_loop, FaceOrientation::Forward);
            faces.add(face)
        }

        let face_a = add_face(
            &mut surfaces,
            &mut faces,
            &mut loops,
            &mut edges,
            &mut curves,
            0.0,
            Vector3::new(0.0, 0.0, -1.0),
        );
        let face_b = add_face(
            &mut surfaces,
            &mut faces,
            &mut loops,
            &mut edges,
            &mut curves,
            width,
            Vector3::new(0.0, 0.0, 1.0),
        );

        (surfaces, faces, loops, edges, curves, face_a, face_b)
    }

    /// Hand-computed VIOLATION: a 0.5 mm wall against a 0.4 mm nozzle
    /// (threshold 0.8 mm) — thinner than the floor, flagged with the
    /// EXACT measured thickness, witnesses naming both faces of the pair.
    #[test]
    fn thin_wall_below_2x_nozzle_is_exact_violation() {
        let (surfaces, faces, loops, edges, curves, face_a, face_b) = rectangle_wall_pair(0.5);
        let verdict = evaluate_min_wall(
            &[face_a, face_b],
            0.4,
            &faces,
            &loops,
            &edges,
            &curves,
            &surfaces,
        )
        .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        match verdict.verdict {
            Verdict::Violation {
                mut witnesses,
                measured,
                limit,
            } => {
                witnesses.sort_unstable();
                let mut expected = vec![face_a, face_b];
                expected.sort_unstable();
                assert_eq!(witnesses, expected);
                assert!(
                    (measured.value - 0.5).abs() < 1e-9,
                    "measured = {}",
                    measured.value
                );
                assert!((limit.value - 0.8).abs() < 1e-9, "limit = {}", limit.value);
            }
            other => panic!("expected Violation, got {other:?}"),
        }
    }

    /// Hand-computed PASS: a 5.0 mm wall against the same 0.8 mm
    /// threshold — margin asserted exactly: `5.0 - 0.8 = 4.2`.
    #[test]
    fn thick_wall_above_2x_nozzle_passes_with_exact_margin() {
        let (surfaces, faces, loops, edges, curves, face_a, face_b) = rectangle_wall_pair(5.0);
        let verdict = evaluate_min_wall(
            &[face_a, face_b],
            0.4,
            &faces,
            &loops,
            &edges,
            &curves,
            &surfaces,
        )
        .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        match verdict.verdict {
            Verdict::Pass { margin } => {
                assert!(
                    (margin.value - 4.2).abs() < 1e-9,
                    "margin = {}",
                    margin.value
                );
            }
            other => panic!("expected Pass, got {other:?}"),
        }
    }

    /// Refusal flow-through: a lone NURBS face (no possible partner, and
    /// itself an unsupported surface kind) must read `Unverifiable` for
    /// `fdm.min_wall`, never `Pass`.
    #[test]
    fn nurbs_face_min_wall_is_unverifiable_never_pass() {
        use crate::dfm::packs::fixtures::nurbs_face;
        let (surfaces, faces, loops, edges, curves, face_id) = nurbs_face();

        let verdict =
            evaluate_min_wall(&[face_id], 0.4, &faces, &loops, &edges, &curves, &surfaces)
                .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        match verdict.verdict {
            Verdict::Unverifiable { regions, .. } => assert_eq!(regions, vec![face_id]),
            other => panic!("expected Unverifiable, got {other:?}"),
        }
    }

    /// Full-pack integration (spec §3.2): `evaluate()` now runs BOTH
    /// `fdm.overhang` and `fdm.min_wall` on one candidate face set — a
    /// thin wall pair (violates min_wall) plus a steep overhang face
    /// (violates overhang) — and the report folds across BOTH rules.
    #[test]
    fn full_pack_evaluate_runs_both_rules_and_folds() {
        use crate::dfm::report::DfmSummary;
        use crate::math::Point3;
        use crate::primitives::face::{Face, FaceOrientation};
        use crate::primitives::r#loop::{Loop, LoopType};
        use crate::primitives::surface::Plane;

        let (mut surfaces, mut faces, mut loops, edges, curves, wall_a, wall_b) =
            rectangle_wall_pair(0.5);

        // Add one more face (steep overhang) to the SAME stores so a
        // single evaluate() call sees both the wall pair and the
        // overhang candidate.
        let theta = 150f64.to_radians();
        let normal = Vector3::new(theta.sin(), 0.0, theta.cos());
        let plane = Plane::from_point_normal(Point3::new(5.0, 5.0, 5.0), normal)
            .unwrap_or_else(|e| panic!("valid plane fixture: {e}"));
        let surface_id = surfaces.add(Box::new(plane));
        let outer_loop = loops.add(Loop::new(2, LoopType::Outer));
        let face = Face::new(2, surface_id, outer_loop, FaceOrientation::Forward);
        let overhang_face_id = faces.add(face);

        let report = evaluate(
            &[wall_a, wall_b, overhang_face_id],
            0.4,
            [0.0, 0.0, 1.0],
            &faces,
            &loops,
            &edges,
            &curves,
            &surfaces,
        )
        .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert_eq!(report.verdicts().len(), 2, "both fdm rules must be present");
        assert!(matches!(
            report.verdicts()[0].verdict,
            Verdict::Violation { .. }
        ));
        assert!(matches!(
            report.verdicts()[1].verdict,
            Verdict::Violation { .. }
        ));
        assert_eq!(report.summary(), DfmSummary::Violations { count: 2 });
    }
}
