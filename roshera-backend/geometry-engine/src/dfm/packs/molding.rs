//! Injection-molding rule pack v1 (spec §3.2).
//!
//! Ships ONE rule this slice: [`evaluate_draft`] (`mold.draft`) — the
//! pack-generality proof the spec calls out explicitly: the SAME
//! [`face_orientation_field`] analyzer FDM's `fdm.overhang` uses, read
//! against a different reference direction (`pull_direction` instead of
//! `build_direction`) with a different threshold and a different
//! violation sense. `mold.undercut` and `mold.trapped_core` need
//! machinery this slice does not build (opposing-normal-range pairing,
//! `internal_voids` — spec S5) and are out of scope here.

use crate::dfm::analyzers::{face_orientation_field, OrientationOutcome};
use crate::dfm::packs::{Rule, RulePack};
use crate::dfm::provenance::RuleProvenance;
use crate::dfm::report::{
    Derivation, DfmError, DfmReport, DfmValue, FaceRef, PackParams, RuleVerdict, SurfaceKind,
    UnverifiableReason, Verdict,
};
use crate::math::enclosure::Interval;
use crate::math::Vector3;
use crate::primitives::curve::CurveStore;
use crate::primitives::edge::EdgeStore;
use crate::primitives::face::{FaceId, FaceStore};
use crate::primitives::r#loop::LoopStore;
use crate::primitives::surface::SurfaceStore;

/// Stable rule id, matches [`crate::dfm::report::RuleVerdict::rule`].
pub const DRAFT_RULE_ID: &str = "mold.draft";

/// Practice-derived provenance for `mold.draft` (spec §3.2.1 "known
/// landscape": injection molding has no single governing standard for
/// draft-angle geometry — the spec expects `Handbook` (Boothroyd &
/// Dewhurst) to eventually dominate here, but no specific edition/page
/// citation is confirmed in this tree today. Per the module's
/// non-negotiable discipline, an unconfirmed citation is never invented;
/// this stays `ShopPractice` and says so honestly rather than dressing
/// the number up as `Handbook` on a citation nobody checked.
pub fn draft_provenance() -> RuleProvenance {
    RuleProvenance::ShopPractice {
        note: "injection-molding draft angle; no governing-standard citation confirmed in-tree"
            .to_string(),
    }
}

/// The molding pack's declared rule list (spec §3.2) tied to `params` the
/// same way [`DfmReport`] ties itself to its own params (see
/// [`crate::dfm::packs::RulePack`]).
pub fn rule_pack(params: PackParams) -> RulePack {
    RulePack {
        params,
        rules: vec![Rule {
            id: DRAFT_RULE_ID,
            provenance: draft_provenance(),
        }],
    }
}

/// Convert [`face_orientation_field`]'s angle convention — `θ` = angle
/// between the face's OUTWARD normal and `pull_direction` (see
/// `analyzers/orientation.rs` module docs) — into an actual draft angle
/// in degrees.
///
/// ## Hand-check (paper, before coding the comparison)
///
/// A wall with ZERO draft is parallel to the pull axis, so its normal is
/// exactly PERPENDICULAR to pull: `θ = 90°`, draft = `|90° − 90°| = 0°`.
/// ✓. Tilting that wall by a draft angle `α` (leaning it away from the
/// pull axis so the part releases) rotates the wall's normal by the SAME
/// angle `α` about the same axis (the normal stays perpendicular to the
/// wall, which just rotated by `α`) — so `θ` becomes `90° ± α` and
/// `|θ − 90°| = α` exactly, regardless of which of the two ways the wall
/// leans. ✓
///
/// This is deliberately UNSIGNED/symmetric: v1 does not distinguish which
/// side of the mold (core vs. cavity) or which lean direction the draft
/// is on — only "how far this face's normal sits from exactly-90°-to-pull"
/// — which is the same simplification `fdm.overhang` makes by only
/// caring about ONE direction. This is a stated v1 limitation, not a
/// silent gap: `mold.draft` answers "is this wall close enough to
/// perfectly vertical to risk sticking", not "which way will it stick".
fn draft_angle_deg(theta_deg: f64) -> f64 {
    (theta_deg - 90.0).abs()
}

