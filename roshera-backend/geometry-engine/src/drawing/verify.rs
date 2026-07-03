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
    /// The same dimension (same quantized value, same orientation, same
    /// measured interval) appears more than once on the sheet, making the
    /// drawing redundant and potentially misleading.
    RedundantDimension,
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
const DIM_MARGIN_MM: f64 = 22.0;
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

    // ── ViewLabelCollision detection ─────────────────────────────────────
    // Check every (ViewLabel, *) pair for overlap using the layout already
    // computed above. At least one of the two items must be a ViewLabel —
    // dim-text pairs fall under the existing DimensionLabelCollision kind.
    let texts: Vec<&super::layout::SheetItem> = layout
        .items
        .iter()
        .filter(|it| {
            matches!(
                it.kind,
                SheetItemKind::ViewLabel | SheetItemKind::DimensionText
            )
        })
        .collect();
    for i in 0..texts.len() {
        for j in (i + 1)..texts.len() {
            let pair_has_label = texts[i].kind == SheetItemKind::ViewLabel
                || texts[j].kind == SheetItemKind::ViewLabel;
            if pair_has_label && texts[i].bbox.intersects(&texts[j].bbox, 0.2) {
                issues.push(error(
                    DrawingIssueKind::ViewLabelCollision,
                    format!(
                        "text '{}' collides with '{}'",
                        texts[i].text.as_deref().unwrap_or("?"),
                        texts[j].text.as_deref().unwrap_or("?")
                    ),
                    None,
                ));
            }
        }
    }

    // ── RedundantDimension detection ─────────────────────────────────────
    check_redundant_dimensions(drawing, h, &mut issues);

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
/// 2. **Same-view same-interval**: within one view, two dimensions with the
///    same orientation (H or V) have interval endpoints that coincide within
///    0.5 mm in sheet space. This catches "10.00 plate-thickness + 10.00
///    bore-length both stacked on the same vertical interval in FRONT".
fn check_redundant_dimensions(drawing: &Drawing, sheet_h: f64, issues: &mut Vec<DrawingIssue>) {
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
