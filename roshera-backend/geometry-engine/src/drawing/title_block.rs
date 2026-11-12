//! The title block: an ISO 7200 document record, as ONE cell model.
//!
//! Every renderer (SVG, DXF) inks the cells this module lays out, and the
//! sheet layout reads its projection-symbol rect from here. Before, the cell
//! ratios were copied into `svg.rs`, `dxf.rs` and `layout.rs`, and the copies
//! drifted: the projection symbol landed on the SCALE label.
//!
//! Every value is one of three kinds, and never a fourth:
//! - typed by a person or an agent (title, creator, approver, material, ...);
//! - measured by the kernel at render time ([`RenderFacts`]: mass from the
//!   part's volume and a NAMED material's density; the sheet certificate's
//!   verdict and digest);
//! - a dash, when neither exists.
//!
//! Layout (A3 shown; every row scales with the block height):
//!
//! ```text
//!   ┌──────────────────────┬─────────────────┬──────────────┐
//!   │ GENERAL TOLERANCES   │ PROJECTION  ⊏◎  │ SCALE        │  band
//!   ├──────┬───────────────┴─────────────────┼──────────────┤
//!   │ LOGO │ TITLE (+ supplementary title)   │ DRAWING NO.  │  row 1
//!   ├──────┴──┬─────────┬──────┬─────────┬───┴──┬───────────┤
//!   │ CREATED │APPROVED │ DATE │MATERIAL │ MASS │ UNITS     │  row 2
//!   ├─────────┼─────────┼──────┴──┬──────┴┬─────┴┬────┬─────┤
//!   │ STATUS  │ DOC TYPE│VERIFIED │ SHEET │ REV  │SIZE│     │  row 3
//!   └─────────┴─────────┴─────────┴───────┴──────┴────┴─────┘
//! ```

use sha2::{Digest, Sha256};

use super::layout::{Rect2, GLYPH_ADVANCE_EM};
use super::sheet_certificate::certify_drawing;
use super::svg::{format_scale, frame_margins, title_block_size};
use super::types::{Drawing, ProjectionAngle, RenderFacts, SheetStamp, ViewSource};
use crate::primitives::topology_builder::BRepModel;

/// Label lettering (mm): the small caps tag at a cell's top-left.
pub const LABEL_FONT_MM: f64 = 2.4;
/// Field value lettering (mm) and the smallest it may step down to.
pub const VALUE_FONT_MM: f64 = 3.6;
pub const VALUE_MIN_FONT_MM: f64 = 1.8;
/// Title headline lettering (mm) -- ISO 7200 large lettering.
pub const TITLE_FONT_MM: f64 = 7.0;
/// Drawing-number headline lettering (mm, monospace).
pub const DRWG_NO_FONT_MM: f64 = 5.4;
/// Inner padding (mm) between a cell's border and its text.
pub const PAD_MM: f64 = 1.6;
/// Baseline of the label below the cell top (mm).
const LABEL_BASELINE_MM: f64 = 2.9;
/// The value may not rise above this (mm below the cell top): clear of the label.
const VALUE_TOP_CLEAR_MM: f64 = 3.3;
/// Average advance of a monospace glyph, as a fraction of the font size.
const MONO_ADVANCE_EM: f64 = 0.6;

/// How a cell's value is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellStyle {
    /// Left-aligned field value.
    Field,
    /// Left-aligned, bold: the status.
    Strong,
    /// Centred headline (the title).
    Headline,
    /// Centred monospace headline (the drawing number).
    Mono,
}

/// One labelled cell of the title block.
#[derive(Debug, Clone)]
pub struct TbCell {
    /// Stable key, e.g. `"material"`.
    pub key: &'static str,
    /// The label inked at the top-left, e.g. `"MATERIAL"`.
    pub label: &'static str,
    /// The value as inked: already fitted (font stepped down, then clipped
    /// with an ellipsis only below [`VALUE_MIN_FONT_MM`]).
    pub value: String,
    /// The cell border, sheet millimetres, y down.
    pub rect: Rect2,
    pub style: CellStyle,
    /// The size the value is set at (mm).
    pub value_font_mm: f64,
    /// The value's anchor: its start (left styles) or centre (headlines).
    pub value_x: f64,
    /// The value's baseline.
    pub value_baseline: f64,
    /// A second, smaller line under the value (the supplementary title).
    pub sub: Option<SubLine>,
    /// Horizontal room the value was fitted into (the cell less padding and
    /// anything else drawn in it).
    pub value_room_mm: f64,
}