fn as_draft_derivation(inner: Derivation) -> Derivation {
    match inner {
        Derivation::Analytic {
            surface_type,
            method,
        } => Derivation::Analytic {
            surface_type,
            method: format!("{method}; mold.draft reads draft angle = |θ − 90°|"),
        },
        // Freeform F1: a bounded derivation re-tags the same way — the
        // linear conversion preserves the enclosure's provenance fields.
        Derivation::BoundedAnalytic {
            method,
            refinement_depth,
            converged,
        } => Derivation::BoundedAnalytic {
            method: format!("{method}; mold.draft reads draft angle = |θ − 90°|"),
            refinement_depth,
            converged,
        },
    }
}

/// See [`crate::dfm::packs::fdm::evaluate_overhang`]'s twin for why a
/// fixed pack-configured constant (here, `min_draft_deg` itself) is still
/// wrapped in `Derivation::Analytic` — the existing in-tree precedent
/// (`report.rs` test fixtures) for tagging a non-geometric constant
/// (`Derivation::BoundedAnalytic`, added by freeform F1, is reserved for
/// enclosure-derived numbers, which a fixed constant is not).
fn constant_derivation(method: &str) -> Derivation {
    Derivation::Analytic {
        surface_type: SurfaceKind::Plane,
        method: method.to_string(),
    }
}

enum FaceOutcome {
    Measured(f64, Derivation),
    /// Freeform F3: a PROVEN enclosure of the face's MINIMUM draft angle,
    /// decided per face by the mutation-proofed
    /// [`Verdict::from_bounded_min`] fold in [`evaluate_draft`].
    Bounded {
        draft_deg: Interval,
        method: String,
        refinement_depth: usize,
        converged: bool,
    },
    Unverifiable(UnverifiableReason),
}

/// Enclosure of the face's MINIMUM draft angle `|θ − 90°|` from proven
/// enclosures of the face's minimum/maximum θ. The V-shape hazard the
/// exact arm documents (the minimum of `|θ − 90°|` over an interval is
/// NOT generally at an endpoint) becomes three PROVEN cases plus an
/// honest wide fallback:
///
/// - the θ-range PROVABLY contains 90° (`min θ ≤ 90` proven via
///   `min_enc.hi ≤ 90`, and `max θ ≥ 90` proven via `max_enc.lo ≥ 90`):
///   by continuity some point of the face has θ = 90° exactly, so the
///   minimum draft is exactly 0;
/// - the range provably sits entirely at/above 90° (`min_enc.lo ≥ 90`):
///   `|θ − 90°|` is increasing there, so min draft = (min θ) − 90°;
/// - entirely at/below 90° (`max_enc.hi ≤ 90`): min draft = 90° − max θ;
/// - otherwise the crossing cannot be decided from the enclosures: the
///   sound answer is `[0, hi]` — 0 cannot be excluded, and the true
///   minimum is ≤ the draft at either true extremum, each of which is
///   bounded by its enclosure's `|· − 90|` ceiling.
fn draft_enclosure(min_enc: &Interval, max_enc: &Interval) -> Option<Interval> {
    let ninety = Interval::point(90.0).ok()?;
    if min_enc.hi() <= 90.0 && max_enc.lo() >= 90.0 {
        return Interval::point(0.0).ok();
    }
    if min_enc.lo() >= 90.0 {
        return Some(min_enc.sub(&ninety));
    }
    if max_enc.hi() <= 90.0 {
        return Some(ninety.sub(max_enc));
    }
    let ceil_a = min_enc.sub(&ninety).abs().hi();
    let ceil_b = max_enc.sub(&ninety).abs().hi();
    Interval::enclosing(0.0, ceil_a.min(ceil_b).max(0.0)).ok()
}

