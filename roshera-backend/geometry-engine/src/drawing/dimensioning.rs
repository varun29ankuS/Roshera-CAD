//! Automatic drawing dimensions (#20, slice 1).
//!
//! An engineering drawing is geometry + DIMENSIONS. The projection pipeline
//! draws the edges; this derives the dimension callouts AUTOMATICALLY from the
//! analytic dimension table (`readable::extract_dimensions`) and projects them
//! through the SAME view matrix the edges use, so each callout lands on the
//! feature it measures. Sound by construction: every value is the exact
//! analytic dimension read off a surface/curve — never measured from the
//! rasterised drawing — and each callout names the B-Rep face(s) it spans, so
//! it is recoverable, not decorative.

use super::projection::project_point;
use super::types::ProjectionType;
use crate::math::{Point3, Vector3};
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::readable::extract_dimensions;
use serde::{Deserialize, Serialize};

/// A 2D dimension annotation in view-space (mm, pre-scale) — the same space the
/// projected polylines live in, so the SVG/DXF renderer maps both uniformly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dimension2d {
    /// Stable id carried from the analytic record (the mould handle).
    pub id: String,
    /// "diameter" | "radius" | "length" | "angle" | "extent".
    pub kind: String,
    pub value: f64,
    pub unit: String,
    /// Drawing label, e.g. "Ø20.00", "40.00", "∠30.0°".
    pub label: String,
    /// View-space endpoints of the measured span. For an angle (no linear
    /// span) `a == b` at the feature anchor.
    pub a: [f64; 2],
    pub b: [f64; 2],
    /// B-Rep face ids the dimension spans (empty for whole-part extents).
    pub entities: Vec<u32>,
    /// Feature axis (world-space unit vector): the 3D direction a view must
    /// look along to see this feature as a true circle. Propagated from
    /// `DimensionRecord::axis`; `None` for extents and spheres.
    #[serde(default)]
    pub axis3: Option<[f64; 3]>,
    /// World-space direction the dimension is measured along (unit-ish).
    /// Used by the extent dedup to distinguish same-value different-axis extents
    /// (e.g. X 40.00 vs Y 40.00 on a cube).
    #[serde(default)]
    pub dir3: Option<[f64; 3]>,
}

impl Dimension2d {
    /// Projected span length in view-space (mm). ~0 for angle/point callouts,
    /// and for spans that project edge-on in this view (a Z extent in Top).
    pub fn projected_span(&self) -> f64 {
        let dx = self.a[0] - self.b[0];
        let dy = self.a[1] - self.b[1];
        (dx * dx + dy * dy).sqrt()
    }
}

/// Derive the 2D dimension callouts for `solid_id` in the given view.
///
/// Each analytic record carries `(anchor, direction, value, kind)`; the 3D span
/// endpoints follow from the kind:
///   * diameter — across the feature: `anchor → anchor − direction·value`
///   * length / extent — along the axis, centred on the anchor:
///     `anchor ∓ direction·(value/2)`
///   * position — from the part-corner datum to the bore axis. The
///     extraction anchors position records at the MIDPOINT between the
///     datum corner and the axis with `direction` pointing corner→axis,
///     so the same centred-span formula recovers the true span:
///     `p0` = the datum-corner end, `p1` = the bore axis.
///   * angle — a point callout at the anchor.
/// Both endpoints are projected through `projection`, so a callout that
/// measures a direction perpendicular to the view collapses to a near-zero
/// span (the caller drops or re-routes those to a view that shows them).
pub fn auto_dimensions(
    model: &BRepModel,
    solid_id: SolidId,
    projection: ProjectionType,
) -> Vec<Dimension2d> {
    let mut out = Vec::new();
    for d in extract_dimensions(model, solid_id) {
        let anchor = Point3::new(d.anchor[0], d.anchor[1], d.anchor[2]);
        let dir = Vector3::new(d.direction[0], d.direction[1], d.direction[2]);
        let (p0, p1) = match d.kind.as_str() {
            "diameter" | "radius" => (anchor, anchor - dir * d.value),
            "length" | "extent" | "position" => (
                anchor - dir * (d.value * 0.5),
                anchor + dir * (d.value * 0.5),
            ),
            _ => (anchor, anchor),
        };
        let a = project_point(projection, p0);
        let b = project_point(projection, p1);
        out.push(Dimension2d {
            id: d.id,
            kind: d.kind,
            value: d.value,
            unit: d.unit,
            label: d.label,
            a,
            b,
            entities: d.entities,
            axis3: d.axis,
            // Defensively normalised: the extraction emits unit directions, but
            // the cross-view dedup quantises components (×100), so a non-unit
            // vector would silently change hash keys.
            dir3: Some(unit3_or_zero(d.direction)),
        });
    }
    out
}

/// Callouts that actually READ in this view: drop the ones whose measured span
/// projects edge-on (e.g. a Z height in Top view), since their line collapses
/// to a point and would clutter without informing. Angles are kept (point
/// callouts). `min_span` is in view-space mm.
pub fn visible_dimensions(
    model: &BRepModel,
    solid_id: SolidId,
    projection: ProjectionType,
    min_span: f64,
) -> Vec<Dimension2d> {
    let mut dims: Vec<Dimension2d> = auto_dimensions(model, solid_id, projection)
        .into_iter()
        .filter(|d| d.kind == "angle" || d.projected_span() >= min_span)
        .collect();
    // Drawing convention: a linear dimension shows just its value — strip the
    // analytic axis tag ("X 80.00" → "80.00"). Ø (diameter), R (radius) and
    // ∠ (angle) prefixes are kept; they are not axis tags.
    for d in &mut dims {
        let first = d.label.chars().next();
        if matches!(first, Some(c) if c.is_ascii_uppercase() && c != 'R' && c != 'S') {
            if let Some(rest) = d.label.strip_prefix(|c: char| c.is_ascii_uppercase()) {
                if let Some(num) = rest.strip_prefix(' ') {
                    d.label = num.to_string();
                }
            }
        }
    }
    select_dimensions(dims)
}

