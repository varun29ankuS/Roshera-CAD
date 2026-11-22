//! CNC milling (3-axis) rule pack — "can this be milled from the setups I
//! plan, and what will hurt?"
//!
//! ## The setup plan is a parameter, not a guess
//!
//! A 3-axis mill cuts with a tool whose axis is fixed per setup. Which
//! inside corners a cutter's SIDE has to form, and which holes a drill can
//! enter, depend on the setups — so [`CncMillingParams::setup_directions`]
//! (unit vectors from the part toward the spindle; default `[+Z, −Z]`: one
//! op from the top, flip, one from the bottom) is part of the question. An
//! agent planning side ops adds `±X` / `±Y`; an angled hole that no listed
//! setup reaches is then a real finding (4/5-axis or a fixture).
//!
//! ## What every rule claims, exactly
//!
//! | rule | claim |
//! |---|---|
//! | `cnc.internal_corner_radius` | every inside corner a rotating cutter's SIDE must form (a sharp concave Line edge parallel to a setup direction that is not a floor corner; a partial-turn cavity cylinder whose axis is parallel to a setup direction) has radius ≥ `min_tool_diameter / 2`. A FLOOR corner — one face a plane facing a setup direction, the other parallel to it — is cut sharp by an end mill's end and is out of scope; every other concave edge (inclined, intersection curve, undetermined convexity) refuses. |
//! | `cnc.corner_reach` | every such radiused vertical corner: axial length ÷ (2·radius) ≤ `max_corner_reach_ratio` — the reach of the largest tool that can finish it. One-sided (full corner length). |
//! | `cnc.hole_depth_ratio` | every hole: drilled length ÷ diameter ≤ `max_drill_depth_ratio` (an unrecognised bore's wall length is only a lower bound: it proves a violation or refuses). |
//! | `cnc.min_hole_diameter` | every hole / bore diameter ≥ `min_drill_diameter`. |
//! | `cnc.standard_drill_size` | every hole ≤ `standard_drill_max_diameter`: distance to the nearest size of `drill_series` ≤ `drill_size_tolerance`. |
//! | `cnc.blind_hole_floor` | every recognised blind hole's floor: drill-point included angle (flat floor = 180°) within `drill_point_tolerance_deg` of an allowed angle (`drill_point_angles_deg`, plus 180° iff `allow_flat_bottom_holes`); unrecognised bores (floor unknown) refuse. |
//! | `cnc.hole_access` | every hole's approach direction (its entry; THRU without an entry feature: either end) lies within `hole_axis_tolerance_deg` of a setup direction — a drill enters along its own axis. This is the AXIS condition; line-of-sight occlusion of the entry by other material is NOT claimed. |
//! | `cnc.min_wall` | every proven wall pair ([`pair_thickness`]) ≥ `min_wall`; unpaired faces refuse. |
//!
//! Every default is shop practice (no governing standard exists for any of
//! these numbers) and says so in its [`RuleProvenance::ShopPractice`].
//!
//! ## Not checked (named so absence is not oversight)
//!
//! Thread depth (threads are cosmetic attributes, no geometry to measure);
//! line-of-sight occlusion and fixturing; deep narrow SLOT width-to-depth
//! (needs void-side pairing `pair_thickness` does not do); floor-fillet
//! radii (formed by the tool's END profile, any ball/bull-nose matches).

use crate::dfm::analyzers::features::{
    concave_features, dot3, ConcaveEdge, ConcaveFeatures, EdgeShape, PARALLEL_COS,
    PERPENDICULAR_COS,
};
use crate::dfm::analyzers::holes::{hole_census, HoleCensus};
use crate::dfm::packs::common::{
    analytic, analytic_on, angle_deg, fold_wall_pairs, solid_face_ids, solid_wall_pairs,
    unit_directions, RuleFold, Sense,
};
use crate::dfm::packs::{Rule, RulePack};
use crate::dfm::provenance::RuleProvenance;
use crate::dfm::report::{
    CncMillingParams, DfmError, DfmReport, DrillSeries, PackParams, RuleVerdict, SurfaceKind,
    UnverifiableReason,
};
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::readable::adjacency::FaceGeometry;

