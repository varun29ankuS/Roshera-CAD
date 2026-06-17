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
            "length" | "extent" => (
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
        if matches!(first, Some(c) if c.is_ascii_uppercase() && c != 'R') {
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
        if seen.insert((q(c.cx), q(c.cy), q(c.r))) {
            groups.entry((q(c.cx), q(c.cy))).or_default().push(c);
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
    view.polylines = edges.visible;
    view.hidden_polylines = edges.hidden;
    view.circles = select_circles(edges.circles);
    view.hidden_circles = select_circles(edges.hidden_circles);
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
    Ok(drawing)
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
        assert!(
            dwg.views.iter().all(|v| !v.dimensions.is_empty()),
            "every view auto-dimensioned"
        );
        let svg = crate::drawing::render_drawing_svg(&dwg);
        // The drawing carries ISO dimension lines (offset, with arrowheads)
        // and the EXACT values — 40 / 30 / 20 each read in the view that
        // reveals them.
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
}
