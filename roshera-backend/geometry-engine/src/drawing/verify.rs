//! Drawing quality verification — the perception/feedback layer for 2D
//! drawings, the sheet-space analogue of the watertight / validity
//! oracles for 3D geometry.
//!
//! A drawing can be *geometrically* correct (every polyline is a true
//! projection) yet read as a bad engineering drawing: views overlapping,
//! falling off the sheet, colliding with the title block, crammed into a
//! corner of an oversized sheet, or dimensions stamped on top of the
//! part with no offset. Those are exactly the defects a human means by
//! "it looks bad". This module makes each of them a *measurable*
//! invariant in sheet millimetres, recoverable to a `(view, kind,
//! message)` triple, so every generated drawing self-reports its quality.
//!
//! All geometry is reasoned about in **SVG sheet coordinates** (origin
//! top-left, +x right, +y DOWN, millimetres) — the same frame
//! [`render_drawing_svg`](super::svg::render_drawing_svg) emits — so a
//! reported collision corresponds 1:1 to what the renderer draws. A view
//! point `(vx, vy)` maps to the sheet as
//! `(pos.x + vx·scale, (sheet_h − pos.y) − vy·scale)`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::layout::{compute_layout, Rect2, SheetItemKind};
use super::svg::frame_margins;
use super::types::{Drawing, ProjectionType};

/// Severity of a single quality finding. `Error` fails the report;
/// `Warning` is advisory (the drawing is usable but sub-standard).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

/// Machine-stable classification of a quality finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrawingIssueKind {
    /// The drawing carries no views at all.
    NoViews,
    /// A view projected to zero edges (nothing to see).
    EmptyView,
    /// A view's geometry extends past the inner drawing frame / margins.
    ViewOutsideFrame,
    /// A view's geometry overlaps the title block.
    ViewOverlapsTitleBlock,
    /// Two views' geometry bounding boxes overlap.
    ViewOverlap,
    /// The views together cover too little of the printable area — the
    /// part reads as tiny on an oversized sheet ("no sense of size").
    SheetUnderutilized,
    /// The standard third-angle arrangement is broken: Top is not above
    /// Front, or Right is not beside Front.
    ProjectionMisaligned,
    /// A dimension callout sits on / inside the part silhouette instead
    /// of being offset clear of it (no extension line / standoff).
    DimensionOnGeometry,
    /// Two dimension labels in the same view overlap each other.
    DimensionLabelCollision,
    /// A view shows geometry but carries no dimensions.
    UndimensionedView,
    /// Two view labels (or a view label and another text item) overlap on
    /// the sheet — the viewer cannot read which view is which.
    ViewLabelCollision,
    /// Two GD&T sheet items (DatumSymbol and/or FcfBlock) overlap each other
    /// with no ViewLabel in the pair — the annotation is unreadable.
    ///
    /// Complements `ViewLabelCollision`: that invariant polices pairs where
    /// at least one item is a ViewLabel; this one polices GD&T-only pairs
    /// that would otherwise slip through the `pair_has_label` guard.
    GdtSymbolCollision,
    /// The same dimension (same quantized value, same orientation, same
    /// measured interval) appears more than once on the sheet, making the
    /// drawing redundant and potentially misleading.
    RedundantDimension,
    /// The isometric cell's shaded-solid raster and its HLR vector wireframe
    /// are drawn through DIFFERENT iso cameras, so the two representations of
    /// the same part disagree on orientation (an asymmetric feature — e.g. a
    /// bore — lands in a different screen corner in the wireframe than in the
    /// shaded overlay). Both must derive from the one shared iso camera
    /// (`render::camera_basis(CanonicalView::Isometric)`).
    IsoOrientationMismatch,
}

/// A single quality finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrawingIssue {
    pub severity: Severity,
    pub kind: DrawingIssueKind,
    pub message: String,
    /// Name of the view the finding belongs to, when view-scoped.
    pub view: Option<String>,
}

/// Structured quality report for a whole drawing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrawingQualityReport {
    /// `true` iff there are no `Error`-severity issues.
    pub passed: bool,
    /// Fraction `[0, 1]` of the printable area covered by view geometry.
    pub sheet_utilization: f64,
    pub issues: Vec<DrawingIssue>,
}

impl DrawingQualityReport {
    /// Count of `Error`-severity findings.
    pub fn error_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .count()
    }

    /// Count of `Warning`-severity findings.
    pub fn warning_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|i| i.severity == Severity::Warning)
            .count()
    }

    /// True if any issue of the given kind is present.
    pub fn has(&self, kind: DrawingIssueKind) -> bool {
        self.issues.iter().any(|i| i.kind == kind)
    }
}

// ── Rect2 helpers used only by this module ────────────────────────────

/// True when `inner` is fully inside `outer` (allowing `tol` mm of overhang).
fn rect_contains(outer: &Rect2, inner: &Rect2, tol: f64) -> bool {
    inner.x0 >= outer.x0 - tol
        && inner.x1 <= outer.x1 + tol
        && inner.y0 >= outer.y0 - tol
        && inner.y1 <= outer.y1 + tol
}

// ── Tunables (all millimetres) ──────────────────────────────────────

/// Slack allowed before two rects count as overlapping / out of bounds.
const SLACK_MM: f64 = 0.5;
/// Dimension band reserved to the left of and below a dimensioned view —
/// matches the standoff + stacking + text in `svg::render_dimensions`.
/// `pub(crate)`: the pictorial-isometric free-space search
/// (`dimensioning::attach_pictorial_iso`) expands dimensioned views by this
/// same margin so it clears dimension ink exactly where this verifier polices.
pub(crate) const DIM_MARGIN_MM: f64 = 22.0;
/// Centre-alignment tolerance for the third-angle arrangement.
const ALIGN_TOL_MM: f64 = 2.0;
/// Below this fraction of the printable area, the sheet reads as empty.
const MIN_UTILIZATION: f64 = 0.10;

