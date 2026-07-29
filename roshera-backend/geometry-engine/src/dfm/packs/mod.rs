//! Rule packs (spec §3.2): declarative "what rules exist, with what
//! provenance" for a manufacturing process, plus the small dispatcher that
//! evaluates the rules this slice implements — `fdm.overhang`
//! ([`fdm::evaluate_overhang`]) and `mold.draft`
//! ([`molding::evaluate_draft`]) — against the one analyzer that exists so
//! far ([`crate::dfm::analyzers::face_orientation_field`]).
//!
//! ## What this slice is NOT
//!
//! Spec §3.2's sketch shows a fully generic `Rule { analyzer: AnalyzerCall,
//! comparator: Comparator, .. }` dispatch table. With exactly one analyzer
//! and two rules in the whole kernel, building that generic engine now
//! would be untested scaffolding wrapped around two straight-line
//! functions — the committed reality (one analyzer, no `Comparator`/
//! `AnalyzerCall` types anywhere in the tree) is the law here, per the
//! executor brief. [`Rule`] / [`RulePack`] ship as the STABLE, greppable
//! DECLARATION of "which rules exist and where their threshold comes
//! from" (id + provenance), so a later generic `analyze()` engine (spec
//! S6/S7, once `pair_thickness`/`bore_metrics`/`internal_voids` exist and
//! there is real dispatch variety to justify one) has a real list to
//! consume instead of re-deriving it from each pack's hand-written
//! evaluator. [`evaluate`] is that "later" work's placeholder-shaped
//! stand-in — an honest, minimal, one-arm-per-pack match, not a stub that
//! always passes or always refuses (which would itself be the "kernel can
//! lie" defect this subsystem exists to remove).
//!
//! ## Why `faces: &[FaceId]`, not `model`/`solid`
//!
//! [`face_orientation_field`] is itself a per-face, store-driven function
//! with no `Solid`/`BRepModel` in its signature, and its own test module
//! builds faces directly rather than through a full solid (booleans/
//! extrude are a `KNOWN_REDS` hazard area its tests deliberately avoid —
//! see `analyzers/orientation.rs`'s test docs). Enumerating "every face of
//! a solid" is one line at the call site
//! (`solid.all_shells().iter().flat_map(|&s| shell_store.get(s).map(|sh|
//! sh.faces.clone())).flatten()`) — pulling that traversal into this
//! module now would add a `Solid`/`ShellStore` dependency this slice's
//! rules do not otherwise need, for a convenience a caller can already
//! express in one line.

pub mod fdm;
pub mod molding;

use crate::dfm::provenance::RuleProvenance;
use crate::dfm::report::{DfmError, DfmReport, PackParams, RulePackId};
use crate::primitives::curve::CurveStore;
use crate::primitives::edge::EdgeStore;
use crate::primitives::face::{FaceId, FaceStore};
use crate::primitives::r#loop::LoopStore;
use crate::primitives::surface::SurfaceStore;

/// One rule's static identity (spec §3.2, adapted to the committed
/// reality): `id` is `&'static str` since every pack definition here is a
/// compile-time Rust literal — this is the exact static counterpart
/// [`crate::dfm::report::RuleVerdict::rule`]'s doc comment names as
/// `Rule::id` when it explains why the WIRE side had to become `String`
/// instead. `provenance` is copied into the `RuleVerdict` this rule
/// produces at evaluation time (see `fdm::evaluate_overhang` /
/// `molding::evaluate_draft`), so a report reader never needs to
/// cross-reference the pack source to learn "where did this threshold
/// come from" (spec §3.2.1 / `report.rs` module docs).
#[derive(Debug, Clone)]
pub struct Rule {
    pub id: &'static str,
    pub provenance: RuleProvenance,
}

/// A named collection of rules for one manufacturing process (spec §3.2).
/// `params` is the SAME [`PackParams`] a [`DfmReport`] for this pack would
/// echo — [`RulePack::id`] derives from it exactly the way
/// [`DfmReport::pack`](crate::dfm::report::DfmReport::pack) derives from
/// [`DfmReport::params`](crate::dfm::report::DfmReport::params), so a
/// `RulePack` can never claim to belong to a different process than its
/// own params.
#[derive(Debug, Clone)]
pub struct RulePack {
    pub params: PackParams,
    pub rules: Vec<Rule>,
}