fn face_outcome(
    face_id: FaceId,
    pull_direction: Vector3,
    face_store: &FaceStore,
    loop_store: &LoopStore,
    edge_store: &EdgeStore,
    curve_store: &CurveStore,
    surface_store: &SurfaceStore,
) -> Result<FaceOutcome, DfmError> {
    let outcome = face_orientation_field(
        face_id,
        pull_direction,
        face_store,
        loop_store,
        edge_store,
        curve_store,
        surface_store,
    )?;
    Ok(match outcome {
        // The face's SHALLOWEST draft point (closest to a perfectly
        // vertical/parallel-to-pull wall) is the worst case for THIS
        // rule — the opposite extremum from `fdm.overhang`, which takes
        // the steepest point. Unlike `degrees_from_vertical` (linear,
        // hence monotonic in θ), `draft_angle_deg = |θ − 90°|` is
        // V-SHAPED around θ=90° — its minimum over an interval is NOT
        // generally at either endpoint. If the face's exact trimmed
        // range `[min_deg, max_deg]` straddles 90° (a curved face whose
        // swept angle passes through exactly-parallel-to-pull), the true
        // minimum draft angle anywhere on the face is exactly 0°,
        // achieved at the interior point θ=90°, and reading only the two
        // endpoints would silently miss it (both endpoints could report
        // several degrees of draft while a point in between has none).
        // Otherwise the whole interval sits strictly on one side of 90°,
        // where `|θ − 90°|` IS monotonic, and the endpoint nearer 90°
        // gives the exact minimum.
        OrientationOutcome::Range {
            min_deg,
            max_deg,
            derivation,
        } => {
            let worst = if min_deg <= 90.0 && 90.0 <= max_deg {
                0.0
            } else {
                draft_angle_deg(min_deg).min(draft_angle_deg(max_deg))
            };
            FaceOutcome::Measured(worst, derivation)
        }
        // Freeform F3: the bounded extremum enclosures map through the
        // V-shape-aware [`draft_enclosure`] — see its doc for the four
        // proven cases.
        OrientationOutcome::BoundedRange {
            min_deg,
            max_deg,
            method,
            refinement_depth,
            converged,
        } => match draft_enclosure(&min_deg, &max_deg) {
            Some(draft_deg) => FaceOutcome::Bounded {
                draft_deg,
                method,
                refinement_depth,
                converged,
            },
            // Interval construction over finite literals cannot fail;
            // refuse honestly rather than panic if it ever did.
            None => FaceOutcome::Unverifiable(UnverifiableReason::UnsupportedSurface {
                surface_type: SurfaceKind::Nurbs,
                analyzer: "mold.draft (internal interval construction failed)".to_string(),
            }),
        },
        OrientationOutcome::Unverifiable { reason } => FaceOutcome::Unverifiable(reason),
    })
}