/// Verify a drawing's layout/annotation quality. Pure function of the
/// `Drawing` — no kernel access — so it is cheap to run on every
/// generated sheet and to gate in tests.
pub fn verify_drawing(drawing: &Drawing) -> DrawingQualityReport {
    let mut issues: Vec<DrawingIssue> = Vec::new();

    if drawing.views.is_empty() {
        issues.push(error(
            DrawingIssueKind::NoViews,
            "drawing has no views".to_string(),
            None,
        ));
        return finalize(issues, 0.0);
    }

    let w = drawing.sheet_size.width();
    let h = drawing.sheet_size.height();

    // ── Single layout computation — all geometry reads come from here ────
    // `compute_layout` is the one canonical placement model: it owns
    // ViewGeometry bboxes, the TitleBlock rect, ViewLabel placements, and
    // PlacedDimension anchors. `verify_drawing` reads from it; it does not
    // recompute any geometry independently.
    let layout = compute_layout(drawing);

    // Derive the drawing frame from the sheet margins. The frame is the
    // inset rectangle that encloses all view geometry and labels; it is
    // NOT a layout item (layout items live inside the frame), but it must
    // be consistent with what the renderer draws. `frame_margins` is the
    // single definition shared by svg.rs, layout.rs, and verify.rs.
    let (ml, mr, mt, mb) = frame_margins(&drawing.sheet_size);
    let frame = Rect2 {
        x0: ml,
        y0: mt,
        x1: w - mr,
        y1: h - mb,
    };

    // The title-block rect is read directly from the layout's TitleBlock
    // item — the same rect the renderer draws and the viewer sees.
    let title_block: Rect2 = layout
        .items
        .iter()
        .find(|it| it.kind == SheetItemKind::TitleBlock)
        .map(|it| it.bbox)
        .unwrap_or_else(|| {
            // Degenerate (zero-view) drawing: use an empty rect at the
            // bottom-right corner so overlap checks never fire spuriously.
            Rect2 {
                x0: frame.x1,
                y0: frame.y1,
                x1: frame.x1,
                y1: frame.y1,
            }
        });

    let mut rects: Vec<(String, Rect2)> = Vec::new();
    let mut ink_area = 0.0;

    for (idx, v) in drawing.views.iter().enumerate() {
        let name = v.name.clone();

        if v.polylines.is_empty() && v.hidden_polylines.is_empty() {
            issues.push(warning(
                DrawingIssueKind::EmptyView,
                format!("view '{name}' projected to no edges"),
                Some(name.clone()),
            ));
        }

        // Read the view's geometry rect from the layout's ViewGeometry item
        // (keyed by owner_view index) instead of calling view_geometry_rect
        // independently. This is the same rect the renderer uses.
        let geo = layout
            .items
            .iter()
            .find(|it| it.kind == SheetItemKind::ViewGeometry && it.owner_view == Some(idx))
            .map(|it| it.bbox);

        if let Some(r) = geo {
            ink_area += r.area();
            // Dimensions render offset LEFT of and BELOW the view (see
            // svg::render_dimensions), so the space they occupy is part of
            // the view's footprint. Account for it on dimensioned views so
            // a callout that would run off-sheet or into a neighbour is
            // caught; the isometric (no dims) uses its bare silhouette.
            let footprint = if v.dimensions.is_empty() {
                r
            } else {
                Rect2 {
                    x0: r.x0 - DIM_MARGIN_MM,
                    y0: r.y0,
                    x1: r.x1,
                    y1: r.y1 + DIM_MARGIN_MM,
                }
            };
            if !rect_contains(&frame, &footprint, SLACK_MM) {
                issues.push(error(
                    DrawingIssueKind::ViewOutsideFrame,
                    format!("view '{name}' (with its dimensions) extends past the drawing frame"),
                    Some(name.clone()),
                ));
            }
            if footprint.intersects(&title_block, SLACK_MM) {
                issues.push(error(
                    DrawingIssueKind::ViewOverlapsTitleBlock,
                    format!("view '{name}' overlaps the title block"),
                    Some(name.clone()),
                ));
            }
            rects.push((name, footprint));
        }
    }

    // Pairwise view overlap.
    for i in 0..rects.len() {
        for j in (i + 1)..rects.len() {
            if rects[i].1.intersects(&rects[j].1, SLACK_MM) {
                issues.push(error(
                    DrawingIssueKind::ViewOverlap,
                    format!("views '{}' and '{}' overlap", rects[i].0, rects[j].0),
                    None,
                ));
            }
        }
    }

    // Sheet utilization.
    let printable = (frame.area() - title_block.area()).max(1.0);
    let utilization = (ink_area / printable).clamp(0.0, 1.0);
    if utilization < MIN_UTILIZATION {
        issues.push(warning(
            DrawingIssueKind::SheetUnderutilized,
            format!(
                "views fill only {:.0}% of the sheet — scale up or use a smaller sheet",
                utilization * 100.0
            ),
            None,
        ));
    }

    check_alignment(drawing, &layout, &mut issues);

    // ── ViewLabelCollision + GdtSymbolCollision detection ────────────────
    // Two sub-checks, both using the layout already computed above:
    //
    // (a) ViewLabel × {ViewLabel | DimensionText | NoteText | GD&T items}:
    //     at least one item must be a ViewLabel → fires ViewLabelCollision.
    //     NoteText items (unit/tolerance/projection note lines) are included:
    //     they are layout items since Task 8 and must not be obscured by view
    //     geometry or labels.
    //     DimText↔DimText collision is handled separately by
    //     check_dimension_label_collisions (DimensionLabelCollision).
    //
    // (b) GD&T × GD&T pairs (DatumSymbol or FcfBlock, NO ViewLabel in pair):
    //     fires GdtSymbolCollision.  These pairs were previously invisible to
    //     the verifier because the `pair_has_label` guard short-circuited
    //     before reaching them — a coverage gap closed here.
    //
    // (c) ViewLabel × ViewGeometry of a DIFFERENT view: a label legitimately
    //     sits near its OWN view's geometry (placement avoids it, but same-view
    //     pairing would false-positive on the fallback slots). Cross-view overlap
    //     IS a placement failure — placement treats all geometry rects as
    //     obstacles, so any such overlap indicates the fallback landed on a
    //     neighbour's geometry.
    let label_items: Vec<&super::layout::SheetItem> = layout
        .items
        .iter()
        .filter(|it| {
            matches!(
                it.kind,
                // ProjectionSymbol is deliberately EXCLUDED from text-collision pairs:
                // it lives inside the title-block zone, which the view-placement checks
                // already treat as an obstacle - pairing it here would double-report
                // every title-block intrusion.
                SheetItemKind::ViewLabel
                    | SheetItemKind::DimensionText
                    | SheetItemKind::NoteText
                    | SheetItemKind::CuttingPlaneLabel
                    | SheetItemKind::DatumSymbol
                    | SheetItemKind::FcfBlock
            )
        })
        .collect();
    for i in 0..label_items.len() {
        for j in (i + 1)..label_items.len() {
            if !label_items[i].bbox.intersects(&label_items[j].bbox, 0.2) {
                continue;
            }
            let pair_has_label = label_items[i].kind == SheetItemKind::ViewLabel
                || label_items[j].kind == SheetItemKind::ViewLabel;
            let pair_is_gdt = matches!(
                label_items[i].kind,
                SheetItemKind::DatumSymbol | SheetItemKind::FcfBlock
            ) && matches!(
                label_items[j].kind,
                SheetItemKind::DatumSymbol | SheetItemKind::FcfBlock
            );
            if pair_has_label {
                issues.push(error(
                    DrawingIssueKind::ViewLabelCollision,
                    format!(
                        "text '{}' collides with '{}'",
                        label_items[i].text.as_deref().unwrap_or("?"),
                        label_items[j].text.as_deref().unwrap_or("?")
                    ),
                    None,
                ));
            } else if pair_is_gdt {
                issues.push(error(
                    DrawingIssueKind::GdtSymbolCollision,
                    format!(
                        "GD&T annotation '{}' overlaps '{}'",
                        label_items[i].text.as_deref().unwrap_or("?"),
                        label_items[j].text.as_deref().unwrap_or("?")
                    ),
                    None,
                ));
            }
        }
    }
    // (b) ViewLabel bbox intersecting a ViewGeometry item of a DIFFERENT view.
    let view_labels: Vec<&super::layout::SheetItem> = layout
        .items
        .iter()
        .filter(|it| it.kind == SheetItemKind::ViewLabel)
        .collect();
    let view_geoms: Vec<&super::layout::SheetItem> = layout
        .items
        .iter()
        .filter(|it| it.kind == SheetItemKind::ViewGeometry)
        .collect();
    for lbl in &view_labels {
        for geo in &view_geoms {
            // Skip same-view pair: a label may legitimately sit near its own
            // geometry (it is anchored to it), so same-view pairing would
            // false-positive on the fallback slots.
            if lbl.owner_view == geo.owner_view {
                continue;
            }
            if lbl.bbox.intersects(&geo.bbox, 0.2) {
                issues.push(error(
                    DrawingIssueKind::ViewLabelCollision,
                    format!(
                        "label '{}' overlaps the geometry of another view",
                        lbl.text.as_deref().unwrap_or("?"),
                    ),
                    None,
                ));
            }
        }
    }

    // ── RedundantDimension detection ─────────────────────────────────────
    check_redundant_dimensions(drawing, h, &mut issues);

    // ── DimensionLabelCollision detection ─────────────────────────────────
    check_dimension_label_collisions(&layout, &mut issues);

    // ── DimensionOnGeometry detection ─────────────────────────────────────
    check_dimension_on_geometry(&layout, drawing, &mut issues);

    // ── UndimensionedView detection ───────────────────────────────────────
    check_undimensioned_views(drawing, &mut issues);

    // ── IsoOrientationMismatch detection ──────────────────────────────────
    check_iso_orientation_agreement(drawing, &mut issues);

    finalize(issues, utilization)
}