impl RulePack {
    pub fn id(&self) -> RulePackId {
        self.params.pack_id()
    }
}

/// The slice's minimal `analyze()`-shaped entry point (spec §3's
/// `analyze(model, solid, pack) -> DfmReport`, adapted: see module docs
/// for why `faces: &[FaceId]` replaces `model, solid` here). Dispatches on
/// `params`'s variant to the one pack that owns it and evaluates every
/// rule that pack implements in this slice, folding the resulting
/// [`crate::dfm::report::RuleVerdict`]s into a [`DfmReport`] via
/// [`DfmReport::new`] — the honesty fold itself is untouched, owned
/// entirely by `report.rs`.
///
/// Returns `Err` only when [`face_orientation_field`] does (spec §4: a
/// dangling face reference is malformed input, never an honest refusal) —
/// propagated straight through from whichever pack's evaluator hit it.
///
/// [`face_orientation_field`]: crate::dfm::analyzers::face_orientation_field
pub fn evaluate(
    params: PackParams,
    faces: &[FaceId],
    face_store: &FaceStore,
    loop_store: &LoopStore,
    edge_store: &EdgeStore,
    curve_store: &CurveStore,
    surface_store: &SurfaceStore,
) -> Result<DfmReport, DfmError> {
    match params {
        PackParams::Fdm {
            nozzle_diameter,
            build_direction,
        } => fdm::evaluate(
            faces,
            nozzle_diameter,
            build_direction,
            face_store,
            loop_store,
            edge_store,
            curve_store,
            surface_store,
        ),
        PackParams::InjectionMolding {
            pull_direction,
            min_draft_deg,
        } => molding::evaluate(
            faces,
            pull_direction,
            min_draft_deg,
            face_store,
            loop_store,
            edge_store,
            curve_store,
            surface_store,
        ),
    }
}

/// Shared test fixtures for `fdm`'s, `molding`'s, and this module's own
/// tests — bare `Face`/`Loop`/`Edge`/`Curve`/`Surface` stores built
/// directly, the same convention `analyzers/orientation.rs`'s own test
/// module uses ("booleans are a KNOWN_REDS hazard area and the loop's
/// edge LIST is all this analyzer reads"). `pub(crate)` (not private) so
/// sibling `#[cfg(test)]` modules in `fdm.rs`/`molding.rs` can reuse them
/// without re-deriving the same plane/NURBS construction three times.
#[cfg(test)]
pub(crate) mod fixtures {
    use crate::math::{Point3, Vector3};
    use crate::primitives::curve::CurveStore;
    use crate::primitives::edge::EdgeStore;
    use crate::primitives::face::{Face, FaceId, FaceOrientation, FaceStore};
    use crate::primitives::r#loop::{Loop, LoopStore, LoopType};
    use crate::primitives::surface::{GeneralNurbsSurface, Plane, SurfaceStore};

    /// A single PLANE face whose outward normal sits at exactly
    /// `theta_deg` from `+Z`, lying in the XZ-plane
    /// (`normal = (sin(theta), 0, cos(theta))`). Against reference
    /// direction `+Z`, `face_orientation_field` reads back exactly
    /// `theta_deg` (see `analyzers/orientation.rs`'s own
    /// `plane_face_reports_single_known_angle` test, which this mirrors)
    /// — a Plane face's normal is constant, so no loop/edge/curve content
    /// is needed; the outer loop is a degenerate empty one, exactly as
    /// `orientation.rs`'s own plane fixture builds it.
    pub(crate) fn plane_face_at_theta_deg(
        theta_deg: f64,
    ) -> (
        SurfaceStore,
        FaceStore,
        LoopStore,
        EdgeStore,
        CurveStore,
        FaceId,
    ) {
        let mut surfaces = SurfaceStore::new();
        let theta = theta_deg.to_radians();
        let normal = Vector3::new(theta.sin(), 0.0, theta.cos());
        let plane = Plane::from_point_normal(Point3::new(0.0, 0.0, 0.0), normal)
            .unwrap_or_else(|e| panic!("valid plane fixture: {e}"));
        let surface_id = surfaces.add(Box::new(plane));

        let mut loops = LoopStore::new();
        let outer_loop = loops.add(Loop::new(0, LoopType::Outer));
        let curves = CurveStore::new();
        let edges = EdgeStore::new();

        let mut faces = FaceStore::new();
        let face = Face::new(0, surface_id, outer_loop, FaceOrientation::Forward);
        let face_id = faces.add(face);

        (surfaces, faces, loops, edges, curves, face_id)
    }

