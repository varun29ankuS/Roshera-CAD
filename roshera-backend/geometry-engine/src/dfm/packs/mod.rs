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
//! ## Why `faces: &[FaceId]`, not `model`/`solid` (S2/S3 rules)
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
//!
//! ## S4: why [`evaluate`]'s `Fdm` arm now takes `model`/`solid_id` too
//!
//! `fdm.min_bore` rides [`crate::dfm::analyzers::bore_metrics`], whose
//! contract is `(model, solid_id)` rather than `faces: &[FaceId]` + bare
//! stores — through-vs-blind is not face-local, it needs the SOLID's own
//! extent along the bore axis (see `analyzers/bore.rs`'s module docs for
//! the full reasoning). Rather than thread yet another
//! caller-supplied-and-possibly-mismatched parameter alongside `faces`,
//! [`evaluate`] now takes the model directly for every pack; the
//! `InjectionMolding` arm is UNCHANGED in substance — it still calls
//! [`molding::evaluate`] with bare stores, just borrowed off the model
//! (`&model.faces`, `&model.loops`, …) instead of accepted as separate
//! parameters.

pub mod fdm;
pub mod molding;

use crate::dfm::provenance::RuleProvenance;
use crate::dfm::report::{DfmError, DfmReport, PackParams, RulePackId};
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;

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
/// `analyze(model, solid, pack) -> DfmReport`). Dispatches on `params`'s
/// variant to the one pack that owns it and evaluates every rule that
/// pack implements in this slice, folding the resulting
/// [`crate::dfm::report::RuleVerdict`]s into a [`DfmReport`] via
/// [`DfmReport::new`] — the honesty fold itself is untouched, owned
/// entirely by `report.rs`.
///
/// `faces` is still the caller-enumerated candidate list (module docs:
/// "why `faces: &[FaceId]`" for the face-local S2/S3 rules) — `model`/
/// `solid_id` are ADDITIONALLY required since S4 so the `Fdm` arm can
/// run `fdm.min_bore` (see "S4" module docs above).
///
/// Returns `Err` when [`face_orientation_field`] does (a dangling face
/// reference), or when [`crate::dfm::analyzers::bore_metrics`] does (a
/// dangling `solid_id`) — spec §4: both are malformed input, never an
/// honest refusal — propagated straight through from whichever pack's
/// evaluator hit it.
///
/// [`face_orientation_field`]: crate::dfm::analyzers::face_orientation_field
pub fn evaluate(
    params: PackParams,
    model: &BRepModel,
    solid_id: SolidId,
    faces: &[FaceId],
) -> Result<DfmReport, DfmError> {
    match params {
        PackParams::Fdm {
            nozzle_diameter,
            build_direction,
        } => fdm::evaluate(model, solid_id, faces, nozzle_diameter, build_direction),
        PackParams::InjectionMolding {
            pull_direction,
            min_draft_deg,
        } => molding::evaluate(
            faces,
            pull_direction,
            min_draft_deg,
            &model.faces,
            &model.loops,
            &model.edges,
            &model.curves,
            &model.surfaces,
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

    /// Wrap already-built bare stores + a face list into a fresh
    /// [`crate::primitives::topology_builder::BRepModel`] with ONE solid
    /// (a single closed shell containing every given face) — the
    /// model-level plumbing S4's `evaluate` (and
    /// [`crate::dfm::analyzers::bore_metrics`]) needs on top of geometry
    /// built the same bare-store way this module's fixtures already
    /// build it. `pub(crate)` so `packs::mod`'s own tests AND
    /// `packs::fdm`'s can wrap a `plane_face_at_theta_deg`/`nurbs_face`
    /// fixture into a model without re-deriving this plumbing per file.
    pub(crate) fn model_with_solid(
        surfaces: SurfaceStore,
        faces: FaceStore,
        loops: LoopStore,
        edges: EdgeStore,
        curves: CurveStore,
        face_ids: &[FaceId],
    ) -> (
        crate::primitives::topology_builder::BRepModel,
        crate::primitives::solid::SolidId,
    ) {
        use crate::primitives::shell::{Shell, ShellType};
        use crate::primitives::solid::Solid;

        let mut model = crate::primitives::topology_builder::BRepModel::new();
        model.surfaces = surfaces;
        model.faces = faces;
        model.loops = loops;
        model.edges = edges;
        model.curves = curves;

        let mut shell = Shell::new(0, ShellType::Closed);
        shell.add_faces(face_ids);
        let shell_id = model.shells.add(shell);
        let solid = Solid::new(0, shell_id);
        let solid_id = model.solids.add(solid);
        (model, solid_id)
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{model_with_solid, nurbs_face, plane_face_at_theta_deg};
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
        let (model, solid_id) = model_with_solid(surfaces, faces, loops, edges, curves, &face_ids);

        let fdm_report = evaluate(
            PackParams::Fdm {
                nozzle_diameter: 0.4,
                build_direction: [0.0, 0.0, 1.0],
            },
            &model,
            solid_id,
            &face_ids,
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
            &model,
            solid_id,
            &face_ids,
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
    /// per the executor brief. Since S5 the FDM pack runs FOUR rules
    /// (`fdm.overhang`, `fdm.min_wall`, `fdm.min_bore`, `fdm.trapped_volume`)
    /// against the same lone NURBS face. `fdm.overhang`/`fdm.min_wall`
    /// still read `Unverifiable` (a NURBS face is unsupported for both), so
    /// `unverifiable` stays 2 — `fdm.min_bore` reads `Pass` (vacuously: a
    /// NURBS face is not even a candidate for `bore_face_ids`'s
    /// concave-cylinder filter, so `bore_metrics` reports zero bores AND
    /// zero refusals for it) and `fdm.trapped_volume` ALSO reads `Pass`
    /// (vacuously: this fixture's solid has no inner shells at all, the
    /// same shape as `fdm.min_bore`'s own vacuous Pass).
    #[test]
    fn nurbs_only_solid_is_inconclusive_never_pass_through_full_report_path() {
        let (surfaces, faces, loops, edges, curves, face_id) = nurbs_face();
        let face_ids = [face_id];
        let (model, solid_id) = model_with_solid(surfaces, faces, loops, edges, curves, &face_ids);

        let report = evaluate(
            PackParams::Fdm {
                nozzle_diameter: 0.4,
                build_direction: [0.0, 0.0, 1.0],
            },
            &model,
            solid_id,
            &face_ids,
        )
        .unwrap_or_else(|e| panic!("malformed-input free fixture: {e}"));

        assert_eq!(report.verdicts().len(), 4, "all four FDM rules present");
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
        match &report.verdicts()[2].verdict {
            Verdict::Pass { .. } => {}
            other => panic!("expected Pass (fdm.min_bore, vacuous), got {other:?}"),
        }
        match &report.verdicts()[3].verdict {
            Verdict::Pass { .. } => {}
            other => panic!("expected Pass (fdm.trapped_volume, vacuous), got {other:?}"),
        }
    }

    /// Provenance presence: EVERY rule verdict this slice produces — FDM
    /// pass, FDM violation, FDM unverifiable, molding pass — carries the
    /// expected [`RuleProvenance::ShopPractice`] variant (spec §3.2.1:
    /// neither pack has a confirmed in-tree citation, so both are
    /// honestly `ShopPractice`, never dressed up as `Standard`).
    #[test]
    fn every_verdict_carries_shop_practice_provenance() {
        // Each case builds its OWN fresh fixture (rather than sharing one
        // set of stores across cases by reference, the old convention):
        // `model_with_solid` takes ownership of the bare stores to wrap
        // them into a `BRepModel`, so a shared instance could not be
        // wrapped twice.
        let fdm_params = || PackParams::Fdm {
            nozzle_diameter: 0.4,
            build_direction: [0.0, 0.0, 1.0],
        };

        let cases: Vec<(PackParams, _, _, Vec<FaceId>)> = vec![
            {
                // vertical wall: safe for both packs
                let (surfaces, faces, loops, edges, curves, face_id) =
                    plane_face_at_theta_deg(90.0);
                let face_ids = vec![face_id];
                let (model, solid_id) =
                    model_with_solid(surfaces, faces, loops, edges, curves, &face_ids);
                (fdm_params(), model, solid_id, face_ids)
            },
            {
                // steep overhang: fdm violates
                let (surfaces, faces, loops, edges, curves, face_id) =
                    plane_face_at_theta_deg(150.0);
                let face_ids = vec![face_id];
                let (model, solid_id) =
                    model_with_solid(surfaces, faces, loops, edges, curves, &face_ids);
                (fdm_params(), model, solid_id, face_ids)
            },
            {
                let (surfaces, faces, loops, edges, curves, face_id) = nurbs_face();
                let face_ids = vec![face_id];
                let (model, solid_id) =
                    model_with_solid(surfaces, faces, loops, edges, curves, &face_ids);
                (fdm_params(), model, solid_id, face_ids)
            },
            {
                let (surfaces, faces, loops, edges, curves, face_id) =
                    plane_face_at_theta_deg(90.0);
                let face_ids = vec![face_id];
                let (model, solid_id) =
                    model_with_solid(surfaces, faces, loops, edges, curves, &face_ids);
                (
                    PackParams::InjectionMolding {
                        pull_direction: [0.0, 0.0, 1.0],
                        min_draft_deg: 1.0,
                    },
                    model,
                    solid_id,
                    face_ids,
                )
            },
        ];

        for (params, model, solid_id, face_ids) in cases {
            let report = evaluate(params, &model, solid_id, &face_ids)
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