/// Third-angle arrangement: Top directly above Front (shared x-centre),
/// Right directly beside Front (shared y-centre).
///
/// Reads each view's geometry rect from the `layout`'s `ViewGeometry` items
/// (keyed by `owner_view` index into `drawing.views`) — no independent
/// coordinate computation. The drawing reference is needed only to look up
/// each view's `ProjectionType`.
fn check_alignment(
    drawing: &Drawing,
    layout: &super::layout::SheetLayout,
    issues: &mut Vec<DrawingIssue>,
) {
    // For each projection kind, find the ViewGeometry item whose owner_view
    // index points to a view with that projection.
    let rect_of = |want: fn(&ProjectionType) -> bool| -> Option<Rect2> {
        drawing
            .views
            .iter()
            .enumerate()
            .find(|(_, v)| want(&v.projection))
            .and_then(|(idx, _)| {
                layout
                    .items
                    .iter()
                    .find(|it| it.kind == SheetItemKind::ViewGeometry && it.owner_view == Some(idx))
                    .map(|it| it.bbox)
            })
    };

    let front = rect_of(|p| matches!(p, ProjectionType::Front));
    let top = rect_of(|p| matches!(p, ProjectionType::Top));
    let right = rect_of(|p| matches!(p, ProjectionType::Right));

    if let (Some(f), Some(t)) = (front, top) {
        let fcx = 0.5 * (f.x0 + f.x1);
        let tcx = 0.5 * (t.x0 + t.x1);
        if (fcx - tcx).abs() > ALIGN_TOL_MM {
            issues.push(warning(
                DrawingIssueKind::ProjectionMisaligned,
                "Top view is not vertically aligned over the Front view (third-angle)".to_string(),
                None,
            ));
        }
    }
    if let (Some(f), Some(r)) = (front, right) {
        let fcy = 0.5 * (f.y0 + f.y1);
        let rcy = 0.5 * (r.y0 + r.y1);
        if (fcy - rcy).abs() > ALIGN_TOL_MM {
            issues.push(warning(
                DrawingIssueKind::ProjectionMisaligned,
                "Right view is not horizontally aligned with the Front view (third-angle)"
                    .to_string(),
                None,
            ));
        }
    }
}

/// Detect dimensions that are logically redundant.
///
/// Two detection modes:
///
/// 1. **Cross-view entity duplicate**: the same B-Rep face set (`d.entities`,
///    non-empty) and dimension kind appears on more than one view — the same
///    named feature is being called out twice. Whole-part extents
///    (`entities` is empty) are skipped here; they legitimately appear in
///    multiple views to give context.
///
///    **Tabled-position exception**: `kind == "position"` dimensions whose
///    entity set intersects a tabled bore's `face_entities` are excluded from
///    this check. Position dims for tabled bores are represented in the hole
///    table (X/Y columns) rather than the general dim stack, so their
///    appearance in multiple views is by design — both views carry the dim for
///    view-space completeness, but neither is rendered as a redundant callout.
///
/// 2. **Same-view same-interval**: within one view, two dimensions with the
///    same orientation (H or V) have interval endpoints that coincide within
///    0.5 mm in sheet space. This catches "10.00 plate-thickness + 10.00
///    bore-length both stacked on the same vertical interval in FRONT".
fn check_redundant_dimensions(drawing: &Drawing, sheet_h: f64, issues: &mut Vec<DrawingIssue>) {
    // Build the tabled-face-id set from the drawing's hole sites (same set
    // that `place_dimensions` uses to suppress rendering). Position dims for
    // tabled bores are excluded from the cross-view duplicate check.
    let tabled_face_ids: std::collections::HashSet<u32> = drawing
        .hole_sites
        .iter()
        .flat_map(|s| s.face_entities.iter().copied())
        .collect();

    // ── Cross-view entity check ──────────────────────────────────────────
    // Key: (sorted entity ids, kind) → Vec<view_name>. Only for non-empty
    // entity lists (named features, not whole-part extents).
    {
        let mut entity_key: HashMap<(Vec<u32>, String), Vec<String>> = HashMap::new();
        for v in &drawing.views {
            for d in &v.dimensions {
                if d.entities.is_empty() {
                    continue;
                }
                // Tabled-position exception: skip position dims for tabled bores.
                // These are correctly in multiple views for projection purposes but
                // rendered only in the hole table, not as redundant dim callouts.
                if d.kind == "position"
                    && !tabled_face_ids.is_empty()
                    && d.entities.iter().any(|eid| tabled_face_ids.contains(eid))
                {
                    continue;
                }
                let mut sorted = d.entities.clone();
                sorted.sort_unstable();
                entity_key
                    .entry((sorted, d.kind.clone()))
                    .or_default()
                    .push(v.name.clone());
            }
        }
        for ((_, kind), views) in &entity_key {
            if views.len() < 2 {
                continue;
            }
            // Report each pair once.
            for i in 0..views.len() {
                for j in (i + 1)..views.len() {
                    issues.push(error(
                        DrawingIssueKind::RedundantDimension,
                        format!(
                            "{} dimension for the same feature appears in both '{}' and '{}'",
                            kind, views[i], views[j]
                        ),
                        None,
                    ));
                }
            }
        }
    }

    // ── Same-view same-interval check ────────────────────────────────────
    // Within each view, look for pairs of dimensions with the same orientation
    // whose projected intervals (lo..hi in sheet space) coincide within 0.5 mm.
    for v in &drawing.views {
        struct Lin {
            lo: f64,
            hi: f64,
            orient: char,
            label: String,
        }
        let mut lins: Vec<Lin> = Vec::new();
        for d in &v.dimensions {
            let a = [
                v.position_mm[0] + d.a[0] * v.scale,
                (sheet_h - v.position_mm[1]) - d.a[1] * v.scale,
            ];
            let b = [
                v.position_mm[0] + d.b[0] * v.scale,
                (sheet_h - v.position_mm[1]) - d.b[1] * v.scale,
            ];
            let dx = (a[0] - b[0]).abs();
            let dy = (a[1] - b[1]).abs();
            if d.kind == "angle" || (dx < 1e-6 && dy < 1e-6) {
                continue;
            }
            let orient = if dx >= dy { 'H' } else { 'V' };
            let (lo, hi) = if orient == 'H' {
                (a[0].min(b[0]), a[0].max(b[0]))
            } else {
                (a[1].min(b[1]), a[1].max(b[1]))
            };
            lins.push(Lin {
                lo,
                hi,
                orient,
                label: d.label.clone(),
            });
        }
        for i in 0..lins.len() {
            for j in (i + 1)..lins.len() {
                if lins[i].orient != lins[j].orient {
                    continue;
                }
                let lo_match = (lins[i].lo - lins[j].lo).abs() < 0.5;
                let hi_match = (lins[i].hi - lins[j].hi).abs() < 0.5;
                if lo_match && hi_match {
                    issues.push(error(
                        DrawingIssueKind::RedundantDimension,
                        format!(
                            "view '{}': '{}' and '{}' bracket the same interval",
                            v.name, lins[i].label, lins[j].label
                        ),
                        Some(v.name.clone()),
                    ));
                }
            }
        }
    }
}

/// Collision tolerance for dimension-text bounding boxes (ISO 129 practice).
const DIM_TEXT_COLLISION_TOL_MM: f64 = 0.2;

