//! What the model has that the sheet does not dimension.
//!
//! The layout does not place every dimension a view carries (a dimension
//! with no room is left off rather than printed over a view), equal features
//! are stated once with their count, and a person can take a dimension off
//! the sheet. None of those may make the sheet LOOK complete.
//!
//! [`dimension_omissions`] walks the LIVE model -- the same dimension table
//! every view is built from, before any thinning -- and reports every
//! dimension that no view of the sheet states. It does not trust anything the
//! sheet says about itself: a sheet built before this accounting existed, or
//! one whose dimension was deleted from the stored record, is caught the same
//! way as one the selection thinned a minute ago.
//!
//! An omission is one of two things in the certificate
//! ([`super::sheet_certificate`]):
//!
//! * **not dimensioned** -- nobody decided it; the sheet is not complete;
//! * **waived** -- a named person suppressed that dimension
//!   ([`super::dimension_edit::Waiver`]); listed, with the name, and the
//!   sheet is complete only in the sense that every gap is signed for.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::dimension_edit::Waiver;
use super::dimensioning::{
    candidates_from_records, convention_records, dimension_identity as identity, Dimension2d,
};
use super::layout::compute_layout;
use super::types::{Drawing, ProjectionType, ViewSource};
use crate::primitives::topology_builder::BRepModel;

/// Why a dimension the model has is not on the sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OmissionReason {
    /// No longer produced: the automatic sheet used to drop every angle of a
    /// view that had more than two. Kept so stored certificates still parse.
    AngleClutter,
    /// No longer produced: equal values used to be collapsed with no count.
    /// Equal features are now counted on one callout and are stated, not
    /// omitted. Kept so stored certificates still parse.
    MergedWithEqualValue,
    /// No longer produced: a view used to keep five diameters. Kept so
    /// stored certificates still parse.
    DiameterLimit,
    /// No longer produced: a view used to keep eight dimensions. Kept so
    /// stored certificates still parse.
    ViewLimit,
    /// The view carries the dimension (itself, or inside a counted callout)
    /// but the layout does not draw it: there was no room for it clear of
    /// the other views, or it was moved off the sheet.
    NotPlaced,
    /// A person took it off the sheet.
    Suppressed,
    /// It is on no view, and no rule above explains why (the sheet's stored
    /// record was changed, or the view that showed it was removed).
    NotOnSheet,
}

impl OmissionReason {
    /// The reason in words, for the certificate line.
    pub fn text(self) -> &'static str {
        match self {
            OmissionReason::AngleClutter => "more than two angles in the view",
            OmissionReason::MergedWithEqualValue => "another dimension of the same value was kept",
            OmissionReason::DiameterLimit => "more diameters than the view keeps",
            OmissionReason::ViewLimit => "more dimensions than the view keeps",
            OmissionReason::NotPlaced => "no room on the sheet to draw it",
            OmissionReason::Suppressed => "suppressed",
            OmissionReason::NotOnSheet => "on no view of the sheet",
        }
    }
}

/// One dimension of the model that no view of the sheet states.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimensionOmission {
    /// The view it would be read in (`"FRONT"`).
    pub view: String,
    /// Index into `Drawing::views` of that view, when the sheet has it.
    pub owner_view: Option<usize>,
    /// The dimension's durable key: what a suppression of it is recorded
    /// under. `None` when its faces carry no persistent id.
    pub key: Option<String>,
    /// `"diameter"`, `"angle"`, `"length"`, ...
    pub kind: String,
    /// The label it would be inked with.
    pub label: String,
    pub value: f64,
    pub unit: String,
    /// B-Rep face ids it spans.
    pub entities: Vec<u32>,
    /// Durable dimension id, when it has one.
    pub pid: Option<String>,
    pub reason: OmissionReason,
    /// Who waived it, when someone did.
    pub waiver: Option<Waiver>,
}

/// The three principal directions a part is dimensioned from. A dimension is
/// owed when it reads in one of them, whichever views the sheet happens to
/// carry: removing the TOP view does not remove the obligation to state what
/// only TOP showed.
const PRINCIPAL: [(ProjectionType, &str); 3] = [
    (ProjectionType::Front, "FRONT"),
    (ProjectionType::Top, "TOP"),
    (ProjectionType::Right, "RIGHT"),
];