/// A smaller second line inside a cell.
#[derive(Debug, Clone)]
pub struct SubLine {
    pub text: String,
    pub font_mm: f64,
    pub x: f64,
    pub baseline: f64,
}

impl TbCell {
    fn advance_em(&self) -> f64 {
        match self.style {
            CellStyle::Mono => MONO_ADVANCE_EM,
            _ => GLYPH_ADVANCE_EM,
        }
    }

    /// Box of the label text.
    pub fn label_box(&self) -> Rect2 {
        let x = self.rect.x0 + PAD_MM;
        let y = self.rect.y0 + LABEL_BASELINE_MM;
        let w = self.label.chars().count() as f64 * GLYPH_ADVANCE_EM * LABEL_FONT_MM;
        Rect2 {
            x0: x,
            y0: y - LABEL_FONT_MM,
            x1: x + w,
            y1: y,
        }
    }

    /// Box of the value text as inked.
    pub fn value_box(&self) -> Rect2 {
        let w = self.value.chars().count() as f64 * self.advance_em() * self.value_font_mm;
        let x0 = match self.style {
            CellStyle::Headline | CellStyle::Mono => self.value_x - 0.5 * w,
            CellStyle::Field | CellStyle::Strong => self.value_x,
        };
        Rect2 {
            x0,
            y0: self.value_baseline - self.value_font_mm,
            x1: x0 + w,
            y1: self.value_baseline,
        }
    }
}

/// The whole block: outer border, cells, the logo box and the projection
/// symbol's box.
#[derive(Debug, Clone)]
pub struct TitleBlockLayout {
    pub outer: Rect2,
    pub cells: Vec<TbCell>,
    /// The owner's mark (ISO 7200 "legal owner").
    pub logo: Rect2,
    /// Where the projection symbol is drawn: inside the PROJECTION cell,
    /// right-aligned, clear of its label and value.
    pub symbol: Rect2,
}

impl TitleBlockLayout {
    /// The cell with `key`, if laid out.
    pub fn cell(&self, key: &str) -> Option<&TbCell> {
        self.cells.iter().find(|c| c.key == key)
    }
}

/// Fit `text` into `room` mm at `font` (stepping down to `min`), then clip
/// with an ellipsis. Returns the inked string and its font.
fn fit(text: &str, room: f64, font: f64, min: f64, em: f64) -> (String, f64) {
    let room = room.max(0.0);
    let chars = text.chars().count().max(1) as f64;
    if chars * font * em <= room {
        return (text.to_string(), font);
    }
    let fitted = room / (chars * em);
    if fitted >= min {
        return (text.to_string(), fitted);
    }
    let max_chars = (room / (min * em)).floor() as usize;
    let mut clipped: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    clipped.push('\u{2026}');
    (clipped, min)
}

fn dash(value: &str) -> String {
    if value.trim().is_empty() {
        "\u{2014}".to_string()
    } else {
        value.trim().to_string()
    }
}

/// The drawing number as stated: the override, else `RSH-` + the first 8
/// characters of the drawing id.
pub fn drawing_number(drawing: &Drawing) -> String {
    drawing
        .title_block
        .drawing_number
        .as_ref()
        .filter(|s| !s.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| {
            let id: String = drawing.id.0.to_string().chars().take(8).collect();
            format!("RSH-{}", id.to_uppercase())
        })
}

/// The document status, derived -- never stored, so it cannot be set
/// without an approver: `FOR REVIEW` until a person approves, then `APPROVED`.
pub fn status(drawing: &Drawing) -> &'static str {
    if drawing.title_block.approved_by.trim().is_empty() {
        "FOR REVIEW"
    } else {
        "APPROVED"
    }
}

/// `"SOUND 1a2b3c4d"`, `"NOT SOUND"`, or `"UNVERIFIED"` (no certificate was
/// computed for this render).
pub fn verification_label(drawing: &Drawing) -> String {
    match &drawing.render_facts.verification {
        Some(s) if s.sound => format!("SOUND {}", s.digest),
        Some(_) => "NOT SOUND".to_string(),
        None => "UNVERIFIED".to_string(),
    }
}