/// Detect pairs of text-class bboxes that overlap — `DimensionText` and
/// `HoleTag` items both carry text that must not collide (ISO 129 prohibits
/// callout stacks that merge into one unreadable blob). Tolerates up to
/// 0.2 mm of positive interior overlap (printer registration noise). Any
/// larger overlap is an `Error`.
fn check_dimension_label_collisions(
    layout: &super::layout::SheetLayout,
    issues: &mut Vec<DrawingIssue>,
) {
    // DimensionText, HoleTag, and CuttingPlaneLabel carry callout text that must not overlap.
    let dim_texts: Vec<&super::layout::SheetItem> = layout
        .items
        .iter()
        .filter(|it| {
            matches!(
                it.kind,
                SheetItemKind::DimensionText
                    | SheetItemKind::HoleTag
                    | SheetItemKind::CuttingPlaneLabel
            )
        })
        .collect();

    for i in 0..dim_texts.len() {
        for j in (i + 1)..dim_texts.len() {
            if dim_texts[i]
                .bbox
                .intersects(&dim_texts[j].bbox, DIM_TEXT_COLLISION_TOL_MM)
            {
                issues.push(error(
                    DrawingIssueKind::DimensionLabelCollision,
                    format!(
                        "callout '{}' overlaps callout '{}'",
                        dim_texts[i].text.as_deref().unwrap_or("?"),
                        dim_texts[j].text.as_deref().unwrap_or("?"),
                    ),
                    None,
                ));
            }
        }
    }

    // ── Hole-table region × dimension text ────────────────────────────────
    // The table (borders + cells) must not sit on any dimension callout.
    // Coverage gap found on the live ring-plate sheet (2026-07-05): the
    // table was planted across the FRONT view's dim band and only the
    // tag×Ø pair was reported. The table REGION is the union of its
    // HoleTableBorder bboxes (the outer border spans the whole table), so
    // one check per offending dim text — not one per separator line.
    let table_region: Option<super::layout::Rect2> = layout
        .items
        .iter()
        .filter(|it| it.kind == SheetItemKind::HoleTableBorder)
        .map(|it| it.bbox)
        .reduce(|a, b| super::layout::Rect2 {
            x0: a.x0.min(b.x0),
            y0: a.y0.min(b.y0),
            x1: a.x1.max(b.x1),
            y1: a.y1.max(b.y1),
        });
    if let Some(tr) = table_region {
        for dt in layout
            .items
            .iter()
            .filter(|it| it.kind == SheetItemKind::DimensionText)
        {
            if tr.intersects(&dt.bbox, DIM_TEXT_COLLISION_TOL_MM) {
                issues.push(error(
                    DrawingIssueKind::DimensionLabelCollision,
                    format!(
                        "hole table overlaps dimension callout '{}'",
                        dt.text.as_deref().unwrap_or("?"),
                    ),
                    None,
                ));
            }
        }
    }
}

/// Detect dimension-text bboxes that intersect their OWN view's `ViewGeometry`
/// rect (ISO 129 §10: callout text must be offset clear of the part silhouette
/// via extension lines — text landing on the outline is a genuine annotation
/// defect, not merely an aesthetic preference).
///
/// Only the SAME view pairing is tested: a `DimensionText` item with
/// `owner_view = Some(idx)` is checked against the `ViewGeometry` item with
/// `owner_view = Some(idx)`. Cross-view pairings are ignored because the
/// verifier already catches cross-view overlaps via `ViewOverlap`.
fn check_dimension_on_geometry(
    layout: &super::layout::SheetLayout,
    drawing: &super::types::Drawing,
    issues: &mut Vec<DrawingIssue>,
) {
    for dt in layout
        .items
        .iter()
        .filter(|it| it.kind == SheetItemKind::DimensionText)
    {
        let Some(view_idx) = dt.owner_view else {
            continue;
        };
        // Find the ViewGeometry item for this exact view.
        let Some(geo) = layout
            .items
            .iter()
            .find(|it| it.kind == SheetItemKind::ViewGeometry && it.owner_view == Some(view_idx))
        else {
            continue;
        };
        if dt.bbox.intersects(&geo.bbox, DIM_TEXT_COLLISION_TOL_MM) {
            let view_name = drawing
                .views
                .get(view_idx)
                .map(|v| v.name.as_str())
                .unwrap_or("?");
            issues.push(error(
                DrawingIssueKind::DimensionOnGeometry,
                format!(
                    "view '{}': dimension '{}' text bbox overlaps the part silhouette (ISO 129: extend callouts clear of the outline)",
                    view_name,
                    dt.text.as_deref().unwrap_or("?"),
                ),
                Some(view_name.to_string()),
            ));
        }
    }
}

/// Detect orthographic views that carry visible geometry but no dimension
/// callouts (`Warning`, not `Error`).
///
/// **Rationale for Warning severity:** under the global deduplication
/// strategy, a view can legitimately carry zero dimensions when its
/// features read best from a different view (e.g. the isometric carries
/// no dims by design; a third orthographic may repeat only whole-part
/// extents already called out in `FRONT`). The drawing is usable —
/// `passed` is NOT gated on this finding — but the drafter should
/// confirm the omission is intentional rather than an oversight.
///
/// `Isometric` and `Custom` projections are skipped: isometric views are
/// never dimensioned (ISO 128-30) and custom views are caller-controlled.
fn check_undimensioned_views(drawing: &super::types::Drawing, issues: &mut Vec<DrawingIssue>) {
    for v in &drawing.views {
        // Skip non-standard projections that are never conventionally dimensioned.
        match v.projection {
            super::types::ProjectionType::Isometric
            | super::types::ProjectionType::Custom { .. } => {
                continue;
            }
            _ => {}
        }
        let has_geometry = !v.polylines.is_empty() || !v.hidden_polylines.is_empty();
        if has_geometry && v.dimensions.is_empty() {
            issues.push(warning(
                DrawingIssueKind::UndimensionedView,
                format!(
                    "view '{}' shows geometry but carries no dimension callouts — confirm omission is intentional",
                    v.name,
                ),
                Some(v.name.clone()),
            ));
        }
    }
}

/// The isometric cell overlays a shaded-solid raster with an HLR vector
/// wireframe. Those two representations are produced by two independent code
/// paths — the raster by `render::CanonicalView::Isometric` (via
/// `render::camera_basis`), the wireframe by
/// `projection::view_matrix_for_projection(Isometric)`. If those two iso
/// cameras ever drift apart, the SAME part is drawn in two orientations and the
/// reader sees it doubled and rotated. This invariant catches that drift.
///
/// **Why this closes the gap, not just the instance:** the semantic checks
/// above (labels, dimensions, sheet fit) never look at whether the raster and
/// the wireframe of the iso cell agree, which is exactly how a rotated-double
/// iso shipped. Here we PROJECT a set of asymmetric world markers (the three
/// world axes — a bore/boss offset from the part centre projects like these)
/// through BOTH iso cameras and require each marker to land in the same screen
/// QUADRANT (same corner) in both. Screen mapping: for the wireframe the SVG
/// group applies `scale(sx, −sx)`, so a larger page-Y (`v·m`) is HIGHER on the
/// sheet; the raster is rendered with +up as its top row and placed un-flipped,
/// so a larger `up·m` is likewise HIGHER — hence both map "larger vertical dot
/// ⇒ screen-up", and agreement is a per-axis sign match of
/// `(u·m, v·m)` against `(right·m, up·m)`.
///
/// Only runs when the drawing actually carries an isometric view with a shaded
/// raster (the exact configuration where the doubling is visible); a drawing
/// with no shaded iso cell has nothing to disagree.
fn check_iso_orientation_agreement(drawing: &Drawing, issues: &mut Vec<DrawingIssue>) {
    let has_shaded_iso = drawing
        .views
        .iter()
        .any(|v| matches!(v.projection, ProjectionType::Isometric) && v.shaded_raster.is_some());
    if !has_shaded_iso {
        return;
    }

    // Wireframe (HLR) iso page axes: u = page-X, v = page-Y (rows of the
    // world→view matrix; the third row / view depth is dropped).
    let vm = super::projection::view_matrix_for_projection(ProjectionType::Isometric);

    // Shaded raster iso camera SCREEN axes (right, up). Derived from the shared
    // single source. If the render module could not build a basis (it always
    // can for the canonical iso), we cannot compare — skip rather than misfire.
    let Some((right, up)) = crate::render::CanonicalView::Isometric.camera_basis() else {
        return;
    };

    if let Some((m, w, s, axis)) = iso_marker_disagreement(&vm, right, up) {
        let which = if axis == 0 { "horizontal" } else { "vertical" };
        issues.push(error(
            DrawingIssueKind::IsoOrientationMismatch,
            format!(
                "isometric cell: the shaded raster and the HLR wireframe use different iso \
                 cameras — marker ({:.0},{:.0},{:.0}) lands on opposite {which} sides \
                 (wireframe {w:+.3} vs shaded {s:+.3}); both must derive from \
                 render::camera_basis(CanonicalView::Isometric)",
                m.x, m.y, m.z
            ),
            None,
        ));
    }
}

