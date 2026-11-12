//! Sheets rebuilt from their history.
//!
//! A drawing is created from a part and then edited: renamed, its title block
//! filled in (material, approval), views added and removed, deleted. Every
//! edit is a recorded event. [`rebuild_drawings`] replays them all, in order,
//! on top of the sheet re-derived from the part as it stands at the drawing's
//! position in the history (moulds folded in, so the sheet follows the part).
//!
//! Before this, only the creation and a projection switch were replayed: a
//! server restart lost every sheet, and a dimension edit -- which rebuilds the
//! part and re-derives its sheets -- silently reset the title block and dropped
//! added views.
//!
//! The part's state at each position comes from the [`FrameCache`], so a
//! history with many sheet edits costs one geometry replay, not one per edit.

use std::collections::HashMap;

use geometry_engine::drawing::projection::project_solid_view;
use geometry_engine::drawing::types::{Drawing, ProjectionType, TitleBlock, ViewSource};
use serde_json::Value;
use uuid::Uuid;

use crate::frames::FrameCache;
use crate::mould::{is_param_meta, OverrideSet};
use crate::replay::{projection_switches, rederive_part_drawing, DrawingRederive};
use crate::types::{Operation, TimelineEvent};

/// The sheets a history rebuilds, and what it could not.
#[derive(Debug, Default)]
pub struct DrawingRebuild {
    /// Rebuilt sheets, keyed by their recorded drawing id.
    pub drawings: HashMap<Uuid, Drawing>,
    /// Sheets or edits that could not be replayed, stated by event.
    pub issues: Vec<String>,
    /// What was derived and what was reused (a test checks this, not a clock).
    pub stats: RebuildStats,
}

/// Derivation work one [`rebuild_drawings`] did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RebuildStats {
    /// Sheets derived from the part (hidden lines, dimensions).
    pub sheets_derived: usize,
    /// Sheets taken from the cache: same geometry, same creation event.
    pub sheets_reused: usize,
    /// Added views projected.
    pub views_projected: usize,
    /// Added views taken from the cache.
    pub views_reused: usize,
}

fn refs(e: &TimelineEvent, key: &str, kind: &str) -> Vec<Uuid> {
    let Operation::Generic { parameters, .. } = &e.operation else {
        return Vec::new();
    };
    parameters
        .get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str()?.strip_prefix(kind)?.parse::<Uuid>().ok())
                .collect()
        })
        .unwrap_or_default()
}

fn params(e: &TimelineEvent) -> Option<&Value> {
    let Operation::Generic { parameters, .. } = &e.operation else {
        return None;
    };
    Some(parameters.get("params").unwrap_or(parameters))
}

fn kind(e: &TimelineEvent) -> &str {
    match &e.operation {
        Operation::Generic { command_type, .. } => command_type.as_str(),
        _ => "",
    }
}

/// The sheet-cache key of the sheet `event` (a `drawing.create_from_part`)
/// derives from `geometry` (the effective geometry events before it) under
/// projection `switch`. Certification and [`rebuild_drawings`] both use it,
/// so a sheet derived by one is reused by the other.
pub fn sheet_key(
    geometry: &[TimelineEvent],
    event: &TimelineEvent,
    switch: Option<geometry_engine::drawing::ProjectionAngle>,
) -> [u8; 32] {
    FrameCache::derived_key(
        &FrameCache::key_of(geometry),
        event,
        format!("{switch:?}").as_bytes(),
    )
}

/// Is `event` part of the geometry a sheet is derived from (what
/// [`rebuild_drawings`] replays): not a `param.*` override, not a document
/// event (`drawing.*`, annotations -- anything with a dotted kind)?
pub(crate) fn is_geometry_event(event: &TimelineEvent) -> bool {
    let k = kind(event);
    !is_param_meta(k) && !k.contains('.')
}