/// Mass as inked: `"0.64 kg"`, grams under 10 g, a dash when unknown.
pub fn mass_label(drawing: &Drawing) -> String {
    match drawing.render_facts.mass_kg {
        Some(kg) if kg >= 0.01 => format!("{kg:.2} kg"),
        Some(kg) => format!("{:.1} g", kg * 1000.0),
        None => "\u{2014}".to_string(),
    }
}

fn tolerance_label(drawing: &Drawing) -> String {
    let t = &drawing.general_tolerance;
    if !t.standard.trim().is_empty() {
        return t.standard.trim().to_string();
    }
    let unit = drawing.document_unit;
    format!(
        "\u{00B1}{} {}, \u{00B1}{}\u{00B0}",
        trimmed(t.linear_mm / unit.per_mm().max(1e-12)),
        unit.suffix(),
        trimmed(t.angular_deg)
    )
}

/// A number with trailing zeros dropped: 0.004, 0.5, 2.
fn trimmed(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

/// Lay out the title block of `drawing` at its place on the sheet: the
/// bottom-right corner of the frame.
pub fn title_block_layout(drawing: &Drawing) -> TitleBlockLayout {
    let sheet = &drawing.sheet_size;
    let (w_sheet, h_sheet) = (sheet.width(), sheet.height());
    let (ml, mr, mt, mb) = frame_margins(sheet);
    let (w, h) = title_block_size(sheet);
    let frame_w = (w_sheet - ml - mr).max(0.0);
    let frame_h = (h_sheet - mt - mb).max(0.0);
    let x = ml + frame_w - w;
    let y = mt + frame_h - h;
    layout_at(drawing, x, y, w, h)
}

fn layout_at(drawing: &Drawing, x: f64, y: f64, w: f64, h: f64) -> TitleBlockLayout {
    let tb = &drawing.title_block;
    let band_h = h * 0.19;
    let row1_h = h * 0.31;
    let row_h = (h - band_h - row1_h) * 0.5;
    let y_band = y;
    let y1 = y_band + band_h;
    let y2 = y1 + row1_h;
    let y3 = y2 + row_h;

    let mut cells: Vec<TbCell> = Vec::new();
    // Split [x, x+w] by weights into cell rects on one row.
    let split = |weights: &[f64], y0: f64, hh: f64| -> Vec<Rect2> {
        let total: f64 = weights.iter().sum();
        let mut cx = x;
        weights
            .iter()
            .map(|wt| {
                let cw = w * wt / total;
                let r = Rect2 {
                    x0: cx,
                    y0,
                    x1: cx + cw,
                    y1: y0 + hh,
                };
                cx += cw;
                r
            })
            .collect()
    };

    // A left-aligned field cell.
    let field = |key: &'static str,
                 label: &'static str,
                 value: String,
                 rect: Rect2,
                 style: CellStyle,
                 reserve_right: f64|
     -> TbCell {
        let room = rect.width() - 2.0 * PAD_MM - reserve_right;
        let baseline = rect.y0 + rect.height() * 0.80;
        let max_font = VALUE_FONT_MM.min(baseline - rect.y0 - VALUE_TOP_CLEAR_MM);
        let (value, font) = fit(
            &value,
            room,
            max_font.max(VALUE_MIN_FONT_MM),
            VALUE_MIN_FONT_MM,
            GLYPH_ADVANCE_EM,
        );
        TbCell {
            key,
            label,
            value,
            rect,
            style,
            value_font_mm: font,
            value_x: rect.x0 + PAD_MM,
            value_baseline: baseline,
            sub: None,
            value_room_mm: room,
        }
    };

    // ── Band: general tolerances | projection | scale ──────────────────────
    let band = split(&[1.5, 1.3, 0.8], y_band, band_h);
    cells.push(field(
        "general_tolerance",
        "GENERAL TOLERANCES",
        tolerance_label(drawing),
        band[0],
        CellStyle::Field,
        0.0,
    ));
    // The symbol sits at the right of its cell, below the label line.
    let proj_rect = band[1];
    let sym_h = (band_h - VALUE_TOP_CLEAR_MM - 0.8).clamp(2.5, 6.5);
    let sym_w = (sym_h * 1.75).min(proj_rect.width() * 0.4);
    let sym_cy = proj_rect.y0 + VALUE_TOP_CLEAR_MM + 0.5 * (band_h - VALUE_TOP_CLEAR_MM);
    let symbol = Rect2 {
        x0: proj_rect.x1 - PAD_MM - sym_w,
        y0: sym_cy - 0.5 * sym_h,
        x1: proj_rect.x1 - PAD_MM,
        y1: sym_cy + 0.5 * sym_h,
    };
    let projection = match drawing.projection {
        ProjectionAngle::First => "FIRST ANGLE",
        ProjectionAngle::Third => "THIRD ANGLE",
    };
    cells.push(field(
        "projection",
        "PROJECTION",
        projection.to_string(),
        proj_rect,
        CellStyle::Field,
        sym_w + PAD_MM,
    ));
    let scale = drawing
        .views
        .first()
        .map(|v| format_scale(v.scale))
        .unwrap_or_else(|| "1:1".to_string());
    cells.push(field(
        "scale",
        "SCALE",
        scale,
        band[2],
        CellStyle::Field,
        0.0,
    ));

    // ── Row 1: logo | title | drawing number ───────────────────────────────
    let logo_w = (w * 0.13).clamp(20.0, 30.0);
    let no_w = w * 0.27;
    let logo = Rect2 {
        x0: x,
        y0: y1,
        x1: x + logo_w,
        y1: y2,
    };
    let title_rect = Rect2 {
        x0: x + logo_w,
        y0: y1,
        x1: x + w - no_w,
        y1: y2,
    };
    let no_rect = Rect2 {
        x0: x + w - no_w,
        y0: y1,
        x1: x + w,
        y1: y2,
    };
    {
        let room = title_rect.width() - 2.0 * PAD_MM;
        let supp = tb.supplementary_title.trim();
        let (baseline, sub) = if supp.is_empty() {
            (title_rect.y0 + row1_h * 0.80, None)
        } else {
            let sub_base = title_rect.y0 + row1_h * 0.90;
            let sub_font = (row1_h * 0.21).clamp(VALUE_MIN_FONT_MM, 3.2);
            let (text, font) = fit(supp, room, sub_font, VALUE_MIN_FONT_MM, GLYPH_ADVANCE_EM);
            (
                title_rect.y0 + row1_h * 0.62,
                Some(SubLine {
                    text,
                    font_mm: font,
                    x: 0.5 * (title_rect.x0 + title_rect.x1),
                    baseline: sub_base,
                }),
            )
        };
        let max_font = TITLE_FONT_MM.min(baseline - title_rect.y0 - VALUE_TOP_CLEAR_MM);
        let (value, font) = fit(
            &dash(&drawing.name),
            room,
            max_font.max(VALUE_MIN_FONT_MM),
            VALUE_MIN_FONT_MM,
            GLYPH_ADVANCE_EM,
        );
        cells.push(TbCell {
            key: "title",
            label: "TITLE",
            value,
            rect: title_rect,
            style: CellStyle::Headline,
            value_font_mm: font,
            value_x: 0.5 * (title_rect.x0 + title_rect.x1),
            value_baseline: baseline,
            sub,
            value_room_mm: room,
        });
    }
    {
        let room = no_rect.width() - 2.0 * PAD_MM;
        let baseline = no_rect.y0 + row1_h * 0.80;
        let max_font = DRWG_NO_FONT_MM.min(baseline - no_rect.y0 - VALUE_TOP_CLEAR_MM);
        let (value, font) = fit(
            &drawing_number(drawing),
            room,
            max_font.max(VALUE_MIN_FONT_MM),
            VALUE_MIN_FONT_MM,
            MONO_ADVANCE_EM,
        );
        cells.push(TbCell {
            key: "drawing_number",
            label: "DRAWING NO.",
            value,
            rect: no_rect,
            style: CellStyle::Mono,
            value_font_mm: font,
            value_x: 0.5 * (no_rect.x0 + no_rect.x1),
            value_baseline: baseline,
            sub: None,
            value_room_mm: room,
        });
    }

    // ── Row 2: created | approved | date | material | mass | units ─────────
    let r2 = split(&[1.15, 1.15, 1.0, 1.6, 0.95, 0.65], y2, row_h);
    let unit = drawing.document_unit.suffix().to_string();
    for (i, (key, label, value)) in [
        ("drawn_by", "CREATED BY", dash(&tb.drawn_by)),
        ("approved_by", "APPROVED BY", dash(&tb.approved_by)),
        ("date", "DATE", dash(&tb.date)),
        ("material", "MATERIAL", dash(&tb.material)),
        ("mass", "MASS", mass_label(drawing)),
        ("units", "UNITS", unit),
    ]
    .into_iter()
    .enumerate()
    {
        cells.push(field(key, label, value, r2[i], CellStyle::Field, 0.0));
    }

    // ── Row 3: status | document type | verified | sheet | rev | size ──────
    let r3 = split(&[1.15, 1.45, 1.45, 0.85, 0.55, 0.65], y3, row_h);
    let sheet_label = format!(
        "{} OF {}",
        tb.sheet_index.max(1),
        tb.sheet_count.max(tb.sheet_index.max(1))
    );
    let row3 = [
        (
            "status",
            "STATUS",
            status(drawing).to_string(),
            CellStyle::Strong,
        ),
        (
            "document_type",
            "DOCUMENT TYPE",
            dash(&tb.document_type),
            CellStyle::Field,
        ),
        (
            "verified",
            "VERIFIED",
            verification_label(drawing),
            CellStyle::Field,
        ),
        ("sheet", "SHEET", sheet_label, CellStyle::Field),
        ("revision", "REV", dash(&tb.revision), CellStyle::Field),
        ("size", "SIZE", drawing.sheet_size.label(), CellStyle::Field),
    ];
    for (i, (key, label, value, style)) in row3.into_iter().enumerate() {
        cells.push(field(key, label, value, r3[i], style, 0.0));
    }

    TitleBlockLayout {
        outer: Rect2 {
            x0: x,
            y0: y,
            x1: x + w,
            y1: y + h,
        },
        cells,
        logo,
        symbol,
    }
}

// ── Render-time facts ────────────────────────────────────────────────────────

/// Nominal density (kg/m³) of a NAMED engineering material, or `None`.
///
/// Matching is by the material's name, most specific first (stainless before
/// plain steel). Values are nominal handbook densities -- a drawing's mass is
/// always nominal. An unrecognised name gives `None`, and the MASS cell then
/// shows a dash: the kernel never guesses a material.
pub fn nominal_density_kg_m3(material: &str) -> Option<f64> {
    let lower = material.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let has_word = |w: &str| words.iter().any(|x| *x == w);
    let has_phrase = |p: &str| lower.contains(p);
    // (words, phrases, density) -- order is precedence.
    let table: [(&[&str], &[&str], f64); 18] = [
        (
            &["stainless", "inox", "304", "304l", "316", "316l"],
            &[],
            8000.0,
        ),
        (&["7075"], &[], 2810.0),
        (
            &["aluminium", "aluminum", "6061", "6082", "5083", "2024"],
            &[],
            2700.0,
        ),
        (&["titanium", "ti6al4v"], &["ti-6al-4v"], 4430.0),
        (&["brass"], &[], 8500.0),
        (&["bronze"], &[], 8800.0),
        (&["copper"], &[], 8960.0),
        (&[], &["ductile iron", "sg iron"], 7100.0),
        (&[], &["cast iron", "grey iron", "gray iron"], 7200.0),
        (&["pom", "acetal", "delrin"], &[], 1410.0),
        (&["nylon", "pa6", "pa66", "polyamide"], &[], 1140.0),
        (&["abs"], &[], 1050.0),
        (&["peek"], &[], 1300.0),
        (&["polycarbonate"], &[], 1200.0),
        (&["hdpe"], &[], 950.0),
        (&["ptfe", "teflon"], &[], 2200.0),
        (&["pla"], &[], 1240.0),
        (
            &[
                "steel", "s235", "s275", "s355", "c45", "en8", "en24", "1018", "1045", "4140",
                "mild",
            ],
            &[],
            7850.0,
        ),
    ];
    table
        .iter()
        .find(|(ws, ps, _)| ws.iter().any(|w| has_word(w)) || ps.iter().any(|p| has_phrase(p)))
        .map(|(_, _, d)| *d)
}

/// Compute the render-time facts of `drawing` against the LIVE `model`:
/// the sheet certificate's verdict + digest, and the part's mass when its
/// material is named. Call it on the model the sheet is rendered from, right
/// before rendering; the result is never persisted (`#[serde(skip)]`), so an
/// old stamp can never vouch for a changed model.
pub fn compute_render_facts(model: &mut BRepModel, drawing: &Drawing) -> RenderFacts {
    let cert = certify_drawing(model, drawing);
    // Digest over the facts in a canonical order: each fact serialised, the
    // strings sorted, so the digest does not depend on collection order.
    let mut lines: Vec<String> = cert
        .facts
        .iter()
        .map(|f| serde_json::to_string(f).unwrap_or_default())
        .collect();
    lines.sort_unstable();
    let mut hasher = Sha256::new();
    hasher.update(if cert.sound {
        b"sound\n" as &[u8]
    } else {
        b"unsound\n"
    });
    for l in &lines {
        hasher.update(l.as_bytes());
        hasher.update(b"\n");
    }
    let digest: String = hasher
        .finalize()
        .iter()
        .take(4)
        .map(|b| format!("{b:02x}"))
        .collect();

    let solid = drawing.views.first().map(|v| {
        let ViewSource::Part { solid_id, .. } = v.source;
        solid_id
    });
    let mass_kg = nominal_density_kg_m3(&drawing.title_block.material).and_then(|rho| {
        let solid = solid?;
        let volume_mm3 = model.mass_properties_for(solid)?.volume;
        (volume_mm3.is_finite() && volume_mm3 > 0.0).then(|| volume_mm3 * 1e-9 * rho)
    });

    RenderFacts {
        mass_kg,
        verification: Some(SheetStamp {
            sound: cert.sound,
            digest,
        }),
    }
}

// ── ISO 7200 field check (advisory) ──────────────────────────────────────────

/// One ISO 7200 gap in the title block. Advisory: it never changes whether
/// the sheet certifies sound.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Iso7200Issue {
    /// The title-block field, e.g. `"approved_by"`.
    pub field: &'static str,
    pub message: String,
}