    /// A single trivial flat NURBS-patch face — refused unconditionally
    /// by `face_orientation_field` (spec §3.1's support table), the fixed
    /// point for every "refusal flows through, never silently becomes
    /// Pass" test in this module and its siblings. Mirrors
    /// `orientation.rs`'s own `nurbs_face_is_unverifiable_naming_the_kind`
    /// fixture.
    pub(crate) fn nurbs_face() -> (
        SurfaceStore,
        FaceStore,
        LoopStore,
        EdgeStore,
        CurveStore,
        FaceId,
    ) {
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
        .unwrap_or_else(|e| panic!("trivial flat NURBS patch fixture: {e}"));
        let surface = GeneralNurbsSurface { nurbs };
        let surface_id = surfaces.add(Box::new(surface));

        let mut loops = LoopStore::new();
        let outer_loop = loops.add(Loop::new(0, LoopType::Outer));
        let curves = CurveStore::new();
        let edges = EdgeStore::new();

        let mut faces = FaceStore::new();
        let face = Face::new(0, surface_id, outer_loop, FaceOrientation::Forward);
        let face_id = faces.add(face);

        (surfaces, faces, loops, edges, curves, face_id)
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{nurbs_face, plane_face_at_theta_deg};
    use super::*;
    use crate::dfm::report::{DfmSummary, Verdict};

    /// THE THESIS TEST — one solid (here, one face, which is all either
    /// rule's aggregation needs to exercise the decision) where FDM's
    /// `fdm.overhang` (build +Z) reads a Violation and molding's
    /// `mold.draft` (pull +Z, 1°) reads its own, DIFFERENT verdict — both
    /// driven by the identical `face_orientation_field` call against the
    /// identical +Z reference direction, differing only in which rule
    /// (threshold + violation sense) reads the angle.
    ///
    /// Face: a PLANE whose normal sits at 150° from +Z (i.e. 30° off
    /// straight-down, mostly downward-and-outward — the underside of a
    /// steep overhang).
    ///
    /// - `fdm.overhang`: `degrees_from_vertical = 150° − 90° = 60°` — over
    ///   the 45° threshold ⇒ Violation, `measured.value == 60.0` exactly.
    /// - `mold.draft`: `draft_angle = |150° − 90°| = 60°` — WAY over the
    ///   1° minimum ⇒ Pass, `margin.value == 59.0` exactly. Distinct
    ///   verdict KIND (Violation vs Pass), same analyzer call, same
    ///   reference direction, different rule — the pack-generality thesis
    ///   spec §3.2 calls out explicitly.
    #[test]
    fn one_analyzer_two_packs_yield_distinct_verdicts() {
        let (surfaces, faces, loops, edges, curves, face_id) = plane_face_at_theta_deg(150.0);
        let face_ids = [face_id];

        let fdm_report = evaluate(
            PackParams::Fdm {
                nozzle_diameter: 0.4,
                build_direction: [0.0, 0.0, 1.0],
            },
            &face_ids,
            &faces,
            &loops,
            &edges,
            &curves,
            &surfaces,
        )
        .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));
        assert_eq!(fdm_report.summary(), DfmSummary::Violations { count: 1 });
        match &fdm_report.verdicts()[0].verdict {
            Verdict::Violation {
                measured, limit, ..
            } => {
                assert!((measured.value - 60.0).abs() < 1e-9, "{measured:?}");
                assert!((limit.value - 45.0).abs() < 1e-9, "{limit:?}");
            }
            other => panic!("expected fdm.overhang Violation, got {other:?}"),
        }

