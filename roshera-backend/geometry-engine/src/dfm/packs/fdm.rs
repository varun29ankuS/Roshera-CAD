//! FDM (fused deposition modeling) rule pack v1 (spec §3.2).
//!
//! Ships ONE rule this slice: [`evaluate_overhang`] (`fdm.overhang`). The
//! remaining v1 rules (`fdm.min_wall`, `fdm.min_bore`,
//! `fdm.trapped_volume`, `fdm.support_volume`) need analyzers that do not
//! exist yet (`pair_thickness`, `bore_metrics`, `internal_voids` — spec S3
//! - S5) and are out of scope here.

use crate::dfm::analyzers::{face_orientation_field, OrientationOutcome};
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

/// The FDM pack's declared rule list (spec §3.2) tied to `params` the same
/// way [`DfmReport`] ties itself to its own params (see
/// [`crate::dfm::packs::RulePack`]).
pub fn rule_pack(params: PackParams) -> RulePack {
    RulePack {
        params,
        rules: vec![Rule {
            id: OVERHANG_RULE_ID,
            provenance: overhang_provenance(),
        }],
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

/// The FDM pack's `evaluate()` arm (spec §3.2 params: `nozzle_diameter`,
/// `build_direction`; defaults 0.4 mm, +Z). `nozzle_diameter` is echoed on
/// the report's [`PackParams`] but not otherwise used — `fdm.min_wall` is
/// the rule that consumes it, and it is out of scope for this slice
/// (needs `pair_thickness`, spec S3).
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
    Ok(DfmReport::new(
        PackParams::Fdm {
            nozzle_diameter,
            build_direction,
        },
        vec![overhang],
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
}