/// Evaluate `mold.draft` over `faces` (spec §3.2: "faces within the draft
/// cone of vertical flagged with exact deficit" — the "draft cone" is the
/// angular band `|θ − pull⋅90°| < min_draft_deg` around exactly-parallel-
/// to-pull; see [`draft_angle_deg`] for the conversion and its
/// hand-check).
///
/// ## Multi-face aggregation
///
/// Same per-rule precedence as [`crate::dfm::packs::fdm::evaluate_overhang`]
/// (a proven `Violation` on any face dominates; `Unverifiable` only wins
/// the rule when no face violates; `measured`/`margin` report the SINGLE
/// worst-case face, `witnesses`/`regions` name every deciding face in
/// ascending `FaceId` order) — mirrored here rather than shared code,
/// since the WORST-CASE DIRECTION is opposite (draft violates on the
/// MINIMUM angle, overhang violates on the MAXIMUM), which would make a
/// shared helper's comparator parameter the only thing distinguishing two
/// three-line call sites.
pub fn evaluate_draft(
    faces: &[FaceId],
    pull_direction: Vector3,
    min_draft_deg: f64,
    face_store: &FaceStore,
    loop_store: &LoopStore,
    edge_store: &EdgeStore,
    curve_store: &CurveStore,
    surface_store: &SurfaceStore,
) -> Result<RuleVerdict, DfmError> {
    let mut violations: Vec<(FaceId, DfmValue)> = Vec::new();
    let mut unverifiable: Vec<(FaceId, UnverifiableReason)> = Vec::new();
    // The SMALLEST proven margin among passing faces — the binding
    // constraint (identical to the previous worst-safe-degree tracking:
    // margin = deg − min_draft_deg is monotone in deg).
    let mut best_margin: Option<DfmValue> = None;

    let limit_value = || {
        DfmValue::new(
            min_draft_deg,
            constant_derivation("mold.draft threshold: min_draft_deg (pack params)"),
        )
    };
    let offer_margin = |candidate: DfmValue, best: &mut Option<DfmValue>| {
        let replace = match best {
            Some(current) => candidate.value < current.value,
            None => true,
        };
        if replace {
            *best = Some(candidate);
        }
    };

    for &face_id in faces {
        match face_outcome(
            face_id,
            pull_direction,
            face_store,
            loop_store,
            edge_store,
            curve_store,
            surface_store,
        )? {
            FaceOutcome::Measured(deg, derivation) => {
                if deg < min_draft_deg {
                    violations.push((face_id, DfmValue::new(deg, as_draft_derivation(derivation))));
                } else {
                    let margin =
                        DfmValue::new(deg - min_draft_deg, as_draft_derivation(derivation));
                    offer_margin(margin, &mut best_margin);
                }
            }
            FaceOutcome::Bounded {
                draft_deg,
                method,
                refinement_depth,
                converged,
            } => {
                // Freeform F3: decided by the mutation-proofed bounded
                // MIN fold (report.rs F1) — a straddling enclosure can
                // NEVER fold to a passing face.
                match Verdict::from_bounded_min(
                    draft_deg,
                    limit_value(),
                    vec![face_id],
                    &method,
                    refinement_depth,
                    converged,
                ) {
                    Verdict::Pass { mut margin } => {
                        margin.derivation = as_draft_derivation(margin.derivation);
                        offer_margin(margin, &mut best_margin);
                    }
                    Verdict::Violation { mut measured, .. } => {
                        measured.derivation = as_draft_derivation(measured.derivation);
                        violations.push((face_id, measured));
                    }
                    Verdict::Unverifiable { reason, .. } => {
                        unverifiable.push((face_id, reason));
                    }
                }
            }
            FaceOutcome::Unverifiable(reason) => unverifiable.push((face_id, reason)),
        }
    }

    if !violations.is_empty() {
        violations.sort_by_key(|(id, _)| *id);
        // For a bounded face the value is the enclosure's PROVEN CEILING
        // (`from_bounded_min`'s violation arm), so the min-fold stays a
        // proven upper bound on the worst face's true draft.
        let worst = violations
            .iter()
            .min_by(|a, b| a.1.value.total_cmp(&b.1.value))
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| {
                DfmValue::new(
                    min_draft_deg,
                    constant_derivation("unreachable: violations checked non-empty above"),
                )
            });
        let witnesses: Vec<FaceRef> = violations.into_iter().map(|(id, _)| id).collect();
        return Ok(RuleVerdict {
            rule: DRAFT_RULE_ID.to_string(),
            verdict: Verdict::Violation {
                witnesses,
                measured: worst,
                limit: limit_value(),
            },
            provenance: draft_provenance(),
        });
    }

    if !unverifiable.is_empty() {
        unverifiable.sort_by_key(|(id, _)| *id);
        let reason = unverifiable[0].1.clone();
        let regions: Vec<FaceRef> = unverifiable.into_iter().map(|(id, _)| id).collect();
        return Ok(RuleVerdict {
            rule: DRAFT_RULE_ID.to_string(),
            verdict: Verdict::Unverifiable { regions, reason },
            provenance: draft_provenance(),
        });
    }

    let margin = best_margin.unwrap_or_else(|| {
        DfmValue::new(
            90.0 - min_draft_deg,
            constant_derivation("mold.draft: no candidate faces supplied"),
        )
    });
    Ok(RuleVerdict {
        rule: DRAFT_RULE_ID.to_string(),
        verdict: Verdict::Pass { margin },
        provenance: draft_provenance(),
    })
}