pub const INTERNAL_CORNER_RADIUS_RULE_ID: &str = "cnc.internal_corner_radius";
pub const CORNER_REACH_RULE_ID: &str = "cnc.corner_reach";
pub const HOLE_DEPTH_RATIO_RULE_ID: &str = "cnc.hole_depth_ratio";
pub const MIN_HOLE_DIAMETER_RULE_ID: &str = "cnc.min_hole_diameter";
pub const STANDARD_DRILL_SIZE_RULE_ID: &str = "cnc.standard_drill_size";
pub const BLIND_HOLE_FLOOR_RULE_ID: &str = "cnc.blind_hole_floor";
pub const HOLE_ACCESS_RULE_ID: &str = "cnc.hole_access";
pub const MIN_WALL_RULE_ID: &str = "cnc.min_wall";

fn practice(note: &str) -> RuleProvenance {
    RuleProvenance::ShopPractice {
        note: note.to_string(),
    }
}

pub fn internal_corner_radius_provenance() -> RuleProvenance {
    practice(
        "a rotating cutter's side leaves at least its own radius in an inside corner (geometric \
         fact); the minimum tool diameter (default 3 mm) is the shop's tooling — shop practice, \
         no governing standard",
    )
}
pub fn corner_reach_provenance() -> RuleProvenance {
    practice(
        "standard end mills cut ~3-4x their diameter (default limit 4; Hubs design guide: \
         cavity depth <= 4x width, https://www.hubs.com/knowledge-base/how-design-parts-cnc-machining/, \
         accessed 2026-11-22; Protolabs allows ~6x) — vendor practice, no governing standard",
    )
}
pub fn hole_depth_ratio_provenance() -> RuleProvenance {
    practice(
        "twist drills to ~10x diameter (default limit 10); deeper needs peck/gun drilling — \
         vendor design-guide practice, no governing standard",
    )
}
pub fn min_hole_diameter_provenance() -> RuleProvenance {
    practice("smallest general-shop drill (default 1.0 mm) — shop practice")
}
pub fn standard_drill_size_provenance() -> RuleProvenance {
    practice(
        "commercial drill series: metric jobber sets in 0.1 mm steps, fractional-inch in 1/64 in \
         steps; an off-series diameter needs a special drill or interpolation — shop practice. \
         Related standard DIN 338:2006-11 (jobber drills; current per din.de, checked \
         2026-11-22) — its diameter table was NOT transcribed, so it is not cited as authority",
    )
}
pub fn blind_hole_floor_provenance() -> RuleProvenance {
    practice(
        "standard twist-drill point angles 118/135/140 deg; a flat floor needs a flat-bottom \
         drill or an end-mill pass — shop practice",
    )
}
pub fn hole_access_provenance() -> RuleProvenance {
    practice(
        "a drill enters along its own axis, so a hole needs a setup along it (geometric fact); \
         the setup list is the caller's machining plan",
    )
}
pub fn min_wall_provenance() -> RuleProvenance {
    practice(
        "thin walls chatter and deflect; default 0.8 mm (metals; 1.5 mm plastics) per the Hubs \
         CNC design guide, https://www.hubs.com/knowledge-base/how-design-parts-cnc-machining/, \
         accessed 2026-11-22 — vendor practice, no governing standard",
    )
}

/// The CNC pack's declared rule list.
pub fn rule_pack(params: PackParams) -> RulePack {
    RulePack {
        params,
        rules: vec![
            Rule {
                id: INTERNAL_CORNER_RADIUS_RULE_ID,
                provenance: internal_corner_radius_provenance(),
            },
            Rule {
                id: CORNER_REACH_RULE_ID,
                provenance: corner_reach_provenance(),
            },
            Rule {
                id: HOLE_DEPTH_RATIO_RULE_ID,
                provenance: hole_depth_ratio_provenance(),
            },
            Rule {
                id: MIN_HOLE_DIAMETER_RULE_ID,
                provenance: min_hole_diameter_provenance(),
            },
            Rule {
                id: STANDARD_DRILL_SIZE_RULE_ID,
                provenance: standard_drill_size_provenance(),
            },
            Rule {
                id: BLIND_HOLE_FLOOR_RULE_ID,
                provenance: blind_hole_floor_provenance(),
            },
            Rule {
                id: HOLE_ACCESS_RULE_ID,
                provenance: hole_access_provenance(),
            },
            Rule {
                id: MIN_WALL_RULE_ID,
                provenance: min_wall_provenance(),
            },
        ],
    }
}