/// First asymmetric-marker screen-QUADRANT disagreement between a wireframe
/// page basis (the rows of `vm`) and a shaded-raster screen basis `(right,
/// up)`, or `None` when every marker lands in the same corner in both. Returns
/// `(marker, wireframe_component, shaded_component, axis)` where `axis` is 0
/// (horizontal) or 1 (vertical).
///
/// Probes the three world axes: an off-centre feature (bore/boss) projects with
/// the same signs as these unit markers, so a per-axis sign mismatch means the
/// feature is in a different screen corner in the two representations.
///
/// Factored out of [`check_iso_orientation_agreement`] so the invariant is
/// directly unit-testable against a deliberately-wrong basis — proving it FIRES
/// on a mismatch without having to revert the production camera unification.
fn iso_marker_disagreement(
    vm: &crate::math::Matrix4,
    right: crate::math::Vector3,
    up: crate::math::Vector3,
) -> Option<(crate::math::Vector3, f64, f64, usize)> {
    use crate::math::{Point3, Vector3};
    // Deadzone: a projection component this close to zero carries no corner
    // information (the marker sits on that screen axis), so it cannot disagree.
    const EPS: f64 = 1e-6;
    let markers = [
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
    ];
    for m in markers {
        let page = vm.transform_point(&Point3::new(m.x, m.y, m.z));
        let wire = [page.x, page.y]; // (u·m, v·m)
        let shaded = [right.dot(&m), up.dot(&m)]; // (right·m, up·m)
        for (axis, (&w, &s)) in wire.iter().zip(shaded.iter()).enumerate() {
            if w.abs() <= EPS || s.abs() <= EPS {
                continue;
            }
            if w.signum() != s.signum() {
                return Some((m, w, s, axis));
            }
        }
    }
    None
}

// ── Standalone harness invariants (2026-08-16 drawing-quality brief) ─────────
//
// The four checks below (`find_text_collisions`, `find_ink_outside_frame`,
// `find_span_overflows`, `section_shows_only_hatch`) are pure functions over
// `SheetLayout` / `ProjectedView` that the visual harness
// (`tests/drawing_visual_harness.rs`) calls directly. They are deliberately
// NOT wired into `verify_drawing`'s `DrawingIssueKind` report: checked
// empirically against the two most elaborate pre-existing fixtures
// (`six_hole_plate`, `ring_plate` in `tests/drawing_quality_oracle.rs`),
// `find_text_collisions` and `find_span_overflows` are BOTH currently silent
// on both — so this is a deliberately conservative policy choice, not a
// response to an observed conflict. These are four brand-new invariants
// that have not been swept across this crate's 180+ drawing-adjacent test
// binaries within this change's budget; folding them into the mandatory
// `passed` gate is a real widening of what every future drawing must satisfy,
// and that call belongs to whoever owns `verify_drawing`'s broader scope,
// made deliberately rather than as a side effect of a Part-2 annotation fix.
// The checks themselves are real, reusable, unit-tested production code
// either way.

/// One pair of overlapping TEXT-bearing sheet items — the general
/// "no annotation text collides" invariant (brief Part 1, invariant #1).
///
/// Complements the existing `DimensionLabelCollision` / `ViewLabelCollision`
/// / `GdtSymbolCollision` checks wired into `verify_drawing`: those police
/// specific KNOWN pairings (ViewLabel × anything, GD&T × GD&T, dim/tag text
/// × dim/tag text). This checks EVERY pair of text-carrying items with no
/// kind restriction, so a pairing the existing checks don't cover (two
/// `NoteText` lines, a `HoleTableText` cell against a `CuttingPlaneLabel`, a
/// `ZoneRef` against a `DatumSymbol`, …) is not silently missed.
#[derive(Debug, Clone)]
pub struct TextCollision {
    pub a_kind: SheetItemKind,
    pub a_text: String,
    pub a_bbox: Rect2,
    pub b_kind: SheetItemKind,
    pub b_text: String,
    pub b_bbox: Rect2,
}

/// `SheetItemKind`s that carry glyphs — the universe [`find_text_collisions`]
/// pairs. Deliberately excludes `ViewGeometry` / `TitleBlock` /
/// `HoleTableBorder` (ink, not text) and `DatumMarker` / `ProjectionSymbol`
/// (documented, intentional adjacency — see their doc comments on
/// [`SheetItemKind`]).
fn is_text_item(kind: SheetItemKind) -> bool {
    matches!(
        kind,
        SheetItemKind::ViewLabel
            | SheetItemKind::DimensionText
            | SheetItemKind::HoleTag
            | SheetItemKind::HoleTableText
            | SheetItemKind::ZoneRef
            | SheetItemKind::NoteText
            | SheetItemKind::CuttingPlaneLabel
            | SheetItemKind::DatumSymbol
            | SheetItemKind::FcfBlock
    )
}

/// Every pair of overlapping text items on the sheet, with full coordinates
/// — a failure this returns names exactly what collided and where, per the
/// brief's "a failure that says 'invariant violated' is worthless at 3am".
pub fn find_text_collisions(layout: &super::layout::SheetLayout) -> Vec<TextCollision> {
    let items: Vec<&super::layout::SheetItem> = layout
        .items
        .iter()
        .filter(|it| is_text_item(it.kind))
        .collect();
    let mut out = Vec::new();
    for i in 0..items.len() {
        for j in (i + 1)..items.len() {
            if items[i]
                .bbox
                .intersects(&items[j].bbox, DIM_TEXT_COLLISION_TOL_MM)
            {
                out.push(TextCollision {
                    a_kind: items[i].kind,
                    a_text: items[i].text.clone().unwrap_or_default(),
                    a_bbox: items[i].bbox,
                    b_kind: items[j].kind,
                    b_text: items[j].text.clone().unwrap_or_default(),
                    b_bbox: items[j].bbox,
                });
            }
        }
    }
    out
}

/// The inner drawing frame rect for a sheet size — the same inset
/// `frame_margins` produces internally to `verify_drawing`, exposed here so
/// callers (the harness) can feed [`find_ink_outside_frame`] the exact rect
/// the renderer draws, without duplicating the margin table.
pub fn frame_rect(sheet: &super::types::SheetSize) -> Rect2 {
    let (ml, mr, mt, mb) = frame_margins(sheet);
    Rect2 {
        x0: ml,
        y0: mt,
        x1: sheet.width() - mr,
        y1: sheet.height() - mb,
    }
}

/// One sheet item whose bbox extends past the drawing frame — brief Part 1,
/// invariant #2: "all ink lies inside the frame".
#[derive(Debug, Clone)]
pub struct InkOutsideFrame {
    pub kind: SheetItemKind,
    pub text: String,
    pub bbox: Rect2,
}