/// A span shorter than this in a view is edge-on there (same threshold the
/// sheet builders use).
const MIN_SPAN: f64 = 0.5;

/// `(orientation, lo, hi)` of a dimension's span in its view: the interval
/// the same-view dedup compares.
fn interval(d: &Dimension2d) -> Option<(char, f64, f64)> {
    let dx = (d.a[0] - d.b[0]).abs();
    let dy = (d.a[1] - d.b[1]).abs();
    if dx < 1e-6 && dy < 1e-6 {
        return None;
    }
    Some(if dx >= dy {
        ('H', d.a[0].min(d.b[0]), d.a[0].max(d.b[0]))
    } else {
        ('V', d.a[1].min(d.b[1]), d.a[1].max(d.b[1]))
    })
}

/// Does a dimension inked in the same view state the same number over the
/// same interval? (The same-view dedup keeps one of two such dimensions --
/// a boss's length and the part's extent over the same span.)
fn same_statement(c: &Dimension2d, inked: &[&Dimension2d]) -> bool {
    // A leader callout brackets no interval (see the same-view dedup).
    if c.kind == "radius" {
        return false;
    }
    let Some((oc, lo, hi)) = interval(c) else {
        return false;
    };
    inked.iter().any(|d| {
        (d.value - c.value).abs() <= 0.01
            && interval(d)
                .is_some_and(|(od, l, h)| od == oc && (l - lo).abs() < 0.5 && (h - hi).abs() < 0.5)
    })
}

/// Every dimension of the drawn solid that no view of `drawing` states, each
/// once, attributed to the first principal view it reads in.
///
/// "Stated" means INKED: placed by the layout -- itself, or as one of the
/// equal features a counted callout ("4× R5") speaks for -- or (a bore's
/// size, depth and corner offsets) carried by a hole-table row or a hole
/// callout.
pub fn dimension_omissions(model: &BRepModel, drawing: &Drawing) -> Vec<DimensionOmission> {
    let Some(solid_id) = drawing.views.first().map(|v| {
        let ViewSource::Part { solid_id, .. } = v.source;
        solid_id
    }) else {
        return Vec::new();
    };
    if model.solids.get(solid_id).is_none() {
        // The part is gone: every fact on the sheet is already dangling, and
        // there is no model to owe a dimension to.
        return Vec::new();
    }
    let _adjacency = crate::readable::adjacency::AdjacencyCacheScope::enter(model);

    // What the sheet inks, per view.
    let layout = compute_layout(drawing);
    let mut inked_ids: Vec<HashSet<&str>> = vec![HashSet::new(); drawing.views.len()];
    for pd in &layout.dimensions {
        if let Some(set) = inked_ids.get_mut(pd.owner_view) {
            set.insert(super::layout::base_dim_id(&pd.dim_id));
        }
    }
    let inked: Vec<Vec<&Dimension2d>> = drawing
        .views
        .iter()
        .enumerate()
        .map(|(vi, v)| {
            v.dimensions
                .iter()
                .filter(|d| inked_ids[vi].contains(d.id.as_str()))
                .collect()
        })
        .collect();
    // A counted callout states every feature it names.
    let statements = |d: &Dimension2d| -> Vec<String> {
        let mut ids = d.members.clone();
        ids.push(identity(d));
        ids
    };
    let inked_identities: HashSet<String> =
        inked.iter().flatten().flat_map(|d| statements(d)).collect();
    let carried: HashSet<String> = drawing
        .views
        .iter()
        .flat_map(|v| v.dimensions.iter())
        .flat_map(statements)
        .collect();
    let chain_located = chain_located(inked.iter().flatten().copied());
    let stated_bores = super::sheet_certificate::stated_bore_faces(drawing);
    // The bore locations a hole-table row or callout states, per view: they
    // are not inked as dimensions, but the sheet prints their numbers. A
    // feature coaxial with the bore (the boss it is drilled through, a
    // counterbore) has the same number over the same interval -- the
    // same-view dedup kept only one of the two -- and is located by it.
    let hole_stated: Vec<Vec<&Dimension2d>> = drawing
        .views
        .iter()
        .map(|v| {
            v.dimensions
                .iter()
                .filter(|d| {
                    d.kind == "position"
                        && !d.entities.is_empty()
                        && d.entities.iter().all(|f| stated_bores.contains(f))
                })
                .collect()
        })
        .collect();
    let section_view = drawing.section.as_ref().map(|s| s.section_view_idx);

    let records = convention_records(model, solid_id);
    let mut covered: HashSet<String> = HashSet::new();
    // (identity, candidate, view name, view index)
    let mut missing: Vec<(String, Dimension2d, &str, Option<usize>)> = Vec::new();
    for (projection, name) in PRINCIPAL {
        let owner = drawing.views.iter().enumerate().position(|(i, v)| {
            v.projection == projection && v.shaded_raster.is_none() && Some(i) != section_view
        });
        let candidates = candidates_from_records(model, records.clone(), projection, MIN_SPAN);
        // The locations this view owes that a chain states, as they read
        // from the datum: a location that states the same place is stated
        // by the chain too (`chain_covers`).
        let chained: Vec<Dimension2d> = candidates
            .iter()
            .filter(|c| c.pid.as_deref().is_some_and(|p| chain_located.contains(p)))
            .cloned()
            .collect();
        let inked_here: &[&Dimension2d] = owner
            .and_then(|i| inked.get(i))
            .map_or(&[][..], |v| v.as_slice());
        for c in candidates {
            let id = identity(&c);
            // A bore's size, depth and corner offsets are stated by its
            // hole-table row or callout.
            let by_hole_statement = !c.entities.is_empty()
                && matches!(
                    c.kind.as_str(),
                    "diameter" | "radius" | "length" | "position"
                )
                && c.entities.iter().all(|f| stated_bores.contains(f));
            let hole_stated_here: &[&Dimension2d] = owner
                .and_then(|i| hole_stated.get(i))
                .map_or(&[][..], |v| v.as_slice());
            let by_chain = chain_covers(&c, &chained);
            if inked_identities.contains(&id)
                || by_chain
                || by_hole_statement
                || same_statement(&c, inked_here)
                || same_statement(&c, hole_stated_here)
            {
                covered.insert(id);
            } else {
                missing.push((id, c, name, owner));
            }
        }
    }

    let mut reported: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for (id, c, name, owner) in missing {
        if covered.contains(&id) || !reported.insert(id.clone()) {
            continue;
        }
        let waiver = c
            .key
            .as_ref()
            .and_then(|k| drawing.dimension_edits.get(k))
            .and_then(|e| e.suppressed.clone());
        let reason = if waiver.is_some() {
            OmissionReason::Suppressed
        } else {
            if carried.contains(&id) {
                OmissionReason::NotPlaced
            } else {
                OmissionReason::NotOnSheet
            }
        };
        out.push(DimensionOmission {
            view: name.to_string(),
            owner_view: owner,
            key: c.key,
            kind: c.kind,
            label: c.label,
            value: c.value,
            unit: c.unit,
            entities: c.entities,
            pid: c.pid,
            reason,
            waiver,
        });
    }
    out
}

