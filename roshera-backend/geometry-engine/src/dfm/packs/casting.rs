//! Casting rule pack (two-part die / permanent mould; die casting is the
//! reference process for the defaults).
//!
//! Every rule rides an analyzer another pack already proved out — the
//! "one analyzer, many packs" architecture (spec §3.2):
//!
//! | rule | analyzer | claim |
//! |---|---|---|
//! | `cast.draft` | [`super::molding::evaluate_draft`] | every face's minimum draft vs `pull_direction` ≥ `min_draft_deg` |
//! | `cast.min_wall` | [`crate::dfm::analyzers::pair_thickness`] | every proven wall ≥ `min_wall` (thinner misruns / cold-shuts) |
//! | `cast.max_wall` | same | every proven wall ≤ `max_wall` (thicker = hot spot, shrink porosity) |
//! | `cast.internal_fillet` | `concave_features` + [`crate::dfm::analyzers::blend_radius`] | every sharp concave edge (radius 0) and every concave blend radius ≥ `min_fillet_radius` |
//! | `cast.cored_hole` | `hole_census` | every hole / bore diameter ≥ `min_cored_hole_diameter` |
//! | `cast.side_core` | `hole_census` | every hole's axis lies along `±pull_direction` — any other hole needs a side core / slide (or is drilled after casting) |
//! | `cast.trapped_core` | [`crate::dfm::analyzers::internal_voids`] | no fully enclosed cavity (a core there can never be removed) |
//!
//! All thresholds are shop practice (die-casting design guides diverge,
//! notably on fillet minimums; the default sits mid-range and the
//! provenance says so). Not checked: parting-line placement, gate/overflow
//! placement, undercuts formed by non-cylindrical geometry (a general
//! visibility analysis, roadmap).

use crate::dfm::analyzers::features::concave_features;
use crate::dfm::analyzers::features::ConcaveFeatures;
use crate::dfm::analyzers::holes::hole_census;
use crate::dfm::analyzers::{blend_radius, BlendRadiusOutcome};
use crate::dfm::packs::cnc::{evaluate_hole_access, evaluate_min_hole_diameter};
use crate::dfm::packs::common::{
    analytic, fold_wall_pairs, relabel, solid_face_ids, solid_wall_pairs, unit_direction, RuleFold,
    Sense,
};
use crate::dfm::packs::{fdm, molding, Rule, RulePack};
use crate::dfm::provenance::RuleProvenance;
use crate::dfm::report::{
    CastingParams, DfmError, DfmReport, PackParams, RuleVerdict, UnverifiableReason,
};
use crate::math::Vector3;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;

pub const DRAFT_RULE_ID: &str = "cast.draft";
pub const MIN_WALL_RULE_ID: &str = "cast.min_wall";
pub const MAX_WALL_RULE_ID: &str = "cast.max_wall";
pub const INTERNAL_FILLET_RULE_ID: &str = "cast.internal_fillet";
pub const CORED_HOLE_RULE_ID: &str = "cast.cored_hole";
pub const SIDE_CORE_RULE_ID: &str = "cast.side_core";
pub const TRAPPED_CORE_RULE_ID: &str = "cast.trapped_core";

fn practice(note: &str) -> RuleProvenance {
    RuleProvenance::ShopPractice {
        note: note.to_string(),
    }
}
pub fn draft_provenance() -> RuleProvenance {
    practice("die-casting draft ~1 deg (typical 1-3 deg by depth/alloy) — design-guide practice")
}
pub fn min_wall_provenance() -> RuleProvenance {
    practice("die-cast aluminium walls fill from ~1 mm (default) — design-guide practice")
}
pub fn max_wall_provenance() -> RuleProvenance {
    practice(
        "thick sections cool last (hot spots, shrink porosity, longer cycle); default 6 mm — \
         design-guide practice, alloy/process dependent",
    )
}
pub fn internal_fillet_provenance() -> RuleProvenance {
    practice(
        "sharp inside corners concentrate stress and impede metal flow; minimum fillet sources \
         diverge 0.5-1.5 mm, default 1.0 mm is mid-range — design-guide practice",
    )
}
pub fn cored_hole_provenance() -> RuleProvenance {
    practice("small cored pins break/erode; default minimum 2.5 mm — design-guide practice")
}
pub fn side_core_provenance() -> RuleProvenance {
    practice(
        "a fixed core pin is withdrawn along the die-opening axis (geometric fact); any other \
         hole axis needs a side core/slide or a post-cast drilling op",
    )
}
pub fn trapped_core_provenance() -> RuleProvenance {
    practice("a fully enclosed cavity's core can never be withdrawn (geometric fact)")
}