fn parallel(a: [f64; 3], b: [f64; 3]) -> bool {
    dot3(a, b).abs() >= PARALLEL_COS
}

/// Is `geom` a face PARALLEL to direction `d` (a wall seen from `d`)?
fn is_wall_along(geom: &FaceGeometry, d: [f64; 3]) -> bool {
    match geom {
        FaceGeometry::Plane { normal, .. } => dot3(*normal, d).abs() <= PERPENDICULAR_COS,
        FaceGeometry::Cylinder { axis, .. } => parallel(*axis, d),
        _ => false,
    }
}

/// Is `geom` a plane whose outward normal points AT direction `d` (a floor
/// an end mill's end faces from setup `d`)?
fn is_floor_facing(geom: &FaceGeometry, d: [f64; 3]) -> bool {
    matches!(geom, FaceGeometry::Plane { normal, .. } if dot3(*normal, d) >= PARALLEL_COS)
}

/// A sharp concave edge an end mill cuts with its END from some setup:
/// one face a floor facing the setup, the other a wall along it.
pub(crate) fn is_floor_corner(
    features: &ConcaveFeatures,
    edge: &ConcaveEdge,
    setups: &[[f64; 3]],
) -> bool {
    let (Some(a), Some(b)) = (
        features.geometry(edge.faces[0]),
        features.geometry(edge.faces[1]),
    ) else {
        return false;
    };
    setups.iter().any(|&d| {
        (is_floor_facing(a, d) && is_wall_along(b, d))
            || (is_floor_facing(b, d) && is_wall_along(a, d))
    })
}

pub fn evaluate_internal_corner_radius(
    features: &ConcaveFeatures,
    setups: &[[f64; 3]],
    min_tool_diameter: f64,
) -> RuleVerdict {
    let mut fold = RuleFold::new(
        INTERNAL_CORNER_RADIUS_RULE_ID,
        internal_corner_radius_provenance(),
        Sense::AtLeast,
        min_tool_diameter / 2.0,
        "cnc.internal_corner_radius limit: min_tool_diameter / 2 (pack params)",
    );
    for e in &features.sharp_edges {
        if is_floor_corner(features, e, setups) {
            continue;
        }
        match &e.shape {
            EdgeShape::Line { direction } if setups.iter().any(|&d| parallel(*direction, d)) => {
                fold.observe(
                    e.faces.to_vec(),
                    0.0,
                    analytic(format!(
                        "sharp vertical inside corner (edge {}): a rotating cutter's side cannot \
                         form radius 0",
                        e.edge
                    )),
                );
            }
            _ => fold.refuse(
                e.faces.to_vec(),
                UnverifiableReason::UnsupportedTopology {
                    detail: format!(
                        "concave edge {} is neither a floor corner nor parallel to a setup \
                         direction: forming it needs a non-square tool or another setup, which \
                         this rule does not assess",
                        e.edge
                    ),
                },
            ),
        }
    }
    for c in &features.cavity_cylinders {
        if c.is_partial() && setups.iter().any(|&d| parallel(c.axis, d)) {
            fold.observe(
                c.faces.clone(),
                c.radius,
                analytic_on(
                    SurfaceKind::Cylinder,
                    "vertical inside-corner radius: partial-turn cavity cylinder parallel to a \
                     setup direction",
                ),
            );
        }
    }
    for u in &features.undetermined {
        fold.refuse(
            u.faces.to_vec(),
            UnverifiableReason::UnsupportedTopology {
                detail: format!(
                    "edge {}: convexity could not be derived, so it may be an inside corner",
                    u.edge
                ),
            },
        );
    }
    for n in &features.graph.nodes {
        if let FaceGeometry::Other { type_name } = &n.geometry {
            if features.resolved_fillets.contains(&n.face) {
                continue; // read exactly as a cylinder above
            }
            fold.refuse(
                vec![n.face],
                UnverifiableReason::UnsupportedTopology {
                    detail: format!(
                        "face {} is a {type_name} surface: whether it hides an inside corner \
                         a cutter must form is not decided",
                        n.face
                    ),
                },
            );
        }
    }
    fold.finish()
}