/// Every layout item that extends past `frame` by more than `SLACK_MM`.
///
/// Complements `verify_drawing`'s `ViewOutsideFrame`, which only checks a
/// VIEW's own footprint (expanded by `DIM_MARGIN_MM` when dimensioned). This
/// is the superset: it walks every item `compute_layout` produced — the hole
/// table, GD&T callouts, the re-attached pictorial isometric, the
/// cutting-plane labels placed by an independent free-rect search — any of
/// which could in principle drift past the margin without tripping the
/// view-scoped check.
///
/// `SheetItemKind::ZoneRef` is deliberately EXCLUDED: per ISO 5457 the
/// zone-grid letters/numbers are furniture placed in the MARGIN, between the
/// drawn frame border and the trimmed sheet edge — that is their correct,
/// intentional position, not ink that escaped the frame. Confirmed
/// empirically: without this exclusion every zone mark on a real sheet fires
/// (found on the first run against the flange fixture, not assumed).
pub fn find_ink_outside_frame(
    layout: &super::layout::SheetLayout,
    frame: Rect2,
) -> Vec<InkOutsideFrame> {
    layout
        .items
        .iter()
        .filter(|it| it.kind != SheetItemKind::ZoneRef)
        .filter(|it| !rect_contains(&frame, &it.bbox, SLACK_MM))
        .map(|it| InkOutsideFrame {
            kind: it.kind,
            text: it.text.clone().unwrap_or_default(),
            bbox: it.bbox,
        })
        .collect()
}

/// A placed dimension whose label text is wider than the span it measures —
/// brief Part 1, invariant #3: "every dimension's text is legible at its own
/// scale". `place_dimensions` always centres a linear dimension's label ON
/// the span (never moves it outside the extension lines onto a leader), so
/// when the approximate text width exceeds the span the number will overflow
/// past its own arrows.
#[derive(Debug, Clone)]
pub struct DimensionSpanOverflow {
    pub owner_view: usize,
    pub label: String,
    pub text_width_mm: f64,
    pub span_mm: f64,
    pub text_anchor: [f64; 2],
}

/// Sub-mm slack before an overflow counts — mirrors `DIM_TEXT_COLLISION_TOL_MM`.
const SPAN_TEXT_TOL_MM: f64 = 0.5;

/// Scan every placed dimension for a label wider than its own span.
/// Point/angle callouts (`line[0] == line[1]`, zero span) are exempt — they
/// are never centred on a span in the first place, so the "outside the span
/// or on a leader" escape the brief names does not apply to them.
pub fn find_span_overflows(layout: &super::layout::SheetLayout) -> Vec<DimensionSpanOverflow> {
    let mut out = Vec::new();
    for pd in &layout.dimensions {
        let dx = pd.line[0][0] - pd.line[1][0];
        let dy = pd.line[0][1] - pd.line[1][1];
        let span = (dx * dx + dy * dy).sqrt();
        if span < 1e-6 {
            continue;
        }
        let text_w = pd.label.chars().count() as f64
            * super::layout::GLYPH_ADVANCE_EM
            * super::layout::DIM_TEXT_FONT_MM;
        if text_w > span + SPAN_TEXT_TOL_MM {
            out.push(DimensionSpanOverflow {
                owner_view: pd.owner_view,
                label: pd.label.clone(),
                text_width_mm: text_w,
                span_mm: span,
                text_anchor: pd.text_anchor,
            });
        }
    }
    out
}

/// True when a view carrying a cutting plane shows ONLY the hatched cut
/// cross-section, with no outline ink beyond it — **finding D1**
/// (2026-08-16 drawing-quality brief), deliberately left `#[ignore]`d in the
/// harness: the section-view repair is a separate pass (Part 3), not this
/// change.
///
/// `drawing/section_view.rs` derives the outline from edges of the SAME
/// triangles the hatch is clipped against (`edge_count`/`tris2d` share one
/// source), so TODAY outline bbox == hatch bbox exactly and this always
/// returns `true` for a real section — confirmed empirically against the
/// live flange fixture (Ø120×14 disc, Ø50 bore, 4×Ø12 bolts) before this
/// function was written, not assumed.
///
/// Approximation: judges "shows more than the cut" only when the outline
/// extends beyond the hatched footprint by more than one hatch pitch
/// (`section_view::HATCH_SPACING`) in some direction. A real silhouette edge
/// for material BEHIND the plane (the far bore wall, the outer profile) is
/// not confined inside the hatched triangles' own bbox, so the repaired
/// section will grow past this margin; today's cut-faces-only output cannot.
pub fn section_shows_only_hatch(view: &super::types::ProjectedView) -> bool {
    if view.hatch_polylines.is_empty() {
        return false; // not a section view (or nothing was cut) — no verdict
    }
    let bbox_of = |polys: &[super::types::Polyline2d]| -> Option<Rect2> {
        let mut b: Option<Rect2> = None;
        for p in polys {
            for pt in &p.points {
                b = Some(match b {
                    None => Rect2 {
                        x0: pt[0],
                        y0: pt[1],
                        x1: pt[0],
                        y1: pt[1],
                    },
                    Some(r) => Rect2 {
                        x0: r.x0.min(pt[0]),
                        y0: r.y0.min(pt[1]),
                        x1: r.x1.max(pt[0]),
                        y1: r.y1.max(pt[1]),
                    },
                });
            }
        }
        b
    };
    let Some(hatch_b) = bbox_of(&view.hatch_polylines) else {
        return false;
    };
    let Some(outline_b) = bbox_of(&view.polylines) else {
        return true; // hatch present, no outline at all — definitely missing
    };
    let margin = super::section_view::HATCH_SPACING;
    let grew = outline_b.x0 < hatch_b.x0 - margin
        || outline_b.x1 > hatch_b.x1 + margin
        || outline_b.y0 < hatch_b.y0 - margin
        || outline_b.y1 > hatch_b.y1 + margin;
    !grew
}

fn error(kind: DrawingIssueKind, message: String, view: Option<String>) -> DrawingIssue {
    DrawingIssue {
        severity: Severity::Error,
        kind,
        message,
        view,
    }
}

fn warning(kind: DrawingIssueKind, message: String, view: Option<String>) -> DrawingIssue {
    DrawingIssue {
        severity: Severity::Warning,
        kind,
        message,
        view,
    }
}

fn finalize(issues: Vec<DrawingIssue>, utilization: f64) -> DrawingQualityReport {
    let passed = !issues.iter().any(|i| i.severity == Severity::Error);
    DrawingQualityReport {
        passed,
        sheet_utilization: utilization,
        issues,
    }
}

#[cfg(test)]
mod iso_orientation_tests {
    use super::*;
    use crate::math::Matrix4;

    /// The LEGACY (pre-fix) hand-rolled iso wireframe basis: page-Y = (1,1,2)/√6
    /// and view depth = (1,1,−1)/√3 — a DIFFERENT octant than the shaded raster.
    /// Kept here only to prove the invariant catches exactly this drift.
    fn legacy_wireframe_matrix() -> Matrix4 {
        let s = 1.0_f64 / 2.0_f64.sqrt();
        let t = 1.0_f64 / 6.0_f64.sqrt();
        let r = 1.0_f64 / 3.0_f64.sqrt();
        Matrix4::new(
            s,
            -s,
            0.0,
            0.0, // u (page-X)
            t,
            t,
            2.0 * t,
            0.0, // v (page-Y) — the defect: (1,1,2)/√6
            r,
            r,
            -r,
            0.0, // w (view depth) — the defect: (1,1,−1)/√3
            0.0,
            0.0,
            0.0,
            1.0,
        )
    }

    /// GREEN: the SHIPPED (unified) iso wireframe basis agrees with the shaded
    /// raster camera on every marker's screen corner — no disagreement.
    #[test]
    fn unified_iso_bases_agree() {
        let vm = super::super::projection::view_matrix_for_projection(ProjectionType::Isometric);
        let (right, up) = crate::render::CanonicalView::Isometric
            .camera_basis()
            .expect("canonical iso basis exists");
        assert!(
            iso_marker_disagreement(&vm, right, up).is_none(),
            "the unified iso wireframe and shaded-raster cameras must place every \
             marker in the same screen corner"
        );
    }