pub fn rule_pack(params: PackParams) -> RulePack {
    let r = |id, provenance| Rule { id, provenance };
    RulePack {
        params,
        rules: vec![
            r(DRAFT_RULE_ID, draft_provenance()),
            r(MIN_WALL_RULE_ID, min_wall_provenance()),
            r(MAX_WALL_RULE_ID, max_wall_provenance()),
            r(INTERNAL_FILLET_RULE_ID, internal_fillet_provenance()),
            r(CORED_HOLE_RULE_ID, cored_hole_provenance()),
            r(SIDE_CORE_RULE_ID, side_core_provenance()),
            r(TRAPPED_CORE_RULE_ID, trapped_core_provenance()),
        ],
    }
}

/// Inside-corner radius rule shared by casting and molding: every sharp
/// concave edge reads radius 0; every concave blend reads its radius
/// (a PROVEN bounded freeform radius decides only when its enclosure
/// separates from the limit); undetermined edges and unsupported blends
/// refuse.
pub(crate) fn evaluate_inside_fillet(
    features: &ConcaveFeatures,
    blends: &BlendRadiusOutcome,
    min_radius: f64,
    rule: &'static str,
    provenance: RuleProvenance,
) -> RuleVerdict {
    let mut fold = RuleFold::new(
        rule,
        provenance,
        Sense::AtLeast,
        min_radius,
        format!("{rule} limit: minimum internal fillet radius (pack params)"),
    );
    for e in &features.sharp_edges {
        fold.observe(
            e.faces.to_vec(),
            0.0,
            analytic(format!(
                "sharp concave edge {}: no fillet, radius 0",
                e.edge
            )),
        );
    }
    for b in &blends.blends {
        match b.radius.bound {
            None => fold.observe(vec![b.face], b.radius.value, b.radius.derivation.clone()),
            Some(bound) if bound.lo >= min_radius || bound.hi < min_radius => {
                let v = if bound.lo >= min_radius {
                    bound.lo
                } else {
                    bound.hi
                };
                fold.observe(vec![b.face], v, b.radius.derivation.clone());
            }
            Some(bound) => fold.refuse(
                vec![b.face],
                UnverifiableReason::BoundNotSeparating {
                    lo: bound.lo,
                    hi: bound.hi,
                    limit: min_radius,
                    refinement_depth: 0,
                    converged: false,
                },
            ),
        }
    }
    for u in &blends.unverifiable {
        fold.refuse(vec![u.face], u.reason.clone());
    }
    for u in &features.undetermined {
        fold.refuse(
            u.faces.to_vec(),
            UnverifiableReason::UnsupportedTopology {
                detail: format!("edge {}: convexity could not be derived", u.edge),
            },
        );
    }
    fold.finish()
}

pub fn evaluate(
    model: &BRepModel,
    solid_id: SolidId,
    params: &CastingParams,
) -> Result<DfmReport, DfmError> {
    let pull = unit_direction(params.pull_direction, "pull direction")?;
    let faces = solid_face_ids(model, solid_id)?;
    let features = concave_features(model, solid_id)?;
    let census = hole_census(model, solid_id, &features);
    let blends = blend_radius(model, solid_id)?;
    let walls = solid_wall_pairs(model, &faces)?;

    let draft = relabel(
        molding::evaluate_draft(
            &faces,
            Vector3::new(pull[0], pull[1], pull[2]),
            params.min_draft_deg,
            &model.faces,
            &model.loops,
            &model.edges,
            &model.curves,
            &model.surfaces,
        )?,
        DRAFT_RULE_ID,
        draft_provenance(),
    );

    let mut min_wall = RuleFold::new(
        MIN_WALL_RULE_ID,
        min_wall_provenance(),
        Sense::AtLeast,
        params.min_wall,
        "cast.min_wall limit: min_wall (pack params)",
    );
    fold_wall_pairs(&mut min_wall, &walls);
    let mut max_wall = RuleFold::new(
        MAX_WALL_RULE_ID,
        max_wall_provenance(),
        Sense::AtMost,
        params.max_wall,
        "cast.max_wall limit: max_wall (pack params)",
    );
    fold_wall_pairs(&mut max_wall, &walls);

    let fillet = evaluate_inside_fillet(
        &features,
        &blends,
        params.min_fillet_radius,
        INTERNAL_FILLET_RULE_ID,
        internal_fillet_provenance(),
    );
    let cored = relabel(
        evaluate_min_hole_diameter(&census, params.min_cored_hole_diameter),
        CORED_HOLE_RULE_ID,
        cored_hole_provenance(),
    );
    let side_core = relabel(
        evaluate_hole_access(&census, &[pull, [-pull[0], -pull[1], -pull[2]]], 1e-6),
        SIDE_CORE_RULE_ID,
        side_core_provenance(),
    );
    let trapped = relabel(
        fdm::evaluate_trapped_volume(model, solid_id)?,
        TRAPPED_CORE_RULE_ID,
        trapped_core_provenance(),
    );

    Ok(DfmReport::new(
        PackParams::Casting(params.clone()),
        vec![
            draft,
            min_wall.finish(),
            max_wall.finish(),
            fillet,
            cored,
            side_core,
            trapped,
        ],
    ))
}