pub fn evaluate_corner_reach(
    features: &ConcaveFeatures,
    setups: &[[f64; 3]],
    max_ratio: f64,
) -> RuleVerdict {
    let mut fold = RuleFold::new(
        CORNER_REACH_RULE_ID,
        corner_reach_provenance(),
        Sense::AtMost,
        max_ratio,
        "cnc.corner_reach limit: max_corner_reach_ratio (pack params)",
    );
    for c in &features.cavity_cylinders {
        if c.is_partial() && setups.iter().any(|&d| parallel(c.axis, d)) && c.radius > 0.0 {
            fold.observe(
                c.faces.clone(),
                c.length() / (2.0 * c.radius),
                analytic_on(
                    SurfaceKind::Cylinder,
                    format!(
                        "corner reach = axial length {:.4} / (2 x radius {:.4})",
                        c.length(),
                        c.radius
                    ),
                ),
            );
        }
    }
    fold.finish()
}

pub fn evaluate_hole_depth_ratio(census: &HoleCensus, max_ratio: f64) -> RuleVerdict {
    let mut fold = RuleFold::new(
        HOLE_DEPTH_RATIO_RULE_ID,
        hole_depth_ratio_provenance(),
        Sense::AtMost,
        max_ratio,
        "cnc.hole_depth_ratio limit: max_drill_depth_ratio (pack params)",
    );
    for h in &census.holes {
        fold.observe(
            h.faces.clone(),
            h.drilled_length / h.stack.diameter,
            analytic_on(
                SurfaceKind::Cylinder,
                format!(
                    "drilled length {:.4} / diameter {:.4} ({} hole)",
                    h.drilled_length,
                    h.stack.diameter,
                    if h.stack.through { "THRU" } else { "blind" }
                ),
            ),
        );
    }
    for b in &census.unrecognised {
        fold.observe_lower_bound(
            b.faces.clone(),
            b.wall_length / b.diameter,
            analytic_on(
                SurfaceKind::Cylinder,
                "bore wall length / diameter (lower bound on the hole's L/D)",
            ),
            "the bore is not a recognised hole stack; its wall can be shorter than the hole",
        );
    }
    fold.finish()
}

pub fn evaluate_min_hole_diameter(census: &HoleCensus, min_diameter: f64) -> RuleVerdict {
    let mut fold = RuleFold::new(
        MIN_HOLE_DIAMETER_RULE_ID,
        min_hole_diameter_provenance(),
        Sense::AtLeast,
        min_diameter,
        "cnc.min_hole_diameter limit: min_drill_diameter (pack params)",
    );
    for h in &census.holes {
        fold.observe(
            h.faces.clone(),
            h.stack.diameter,
            analytic_on(SurfaceKind::Cylinder, "drill diameter: 2 x wall radius"),
        );
    }
    for b in &census.unrecognised {
        fold.observe(
            b.faces.clone(),
            b.diameter,
            analytic_on(SurfaceKind::Cylinder, "bore diameter: 2 x wall radius"),
        );
    }
    fold.finish()
}

