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

use serde::{Deserialize, Serialize};

use super::svg::{frame_margins, title_block_size};
use super::types::{Drawing, ProjectedView, ProjectionType};

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

// ── Internal rectangle (SVG coords, y down) ─────────────────────────

#[derive(Clone, Copy)]
struct Rect {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Rect {
    fn w(&self) -> f64 {
        (self.x1 - self.x0).max(0.0)
    }
    fn h(&self) -> f64 {
        (self.y1 - self.y0).max(0.0)
    }
    fn area(&self) -> f64 {
        self.w() * self.h()
    }
    fn cx(&self) -> f64 {
        0.5 * (self.x0 + self.x1)
    }
    fn cy(&self) -> f64 {
        0.5 * (self.y0 + self.y1)
    }

    /// Overlap with positive interior area beyond `tol` mm of slack.
    fn intersects(&self, o: &Rect, tol: f64) -> bool {
        self.x0 < o.x1 - tol && o.x0 < self.x1 - tol && self.y0 < o.y1 - tol && o.y0 < self.y1 - tol
    }

    /// Is `o` fully inside `self` (allowing `tol` mm of overhang)?
    fn contains_rect(&self, o: &Rect, tol: f64) -> bool {
        o.x0 >= self.x0 - tol
            && o.x1 <= self.x1 + tol
            && o.y0 >= self.y0 - tol
            && o.y1 <= self.y1 + tol
    }

    /// Signed clearance of a point from the rectangle: positive = outside
    /// distance to the nearest edge, negative = penetration depth inside.
    fn point_clearance(&self, x: f64, y: f64) -> f64 {
        let dx = (self.x0 - x).max(x - self.x1).max(0.0);
        let dy = (self.y0 - y).max(y - self.y1).max(0.0);
        if dx == 0.0 && dy == 0.0 {
            // Inside — return the (negative) distance to the closest edge.
            let inside = (x - self.x0)
                .min(self.x1 - x)
                .min(y - self.y0)
                .min(self.y1 - y);
            -inside
        } else {
            (dx * dx + dy * dy).sqrt()
        }
    }
}

/// Map a view-space point to SVG sheet coordinates.
fn to_sheet(view: &ProjectedView, sheet_h: f64, p: [f64; 2]) -> [f64; 2] {
    [
        view.position_mm[0] + p[0] * view.scale,
        (sheet_h - view.position_mm[1]) - p[1] * view.scale,
    ]
}

/// Bounding rectangle (sheet coords) of a view's drawn edges — visible +
/// hidden polylines, falling back to the stored `extent` corners when a
/// view has no polylines yet.
fn view_geometry_rect(view: &ProjectedView, sheet_h: f64) -> Option<Rect> {
    let mut x0 = f64::INFINITY;
    let mut y0 = f64::INFINITY;
    let mut x1 = f64::NEG_INFINITY;
    let mut y1 = f64::NEG_INFINITY;
    let mut any = false;
    let mut fold = |p: [f64; 2]| {
        let s = to_sheet(view, sheet_h, p);
        x0 = x0.min(s[0]);
        x1 = x1.max(s[0]);
        y0 = y0.min(s[1]);
        y1 = y1.max(s[1]);
    };
    for pl in view.polylines.iter().chain(view.hidden_polylines.iter()) {
        for &p in &pl.points {
            fold(p);
            any = true;
        }
    }
    if !any && !view.extent.is_empty() {
        fold([view.extent.min_x, view.extent.min_y]);
        fold([view.extent.max_x, view.extent.max_y]);
        any = true;
    }
    if any && x1 > x0 && y1 > y0 {
        Some(Rect { x0, y0, x1, y1 })
    } else {
        None
    }
}

// ── Tunables (all millimetres) ──────────────────────────────────────

/// Slack allowed before two rects count as overlapping / out of bounds.
const SLACK_MM: f64 = 0.5;
/// A dimension's label centre must clear the silhouette by at least this.
const DIM_MIN_STANDOFF_MM: f64 = 3.0;
/// Centre-alignment tolerance for the third-angle arrangement.
const ALIGN_TOL_MM: f64 = 2.0;
/// Below this fraction of the printable area, the sheet reads as empty.
const MIN_UTILIZATION: f64 = 0.10;
/// Approximate width of one label glyph at the 3.6 mm SVG label font.
const LABEL_CHAR_W_MM: f64 = 2.0;
const LABEL_H_MM: f64 = 3.6;

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
    let (ml, mr, mt, mb) = frame_margins(&drawing.sheet_size);
    let (tb_w, tb_h) = title_block_size(&drawing.sheet_size);
    let frame = Rect {
        x0: ml,
        y0: mt,
        x1: w - mr,
        y1: h - mb,
    };
    let title_block = Rect {
        x0: frame.x1 - tb_w,
        y0: frame.y1 - tb_h,
        x1: frame.x1,
        y1: frame.y1,
    };