/// Keep a COMPLEX part's drawing readable. A revolved bell nozzle has ~9 cone
/// bands, so the raw auto-dimensions stack dozens of overlapping ∠/Ø callouts
/// (KNOWN_BUGS DRW-DIM-EXPLOSION). Select the few that DEFINE the part:
///   1. drop per-band cone half-angles when there are several (clutter, not
///      something a drawing dimensions per band);
///   2. collapse near-equal values (a stack of Ø72.0/Ø72.0/… → one);
///   3. cap diameters to the most significant — the largest (envelope) plus
///      the smallest (throat/bore) — dropping the intermediate contour rings;
///   4. cap the per-view total so callouts never overlap.
fn select_dimensions(mut dims: Vec<Dimension2d>) -> Vec<Dimension2d> {
    use std::collections::HashSet;

    // 1. Per-band angle clutter.
    let angle_count = dims.iter().filter(|d| d.kind == "angle").count();
    if angle_count > 2 {
        dims.retain(|d| d.kind != "angle");
    }

    // 2. Collapse near-equal (kind, value) to a single representative (0.5 mm).
    let mut seen: HashSet<(String, i64)> = HashSet::new();
    dims.retain(|d| seen.insert((d.kind.clone(), (d.value * 2.0).round() as i64)));

    // 3. Cap diameters/radii: keep the largest 3 + smallest 2 distinct (envelope
    //    + throat), drop the rest of a contour's rings.
    const MAX_DIA: usize = 5;
    let mut dia: Vec<Dimension2d> = dims
        .iter()
        .filter(|d| d.kind == "diameter" || d.kind == "radius")
        .cloned()
        .collect();
    if dia.len() > MAX_DIA {
        dia.sort_by(|a, b| {
            b.value
                .partial_cmp(&a.value)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut keep: Vec<Dimension2d> = dia.iter().take(3).cloned().collect();
        keep.extend(dia.iter().rev().take(2).cloned());
        let kept: HashSet<String> = keep.iter().map(|d| d.id.clone()).collect();
        dims.retain(|d| (d.kind != "diameter" && d.kind != "radius") || kept.contains(&d.id));
    }

    // 4. Hard per-view cap, prioritising extents (overall envelope) > diameters
    //    > the rest, so the most informative callouts survive.
    const MAX_PER_VIEW: usize = 8;
    if dims.len() > MAX_PER_VIEW {
        let rank = |k: &str| match k {
            "extent" | "length" => 0,
            "diameter" | "radius" => 1,
            _ => 2,
        };
        dims.sort_by_key(|d| rank(&d.kind));
        dims.truncate(MAX_PER_VIEW);
    }
    dims
}

/// World-space view direction (unit vector, pointing INTO the scene) that a
/// standard projection camera looks along. Used by `dedup_dimensions_global`
/// to decide which view is "axial" for a diameter callout.
fn view_dir(p: &ProjectionType) -> Option<[f64; 3]> {
    match p {
        ProjectionType::Front => Some([0.0, -1.0, 0.0]),
        ProjectionType::Top => Some([0.0, 0.0, -1.0]),
        ProjectionType::Right => Some([-1.0, 0.0, 0.0]),
        ProjectionType::Bottom => Some([0.0, 0.0, 1.0]),
        ProjectionType::Left => Some([1.0, 0.0, 0.0]),
        _ => None,
    }
}

/// Dot product of two `[f64; 3]` arrays.
/// Unit-normalise, or return the zero vector for degenerate input (a zero
/// direction hashes to the "directionless" key and never falsely groups).
fn unit3_or_zero(v: [f64; 3]) -> [f64; 3] {
    let m = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if m > 1e-12 {
        [v[0] / m, v[1] / m, v[2] / m]
    } else {
        [0.0, 0.0, 0.0]
    }
}

fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Each feature dimensioned exactly once, where it reads best (ISO 129-1
/// §5.1). Two passes:
///
/// **Pass A — same-view dedup**: within each view, equal value (within 0.01 mm)
/// + same orientation (H or V, as `svg.rs` classifies by dx≥dy) + same
/// projected interval endpoints (within 0.5 mm sheet-space) → keep the
/// highest-priority one. Priority rank: extent (0) > diameter/radius (1) >
/// length (2); equal rank → first occurrence.
///
/// **Pass B — cross-view dedup**: the same (sorted entity ids, kind, quantized
/// value) in multiple views → keep one. Diameters/radii keep the AXIAL view
/// (`|axis3 · view_dir| > 0.99`), else keep the view with the largest
/// projected span, tie → lowest view index. Whole-part extents (empty
/// `entities`) are deduplicated by (kind, quantized value, quantized 3D
/// direction) — identical direction+value = duplicate; different direction at
/// same value = not a duplicate and must survive.
///
/// Angle dimensions are exempt from both passes.
pub fn dedup_dimensions_global(drawing: &mut super::types::Drawing) {
    // ── Pass A: same-view same-interval dedup ────────────────────────────────
    for view in &mut drawing.views {
        // Classification helper: orientation 'H' or 'V', and projected
        // interval [lo, hi] in view-space (pre-scale; scale cancels within
        // one view because we're comparing within the same view). We use
        // view-space coordinates (d.a, d.b) directly — they are in view-space
        // mm already, and scale is uniform across the view.
        struct Lin {
            idx: usize,
            orient: char,
            lo: f64,
            hi: f64,
            rank: u8,
        }
        let mut lins: Vec<Lin> = Vec::new();
        for (idx, d) in view.dimensions.iter().enumerate() {
            if d.kind == "angle" {
                continue;
            }
            let dx = (d.a[0] - d.b[0]).abs();
            let dy = (d.a[1] - d.b[1]).abs();
            if dx < 1e-6 && dy < 1e-6 {
                continue;
            }
            let orient = if dx >= dy { 'H' } else { 'V' };
            let (lo, hi) = if orient == 'H' {
                (d.a[0].min(d.b[0]), d.a[0].max(d.b[0]))
            } else {
                (d.a[1].min(d.b[1]), d.a[1].max(d.b[1]))
            };
            let rank: u8 = match d.kind.as_str() {
                "extent" => 0,
                "diameter" | "radius" => 1,
                _ => 2,
            };
            lins.push(Lin {
                idx,
                orient,
                lo,
                hi,
                rank,
            });
        }

        // Mark duplicates: for each group with same orient+interval, keep
        // the minimum-rank (highest-priority) one, then first by index.
        let mut keep = vec![true; view.dimensions.len()];
        let tol_val = 0.01_f64;
        let tol_int = 0.5_f64;
        for i in 0..lins.len() {
            if !keep[lins[i].idx] {
                continue;
            }
            let vi = &view.dimensions[lins[i].idx];
            for j in (i + 1)..lins.len() {
                if !keep[lins[j].idx] {
                    continue;
                }
                let vj = &view.dimensions[lins[j].idx];
                if lins[i].orient != lins[j].orient {
                    continue;
                }
                if (vi.value - vj.value).abs() > tol_val {
                    continue;
                }
                let lo_match = (lins[i].lo - lins[j].lo).abs() < tol_int;
                let hi_match = (lins[i].hi - lins[j].hi).abs() < tol_int;
                if lo_match && hi_match {
                    // Drop the lower-priority one; if equal rank, drop j (keep i
                    // = first occurrence).
                    if lins[j].rank < lins[i].rank {
                        keep[lins[i].idx] = false;
                        // i is dead: stop comparing it against later dims — a
                        // dead entry must not kill survivors it happens to
                        // match (the survivor j runs its own outer pass).
                        break;
                    } else {
                        keep[lins[j].idx] = false;
                    }
                }
            }
        }
        let mut ki = 0_usize;
        view.dimensions.retain(|_| {
            let r = keep[ki];
            ki += 1;
            r
        });
    }

    // ── Pass B: cross-view dedup ─────────────────────────────────────────────

    // We need the view projection types to decide axial preference. Collect
    // them up front so we can borrow immutably later.
    let projs: Vec<ProjectionType> = drawing.views.iter().map(|v| v.projection).collect();
    let n_views = drawing.views.len();

    // Two separate group maps: one for entity-bearing dims (non-empty entities),
    // one for whole-part extents (empty entities, keyed by dir3 too).
    //
    // For entity dims key = (sorted entities, kind, quantized value ×100).
    // For extents key = (kind, quantized value ×100, quantized dir3 ×100).

    // Collect all dims into a flat list: (view_idx, dim_idx, …fields…)
    // We will decide which view keeps each group, then mark the rest for removal.

    // entity key → Vec<(view_idx, dim_idx, projected_span, is_axial)>
    let mut entity_groups: std::collections::HashMap<
        (Vec<u32>, String, i64),
        Vec<(usize, usize, f64, bool)>,
    > = std::collections::HashMap::new();

    // extent key → Vec<(view_idx, dim_idx, projected_span)>
    let mut extent_groups: std::collections::HashMap<
        (String, i64, [i64; 3]),
        Vec<(usize, usize, f64)>,
    > = std::collections::HashMap::new();

    for vi in 0..n_views {
        let vdir = view_dir(&projs[vi]);
        for di in 0..drawing.views[vi].dimensions.len() {
            let d = &drawing.views[vi].dimensions[di];
            if d.kind == "angle" {
                continue;
            }
            let qval = (d.value * 100.0).round() as i64;
            if d.entities.is_empty() {
                // Whole-part extent — key includes quantized direction.
                let dir = d.dir3.unwrap_or([0.0, 0.0, 0.0]);
                let qdir = [
                    (dir[0] * 100.0).round() as i64,
                    (dir[1] * 100.0).round() as i64,
                    (dir[2] * 100.0).round() as i64,
                ];
                // Normalise sign: the direction (100,0,0) and (-100,0,0) are the
                // same axis — pick the form whose first non-zero component is
                // positive so X 40.00 from the +X direction and -X direction hash
                // together.
                let qdir = {
                    let first_nonzero = qdir.iter().find(|&&v| v != 0).copied().unwrap_or(0);
                    if first_nonzero < 0 {
                        [-qdir[0], -qdir[1], -qdir[2]]
                    } else {
                        qdir
                    }
                };
                let span = d.projected_span();
                extent_groups
                    .entry((d.kind.clone(), qval, qdir))
                    .or_default()
                    .push((vi, di, span));
            } else {
                // Feature dim — key is sorted entity ids + kind + value.
                let mut sorted_ents = d.entities.clone();
                sorted_ents.sort_unstable();
                // Axial-view preference applies ONLY to diameters/radii (the
                // "dimension the hole where it shows as a circle" convention).
                // A length dim must never prefer its axial view — there its
                // span projects to zero and the callout is unreadable.
                let is_axial = matches!(d.kind.as_str(), "diameter" | "radius")
                    && match (d.axis3, vdir) {
                        (Some(ax), Some(vd)) => dot3(ax, vd).abs() > 0.99,
                        _ => false,
                    };
                let span = d.projected_span();
                entity_groups
                    .entry((sorted_ents, d.kind.clone(), qval))
                    .or_default()
                    .push((vi, di, span, is_axial));
            }
        }
    }

    // Build a removal set: (view_idx, dim_idx).
    let mut remove: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();

    // Entity groups: keep one per group.
    for (_, mut entries) in entity_groups {
        if entries.len() < 2 {
            continue;
        }
        // Prefer the axial view for diameters/radii; among those, or if none
        // are axial, keep the view with the largest projected span. Tie → lowest
        // view index.
        let axial_idx = entries.iter().position(|e| e.3); // is_axial
        let keeper_view_idx = if let Some(ai) = axial_idx {
            entries[ai].0
        } else {
            // Largest span wins; tie → smallest view index.
            entries.sort_by(|a, b| {
                b.2.partial_cmp(&a.2)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.0.cmp(&b.0))
            });
            entries[0].0
        };
        for (vi, di, _, _) in &entries {
            if *vi != keeper_view_idx {
                remove.insert((*vi, *di));
            }
        }
    }

    // Extent groups: keep the view with the largest projected span, tie → lowest
    // view index.
    for (_, mut entries) in extent_groups {
        if entries.len() < 2 {
            continue;
        }
        entries.sort_by(|a, b| {
            b.2.partial_cmp(&a.2)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        let keeper_view_idx = entries[0].0;
        for (vi, di, _) in &entries {
            if *vi != keeper_view_idx {
                remove.insert((*vi, *di));
            }
        }
    }

    // Apply removals.
    for vi in 0..n_views {
        let mut di = 0_usize;
        drawing.views[vi].dimensions.retain(|_| {
            let r = !remove.contains(&(vi, di));
            di += 1;
            r
        });
    }
}

/// Assemble a standard third-angle engineering drawing — Front, Top, Right —
/// of a solid, with the analytic dimensions auto-placed on each view (each view
/// carries only the callouts that READ in it; edge-on ones are dropped). The
/// result renders directly via `render_drawing_svg` / `render_drawing_dxf`.
/// This is the "automatic drawing" verb: solid in, dimensioned drawing out, no
/// human placement.
pub fn standard_drawing(
    model: &BRepModel,
    solid_id: SolidId,
    part_uuid: uuid::Uuid,
    sheet: super::types::SheetSize,
    scale: f64,
) -> Result<super::types::Drawing, super::projection::ProjectionError> {
    use super::projection::project_solid_view;
    use super::types::{Drawing, ViewSource};

    let mut drawing = Drawing::new("Auto Drawing", sheet);
    drawing.set_unit_notes(model.document_unit());
    let source = ViewSource::Part {
        part_id: part_uuid,
        solid_id,
    };
    // Third-angle layout: Top ABOVE Front, Right to the RIGHT of Front.
    let layout = [
        (ProjectionType::Front, "FRONT", [80.0, 110.0]),
        (ProjectionType::Top, "TOP", [80.0, 210.0]),
        (ProjectionType::Right, "RIGHT", [210.0, 110.0]),
    ];
    // A span shorter than ~0.5 model-units in a view is edge-on → drop it.
    let min_span = 0.5_f64;
    for (proj, name, pos) in layout {
        let mut view = project_solid_view(model, source.clone(), proj, name, pos, scale)?;
        view.dimensions = visible_dimensions(model, solid_id, proj, min_span);
        view.centerlines = super::centerlines::centerlines(model, solid_id, proj);
        drawing.add_view(view);
    }
    dedup_dimensions_global(&mut drawing);
    Ok(drawing)
}

/// As [`standard_drawing`], but with HIDDEN-LINE REMOVAL: each view's edges are
/// split by the analytic raytrace eye into visible (solid `polylines`) and
/// occluded (dashed `hidden_polylines`). This is the mechanically-correct
/// drawing — an opaque part, not a see-through wireframe. The extent is kept
/// from the full wireframe so layout is unchanged. Sound: every visible/hidden
/// verdict is an exact ray↔surface test, never a rasterised z-buffer.
pub fn standard_drawing_hlr(
    model: &BRepModel,
    solid_id: SolidId,
    part_uuid: uuid::Uuid,
    sheet: super::types::SheetSize,
    scale: f64,
) -> Result<super::types::Drawing, super::projection::ProjectionError> {
    use super::types::{Drawing, ViewSource};

    let mut drawing = Drawing::new("Auto Drawing (HLR)", sheet);
    drawing.set_unit_notes(model.document_unit());
    let source = ViewSource::Part {
        part_id: part_uuid,
        solid_id,
    };
    let layout = [
        (ProjectionType::Front, "FRONT", [80.0, 110.0]),
        (ProjectionType::Top, "TOP", [80.0, 210.0]),
        (ProjectionType::Right, "RIGHT", [210.0, 110.0]),
    ];
    let min_span = 0.5_f64;
    for (proj, name, pos) in layout {
        drawing.add_view(build_hlr_view(
            model, solid_id, source, proj, name, pos, scale, min_span,
        )?);
    }
    dedup_dimensions_global(&mut drawing);
    Ok(drawing)
}

/// De-clutter projected circles: a revolved part draws a ring per band, so the
/// TOP view stacks dozens of concentric circles. Dedupe exact coincidents, then
/// for each CONCENTRIC group (same centre) cap the rings to the largest 3 +
/// smallest 2 (envelope + throat/bore). Circles at DIFFERENT centres (a bolt
/// pattern — same radius, scattered) are all kept.
fn select_circles(
    circles: Vec<super::types::ProjectedCircle>,
) -> Vec<super::types::ProjectedCircle> {
    use std::collections::{HashMap, HashSet};
    let q = |v: f64| (v * 10.0).round() as i64;
    let mut seen: HashSet<(i64, i64, i64)> = HashSet::new();
    let mut groups: HashMap<(i64, i64), Vec<super::types::ProjectedCircle>> = HashMap::new();
    for c in circles {
        let key = (q(c.cx), q(c.cy), q(c.r));
        if seen.insert(key) {
            groups.entry((key.0, key.1)).or_default().push(c);
        } else if let Some(g) = groups.get_mut(&(key.0, key.1)) {
            // Coincident duplicate: keep ONE drawn circle but MERGE the entity
            // identity — dropping the duplicate's face_ids would sever the
            // link from a coaxial feature's rim back to its face.
            if let Some(kept) = g.iter_mut().find(|k| q(k.r) == key.2) {
                for f in &c.face_ids {
                    if !kept.face_ids.contains(f) {
                        kept.face_ids.push(*f);
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    for (_, mut g) in groups {
        if g.len() > 5 {
            g.sort_by(|a, b| b.r.partial_cmp(&a.r).unwrap_or(std::cmp::Ordering::Equal));
            out.extend(g.iter().take(3).cloned());
            out.extend(g.iter().rev().take(2).cloned());
        } else {
            out.extend(g);
        }
    }
    out
}

/// Build one HLR view: wireframe (for extent + placement), edges split
/// into visible / hidden by the raytrace eye, plus auto dimensions and
/// centerlines. Shared by [`standard_drawing_hlr`] and
/// [`standard_drawing_auto`].
fn build_hlr_view(
    model: &BRepModel,
    solid_id: SolidId,
    source: super::types::ViewSource,
    proj: ProjectionType,
    name: &str,
    pos: [f64; 2],
    scale: f64,
    min_span: f64,
) -> Result<super::types::ProjectedView, super::projection::ProjectionError> {
    use super::projection::{project_solid_view, DEFAULT_CURVE_SAMPLES};
    use super::visibility::project_solid_edges_visibility;

    let mut view = project_solid_view(model, source, proj, name, pos, scale)?;
    let edges = project_solid_edges_visibility(model, solid_id, proj, DEFAULT_CURVE_SAMPLES)?;
    if matches!(proj, ProjectionType::Isometric) {
        // The isometric is a PICTORIAL reference, drawn as a clean solid
        // wireframe. Per-segment occlusion mis-classifies a curved rim in the
        // oblique iso view: the rim is mostly near-silhouette (its tangent is
        // ~perpendicular to the view), where the occlusion raycast grazes the
        // surface and reports "hidden", so the whole rim renders dashed — an
        // all-dashed "ghost". Showing every edge solid (the standard isometric
        // convention, which omits hidden lines) removes the ghost without
        // relying on the fragile oblique-view occlusion test.
        let mut all = edges.visible;
        all.extend(edges.hidden);
        view.polylines = all;
        view.hidden_polylines = Vec::new();
        let mut all_circles = edges.circles;
        all_circles.extend(edges.hidden_circles);
        view.circles = select_circles(all_circles);
        view.hidden_circles = Vec::new();
    } else {
        view.polylines = edges.visible;
        view.hidden_polylines = edges.hidden;
        view.circles = select_circles(edges.circles);
        view.hidden_circles = select_circles(edges.hidden_circles);
    }
    view.dimensions = visible_dimensions(model, solid_id, proj, min_span);
    view.centerlines = super::centerlines::centerlines(model, solid_id, proj);
    Ok(view)
}

/// Snap a fill scale down to the nearest preferred drafting ratio so the
/// title block reads a clean "2:1" / "1:2" rather than "2.37:1".
fn snap_scale(s: f64) -> f64 {
    const LADDER: [f64; 21] = [
        100.0, 50.0, 20.0, 10.0, 5.0, 4.0, 2.5, 2.0, 1.5, 1.0, 0.75, 0.5, 0.4, 0.25, 0.2, 0.1,
        0.08, 0.05, 0.04, 0.02, 0.01,
    ];
    for &v in LADDER.iter() {
        if s >= v - 1e-9 {
            return v;
        }
    }
    s.max(0.005)
}

/// Pick the smallest ISO sheet whose drawing area comfortably suits a
/// part of the given largest dimension (mm), matching common drafting
/// practice (small parts on A4, growing to A0). The fill scale then sizes
/// the part within the chosen sheet.
fn pick_sheet(max_dim: f64) -> super::types::SheetSize {
    use super::types::SheetSize::*;
    if max_dim <= 90.0 {
        A4
    } else if max_dim <= 180.0 {
        A3
    } else if max_dim <= 360.0 {
        A2
    } else if max_dim <= 700.0 {
        A1
    } else {
        A0
    }
}

/// Compute the fill scale and a CENTERED 2×2 placement for the standard
/// four-view sheet:
///
/// ```text
///   TOP    ISO
///   FRONT  RIGHT
/// ```
///
/// Top is directly above Front (shared centre-x), Right is level with
/// Front (shared centre-y) — proper third angle — and the isometric
/// pictorial fills the otherwise-empty top-right quadrant. Each view is
/// centred in its grid cell; the group is centred in the drawing area
/// with room reserved for dimensions. Returns `(scale, [front, top,
/// right, iso] position_mm)`.
fn layout_four_view(
    sheet: &super::types::SheetSize,
    fe: super::types::ViewExtent,
    te: super::types::ViewExtent,
    re: super::types::ViewExtent,
    ie: super::types::ViewExtent,
) -> (f64, [[f64; 2]; 4]) {
    let w = sheet.width();
    let h = sheet.height();
    let (ml, mr, mt, mb) = super::svg::frame_margins(sheet);
    let (_tb_w, tb_h) = super::svg::title_block_size(sheet);

    // Reserve dimension room on the left + bottom + between columns, and
    // the title-block band along the bottom, then center the group.
    const PAD_LEFT: f64 = 22.0;
    const PAD_BOTTOM: f64 = 18.0;
    // VGAP must clear the upper view's BELOW dimension band (~22 mm) plus the
    // lower view's title (~6 mm); HGAP clears the right column's LEFT dims.
    const VGAP: f64 = 32.0;
    const HGAP: f64 = 30.0;

    let avail_x0 = ml + PAD_LEFT;
    let avail_x1 = w - mr;
    let avail_y0 = mt;
    let avail_y1 = h - mb - tb_h - PAD_BOTTOM;
    let avail_w = (avail_x1 - avail_x0).max(10.0);
    let avail_h = (avail_y1 - avail_y0).max(10.0);

    let (fw, fh) = (fe.width(), fe.height());
    let (tw, th) = (te.width(), te.height());
    let (rw, rh) = (re.width(), re.height());
    let (iw, ih) = (ie.width(), ie.height());

    // Left column = max(Front, Top) width; right column = max(Right, Iso).
    // Top row height = max(Top, Iso); bottom row = max(Front, Right).
    let left_w = fw.max(tw);
    let right_w = rw.max(iw);
    let top_h = th.max(ih);
    let bot_h = fh.max(rh);

    let unit_w = (left_w + right_w).max(1e-6);
    let unit_h = (top_h + bot_h).max(1e-6);
    let s_w = (avail_w - HGAP) / unit_w;
    let s_h = (avail_h - VGAP) / unit_h;
    let mut scale = 0.9 * s_w.min(s_h);
    if !scale.is_finite() || scale <= 0.0 {
        scale = 1.0;
    }
    scale = snap_scale(scale);

    let lw = left_w * scale;
    let rwc = right_w * scale;
    let trh = top_h * scale;
    let brh = bot_h * scale;
    let g_w = lw + HGAP + rwc;
    let g_h = trh + VGAP + brh;
    let gx = avail_x0 + 0.5 * (avail_w - g_w);
    let gy = avail_y0 + 0.5 * (avail_h - g_h);

    // Cell origins (top-left, sheet coords y down).
    let left_cx = gx + 0.5 * lw;
    let right_cx = gx + lw + HGAP + 0.5 * rwc;
    let top_cy = gy + 0.5 * trh;
    let bot_cy = gy + trh + VGAP + 0.5 * brh;

    // A view of extent `e` centred on (cx, cy): top-left at
    // (cx − e.w·s/2, cy − e.h·s/2).
    let place = |cx: f64, cy: f64, e: super::types::ViewExtent| -> [f64; 2] {
        let xtl = cx - 0.5 * e.width() * scale;
        let ytl = cy - 0.5 * e.height() * scale;
        // Invert the render transform: sheet_x = pos.x + min_x·s,
        // sheet_y_top = (h − pos.y) − max_y·s.
        [xtl - e.min_x * scale, h - ytl - e.max_y * scale]
    };
    (
        scale,
        [
            place(left_cx, bot_cy, fe),  // FRONT  (bottom-left)
            place(left_cx, top_cy, te),  // TOP    (top-left)
            place(right_cx, bot_cy, re), // RIGHT  (bottom-right)
            place(right_cx, top_cy, ie), // ISO    (top-right)
        ],
    )
}

/// Fully automatic standard drawing: picks the sheet size and fill scale
/// from the part's size, lays the three third-angle views out CENTERED in
/// the drawing area with room for dimensions, and renders them with HLR +
/// auto dimensions + centerlines. This is what "right-click → drawing"
/// uses so a small part fills a small sheet instead of floating in a
/// corner of an oversized one.
pub fn standard_drawing_auto(
    model: &BRepModel,
    solid_id: SolidId,
    part_uuid: uuid::Uuid,
) -> Result<super::types::Drawing, super::projection::ProjectionError> {
    use super::types::{Drawing, ViewSource};

    let source = ViewSource::Part {
        part_id: part_uuid,
        solid_id,
    };
    let min_span = 0.5_f64;
    // Order matches `layout_four_view`'s returned positions:
    // [Front, Top, Right, Iso]. Only the orthographic views are
    // dimensioned; the isometric is a clean pictorial reference.
    let specs = [
        (ProjectionType::Front, "FRONT", true),
        (ProjectionType::Top, "TOP", true),
        (ProjectionType::Right, "RIGHT", true),
        (ProjectionType::Isometric, "ISOMETRIC", false),
    ];

    // Pass 1 — unit-scale extents to drive sheet + scale + placement. The
    // sheet size keys off the ORTHOGRAPHIC max dimension (true part size),
    // not the larger isometric silhouette.
    let mut extents = Vec::with_capacity(4);
    for (proj, name, _) in specs {
        let v = build_hlr_view(
            model,
            solid_id,
            source,
            proj,
            name,
            [0.0, 0.0],
            1.0,
            min_span,
        )?;
        extents.push(v.extent);
    }
    let max_dim = extents
        .iter()
        .take(3)
        .map(|e| e.width().max(e.height()))
        .fold(0.0_f64, f64::max)
        .max(1.0);
    let sheet = pick_sheet(max_dim);
    let (scale, positions) =
        layout_four_view(&sheet, extents[0], extents[1], extents[2], extents[3]);

    // Pass 2 — build the placed, scaled views.
    let mut drawing = Drawing::new("Auto Drawing", sheet);
    drawing.set_unit_notes(model.document_unit());
    for (i, (proj, name, dimensioned)) in specs.iter().enumerate() {
        let mut view = build_hlr_view(
            model,
            solid_id,
            source,
            *proj,
            name,
            positions[i],
            scale,
            min_span,
        )?;
        if !dimensioned {
            view.dimensions.clear();
        }
        drawing.add_view(view);
    }
    dedup_dimensions_global(&mut drawing);

    // ── Hole table population ──────────────────────────────────────────────────
    // Extract all dimension records for the solid and derive the hole table.
    // This must happen AFTER dedup so entity merging is stable.
    attach_hole_table(model, solid_id, scale, &mut drawing);

    Ok(drawing)
}

/// Populate `drawing.hole_sites` and `drawing.axial_view_idx` from the
/// kernel's analytic dimension table.
///
/// - `scale`: the drawing scale (used to convert world-space anchor coords
///   into the axial view's view-space so tag callouts land on the circles).
/// - Bore axial centres in the TOP view are derived by projecting the bore's
///   world anchor through the TOP projection (camera looks down −Z:
///   view_x = world_x, view_y = world_y).
fn attach_hole_table(
    model: &crate::primitives::topology_builder::BRepModel,
    solid_id: SolidId,
    _scale: f64,
    drawing: &mut super::types::Drawing,
) {
    use super::hole_table::build_hole_table;
    use crate::readable::extract_dimensions;

    let dims = extract_dimensions(model, solid_id);

    // Determine part extents for THRU detection from the whole-part extent
    // records (X/Y/Z).
    //
    // CONTRACT (with `extract_dimensions` in readable/dimensions.rs): the
    // whole-part AABB extents are emitted with `kind == "extent"` AND an
    // EMPTY `entities` list — that emptiness is the discriminator between
    // "whole-part extent" and any future face-scoped extent record. `kind`
    // is the primary key here; `entities.is_empty()` is the whole-part
    // qualifier. If extraction ever starts emitting extent records WITH
    // entity ids, those are face-scoped and must NOT feed part_extents —
    // this filter is therefore load-bearing for THRU detection: dropping
    // it would shrink part_extents to a face extent and misclassify THRU
    // bores as blind (and vice versa if the filter silently matched
    // nothing, every bore would fall back to BLIND via f64::MAX below).
    let mut part_extents = [f64::INFINITY; 3];
    for d in &dims {
        if d.kind == "extent" && d.entities.is_empty() {
            let ax = d.direction;
            let idx = if ax[0].abs() >= ax[1].abs() && ax[0].abs() >= ax[2].abs() {
                0
            } else if ax[1].abs() >= ax[2].abs() {
                1
            } else {
                2
            };
            // Take the minimum of what we see (extent records are one per axis).
            if d.value < part_extents[idx] {
                part_extents[idx] = d.value;
            }
        }
    }
    // Fallback: if no extent records (degenerate part), use large value so
    // everything is treated as blind (conservative).
    let part_extents = [
        if part_extents[0].is_finite() {
            part_extents[0]
        } else {
            f64::MAX
        },
        if part_extents[1].is_finite() {
            part_extents[1]
        } else {
            f64::MAX
        },
        if part_extents[2].is_finite() {
            part_extents[2]
        } else {
            f64::MAX
        },
    ];

    let mut sites = build_hole_table(&dims, part_extents);
    if sites.is_empty() {
        // No cylindrical bores — no hole table.
        return;
    }

    // Find the axial view. The bore axes are stored in the diameter records.
    // Collect the dominant bore axis (most common dominant axis among bore records).
    let mut axis_votes = [0usize; 3];
    for d in &dims {
        if d.kind != "diameter" || d.entities.is_empty() {
            continue;
        }
        if let Some(ax) = d.axis {
            let abs = [ax[0].abs(), ax[1].abs(), ax[2].abs()];
            let dom = if abs[0] >= abs[1] && abs[0] >= abs[2] {
                0
            } else if abs[1] >= abs[2] {
                1
            } else {
                2
            };
            axis_votes[dom] += 1;
        }
    }
    let dominant_bore_axis = axis_votes
        .iter()
        .enumerate()
        .max_by_key(|&(_, &v)| v)
        .map(|(i, _)| i)
        .unwrap_or(2); // default Z

    // Find the view that is axial for the dominant bore axis: the camera
    // must look nearly along that axis.
    // view_dir returns the direction the camera looks INTO the scene:
    //   Front → [0,-1,0], Top → [0,0,-1], Right → [-1,0,0]
    let axial_view_idx = drawing.views.iter().enumerate().find_map(|(i, v)| {
        let vd = match v.projection {
            ProjectionType::Front => [0.0_f64, -1.0, 0.0],
            ProjectionType::Top => [0.0, 0.0, -1.0],
            ProjectionType::Right => [-1.0, 0.0, 0.0],
            ProjectionType::Bottom => [0.0, 0.0, 1.0],
            ProjectionType::Left => [1.0, 0.0, 0.0],
            _ => return None,
        };
        // The camera is axial when |vd[dominant]| ≈ 1.
        if vd[dominant_bore_axis].abs() > 0.9 {
            Some(i)
        } else {
            None
        }
    });
    drawing.axial_view_idx = axial_view_idx;

    // Attach axial-view 2D centres to each site.
    //
    // Strategy: for each HoleSite, we know its (x_mm, y_mm) position offsets
    // from the part corner datum. The position records carry the datum origin
    // in `datum.origin`. So the bore world-centre along the two perpendicular
    // axes is: datum.origin[perp_i] + site.x_mm (or y_mm).
    //
    // We then project that 3D centre into the axial view (2D) and match it
    // to the nearest unassigned circle in that view (by distance to circle
    // centre, within 1 mm tolerance). Each circle is consumed exactly once so
    // no two sites share a circle.
    if let Some(ax_idx) = axial_view_idx {
        // Collect bore world centres keyed by (x_mm, y_mm) from position records.
        // For a Z-axis bore the datum.origin gives the part corner in world space.
        // bore_world[perp0] = datum.origin[perp0] + position_value[perp0]
        // bore_world[perp1] = datum.origin[perp1] + position_value[perp1]
        // bore_world[dominant] = datum.origin[dominant] (at the drilled-face plane)
        //
        // Build per-entity-set → bore_world_centre.
        let mut bore_centres: std::collections::HashMap<Vec<u32>, [f64; 3]> =
            std::collections::HashMap::new();

        // Collect position records grouped by entity set.
        let mut pos_by_ent: std::collections::HashMap<
            Vec<u32>,
            Vec<&crate::readable::DimensionRecord>,
        > = std::collections::HashMap::new();
        for d in &dims {
            if d.kind == "position" && !d.entities.is_empty() {
                let mut key = d.entities.clone();
                key.sort_unstable();
                pos_by_ent.entry(key).or_default().push(d);
            }
        }

        for d in &dims {
            if d.kind == "diameter" && !d.entities.is_empty() {
                let mut key = d.entities.clone();
                key.sort_unstable();
                if bore_centres.contains_key(&key) {
                    continue; // already computed
                }
                let axis = match d.axis {
                    Some(a) => a,
                    None => continue,
                };
                let abs = [axis[0].abs(), axis[1].abs(), axis[2].abs()];
                let dominant = if abs[0] >= abs[1] && abs[0] >= abs[2] {
                    0
                } else if abs[1] >= abs[2] {
                    1
                } else {
                    2
                };
                let perps = match dominant {
                    0 => [1usize, 2],
                    1 => [0, 2],
                    _ => [0, 1],
                };

                if let Some(pos_recs) = pos_by_ent.get(&key) {
                    // Use the datum origin from the first position record.
                    let datum_origin = pos_recs
                        .iter()
                        .find_map(|r| r.datum.as_ref().map(|dt| dt.origin))
                        .unwrap_or([0.0; 3]);
                    let mut centre = datum_origin;
                    for pr in pos_recs {
                        // Direction tells us which perpendicular axis this is.
                        let dir = pr.direction;
                        let perp_idx = if dir[perps[0]].abs() > dir[perps[1]].abs() {
                            perps[0]
                        } else {
                            perps[1]
                        };
                        let sign = if dir[perp_idx] >= 0.0 { 1.0 } else { -1.0 };
                        centre[perp_idx] = datum_origin[perp_idx] + pr.value * sign;
                    }
                    bore_centres.insert(key, centre);
                } else {
                    // No position records: fall back to the diameter record anchor
                    // minus the bore radius in the seam direction (anchor is on the
                    // lateral face rim, so subtract radius to get the axis).
                    let rd = d.direction; // seam direction (from axis toward rim)
                    let radius = d.value * 0.5;
                    let centre = [
                        d.anchor[0] - rd[0] * radius,
                        d.anchor[1] - rd[1] * radius,
                        d.anchor[2] - rd[2] * radius,
                    ];
                    bore_centres.insert(key, centre);
                }
            }
        }

        let view = &drawing.views[ax_idx];

        // Project a world 3D point to the axial view's 2D view-space.
        let project_world = |p: [f64; 3]| -> [f64; 2] {
            match view.projection {
                ProjectionType::Top => [p[0], p[1]],
                ProjectionType::Front => [p[0], p[2]],
                ProjectionType::Right => [-p[1], p[2]],
                ProjectionType::Bottom => [p[0], p[1]],
                ProjectionType::Left => [p[1], p[2]],
                _ => [p[0], p[1]],
            }
        };

        // ── Entity-keyed site → circle assignment ──────────────────────────
        //
        // The projected circle carries `face_ids` — every B-Rep face adjacent
        // to the rim edges that produced it (populated at the projection site
        // in `project_solid_edges_visibility`). The site carries the bore's
        // lateral-face ids (`HoleSite::face_entities`, from the diameter
        // extraction record). A non-empty intersection IS this bore's rim:
        // no coordinate heuristics, no greedy consumption, correct for
        // off-origin and transformed parts by construction. A through-bore
        // has two coaxial rims (top + bottom) sharing the lateral face; in
        // the axial view they project to the same centre, so matching either
        // anchors the tag identically. The radius gate keeps a same-face but
        // different-radius rim (e.g. a chamfer ring) from matching.
        for site in &mut sites {
            let r_target = site.diameter_mm * 0.5;
            let r_ok = |r: f64| (r - r_target).abs() < r_target * 0.05 + 0.1;
            let matched = view
                .circles
                .iter()
                .chain(view.hidden_circles.iter())
                .find(|c| r_ok(c.r) && c.face_ids.iter().any(|f| site.face_entities.contains(f)));
            if let Some(c) = matched {
                site.axial_centre = Some([c.cx, c.cy]);
                continue;
            }
            // Fallback: the rim did not survive projection as an analytic
            // circle (e.g. a mixed-visibility rim rendered as arc polylines).
            // Project the bore's analytic world centre from the extraction
            // records — still exact data, just not snapped to drawn ink.
            let mut key = site.face_entities.clone();
            key.sort_unstable();
            if let Some(&wc) = bore_centres.get(&key) {
                site.axial_centre = Some(project_world(wc));
            }
        }
    }

    drawing.hole_sites = sites;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    fn has(dims: &[Dimension2d], kind: &str, value: f64) -> bool {
        dims.iter()
            .any(|d| d.kind == kind && (d.value - value).abs() < 1e-3)
    }

    #[test]
    fn box_front_view_dimensions_match_built_and_project_true_length() {
        // Box 40(X) × 30(Y) × 20(Z). Front view (camera +Y) shows X→right,
        // Z→up. So the X(40) and Z(20) extents read at TRUE projected length;
        // the Y(30) extent projects edge-on (depth) → near-zero span.
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(40.0, 30.0, 20.0)
            .expect("box"));
        let dims = auto_dimensions(&m, b, ProjectionType::Front);
        assert!(has(&dims, "extent", 40.0), "X extent present: {dims:?}");
        assert!(has(&dims, "extent", 30.0), "Y extent present");
        assert!(has(&dims, "extent", 20.0), "Z extent present");

        // Built == drawn: the X extent projects to a ~40mm span in Front.
        let x = dims
            .iter()
            .find(|d| d.kind == "extent" && (d.value - 40.0).abs() < 1e-3)
            .expect("X extent");
        assert!(
            (x.projected_span() - 40.0).abs() < 1e-6,
            "X span {} != 40",
            x.projected_span()
        );
        // The Y (depth) extent projects edge-on in Front → ~0 span.
        let y = dims
            .iter()
            .find(|d| d.kind == "extent" && (d.value - 30.0).abs() < 1e-3)
            .expect("Y extent");
        assert!(
            y.projected_span() < 1e-6,
            "Y depth should project edge-on, got {}",
            y.projected_span()
        );

        // visible_dimensions drops the edge-on Y extent.
        let vis = visible_dimensions(&m, b, ProjectionType::Front, 1.0);
        assert!(has(&vis, "extent", 40.0) && has(&vis, "extent", 20.0));
        assert!(
            !has(&vis, "extent", 30.0),
            "edge-on Y extent dropped from Front"
        );
    }

    #[test]
    fn standard_drawing_renders_a_dimensioned_svg() {
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(40.0, 30.0, 20.0)
            .expect("box"));
        let dwg = standard_drawing(
            &m,
            b,
            uuid::Uuid::nil(),
            super::super::types::SheetSize::A3,
            1.0,
        )
        .expect("standard drawing");
        assert_eq!(dwg.views.len(), 3, "front/top/right");

        // After dedup, each extent value appears EXACTLY once sheet-wide.
        // A plain box (no cylinders) has three extents: X=40, Y=30, Z=20.
        // Each shows in the view where it has its best (largest) projected span,
        // which for a box at the standard layout is: FRONT (X+Z), TOP (Y).
        // Some views may have zero callouts after dedup — that is correct.
        let sheet_dims: Vec<&Dimension2d> = dwg.views.iter().flat_map(|v| &v.dimensions).collect();
        let count = |kind: &str, val: f64| {
            sheet_dims
                .iter()
                .filter(|d| d.kind == kind && (d.value - val).abs() < 1e-3)
                .count()
        };
        assert_eq!(count("extent", 40.0), 1, "X=40 once sheet-wide");
        assert_eq!(count("extent", 30.0), 1, "Y=30 once sheet-wide");
        assert_eq!(count("extent", 20.0), 1, "Z=20 once sheet-wide");

        let svg = crate::drawing::render_drawing_svg(&dwg);
        // The drawing carries ISO dimension lines (offset, with arrowheads)
        // and the EXACT values — 40 / 30 / 20 each appear in the SVG ink.
        assert!(svg.contains("dim-line"), "dimension lines rendered");
        assert!(svg.contains("dim-arrow"), "dimension arrowheads rendered");
        assert!(svg.contains("40.00"), "40mm extent value drawn");
        assert!(svg.contains("30.00"), "30mm extent value drawn");
        assert!(svg.contains("20.00"), "20mm extent value drawn");
    }

    #[test]
    fn bored_plate_diameter_callout_is_built_and_recoverable() {
        let mut m = BRepModel::new();
        let plate = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(50.0, 50.0, 16.0)
            .expect("plate"));
        let bore = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, -20.0), Vector3::Z, 10.0, 80.0)
            .expect("bore"));
        let part = crate::operations::boolean::boolean_operation(
            &mut m,
            plate,
            bore,
            crate::operations::boolean::BooleanOp::Difference,
            crate::operations::boolean::BooleanOptions::default(),
        )
        .expect("bore");
        // Top view (camera +Z) shows the Ø20 bore across its full diameter.
        let dims = auto_dimensions(&m, part, ProjectionType::Top);
        let dia = dims
            .iter()
            .find(|d| d.kind == "diameter" && (d.value - 20.0).abs() < 1e-3)
            .expect("Ø20 bore callout");
        assert!(
            (dia.projected_span() - 20.0).abs() < 1e-6,
            "Ø20 spans 20mm in Top"
        );
        assert!(
            !dia.entities.is_empty(),
            "diameter callout names its face (recoverable)"
        );
        assert!(
            dia.label.contains("20"),
            "label carries the value: {}",
            dia.label
        );
    }

    /// Build the shared 40×40×10 plate-with-Ø5-bore fixture used by the dedup
    /// tests. A 2.5-radius (Ø5) cylinder drilled Z-axis through a 40×40×10
    /// plate.
    fn bored_plate_5mm() -> (crate::primitives::topology_builder::BRepModel, SolidId) {
        let mut m = BRepModel::new();
        let plate = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(40.0, 40.0, 10.0)
            .expect("plate"));
        let bore = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, -6.0), Vector3::Z, 2.5, 12.0)
            .expect("bore"));
        let part = crate::operations::boolean::boolean_operation(
            &mut m,
            plate,
            bore,
            crate::operations::boolean::BooleanOp::Difference,
            crate::operations::boolean::BooleanOptions::default(),
        )
        .expect("drill");
        (m, part)
    }

    #[test]
    fn same_view_equal_value_parallel_same_interval_collapses_to_one() {
        // Plate 40×40×10 with one Ø5 through-bore: FRONT shows the Z extent
        // (10.00) AND the bore length (10.00) — same value, both vertical,
        // same z-interval. Exactly one must survive, and it is the extent.
        let (m, part) = bored_plate_5mm();
        let mut dwg = standard_drawing(
            &m,
            part,
            uuid::Uuid::nil(),
            super::super::types::SheetSize::A3,
            1.0,
        )
        .expect("sheet");
        dedup_dimensions_global(&mut dwg);
        let front = dwg.views.iter().find(|v| v.name == "FRONT").expect("front");
        let tens: Vec<_> = front
            .dimensions
            .iter()
            .filter(|d| (d.value - 10.0).abs() < 1e-6)
            .collect();
        assert_eq!(tens.len(), 1, "one 10.00 in FRONT, got {tens:?}");
        assert_eq!(tens[0].kind, "extent", "the envelope extent wins the tie");
    }

    #[test]
    fn cross_view_diameter_survives_only_in_axial_view() {
        // The bore's Ø5 reads in TOP (axial view: camera −Z ∥ bore axis Z,
        // hole shows as a true circle) and in FRONT (rectangle). It must
        // survive ONLY in TOP.
        let (m, part) = bored_plate_5mm();
        let mut dwg = standard_drawing(
            &m,
            part,
            uuid::Uuid::nil(),
            super::super::types::SheetSize::A3,
            1.0,
        )
        .expect("sheet");
        dedup_dimensions_global(&mut dwg);
        let in_view = |name: &str| {
            dwg.views
                .iter()
                .find(|v| v.name == name)
                .map(|v| {
                    v.dimensions
                        .iter()
                        .any(|d| d.kind == "diameter" && (d.value - 5.0).abs() < 1e-6)
                })
                .unwrap_or(false)
        };
        assert!(in_view("TOP"), "Ø5 dimensioned in its circle view");
        assert!(!in_view("FRONT"), "Ø5 not repeated in FRONT");
        assert!(!in_view("RIGHT"), "Ø5 not repeated in RIGHT");
    }
}