/// Rebuild every sheet `events` creates, with its recorded edits applied.
pub fn rebuild_drawings(events: &[TimelineEvent], frames: &FrameCache) -> DrawingRebuild {
    let mut ordered: Vec<&TimelineEvent> = events.iter().collect();
    ordered.sort_by_key(|e| e.sequence_number);
    // The geometry as it replays: moulds folded into the events they target,
    // document events (drawings, annotations) left out. A sheet event at
    // history position k sees the geometry prefix that precedes it.
    let overrides = OverrideSet::collect(events);
    let switches = projection_switches(events);
    let mut geometry: Vec<TimelineEvent> = Vec::new();
    let mut out = DrawingRebuild::default();

    for e in ordered {
        let k = kind(e);
        if is_param_meta(k) {
            continue;
        }
        if !k.starts_with("drawing.") {
            if !k.contains('.') {
                geometry.push(overrides.overridden_event(e).unwrap_or_else(|| e.clone()));
            }
            continue;
        }
        let target = refs(e, "inputs", "drawing:")
            .into_iter()
            .chain(refs(e, "outputs", "drawing:"))
            .next();
        match k {
            "drawing.create_from_part" => {
                let switch = target.and_then(|id| switches.get(&id).copied());
                // The derived sheet depends only on the geometry before this
                // event, the event, and the projection switch: reuse it when
                // all three are unchanged (an edit-only undo or redo).
                let key = sheet_key(&geometry, e, switch);
                let derived = match frames.sheet(&key) {
                    Some(hit) => {
                        out.stats.sheets_reused += 1;
                        DrawingRederive::Rebuilt(hit.0, Box::new(hit.1.clone()))
                    }
                    None => {
                        let (model, outcome, _) = frames.replay(&geometry);
                        let d = rederive_part_drawing(&model, e, &outcome.id_remap, switch);
                        if let DrawingRederive::Rebuilt(id, sheet) = &d {
                            out.stats.sheets_derived += 1;
                            frames.store_sheet(key, *id, (**sheet).clone());
                        }
                        d
                    }
                };
                match derived {
                    DrawingRederive::Rebuilt(id, sheet) => {
                        let mut sheet = *sheet;
                        // The name the sheet was created with (recorded since
                        // 11-12; older events keep the derived default).
                        if let Some(name) = params(e)
                            .and_then(|p| p.get("name"))
                            .and_then(Value::as_str)
                            .filter(|n| !n.trim().is_empty())
                        {
                            sheet.name = name.to_string();
                        }
                        sheet.stabilize_view_ids();
                        out.drawings.insert(id, sheet);
                    }
                    DrawingRederive::Dangling(what) => out.issues.push(format!(
                        "sequence {}: the sheet's part {what} is gone",
                        e.sequence_number
                    )),
                    DrawingRederive::Failed(why) => out.issues.push(format!(
                        "sequence {}: the sheet could not be rebuilt: {why}",
                        e.sequence_number
                    )),
                }
            }
            "drawing.rename" => {
                let name = params(e)
                    .and_then(|p| p.get("new_name"))
                    .and_then(Value::as_str);
                if let (Some(id), Some(name)) = (target, name) {
                    if let Some(d) = out.drawings.get_mut(&id) {
                        d.name = name.to_string();
                    }
                }
            }
            "drawing.title_block.update" => {
                let tb = params(e)
                    .and_then(|p| p.get("title_block"))
                    .and_then(|v| serde_json::from_value::<TitleBlock>(v.clone()).ok());
                match (target, tb) {
                    (Some(id), Some(tb)) => {
                        if let Some(d) = out.drawings.get_mut(&id) {
                            d.title_block = tb;
                        }
                    }
                    _ => out.issues.push(format!(
                        "sequence {}: a title-block edit could not be read",
                        e.sequence_number
                    )),
                }
            }
            "drawing.add_view" => {
                let Some(id) = target else { continue };
                if !out.drawings.contains_key(&id) {
                    continue;
                }
                let p = params(e);
                let parsed = p.and_then(|p| {
                    Some((
                        p.get("name")?.as_str()?.to_string(),
                        serde_json::from_value::<ViewSource>(p.get("source")?.clone()).ok()?,
                        serde_json::from_value::<ProjectionType>(p.get("projection")?.clone())
                            .ok()?,
                        serde_json::from_value::<[f64; 2]>(p.get("position_mm")?.clone()).ok()?,
                        p.get("scale")?.as_f64()?,
                    ))
                });
                let Some((name, source, projection, position, scale)) = parsed else {
                    out.issues.push(format!(
                        "sequence {}: an added view could not be read",
                        e.sequence_number
                    ));
                    continue;
                };
                // The view's part, as the geometry stands at this point:
                // projected once per (geometry, event), reused after that.
                let key = FrameCache::derived_key(&FrameCache::key_of(&geometry), e, &[]);
                let projected = match frames.view(&key) {
                    Some(hit) => {
                        out.stats.views_reused += 1;
                        Ok((*hit).clone())
                    }
                    None => {
                        let (model, outcome, _) = frames.replay(&geometry);
                        let ViewSource::Part { part_id, solid_id } = source;
                        let live = outcome
                            .id_remap
                            .get(&(solid_id as u64))
                            .copied()
                            .unwrap_or(solid_id as u64) as u32;
                        let r = project_solid_view(
                            &model,
                            ViewSource::Part {
                                part_id,
                                solid_id: live,
                            },
                            projection,
                            name,
                            position,
                            scale,
                        );
                        if let Ok(v) = &r {
                            out.stats.views_projected += 1;
                            frames.store_view(key, v.clone());
                        }
                        r
                    }
                };
                match projected {
                    Ok(mut view) => {
                        if let Some(vid) = refs(e, "outputs", "view:").into_iter().next() {
                            view.id = geometry_engine::drawing::types::ProjectedViewId(vid);
                        }
                        if let Some(d) = out.drawings.get_mut(&id) {
                            d.add_view(view);
                        }
                    }
                    Err(err) => out.issues.push(format!(
                        "sequence {}: an added view could not be projected: {err}",
                        e.sequence_number
                    )),
                }
            }
            "drawing.remove_view" => {
                let vid = params(e)
                    .and_then(|p| p.get("view_id"))
                    .and_then(|v| serde_json::from_value::<Uuid>(v.clone()).ok());
                if let (Some(id), Some(vid)) = (target, vid) {
                    if let Some(d) = out.drawings.get_mut(&id) {
                        let removed =
                            d.remove_view(geometry_engine::drawing::types::ProjectedViewId(vid));
                        if !removed {
                            out.issues.push(format!(
                                "sequence {}: the removed view {vid} is not on the rebuilt sheet",
                                e.sequence_number
                            ));
                        }
                    }
                }
            }
            "drawing.view.update" => {
                let p = params(e);
                let vid = p
                    .and_then(|p| p.get("view_id"))
                    .and_then(|v| serde_json::from_value::<Uuid>(v.clone()).ok());
                let edit = p.and_then(|p| {
                    serde_json::from_value::<geometry_engine::drawing::types::ViewEdit>(p.clone())
                        .ok()
                });
                match (target, vid, edit) {
                    (Some(id), Some(vid), Some(edit)) => {
                        if let Some(d) = out.drawings.get_mut(&id) {
                            if let Err(err) = d.edit_view(
                                geometry_engine::drawing::types::ProjectedViewId(vid),
                                &edit,
                            ) {
                                out.issues.push(format!(
                                    "sequence {}: a view edit no longer applies: {err}",
                                    e.sequence_number
                                ));
                            }
                        }
                    }
                    _ => out.issues.push(format!(
                        "sequence {}: a view edit could not be read",
                        e.sequence_number
                    )),
                }
            }
            "drawing.delete" => {
                if let Some(id) = target {
                    out.drawings.remove(&id);
                }
            }
            // Projection switches are applied at creation (`switches`);
            // exports, renders and quality runs change nothing on the sheet.
            _ => {}
        }
    }
    out
}