/// Nearest size of `series` to `d` and the distance to it, mm.
pub(crate) fn nearest_series_size(d: f64, series: DrillSeries) -> (f64, f64) {
    let near = |step: f64| {
        let k = (d / step).round().max(1.0);
        let s = k * step;
        (s, (d - s).abs())
    };
    let metric = near(0.1);
    let inch = near(25.4 / 64.0);
    match series {
        DrillSeries::Metric => metric,
        DrillSeries::InchFractional => inch,
        DrillSeries::MetricOrInch => {
            if inch.1 < metric.1 {
                inch
            } else {
                metric
            }
        }
    }
}

pub fn evaluate_standard_drill_size(census: &HoleCensus, p: &CncMillingParams) -> RuleVerdict {
    let mut fold = RuleFold::new(
        STANDARD_DRILL_SIZE_RULE_ID,
        standard_drill_size_provenance(),
        Sense::AtMost,
        p.drill_size_tolerance,
        "cnc.standard_drill_size limit: drill_size_tolerance (pack params)",
    );
    let mut check = |faces: Vec<u32>, d: f64| {
        if d > p.standard_drill_max_diameter {
            return; // milled / bored, outside this rule's scope
        }
        let (size, dev) = nearest_series_size(d, p.drill_series);
        fold.observe(
            faces,
            dev,
            analytic_on(
                SurfaceKind::Cylinder,
                format!(
                    "|diameter {d:.4} - nearest {:?} drill {size:.4}|",
                    p.drill_series
                ),
            ),
        );
    };
    for h in &census.holes {
        check(h.faces.clone(), h.stack.diameter);
    }
    for b in &census.unrecognised {
        check(b.faces.clone(), b.diameter);
    }
    fold.finish()
}

pub fn evaluate_blind_hole_floor(census: &HoleCensus, p: &CncMillingParams) -> RuleVerdict {
    let mut fold = RuleFold::new(
        BLIND_HOLE_FLOOR_RULE_ID,
        blind_hole_floor_provenance(),
        Sense::AtMost,
        p.drill_point_tolerance_deg,
        "cnc.blind_hole_floor limit: drill_point_tolerance_deg (pack params)",
    );
    let mut allowed = p.drill_point_angles_deg.clone();
    if p.allow_flat_bottom_holes {
        allowed.push(180.0);
    }
    // Drilled holes only: a blind bore above the drill-series scope (a
    // bearing seat, a counterbore-sized pocket) is milled or interpolated,
    // and its flat floor is the intended shape, not a drill-point defect.
    for h in census
        .holes
        .iter()
        .filter(|h| !h.stack.through && h.stack.diameter <= p.standard_drill_max_diameter)
    {
        let angle = h.stack.drill_point_deg.unwrap_or(180.0);
        let (nearest, dev) = allowed
            .iter()
            .map(|&a| (a, (angle - a).abs()))
            .min_by(|x, y| x.1.total_cmp(&y.1))
            .unwrap_or((f64::NAN, 180.0));
        fold.observe(
            h.faces.clone(),
            dev,
            analytic_on(
                SurfaceKind::Cone,
                format!(
                    "blind floor included angle {angle:.4} deg ({}) vs nearest allowed {nearest:.4}",
                    if h.stack.drill_point_deg.is_some() {
                        "drill point"
                    } else {
                        "flat floor = 180"
                    }
                ),
            ),
        );
    }
    for b in census
        .unrecognised
        .iter()
        .filter(|b| b.diameter <= p.standard_drill_max_diameter)
    {
        fold.refuse(
            b.faces.clone(),
            UnverifiableReason::UnsupportedTopology {
                detail: "bore is not a recognised hole stack: whether it is blind, and its \
                         floor, are unknown"
                    .to_string(),
            },
        );
    }
    fold.finish()
}