    /// RED / mutation proof (self-contained): feed the invariant the LEGACY
    /// wireframe basis and it must fire — the disagreement is on the VERTICAL
    /// axis (a +X marker projects UP in the legacy wireframe but DOWN in the
    /// shaded raster). This is the exact doubled/rotated-iso defect.
    #[test]
    fn legacy_iso_basis_is_caught() {
        let (right, up) = crate::render::CanonicalView::Isometric
            .camera_basis()
            .expect("canonical iso basis exists");
        let disagreement = iso_marker_disagreement(&legacy_wireframe_matrix(), right, up)
            .expect("legacy iso basis MUST be caught as a mismatch");
        let (_marker, wire, shaded, axis) = disagreement;
        assert_eq!(axis, 1, "the legacy defect is a vertical (screen-Y) flip");
        assert!(
            wire.signum() != shaded.signum(),
            "the marker lands on opposite vertical sides ({wire:+.3} vs {shaded:+.3})"
        );
    }

    /// End-to-end: an auto sheet's shaded isometric cell passes the orientation
    /// invariant, and the report carries no IsoOrientationMismatch.
    #[test]
    fn auto_sheet_iso_cell_orientation_agrees() {
        use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

        let mut model = BRepModel::new();
        let sid = match TopologyBuilder::new(&mut model)
            .create_box_3d(40.0, 30.0, 20.0)
            .expect("box")
        {
            GeometryId::Solid(s) => s,
            o => panic!("{o:?}"),
        };
        let drawing =
            super::super::dimensioning::standard_drawing_auto(&model, sid, uuid::Uuid::nil())
                .expect("auto sheet");
        assert!(
            drawing
                .views
                .iter()
                .any(|v| matches!(v.projection, ProjectionType::Isometric)
                    && v.shaded_raster.is_some()),
            "fixture must carry a shaded iso cell (else the invariant is vacuous)"
        );

        let report = verify_drawing(&drawing);
        assert!(
            !report.has(DrawingIssueKind::IsoOrientationMismatch),
            "shipped pipeline must not report an iso-orientation mismatch: {:?}",
            report.issues
        );
    }

    /// The invariant is silent when there is no shaded iso cell to disagree.
    #[test]
    fn no_shaded_iso_no_check() {
        let mut issues = Vec::new();
        let drawing = Drawing::new("t", super::super::types::SheetSize::A3);
        check_iso_orientation_agreement(&drawing, &mut issues);
        assert!(issues.is_empty(), "no iso cell ⇒ nothing to check");
    }
}

// ── Standalone harness invariant tests (2026-08-16 drawing-quality) ──────────
//
// Detector-proof specimens: each test builds a deliberately bad input and
// proves the check fires, plus a clean input and proves it stays silent.
// Mutation proof for each: return `Vec::new()` / `false` unconditionally from
// the function under test → the "must fire" half of the pair goes RED.
#[cfg(test)]
mod harness_invariant_tests {
    use super::*;
    use crate::drawing::layout::{ArrowSpec, PlacedDimension, SheetItem, SheetLayout};
    use crate::drawing::types::{
        Polyline2d, ProjectedView, ProjectedViewId, ProjectionType, ViewExtent, ViewSource,
    };

    fn empty_layout() -> SheetLayout {
        SheetLayout {
            sheet: Rect2 {
                x0: 0.0,
                y0: 0.0,
                x1: 420.0,
                y1: 297.0,
            },
            items: Vec::new(),
            dimensions: Vec::new(),
            hole_tags: Vec::new(),
        }
    }

    fn text_item(kind: SheetItemKind, text: &str, bbox: Rect2) -> SheetItem {
        SheetItem {
            kind,
            bbox,
            owner_view: None,
            text: Some(text.to_string()),
        }
    }

    // ── find_text_collisions ──────────────────────────────────────────────

    /// Two `NoteText` lines overlapping — a pairing the EXISTING wired checks
    /// (`check_dimension_label_collisions`'s DimensionText/HoleTag/
    /// CuttingPlaneLabel trio, and the ViewLabel/GD&T pairing in
    /// `verify_drawing`) do not cover, since neither is a ViewLabel and
    /// neither is DatumSymbol/FcfBlock. `find_text_collisions` must still
    /// catch it.
    #[test]
    fn note_text_pair_overlap_detected() {
        let mut layout = empty_layout();
        let a = Rect2 {
            x0: 10.0,
            y0: 10.0,
            x1: 40.0,
            y1: 14.0,
        };
        let b = Rect2 {
            x0: 20.0,
            y0: 10.0,
            x1: 50.0,
            y1: 14.0,
        };
        layout
            .items
            .push(text_item(SheetItemKind::NoteText, "note one", a));
        layout
            .items
            .push(text_item(SheetItemKind::NoteText, "note two", b));
        let hits = find_text_collisions(&layout);
        assert_eq!(hits.len(), 1, "overlapping NoteText pair must be caught");
        assert_eq!(hits[0].a_text, "note one");
        assert_eq!(hits[0].b_text, "note two");
    }

    /// Two non-overlapping text items produce no collision.
    #[test]
    fn non_overlapping_text_items_produce_no_collision() {
        let mut layout = empty_layout();
        layout.items.push(text_item(
            SheetItemKind::NoteText,
            "note one",
            Rect2 {
                x0: 10.0,
                y0: 10.0,
                x1: 20.0,
                y1: 14.0,
            },
        ));
        layout.items.push(text_item(
            SheetItemKind::NoteText,
            "note two",
            Rect2 {
                x0: 100.0,
                y0: 100.0,
                x1: 110.0,
                y1: 104.0,
            },
        ));
        assert!(find_text_collisions(&layout).is_empty());
    }

    /// `DatumMarker` and `ProjectionSymbol` are deliberately excluded from the
    /// text-collision universe (documented, intentional adjacency on
    /// `SheetItemKind`) — an overlap involving either must NOT be reported.
    #[test]
    fn datum_marker_and_projection_symbol_excluded() {
        let mut layout = empty_layout();
        let shared = Rect2 {
            x0: 0.0,
            y0: 0.0,
            x1: 5.0,
            y1: 5.0,
        };
        layout.items.push(SheetItem {
            kind: SheetItemKind::DatumMarker,
            bbox: shared,
            owner_view: None,
            text: Some("0,0".to_string()),
        });
        layout.items.push(SheetItem {
            kind: SheetItemKind::ProjectionSymbol,
            bbox: shared,
            owner_view: None,
            text: None,
        });
        assert!(find_text_collisions(&layout).is_empty());
    }

    // ── find_ink_outside_frame ─────────────────────────────────────────────

    /// An item entirely within the frame produces no finding.
    #[test]
    fn ink_inside_frame_not_flagged() {
        let mut layout = empty_layout();
        layout.items.push(text_item(
            SheetItemKind::ViewLabel,
            "FRONT (1:1)",
            Rect2 {
                x0: 50.0,
                y0: 50.0,
                x1: 90.0,
                y1: 55.0,
            },
        ));
        let frame = Rect2 {
            x0: 10.0,
            y0: 10.0,
            x1: 400.0,
            y1: 280.0,
        };
        assert!(find_ink_outside_frame(&layout, frame).is_empty());
    }

    /// An item that pokes past the frame's right edge must be flagged, with
    /// coordinates.
    #[test]
    fn ink_past_frame_edge_flagged() {
        let mut layout = empty_layout();
        layout.items.push(text_item(
            SheetItemKind::CuttingPlaneLabel,
            "A",
            Rect2 {
                x0: 395.0,
                y0: 50.0,
                x1: 410.0,
                y1: 55.0,
            },
        ));
        let frame = Rect2 {
            x0: 10.0,
            y0: 10.0,
            x1: 400.0,
            y1: 280.0,
        };
        let hits = find_ink_outside_frame(&layout, frame);
        assert_eq!(
            hits.len(),
            1,
            "item crossing the frame edge must be flagged"
        );
        assert_eq!(hits[0].text, "A");
        assert!(
            hits[0].bbox.x1 > frame.x1,
            "reported bbox must carry the overrun coordinate"
        );
    }