/// Check the title block against ISO 7200's data fields: the mandatory ones
/// present, and the stated lengths and formats.
pub fn check_iso_7200(drawing: &Drawing) -> Vec<Iso7200Issue> {
    let tb = &drawing.title_block;
    let mut out = Vec::new();
    let mut flag = |field: &'static str, message: String| out.push(Iso7200Issue { field, message });
    if drawing.name.trim().is_empty() {
        flag("title", "the title is mandatory".into());
    } else if drawing.name.trim().chars().count() > 30 {
        flag(
            "title",
            format!(
                "title is {} characters; ISO 7200 allows 30 -- move the detail into the supplementary title",
                drawing.name.trim().chars().count()
            ),
        );
    }
    if tb.drawn_by.trim().is_empty() {
        flag("drawn_by", "the creator is mandatory".into());
    }
    if tb.approved_by.trim().is_empty() {
        flag(
            "approved_by",
            "the approval person is mandatory; the sheet stays FOR REVIEW until a person approves it".into(),
        );
    }
    if !is_iso_8601_date(tb.date.trim()) {
        flag(
            "date",
            format!(
                "date of issue must be ISO 8601 (YYYY-MM-DD), got '{}'",
                tb.date.trim()
            ),
        );
    }
    let number = drawing_number(drawing);
    if number.chars().count() > 16 {
        flag(
            "drawing_number",
            format!("identification number '{number}' exceeds 16 characters"),
        );
    }
    if tb.document_type.trim().is_empty() {
        flag("document_type", "the document type is mandatory".into());
    }
    let rev = tb.revision.trim();
    if rev.chars().count() > 2 {
        flag(
            "revision",
            format!("revision index '{rev}' exceeds 2 characters"),
        );
    }
    if rev.contains('I') || rev.contains('O') {
        flag(
            "revision",
            format!("revision index '{rev}' uses I or O, which read as 1 and 0"),
        );
    }
    out
}

fn is_iso_8601_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    if !(digits(0..4) && digits(5..7) && digits(8..10)) {
        return false;
    }
    let month: u32 = s[5..7].parse().unwrap_or(0);
    let day: u32 = s[8..10].parse().unwrap_or(0);
    (1..=12).contains(&month) && (1..=31).contains(&day)
}