pub fn evaluate_hole_access(census: &HoleCensus, setups: &[[f64; 3]], tol_deg: f64) -> RuleVerdict {
    let mut fold = RuleFold::new(
        HOLE_ACCESS_RULE_ID,
        hole_access_provenance(),
        Sense::AtMost,
        tol_deg,
        "cnc.hole_access limit: hole_axis_tolerance_deg (pack params)",
    );
    let best = |dirs: &[[f64; 3]]| {
        dirs.iter()
            .flat_map(|&e| setups.iter().map(move |&d| angle_deg(e, d)))
            .fold(180.0f64, f64::min)
    };
    for h in &census.holes {
        let m = best(&h.approach_directions());
        fold.observe(
            h.faces.clone(),
            m,
            analytic_on(
                SurfaceKind::Cylinder,
                "angle between the hole's approach direction and the nearest setup direction \
                 (axis condition only; occlusion not assessed)",
            ),
        );
    }
    for b in &census.unrecognised {
        let a = b.axis;
        let plus = best(&[a]);
        let minus = best(&[[-a[0], -a[1], -a[2]]]);
        let derivation = analytic_on(
            SurfaceKind::Cylinder,
            "angle between the bore axis (either end) and the nearest setup direction",
        );
        if plus.min(minus) > tol_deg {
            // Neither end lines up with any setup: proven, whichever end is open.
            fold.observe(b.faces.clone(), plus.min(minus), derivation);
        } else if plus <= tol_deg && minus <= tol_deg {
            // Both ends line up: reachable whichever end is open.
            fold.observe(b.faces.clone(), plus.max(minus), derivation);
        } else {
            fold.refuse(
                b.faces.clone(),
                UnverifiableReason::UnsupportedTopology {
                    detail: "only one end of this unrecognised bore lines up with a setup, and \
                             which end is open is unknown"
                        .to_string(),
                },
            );
        }
    }
    fold.finish()
}

pub fn evaluate_min_wall(
    model: &BRepModel,
    faces: &[u32],
    min_wall: f64,
    rule: &'static str,
    provenance: RuleProvenance,
) -> Result<RuleVerdict, DfmError> {
    let outcome = solid_wall_pairs(model, faces)?;
    let mut fold = RuleFold::new(
        rule,
        provenance,
        Sense::AtLeast,
        min_wall,
        format!("{rule} limit: min_wall (pack params)"),
    );
    fold_wall_pairs(&mut fold, &outcome);
    Ok(fold.finish())
}

/// Run every CNC milling rule against the solid.
pub fn evaluate(
    model: &BRepModel,
    solid_id: SolidId,
    params: &CncMillingParams,
) -> Result<DfmReport, DfmError> {
    let setups = unit_directions(&params.setup_directions, "setup direction")?;
    if setups.is_empty() {
        return Err(DfmError::InvalidParams {
            detail: "cnc_3_axis needs at least one setup direction".to_string(),
        });
    }
    let faces = solid_face_ids(model, solid_id)?;
    let features = concave_features(model, solid_id)?;
    let census = hole_census(model, solid_id, &features);
    let verdicts = vec![
        evaluate_internal_corner_radius(&features, &setups, params.min_tool_diameter),
        evaluate_corner_reach(&features, &setups, params.max_corner_reach_ratio),
        evaluate_hole_depth_ratio(&census, params.max_drill_depth_ratio),
        evaluate_min_hole_diameter(&census, params.min_drill_diameter),
        evaluate_standard_drill_size(&census, params),
        evaluate_blind_hole_floor(&census, params),
        evaluate_hole_access(&census, &setups, params.hole_axis_tolerance_deg),
        evaluate_min_wall(
            model,
            &faces,
            params.min_wall,
            MIN_WALL_RULE_ID,
            min_wall_provenance(),
        )?,
    ];
    Ok(DfmReport::new(
        PackParams::Cnc3Axis(params.clone()),
        verdicts,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_series_snaps_to_tenths_and_inch_to_64ths() {
        let (s, d) = nearest_series_size(6.6, DrillSeries::Metric);
        assert!((s - 6.6).abs() < 1e-9 && d < 1e-9);
        let (s, d) = nearest_series_size(6.35, DrillSeries::Metric);
        assert!((s - 6.3).abs() < 1e-9 || (s - 6.4).abs() < 1e-9);
        assert!((d - 0.05).abs() < 1e-9);
        let (_, d) = nearest_series_size(6.35, DrillSeries::InchFractional);
        assert!(d < 1e-9, "1/4 in = 6.35 mm is a fractional drill: {d}");
    }
}