    /// `frame_rect` produces a rect strictly inside the sheet bounds for a
    /// real sheet size (non-vacuous: margins are non-zero).
    #[test]
    fn frame_rect_is_inset_from_sheet() {
        use crate::drawing::types::SheetSize;
        let r = frame_rect(&SheetSize::A3);
        assert!(r.x0 > 0.0 && r.y0 > 0.0);
        assert!(r.x1 < SheetSize::A3.width());
        assert!(r.y1 < SheetSize::A3.height());
    }

    // ── find_span_overflows ────────────────────────────────────────────────

    fn placed_dim(label: &str, span: f64) -> PlacedDimension {
        PlacedDimension {
            line: [[0.0, 0.0], [span, 0.0]],
            ext: [[[0.0, 0.0], [0.0, 0.0]], [[0.0, 0.0], [0.0, 0.0]]],
            arrows: [
                ArrowSpec {
                    tip: [0.0, 0.0],
                    dir: [1.0, 0.0],
                },
                ArrowSpec {
                    tip: [span, 0.0],
                    dir: [-1.0, 0.0],
                },
            ],
            text_anchor: [span * 0.5, -1.4],
            text_rot_deg: 0.0,
            label: label.to_string(),
            owner_view: 0,
        }
    }

    /// A long label ("Ø120.00") on a 1 mm span is unreadable — the text is
    /// centred ON a span far narrower than the glyphs themselves.
    #[test]
    fn narrow_span_wide_label_flagged() {
        let mut layout = empty_layout();
        layout.dimensions.push(placed_dim("\u{00d8}120.00", 1.0));
        let overflows = find_span_overflows(&layout);
        assert_eq!(overflows.len(), 1, "narrow-span wide label must be flagged");
        assert!(overflows[0].text_width_mm > overflows[0].span_mm);
    }

    /// A comfortably wide span (90 mm) for a short label ("120.00") never
    /// overflows.
    #[test]
    fn comfortable_span_not_flagged() {
        let mut layout = empty_layout();
        layout.dimensions.push(placed_dim("120.00", 90.0));
        assert!(find_span_overflows(&layout).is_empty());
    }

    /// Degenerate (point/angle) callouts — `line[0] == line[1]`, zero span —
    /// are exempt: `place_dimensions` never centres them ON a span.
    #[test]
    fn degenerate_point_callout_exempt() {
        let mut layout = empty_layout();
        let mut pd = placed_dim("45.0\u{00b0}", 0.0);
        pd.line = [[10.0, 10.0], [10.0, 10.0]];
        layout.dimensions.push(pd);
        assert!(find_span_overflows(&layout).is_empty());
    }

    // ── section_shows_only_hatch ───────────────────────────────────────────

    fn view_with(polylines: Vec<Polyline2d>, hatch: Vec<Polyline2d>) -> ProjectedView {
        ProjectedView {
            id: ProjectedViewId::new(),
            name: "SECTION A-A".to_string(),
            projection: ProjectionType::Custom {
                rotation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            },
            source: ViewSource::Part {
                part_id: uuid::Uuid::nil(),
                solid_id: 0,
            },
            position_mm: [0.0, 0.0],
            scale: 1.0,
            polylines,
            extent: ViewExtent {
                min_x: 0.0,
                min_y: 0.0,
                max_x: 10.0,
                max_y: 10.0,
            },
            dimensions: Vec::new(),
            centerlines: Vec::new(),
            hidden_polylines: Vec::new(),
            circles: Vec::new(),
            hidden_circles: Vec::new(),
            shaded_raster: None,
            hatch_polylines: hatch,
            polyline_sources: Vec::new(),
        }
    }

    /// TODAY's real `section_view.rs` shape: outline == boundary of the SAME
    /// triangles the hatch is clipped against, so outline bbox == hatch bbox.
    /// `section_shows_only_hatch` must return `true` (the D1 defect).
    #[test]
    fn outline_matching_hatch_bbox_is_flagged() {
        let outline = Polyline2d::from_points(vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]);
        let hatch = Polyline2d::from_points(vec![[1.0, 1.0], [9.0, 9.0]]);
        let view = view_with(vec![outline], vec![hatch]);
        assert!(
            section_shows_only_hatch(&view),
            "outline confined to the hatched footprint must be flagged as cut-faces-only"
        );
    }

    /// Once a repaired section adds silhouette ink for material BEHIND the
    /// plane (extending well past the hatched triangles' own bbox), the
    /// check must go quiet.
    #[test]
    fn outline_extending_past_hatch_bbox_is_not_flagged() {
        let outline = Polyline2d::from_points(vec![[0.0, 0.0], [10.0, 0.0], [10.0, 50.0]]);
        let hatch = Polyline2d::from_points(vec![[1.0, 1.0], [9.0, 9.0]]);
        let view = view_with(vec![outline], vec![hatch]);
        assert!(
            !section_shows_only_hatch(&view),
            "outline ink well beyond the hatched footprint must NOT be flagged"
        );
    }

    /// A non-section view (no hatch at all) is not judged — the check would
    /// otherwise misfire on every ordinary orthographic view.
    #[test]
    fn no_hatch_no_verdict() {
        let outline = Polyline2d::from_points(vec![[0.0, 0.0], [10.0, 0.0]]);
        let view = view_with(vec![outline], Vec::new());
        assert!(!section_shows_only_hatch(&view));
    }

    /// LIVE PIPELINE PROOF: the real `standard_drawing_auto` flange fixture's
    /// SECTION A-A view — the exact shape finding D1 describes — must be
    /// flagged by `section_shows_only_hatch` TODAY. This is the harness's own
    /// self-check that the approximation is not accidentally green (advisor
    /// caution: a staged invariant that is already green is worse than none).
    #[test]
    fn flange_section_view_is_flagged_today() {
        use crate::math::{Point3, Vector3};
        use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
        use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

        let mut m = BRepModel::new();
        let disc = match TopologyBuilder::new(&mut m)
            .create_cylinder_3d(
                Point3::new(0.0, 0.0, 0.0),
                Vector3::new(0.0, 0.0, 1.0),
                60.0,
                14.0,
            )
            .expect("disc")
        {
            GeometryId::Solid(s) => s,
            o => panic!("{o:?}"),
        };
        let bore = match TopologyBuilder::new(&mut m)
            .create_cylinder_3d(
                Point3::new(0.0, 0.0, -5.0),
                Vector3::new(0.0, 0.0, 1.0),
                25.0,
                24.0,
            )
            .expect("bore")
        {
            GeometryId::Solid(s) => s,
            o => panic!("{o:?}"),
        };
        let mut cur = boolean_operation(
            &mut m,
            disc,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("bore cut");
        for (x, y) in [(45.0, 0.0), (-45.0, 0.0), (0.0, 45.0), (0.0, -45.0)] {
            let hole = match TopologyBuilder::new(&mut m)
                .create_cylinder_3d(
                    Point3::new(x, y, -5.0),
                    Vector3::new(0.0, 0.0, 1.0),
                    6.0,
                    24.0,
                )
                .expect("hole")
            {
                GeometryId::Solid(s) => s,
                o => panic!("{o:?}"),
            };
            cur = boolean_operation(
                &mut m,
                cur,
                hole,
                BooleanOp::Difference,
                BooleanOptions::default(),
            )
            .expect("bolt hole cut");
        }
        let drawing =
            crate::drawing::dimensioning::standard_drawing_auto(&m, cur, uuid::Uuid::nil())
                .expect("auto sheet");
        let section = drawing
            .views
            .iter()
            .find(|v| v.name == "SECTION A-A")
            .expect("flange sheet must carry a SECTION A-A view");
        assert!(
            !section.hatch_polylines.is_empty(),
            "specimen must actually be a hatched section, else this proves nothing"
        );
        assert!(
            section_shows_only_hatch(section),
            "D1 is unresolved today — the live section view must still read as \
             cut-faces-only; this test flips to failing the moment the Part 3 \
             section-view repair lands, which is the signal to un-ignore the \
             harness's own D1 assertion"
        );
    }
}