/// The molding pack's `evaluate()` arm (spec §3.2 params: `pull_direction`,
/// `min_draft_deg`; default 1°).
pub fn evaluate(
    faces: &[FaceId],
    pull_direction: [f64; 3],
    min_draft_deg: f64,
    face_store: &FaceStore,
    loop_store: &LoopStore,
    edge_store: &EdgeStore,
    curve_store: &CurveStore,
    surface_store: &SurfaceStore,
) -> Result<DfmReport, DfmError> {
    let dir = Vector3::new(pull_direction[0], pull_direction[1], pull_direction[2]);
    let draft = evaluate_draft(
        faces,
        dir,
        min_draft_deg,
        face_store,
        loop_store,
        edge_store,
        curve_store,
        surface_store,
    )?;
    Ok(DfmReport::new(
        PackParams::InjectionMolding {
            pull_direction,
            min_draft_deg,
        },
        vec![draft],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dfm::packs::fixtures::plane_face_at_theta_deg;
    use crate::math::Point3;
    use crate::primitives::curve::{Arc, Curve, Line, ParameterRange};
    use crate::primitives::edge::{Edge, EdgeOrientation};
    use crate::primitives::face::{Face, FaceOrientation};
    use crate::primitives::r#loop::{Loop, LoopType};
    use crate::primitives::surface::Cylinder;
    use std::f64::consts::PI;

    /// Hand-computed VIOLATION: a wall with only 0.5° of draft against a
    /// 1° minimum. `θ = 89.5°` from pull `+Z` ⇒
    /// `draft_angle = |89.5° − 90°| = 0.5°`, exactly, asserted to
    /// floating-point noise only.
    #[test]
    fn wall_with_half_degree_draft_is_exact_violation() {
        let (surfaces, faces, loops, edges, curves, face_id) = plane_face_at_theta_deg(89.5);
        let verdict = evaluate_draft(
            &[face_id],
            Vector3::Z,
            1.0,
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
                    (measured.value - 0.5).abs() < 1e-9,
                    "measured = {}",
                    measured.value
                );
                assert!((limit.value - 1.0).abs() < 1e-9, "limit = {}", limit.value);
            }
            other => panic!("expected Violation, got {other:?}"),
        }
    }

    /// Hand-computed PASS: a wall with 2° of draft against a 1° minimum.
    /// Margin asserted exactly: `2° − 1° = 1°`.
    #[test]
    fn wall_with_2_degree_draft_passes_with_exact_margin() {
        let (surfaces, faces, loops, edges, curves, face_id) = plane_face_at_theta_deg(92.0);
        let verdict = evaluate_draft(
            &[face_id],
            Vector3::Z,
            1.0,
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
                    (margin.value - 1.0).abs() < 1e-9,
                    "margin = {}",
                    margin.value
                );
            }
            other => panic!("expected Pass, got {other:?}"),
        }
    }

    /// A half-cylinder face whose exact trimmed angular range (against
    /// pull direction `+X`) is `[0°, 180°]` — straddling 90° — built the
    /// same way `analyzers/orientation.rs`'s own
    /// `half_cylinder_reports_half_range_not_full_2pi` fixture is (radius
    /// 2, height 5, half-domain `u ∈ [0, π]`, reference `+X`).
    fn half_cylinder_face_full_range_vs_x() -> (
        SurfaceStore,
        FaceStore,
        LoopStore,
        EdgeStore,
        CurveStore,
        FaceId,
    ) {
        let radius = 2.0;
        let height = 5.0;
        let mut surfaces = SurfaceStore::new();
        let cylinder = Cylinder::new(Point3::new(0.0, 0.0, 0.0), Vector3::Z, radius)
            .unwrap_or_else(|e| panic!("valid cylinder fixture: {e}"));
        let surface_id = surfaces.add(Box::new(cylinder));

        let bottom_arc = Arc::new(Point3::new(0.0, 0.0, 0.0), Vector3::Z, radius, 0.0, PI)
            .unwrap_or_else(|e| panic!("valid arc fixture: {e}"));
        let top_arc = Arc::new(Point3::new(0.0, 0.0, height), Vector3::Z, radius, 0.0, PI)
            .unwrap_or_else(|e| panic!("valid arc fixture: {e}"));
        let line_u0 = Line::new(
            Point3::new(radius, 0.0, 0.0),
            Point3::new(radius, 0.0, height),
        );
        let line_upi = Line::new(
            Point3::new(-radius, 0.0, 0.0),
            Point3::new(-radius, 0.0, height),
        );

        let mut curves = CurveStore::new();
        let mut edges = EdgeStore::new();
        let mut loops = LoopStore::new();
        let mut loop_ = Loop::new(0, LoopType::Outer);
        let curve_list: Vec<Box<dyn Curve>> = vec![
            Box::new(bottom_arc),
            Box::new(line_upi),
            Box::new(top_arc),
            Box::new(line_u0),
        ];
        for curve in curve_list {
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

        let mut faces = FaceStore::new();
        let face = Face::new(0, surface_id, outer_loop, FaceOrientation::Forward);
        let face_id = faces.add(face);

        (surfaces, faces, loops, edges, curves, face_id)
    }

    /// Regression test for the straddle-90° fix in `face_outcome`:
    /// `draft_angle_deg = |θ − 90°|` is V-shaped, so a face whose EXACT
    /// range straddles 90° has a true minimum of 0° at the interior
    /// point, not at either endpoint. This fixture's range is exactly
    /// `[0°, 180°]` against pull `+X` — both endpoints individually give
    /// `draft_angle_deg = 90°`, which would (wrongly) read as a huge
    /// 90°-margin Pass if only the endpoints were checked. The correct
    /// answer is a Violation at exactly `0°` draft (the point θ=90°,
    /// squarely inside the range, is a genuine wall-parallel-to-pull
    /// point on this face).
    #[test]
    fn straddling_range_reports_exact_zero_draft_not_endpoint_value() {
        let (surfaces, faces, loops, edges, curves, face_id) = half_cylinder_face_full_range_vs_x();
        let verdict = evaluate_draft(
            &[face_id],
            Vector3::X,
            1.0,
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
                assert_eq!(witnesses, vec![face_id]);
                assert!(
                    measured.value.abs() < 1e-9,
                    "measured should be exactly 0°, got {}",
                    measured.value
                );
            }
            other => panic!(
                "expected Violation at 0° draft (straddle-90 fix) — got {other:?}; \
                 an endpoint-only (unfixed) implementation would wrongly report Pass here"
            ),
        }
    }

    // ----- Freeform F3: bounded draft through the pack fold -----

    /// A near-vertical freeform ramp (slope tan(89.5°) as a bilinear
    /// NURBS plane, Forward): the outward normal sits 89.5° from pull, so
    /// the true draft is exactly 0.5° — a PROVEN bounded violation of the
    /// 1° floor, with the measured value the enclosure's proven CEILING
    /// (`from_bounded_min`'s violation arm) and the wire bound attached.
    #[test]
    fn freeform_near_vertical_wall_is_a_proven_bounded_draft_violation() {
        let mut surfaces = crate::primitives::surface::SurfaceStore::new();
        let slope = 89.5f64.to_radians().tan();
        let nurbs = crate::math::nurbs::NurbsSurface::new(
            vec![
                vec![Point3::new(0.0, 0.0, 0.0), Point3::new(0.0, 1.0, 0.0)],
                vec![Point3::new(1.0, 0.0, slope), Point3::new(1.0, 1.0, slope)],
            ],
            vec![vec![1.0, 1.0], vec![1.0, 1.0]],
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
            1,
            1,
        )
        .unwrap_or_else(|e| panic!("valid near-vertical ramp patch: {e}"));
        let surface_id = surfaces.add(Box::new(crate::primitives::surface::GeneralNurbsSurface {
            nurbs,
        }));
        let mut loops = crate::primitives::r#loop::LoopStore::new();
        let outer_loop = loops.add(Loop::new(0, LoopType::Outer));
        let mut faces = crate::primitives::face::FaceStore::new();
        let face_id = faces.add(Face::new(
            0,
            surface_id,
            outer_loop,
            FaceOrientation::Forward,
        ));
        let edges = crate::primitives::edge::EdgeStore::new();
        let curves = crate::primitives::curve::CurveStore::new();

        let verdict = evaluate_draft(
            &[face_id],
            Vector3::Z,
            1.0,
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
                // Proven ceiling: at least the true 0.5°, below the 1° floor.
                assert!(
                    measured.value >= 0.5 - 1e-6 && measured.value < 1.0,
                    "measured must be a proven ceiling in [0.5, 1.0): {}",
                    measured.value
                );
                let bound = measured
                    .bound
                    .unwrap_or_else(|| panic!("bounded violation must carry its enclosure"));
                assert!(
                    bound.lo <= 0.5 + 1e-6 && bound.hi >= 0.5 - 1e-6,
                    "bound [{}, {}] must contain the true 0.5°",
                    bound.lo,
                    bound.hi
                );
                assert!(matches!(
                    measured.derivation,
                    Derivation::BoundedAnalytic { .. }
                ));
                assert!((limit.value - 1.0).abs() < 1e-12);
            }
            other => panic!("expected a proven bounded draft Violation, got {other:?}"),
        }
    }
}