        let molding_report = evaluate(
            PackParams::InjectionMolding {
                pull_direction: [0.0, 0.0, 1.0],
                min_draft_deg: 1.0,
            },
            &face_ids,
            &faces,
            &loops,
            &edges,
            &curves,
            &surfaces,
        )
        .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));
        assert_eq!(molding_report.summary(), DfmSummary::Pass);
        match &molding_report.verdicts()[0].verdict {
            Verdict::Pass { margin } => {
                assert!((margin.value - 59.0).abs() < 1e-9, "{margin:?}");
            }
            other => panic!("expected mold.draft Pass, got {other:?}"),
        }

        // Same analyzer call, same reference direction — different rule,
        // genuinely different verdict KIND, not just a different number.
        assert_ne!(fdm_report.summary(), molding_report.summary());
    }

    /// Refusal flow-through: a solid whose only face is NURBS (which
    /// `face_orientation_field` AND `pair_thickness` both refuse
    /// unconditionally, spec §3.1) must read `Inconclusive` — NEVER
    /// `Pass` — through the FULL path: both analyzers' refusals → each
    /// rule's own aggregation → the committed `DfmReport::new` honesty
    /// fold. Exercises the fold end-to-end through this slice's own code,
    /// per the executor brief. Since S3 the FDM pack runs TWO rules
    /// (`fdm.overhang`, `fdm.min_wall`) against the same lone NURBS face,
    /// so BOTH read `Unverifiable` — `unverifiable: 2`, not 1.
    #[test]
    fn nurbs_only_solid_is_inconclusive_never_pass_through_full_report_path() {
        let (surfaces, faces, loops, edges, curves, face_id) = nurbs_face();
        let face_ids = [face_id];

        let report = evaluate(
            PackParams::Fdm {
                nozzle_diameter: 0.4,
                build_direction: [0.0, 0.0, 1.0],
            },
            &face_ids,
            &faces,
            &loops,
            &edges,
            &curves,
            &surfaces,
        )
        .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert_eq!(
            report.summary(),
            DfmSummary::Inconclusive { unverifiable: 2 }
        );
        assert_ne!(
            report.summary(),
            DfmSummary::Pass,
            "a NURBS-only solid must never report Pass"
        );
        match &report.verdicts()[0].verdict {
            Verdict::Unverifiable { regions, .. } => assert_eq!(regions, &[face_id]),
            other => panic!("expected Unverifiable (fdm.overhang), got {other:?}"),
        }
        match &report.verdicts()[1].verdict {
            Verdict::Unverifiable { regions, .. } => assert_eq!(regions, &[face_id]),
            other => panic!("expected Unverifiable (fdm.min_wall), got {other:?}"),
        }
    }

    /// Provenance presence: EVERY rule verdict this slice produces — FDM
    /// pass, FDM violation, FDM unverifiable, molding pass — carries the
    /// expected [`RuleProvenance::ShopPractice`] variant (spec §3.2.1:
    /// neither pack has a confirmed in-tree citation, so both are
    /// honestly `ShopPractice`, never dressed up as `Standard`).
    #[test]
    fn every_verdict_carries_shop_practice_provenance() {
        let safe = plane_face_at_theta_deg(90.0); // vertical wall: safe for both packs
        let steep = plane_face_at_theta_deg(150.0); // steep overhang: fdm violates
        let nurbs = nurbs_face();

        let cases = vec![
            (
                PackParams::Fdm {
                    nozzle_diameter: 0.4,
                    build_direction: [0.0, 0.0, 1.0],
                },
                [safe.5],
                &safe.0,
                &safe.1,
                &safe.2,
                &safe.3,
                &safe.4,
            ),
            (
                PackParams::Fdm {
                    nozzle_diameter: 0.4,
                    build_direction: [0.0, 0.0, 1.0],
                },
                [steep.5],
                &steep.0,
                &steep.1,
                &steep.2,
                &steep.3,
                &steep.4,
            ),
            (
                PackParams::Fdm {
                    nozzle_diameter: 0.4,
                    build_direction: [0.0, 0.0, 1.0],
                },
                [nurbs.5],
                &nurbs.0,
                &nurbs.1,
                &nurbs.2,
                &nurbs.3,
                &nurbs.4,
            ),
            (
                PackParams::InjectionMolding {
                    pull_direction: [0.0, 0.0, 1.0],
                    min_draft_deg: 1.0,
                },
                [safe.5],
                &safe.0,
                &safe.1,
                &safe.2,
                &safe.3,
                &safe.4,
            ),
        ];

        for (params, face_ids, surfaces, faces, loops, edges, curves) in cases {
            let report = evaluate(params, &face_ids, faces, loops, edges, curves, surfaces)
                .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));
            for rv in report.verdicts() {
                assert!(
                    matches!(rv.provenance, RuleProvenance::ShopPractice { .. }),
                    "rule {} carried {:?}, expected ShopPractice",
                    rv.rule,
                    rv.provenance
                );
            }
        }
    }
}