/// The location records (by pid) a CHAIN states
/// ([`Dimension2d::chain_from`]), from the inked dimensions: a feature counts
/// as located only while its chain is inked AND the chain's anchor is itself
/// located -- from the datum, or by a chain that is (to any depth). Remove
/// the anchor and the whole chain is owed again.
pub(crate) fn chain_located<'d>(inked: impl Iterator<Item = &'d Dimension2d>) -> HashSet<&'d str> {
    let inked: Vec<&Dimension2d> = inked.collect();
    let from_datum: HashSet<&str> = inked
        .iter()
        .filter(|d| d.chain_from.is_none())
        .filter_map(|d| d.pid.as_deref())
        .collect();
    let chains: Vec<(&str, &str)> = inked
        .iter()
        .filter_map(|d| Some((d.pid.as_deref()?, d.chain_from.as_deref()?)))
        .collect();
    let mut located: HashSet<&str> = HashSet::new();
    loop {
        let before = located.len();
        for &(far, anchor) in &chains {
            if from_datum.contains(anchor) || located.contains(anchor) {
                located.insert(far);
            }
        }
        if located.len() == before {
            return located;
        }
    }
}

/// Does a CHAIN state the location `c` owes? When `c` is itself one of the
/// `chained` locations (by its record), or states the same place as one of
/// them ([`same_statement`]): two features at one location from the datum
/// are both stated by one number, and when that number becomes a chain in
/// a DETAIL view the chain states both (NIST FTC 08: faces 255 and 267 at
/// 29.91, the second reported not dimensioned once 255's moved).
fn chain_covers(c: &Dimension2d, chained: &[Dimension2d]) -> bool {
    if c.kind != "position" {
        return false;
    }
    let by_record = c
        .pid
        .as_deref()
        .is_some_and(|p| chained.iter().any(|x| x.pid.as_deref() == Some(p)));
    let refs: Vec<&Dimension2d> = chained.iter().collect();
    by_record || same_statement(c, &refs)
}