    let mut rects: Vec<(String, Rect)> = Vec::new();
    let mut ink_area = 0.0;

    for v in &drawing.views {
        let name = v.name.clone();

        if v.polylines.is_empty() && v.hidden_polylines.is_empty() {
            issues.push(warning(
                DrawingIssueKind::EmptyView,
                format!("view '{name}' projected to no edges"),
                Some(name.clone()),
            ));
        } else if v.dimensions.is_empty() {
            issues.push(warning(
                DrawingIssueKind::UndimensionedView,
                format!("view '{name}' carries no dimensions"),
                Some(name.clone()),
            ));
        }

        if let Some(r) = view_geometry_rect(v, h) {
            ink_area += r.area();
            if !frame.contains_rect(&r, SLACK_MM) {
                issues.push(error(
                    DrawingIssueKind::ViewOutsideFrame,
                    format!("view '{name}' extends past the drawing frame / margins"),
                    Some(name.clone()),
                ));
            }
            if r.intersects(&title_block, SLACK_MM) {
                issues.push(error(
                    DrawingIssueKind::ViewOverlapsTitleBlock,
                    format!("view '{name}' overlaps the title block"),
                    Some(name.clone()),
                ));
            }
            check_dimensions(v, h, &r, &mut issues);
            rects.push((name, r));
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

    check_alignment(drawing, h, &mut issues);

    finalize(issues, utilization)
}

/// Per-view dimension checks: each callout must stand clear of the
/// silhouette, and labels must not collide with each other.
fn check_dimensions(
    view: &ProjectedView,
    sheet_h: f64,
    geom: &Rect,
    issues: &mut Vec<DrawingIssue>,
) {
    let mut boxes: Vec<(String, Rect)> = Vec::new();
    for d in &view.dimensions {
        let mid = [0.5 * (d.a[0] + d.b[0]), 0.5 * (d.a[1] + d.b[1])];
        let s = to_sheet(view, sheet_h, mid);

        if geom.point_clearance(s[0], s[1]) < DIM_MIN_STANDOFF_MM {
            issues.push(warning(
                DrawingIssueKind::DimensionOnGeometry,
                format!(
                    "dimension '{}' sits on the part outline (no offset / extension line)",
                    d.label
                ),
                Some(view.name.clone()),
            ));
        }

        let half_w = 0.5 * (d.label.chars().count() as f64) * LABEL_CHAR_W_MM;
        let lbox = Rect {
            x0: s[0] - half_w,
            y0: s[1] - 0.5 * LABEL_H_MM,
            x1: s[0] + half_w,
            y1: s[1] + 0.5 * LABEL_H_MM,
        };
        for (plabel, pbox) in &boxes {
            if lbox.intersects(pbox, 0.0) {
                issues.push(warning(
                    DrawingIssueKind::DimensionLabelCollision,
                    format!("dimension labels '{plabel}' and '{}' overlap", d.label),
                    Some(view.name.clone()),
                ));
            }
        }
        boxes.push((d.label.clone(), lbox));
    }
}

/// Third-angle arrangement: Top directly above Front (shared x-centre),
/// Right directly beside Front (shared y-centre).
fn check_alignment(drawing: &Drawing, sheet_h: f64, issues: &mut Vec<DrawingIssue>) {
    let rect_of = |want: fn(&ProjectionType) -> bool| -> Option<Rect> {
        drawing
            .views
            .iter()
            .find(|v| want(&v.projection))
            .and_then(|v| view_geometry_rect(v, sheet_h))
    };
    let front = rect_of(|p| matches!(p, ProjectionType::Front));
    let top = rect_of(|p| matches!(p, ProjectionType::Top));
    let right = rect_of(|p| matches!(p, ProjectionType::Right));

    if let (Some(f), Some(t)) = (front, top) {
        if (f.cx() - t.cx()).abs() > ALIGN_TOL_MM {
            issues.push(warning(
                DrawingIssueKind::ProjectionMisaligned,
                "Top view is not vertically aligned over the Front view (third-angle)".to_string(),
                None,
            ));
        }
    }
    if let (Some(f), Some(r)) = (front, right) {
        if (f.cy() - r.cy()).abs() > ALIGN_TOL_MM {
            issues.push(warning(
                DrawingIssueKind::ProjectionMisaligned,
                "Right view is not horizontally aligned with the Front view (third-angle)"
                    .to_string(),
                None,
            ));
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