/// One line per (view, kind) for a person: `"3 angles not dimensioned on
/// FRONT"`. Waived omissions are not counted here.
pub fn omission_summary(omissions: &[DimensionOmission]) -> Vec<String> {
    let mut groups: Vec<((&str, &str), usize)> = Vec::new();
    for o in omissions.iter().filter(|o| o.waiver.is_none()) {
        let key = (o.view.as_str(), o.kind.as_str());
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, n)) => *n += 1,
            None => groups.push((key, 1)),
        }
    }
    groups
        .into_iter()
        .map(|((view, kind), n)| {
            let noun = match kind {
                "diameter" => "diameter",
                "radius" => "radius",
                "angle" => "angle",
                "length" => "length",
                "extent" => "overall size",
                "position" | "location" => "location",
                "pitch" => "pitch",
                "fillet" => "fillet radius",
                "chamfer" => "chamfer",
                "hole_callout" => "hole callout",
                "pcd" => "pitch circle",
                _ => "dimension",
            };
            let plural = if n == 1 {
                noun.to_string()
            } else if noun == "radius" {
                "radii".to_string()
            } else if noun == "fillet radius" {
                "fillet radii".to_string()
            } else if noun == "pitch" {
                "pitches".to_string()
            } else {
                format!("{noun}s")
            };
            format!("{n} {plural} not dimensioned on {view}")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn location(pid: &str, chain_from: Option<&str>) -> Dimension2d {
        Dimension2d {
            id: pid.to_string(),
            kind: "position".to_string(),
            value: 1.0,
            unit: "mm".to_string(),
            label: "1".to_string(),
            a: [0.0, 0.0],
            b: [1.0, 0.0],
            entities: Vec::new(),
            axis3: None,
            dir3: None,
            pid: Some(pid.to_string()),
            datum: None,
            tolerance: None,
            key: None,
            members: Vec::new(),
            member_points: Vec::new(),
            chain_from: chain_from.map(str::to_string),
        }
    }

    /// A location stated by the same number as a chain-located one is
    /// covered by that chain too; a location elsewhere is not.
    ///
    /// Mutation: cover by record only (drop the `same_statement` arm) ->
    /// the coincident feature is owed -> RED.
    #[test]
    fn a_chain_states_a_coincident_location_too() {
        let mut chained = location("p255", None);
        chained.a = [0.0, 0.0];
        chained.b = [29.91, 0.0];
        chained.value = 29.91;
        let mut coincident = location("p267", None);
        coincident.a = [0.0, 3.0];
        coincident.b = [29.91, 3.0];
        coincident.value = 29.91;
        let mut elsewhere = location("p300", None);
        elsewhere.a = [0.0, 0.0];
        elsewhere.b = [40.0, 0.0];
        elsewhere.value = 40.0;
        let chained = vec![chained];
        assert!(chain_covers(&chained[0], &chained), "its own record");
        assert!(chain_covers(&coincident, &chained), "the same place");
        assert!(!chain_covers(&elsewhere, &chained), "another place");
    }

    /// A chain locates its feature only while its anchor is located from
    /// the datum (to any depth: A from the datum, B from A, C from B). Take
    /// the anchor off the sheet and the whole chain is owed again.
    ///
    /// Mutation: credit every inked chain regardless of its anchor -> B and
    /// C stay located without A -> RED.
    #[test]
    fn a_chain_locates_its_feature_only_while_its_anchor_is_located() {
        let a = location("A", None);
        let b = location("B", Some("A"));
        let c = location("C", Some("B"));
        let all = chain_located([&a, &b, &c].into_iter());
        assert!(all.contains("B") && all.contains("C"), "{all:?}");
        assert!(
            !all.contains("A"),
            "A is located from the datum, not by a chain"
        );
        let without_anchor = chain_located([&b, &c].into_iter());
        assert!(without_anchor.is_empty(), "{without_anchor:?}");
    }
}
