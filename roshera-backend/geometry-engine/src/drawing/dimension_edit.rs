//! What an engineer (or an agent) changes about a dimension on a sheet.
//!
//! A sheet's dimensions are derived from the model every time the sheet is
//! built: their values are MEASURED, never typed. What a person adds on top is
//! recorded here, per dimension, as a [`DimensionEdit`]:
//!
//! * a **tolerance** (symmetric, bilateral, limits of size, an ISO 286 class,
//!   basic, reference);
//! * **text** before and after the value (`4X`, `TYP`, `THRU`, a thread
//!   designation such as `M12x1.75-6H`);
//! * a **move** of the text / dimension line;
//! * a **suppression**: the dimension is taken off the sheet, by a named
//!   person, for a stated reason.
//!
//! The edits live on the sheet ([`Drawing::dimension_edits`]) keyed by the
//! dimension's durable [`Dimension2d::key`], beside the derived dimensions and
//! never inside them, so a rebuilt sheet (a model change, an undo, a restart)
//! re-applies each edit to the dimension it was made on. An edit whose
//! dimension is gone is reported by the sheet certificate as `dangling`; it is
//! never moved to another dimension.
//!
//! ## What the kernel verifies, and what it only records
//!
//! The value stays measured: no edit can change the number. A tolerance is
//! validated (an ISO 286 class must resolve from the tables in
//! [`crate::gdt::iso286`] at the measured size). Text before / after the value
//! is free: a thread designation or a standard reference contains digits and
//! the kernel has no way to check it, so it is inked as written and the
//! certificate lists it as **authored text, not verified**. Suppressing a
//! dimension does not make the sheet complete: the certificate lists it as a
//! waived omission naming who suppressed it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::dimensioning::Dimension2d;
use super::types::{Drawing, ToleranceRef};
use crate::gdt::iso286::{self, FitRefusal};
use crate::primitives::topology_builder::BRepModel;
use crate::units::LengthUnit;

/// Namespace of the keys minted for callouts that carry no dimension `pid`.
const DIM_KEY_NS: Uuid = Uuid::from_u128(0x6e5a_1f00_0000_4000_8000_6469_6d6b_6579);

/// Longest text accepted before or after a dimension's value.
pub const MAX_DIM_TEXT_CHARS: usize = 40;
/// Furthest a dimension's text / line may be moved from where the layout put
/// it, in sheet millimetres (an A0 sheet is 1189 mm wide).
pub const MAX_DIM_SHIFT_MM: f64 = 1200.0;

/// The durable handle of a dimension: its `pid` when it has one; else an id
/// derived from the persistent ids of the faces it names, its kind and its
/// direction (a pattern's two pitches name the same holes). `None` when a
/// named face has no persistent id, or when it names no face at all.
pub fn dimension_key(
    model: &BRepModel,
    kind: &str,
    entities: &[u32],
    direction: [f64; 3],
    pid: Option<&str>,
) -> Option<String> {
    if let Some(pid) = pid {
        return Some(pid.to_string());
    }
    if entities.is_empty() {
        return None;
    }
    let mut pids: Vec<u128> = Vec::with_capacity(entities.len());
    for face in entities {
        pids.push(model.face_pids.get(face)?.as_u128());
    }
    pids.sort_unstable();
    pids.dedup();
    let mut buf: Vec<u8> = Vec::with_capacity(8 + pids.len() * 16 + kind.len() + 3);
    buf.extend_from_slice(&(pids.len() as u32).to_be_bytes());
    for p in pids {
        buf.extend_from_slice(&p.to_be_bytes());
    }
    buf.extend_from_slice(&(kind.len() as u32).to_be_bytes());
    buf.extend_from_slice(kind.as_bytes());
    // The axis the dimension runs along, sign-free (a pitch measured along
    // +X and along -X is one pitch), to one part in a hundred.
    let mut q = [
        (direction[0] * 100.0).round() as i64,
        (direction[1] * 100.0).round() as i64,
        (direction[2] * 100.0).round() as i64,
    ];
    if q.iter().find(|v| **v != 0).is_some_and(|v| *v < 0) {
        q = [-q[0], -q[1], -q[2]];
    }
    for c in q {
        buf.push(c.clamp(-127, 127) as i8 as u8);
    }
    Some(Uuid::new_v5(&DIM_KEY_NS, &buf).hyphenated().to_string())
}

/// A tolerance authored on one dimension of a sheet.
///
/// Numbers are in the unit the dimension is inked in (the document unit;
/// degrees for an angle).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DimTolerance {
    /// `25 ±0.15`.
    Symmetric { pm: f64 },
    /// Two signed limit deviations, `upper > lower`: `25 +0.05/-0.10`,
    /// `25 +0.10/+0.05`, `25 0/-0.2`.
    Bilateral { upper: f64, lower: f64 },
    /// Limits of size, absolute: `25.15/24.85`.
    Limits { upper: f64, lower: f64 },
    /// One ISO 286 class, e.g. `H7` or `g6`; its limit deviations are
    /// resolved from the measured size and inked beside it.
    Fit { designation: String },
    /// A theoretically exact (basic) dimension, inked in a box.
    Basic,
    /// A reference dimension, inked in parentheses.
    Reference,
}

impl DimTolerance {
    /// Lower-case name of the form, as the wire and the readback spell it.
    pub fn kind(&self) -> &'static str {
        match self {
            DimTolerance::Symmetric { .. } => "symmetric",
            DimTolerance::Bilateral { .. } => "bilateral",
            DimTolerance::Limits { .. } => "limits",
            DimTolerance::Fit { .. } => "fit",
            DimTolerance::Basic => "basic",
            DimTolerance::Reference => "reference",
        }
    }
}

/// Who took a dimension off the sheet, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waiver {
    /// The person or agent who suppressed it. Never empty.
    pub by: String,
    /// Their reason, as written. May be empty.
    #[serde(default)]
    pub reason: String,
}

/// Everything authored on one dimension. The whole record is stored and
/// replayed as one value: an edit replaces the previous record.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DimensionEdit {
    /// The tolerance, when one is authored on this dimension. It takes
    /// precedence over a size tolerance on the feature (the GD&T sidecar).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<DimTolerance>,
    /// Text inked before the value (`4X`). Authored, not verified.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prefix: String,
    /// Text inked after the value (`TYP`, `M12x1.75-6H`). Authored, not
    /// verified.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub suffix: String,
    /// How far the text / dimension line was dragged from where the layout
    /// put it, sheet millimetres `[right, down]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_shift_mm: Option<[f64; 2]>,
    /// Set when the dimension is taken off the sheet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suppressed: Option<Waiver>,
    /// Which band of its view, and which lane of that band, the dimension
    /// was put in (a layout verb: `drawing_move_dimension`,
    /// `drawing_align`). The layout honours it before its own search and
    /// falls back to that search when the lane has no room.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<DimPlacement>,
}

/// Where a dimension was put among its view's annotation bands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DimPlacement {
    /// The band (side of the view's geometry). A straight dimension takes
    /// only the two bands its line can run along (a horizontal one top or
    /// bottom, a vertical one left or right); a leader callout's shoulder
    /// goes left or right.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub band: Option<super::spaces::BandSide>,
    /// The lane, 1 = nearest the outline (`STANDOFF` out), each next one
    /// `STACK` further.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<u32>,
}

impl DimPlacement {
    /// True when it pins nothing.
    pub fn is_empty(&self) -> bool {
        self.band.is_none() && self.lane.is_none()
    }
}

impl DimensionEdit {
    /// True when nothing is authored: the record need not be kept.
    pub fn is_empty(&self) -> bool {
        self.tolerance.is_none()
            && self.prefix.is_empty()
            && self.suffix.is_empty()
            && self.text_shift_mm.is_none()
            && self.suppressed.is_none()
            && self.placement.is_none_or(|p| p.is_empty())
    }
}

/// Why an edit of a dimension is refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DimensionEditError {
    /// No dimension with that id (or key) is on the sheet.
    #[error("no dimension '{0}' on this sheet")]
    NoSuchDimension(String),
    /// The dimension names faces without a persistent id, so an edit could
    /// not be re-applied to it after the next rebuild.
    #[error(
        "dimension '{0}' has no persistent identity (its faces carry no persistent id), \
         so an edit of it cannot be recorded"
    )]
    NotAddressable(String),
    /// The tolerance form does not apply to this kind of dimension.
    #[error("a {form} tolerance cannot be put on a {kind} callout: {why}")]
    ToleranceNotSupported {
        form: String,
        kind: String,
        why: String,
    },
    /// The tolerance numbers are not a tolerance.
    #[error("{0}")]
    BadTolerance(String),
    /// The ISO 286 class does not resolve at the measured size.
    #[error("{0}")]
    Fit(FitRefusal),
    /// Text before / after the value that cannot be inked.
    #[error("{0}")]
    BadText(String),
    /// A move that is not a finite distance on the sheet.
    #[error("a dimension can be moved by a finite distance of at most 1200 mm")]
    BadShift,
    /// A suppression without a name.
    #[error("suppressing a dimension needs the name of who suppresses it")]
    UnsignedWaiver,
    /// A lane outside the layout's lanes.
    #[error("a dimension's lane is 1 (nearest the outline) to {max}")]
    BadLane { max: usize },
}

/// The kinds a size tolerance can be inked on: the label states one value.
fn tolerable_kind(kind: &str) -> bool {
    matches!(
        kind,
        "diameter"
            | "radius"
            | "length"
            | "extent"
            | "position"
            | "location"
            | "pitch"
            | "pcd"
            | "hole_callout"
            | "fillet"
            | "angle"
    )
}

/// Byte range of the VALUE in a dimension label: the first number that is not
/// a count (`4×`, `6x`). `None` when the label states no number.
fn value_run(label: &str) -> Option<(usize, usize)> {
    let chars: Vec<(usize, char)> = label.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].1.is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = chars[i].0;
        let mut j = i;
        while j < chars.len() {
            let c = chars[j].1;
            let digit_follows = chars.get(j + 1).is_some_and(|n| n.1.is_ascii_digit());
            if c.is_ascii_digit() || ((c == '.' || c == ',') && digit_follows) {
                j += 1;
            } else {
                break;
            }
        }
        let end = chars.get(j).map_or(label.len(), |c| c.0);
        // A count is a number followed by a multiplication sign.
        let mut k = j;
        while chars.get(k).is_some_and(|c| c.1 == ' ') {
            k += 1;
        }
        let is_count = chars
            .get(k)
            .is_some_and(|c| matches!(c.1, '\u{00D7}' | 'x' | 'X'));
        if !is_count {
            return Some((start, end));
        }
        i = j;
    }
    None
}

/// The value a label states, with the number of decimals it is written to.
pub(crate) fn label_value(label: &str) -> Option<(f64, usize)> {
    let (s, e) = value_run(label)?;
    let text = label[s..e].replace(',', ".");
    let decimals = text.split_once('.').map_or(0, |(_, frac)| frac.len());
    text.parse::<f64>().ok().map(|v| (v, decimals))
}

/// Fewest decimals (0..=4) that write `v` exactly.
fn decimals_needed(v: f64) -> usize {
    (0..=4usize)
        .find(|n| {
            let scaled = v.abs() * 10f64.powi(*n as i32);
            (scaled - scaled.round()).abs() < 1e-6
        })
        .unwrap_or(4)
}

/// A signed limit deviation: `+0.05`, `-0.10`; zero is `0` (ISO 129-1).
fn deviation_text(v: f64, decimals: usize) -> String {
    if v.abs() < 1e-9 {
        "0".to_string()
    } else {
        format!("{v:+.decimals$}")
    }
}

/// A tolerance stacked beside the value: upper over lower.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimStack {
    /// Text before the stack (prefix, symbol, value, class).
    pub lead: String,
    /// The upper line: upper deviation, or the upper limit of size.
    pub upper: String,
    /// The lower line.
    pub lower: String,
    /// Text after the stack (unit sign, suffix).
    pub trail: String,
    /// The two lines ARE the dimension (limits of size), so they are drawn
    /// on the value's own baseline and the line above it; deviations beside
    /// a value straddle its mid-height.
    #[serde(default)]
    pub full_size: bool,
}

/// Rise of the upper line of two full-height stacked limits above the
/// baseline of the lower one.
pub const LIMIT_LINE_PITCH_MM: f64 = 3.7;

/// Font height of the two stacked tolerance lines: the value's own, so a
/// deviation prints 2.5 mm capitals like every other number on the sheet
/// (ISO 3098's minimum; they were 0.7 of the value's, 1.5 mm capitals).
pub const TOL_FONT_MM: f64 = super::layout::DIM_TEXT_FONT_MM;
/// A stacked deviation pair straddles the value's mid-height: the upper
/// line's baseline this far above the value's, the lower line's
/// [`TOL_LOWER_DROP_MM`] below it.
pub const TOL_UPPER_RISE_MM: f64 = 2.2;
/// See [`TOL_UPPER_RISE_MM`].
pub const TOL_LOWER_DROP_MM: f64 = 1.6;
/// Gap between the value, the stacked tolerance and any trailing text.
pub const STACK_GAP_MM: f64 = 0.6;

impl DimStack {
    /// Widths on the sheet of the three runs: the lead, the stacked pair,
    /// and the trailing text with the gap before it (zero when there is no
    /// trailing text). The same glyph-advance model as every other text box.
    pub fn run_widths(&self) -> (f64, f64, f64) {
        use super::layout::{DIM_TEXT_FONT_MM, GLYPH_ADVANCE_EM};
        let width = |t: &str, font: f64| t.chars().count() as f64 * GLYPH_ADVANCE_EM * font;
        let trail = self.trail.trim();
        let font = if self.full_size {
            DIM_TEXT_FONT_MM
        } else {
            TOL_FONT_MM
        };
        (
            width(&self.lead, DIM_TEXT_FONT_MM),
            width(&self.upper, font).max(width(&self.lower, font)),
            if trail.is_empty() {
                0.0
            } else {
                STACK_GAP_MM + width(trail, DIM_TEXT_FONT_MM)
            },
        )
    }

    /// Width of the whole stacked text on the sheet.
    pub fn width_mm(&self) -> f64 {
        let (lead, tol, trail) = self.run_widths();
        lead + STACK_GAP_MM + tol + trail
    }
}

/// The ink of one dimension: its measured label with everything authored on
/// it. One function makes it for the layout, every renderer and the
/// certificate, so what is inked and what is certified cannot differ.
#[derive(Debug, Clone, PartialEq)]
pub struct DimInk {
    /// The whole text on one line (`Ø25.00 H7 (+0.021/0)`): what the DXF
    /// inks, what the layout measures, what the certificate states.
    pub label: String,
    /// The same text with the two tolerance lines stacked, when the form has
    /// two (bilateral, limits, a resolved fit). The SVG and PDF ink this.
    pub stack: Option<DimStack>,
    /// A basic dimension: the text is drawn in a box.
    pub boxed: bool,
    /// Authored text on the dimension the kernel did not verify.
    pub authored: Vec<String>,
    /// The tolerance as a readback answers it. `limits` are absolute, in the
    /// unit the dimension is inked in.
    pub tolerance: Option<ToleranceRef>,
    /// Set when an ISO 286 class is inked WITHOUT limits because it does not
    /// resolve at the measured size.
    pub unresolved: Option<String>,
}

impl DimInk {
    /// Width of the text as the sheet draws it: the stacked form when there
    /// is one (it is narrower than the one-line text), else the label.
    pub fn width_mm(&self) -> f64 {
        use super::layout::{DIM_TEXT_FONT_MM, GLYPH_ADVANCE_EM};
        match &self.stack {
            Some(stack) => stack.width_mm(),
            None => self.label.chars().count() as f64 * GLYPH_ADVANCE_EM * DIM_TEXT_FONT_MM,
        }
    }
}

/// The tolerance a feature's size annotation (GD&T sidecar, millimetres)
/// puts on a dimension inked in `unit`.
fn tolerance_of_feature(t: &ToleranceRef, unit: LengthUnit) -> Option<DimTolerance> {
    let k = unit.per_mm();
    match (t.kind.as_str(), t.limits, &t.designation) {
        ("fit", _, Some(designation)) => Some(DimTolerance::Fit {
            designation: designation.clone(),
        }),
        ("limits", Some([lo, hi]), _) => Some(DimTolerance::Limits {
            upper: hi * k,
            lower: lo * k,
        }),
        ("plus_minus", Some([lo, hi]), _) => {
            let upper = (hi - t.nominal) * k;
            let lower = (lo - t.nominal) * k;
            if (upper + lower).abs() < 1e-9 {
                Some(DimTolerance::Symmetric { pm: upper })
            } else {
                Some(DimTolerance::Bilateral { upper, lower })
            }
        }
        _ => None,
    }
}

/// The ink of `dim` with `edit` (when one is authored on it) applied, on a
/// sheet whose document unit is `unit`.
pub fn dimension_ink(dim: &Dimension2d, edit: Option<&DimensionEdit>, unit: LengthUnit) -> DimInk {
    let authored_tolerance = edit.and_then(|e| e.tolerance.clone());
    let from_feature = authored_tolerance.is_none();
    let tolerance = authored_tolerance.or_else(|| {
        dim.tolerance
            .as_ref()
            .and_then(|t| tolerance_of_feature(t, unit))
    });

    let label = dim.label.as_str();
    let run = value_run(label);
    let (lead, number, rest) = match run {
        Some((s, e)) => (&label[..s], &label[s..e], &label[e..]),
        None => (label, "", ""),
    };
    // A degree sign belongs to the value: the tolerance goes after it.
    let (number_shown, rest_after) = match rest.strip_prefix('\u{00B0}') {
        Some(after) => (format!("{number}\u{00B0}"), after),
        None => (number.to_string(), rest),
    };
    let stated = label_value(label);
    let nominal = stated.map(|(v, _)| v);
    let label_decimals = stated.map_or(0, |(_, d)| d);

    let mut text = label.to_string();
    let mut stack: Option<DimStack> = None;
    let mut boxed = false;
    let mut unresolved: Option<String> = None;
    let mut readback: Option<ToleranceRef> = None;
    let tref = |kind: &str, limits: Option<[f64; 2]>, designation: Option<String>| ToleranceRef {
        feature_pid: if from_feature {
            dim.tolerance.as_ref().and_then(|t| t.feature_pid.clone())
        } else {
            None
        },
        annotation_index: if from_feature {
            dim.tolerance.as_ref().map_or(0, |t| t.annotation_index)
        } else {
            0
        },
        kind: kind.to_string(),
        nominal: nominal.unwrap_or(dim.value),
        limits,
        designation,
    };

    match (&tolerance, run) {
        (Some(DimTolerance::Symmetric { pm }), Some(_)) => {
            let d = decimals_needed(*pm);
            text = format!("{lead}{number_shown} \u{00B1}{:.d$}{rest_after}", pm.abs());
            readback = Some(tref(
                "symmetric",
                nominal.map(|n| [n - pm.abs(), n + pm.abs()]),
                None,
            ));
        }
        (Some(DimTolerance::Bilateral { upper, lower }), Some(_)) => {
            let d = decimals_needed(*upper).max(decimals_needed(*lower));
            let (u, l) = (deviation_text(*upper, d), deviation_text(*lower, d));
            text = format!("{lead}{number_shown} {u}/{l}{rest_after}");
            stack = Some(DimStack {
                lead: format!("{lead}{number_shown}"),
                upper: u,
                lower: l,
                trail: rest_after.to_string(),
                full_size: false,
            });
            readback = Some(tref(
                "bilateral",
                nominal.map(|n| [n + lower, n + upper]),
                None,
            ));
        }
        (Some(DimTolerance::Limits { upper, lower }), Some(_)) => {
            let d = label_decimals
                .max(decimals_needed(*upper))
                .max(decimals_needed(*lower));
            let (u, l) = (format!("{upper:.d$}"), format!("{lower:.d$}"));
            text = format!("{lead}{u}/{l}{rest}");
            stack = Some(DimStack {
                lead: lead.to_string(),
                upper: u,
                lower: l,
                trail: rest.to_string(),
                full_size: true,
            });
            readback = Some(tref("limits", Some([*lower, *upper]), None));
        }
        (Some(DimTolerance::Fit { designation }), Some(_)) => {
            // ISO 286 is tabulated in millimetres.
            let resolved = match (nominal, unit) {
                (Some(n), LengthUnit::Millimetre) => {
                    iso286::resolve_class(n, designation).map_err(|e| e.to_string())
                }
                (Some(_), _) => Err("ISO 286 classes apply to millimetre sizes".to_string()),
                (None, _) => Err("the label states no size".to_string()),
            };
            match resolved {
                Ok(class) => {
                    let d = decimals_needed(class.upper_mm).max(decimals_needed(class.lower_mm));
                    let (u, l) = (
                        deviation_text(class.upper_mm, d),
                        deviation_text(class.lower_mm, d),
                    );
                    text = format!("{lead}{number_shown} {} ({u}/{l}){rest_after}", class.class);
                    stack = Some(DimStack {
                        lead: format!("{lead}{number_shown} {}", class.class),
                        upper: u,
                        lower: l,
                        trail: rest_after.to_string(),
                        full_size: false,
                    });
                    readback = Some(tref(
                        "fit",
                        nominal.map(|n| [n + class.lower_mm, n + class.upper_mm]),
                        Some(class.class.clone()),
                    ));
                }
                Err(why) => {
                    // The class as written, no limits: none are invented.
                    let class = designation.trim();
                    text = format!("{lead}{number_shown} {class}{rest_after}");
                    readback = Some(tref("fit", None, Some(class.to_string())));
                    unresolved = Some(format!("{class} has no limits here: {why}"));
                }
            }
        }
        (Some(DimTolerance::Basic), _) => {
            boxed = true;
            readback = Some(tref("basic", None, None));
        }
        (Some(DimTolerance::Reference), _) => {
            text = format!("({label})");
            readback = Some(tref("reference", None, None));
        }
        // A tolerance on a label that states no value is not inked: the
        // edit route refuses to author one, and a feature tolerance on such
        // a callout (none exists today) stays in the readback only.
        (Some(_), None) | (None, _) => {}
    }

    let mut authored = Vec::new();
    let prefix = edit.map(|e| e.prefix.trim()).unwrap_or("");
    let suffix = edit.map(|e| e.suffix.trim()).unwrap_or("");
    if !prefix.is_empty() {
        text = format!("{prefix} {text}");
        if let Some(s) = &mut stack {
            s.lead = format!("{prefix} {}", s.lead);
        }
        authored.push(prefix.to_string());
    }
    if !suffix.is_empty() {
        text = format!("{text} {suffix}");
        if let Some(s) = &mut stack {
            s.trail = format!("{} {suffix}", s.trail);
        }
        authored.push(suffix.to_string());
    }

    DimInk {
        label: text,
        stack,
        boxed,
        authored,
        tolerance: readback,
        unresolved,
    }
}

fn check_text(what: &str, text: &str) -> Result<(), DimensionEditError> {
    if text.chars().count() > MAX_DIM_TEXT_CHARS {
        return Err(DimensionEditError::BadText(format!(
            "the {what} is longer than {MAX_DIM_TEXT_CHARS} characters; \
             put long text in a note"
        )));
    }
    if text.chars().any(char::is_control) {
        return Err(DimensionEditError::BadText(format!(
            "the {what} must be one line of printable text"
        )));
    }
    Ok(())
}

/// Check `edit` against the dimension it is authored on. `dim` is `None`
/// when the dimension is not on the sheet (a dimension the selection left
/// off, addressed by its key): only a suppression can be recorded on it.
pub fn validate_dimension_edit(
    dim: Option<&Dimension2d>,
    edit: &DimensionEdit,
    unit: LengthUnit,
) -> Result<(), DimensionEditError> {
    check_text("text before the value", &edit.prefix)?;
    check_text("text after the value", &edit.suffix)?;
    if let Some(shift) = edit.text_shift_mm {
        if !shift
            .iter()
            .all(|c| c.is_finite() && c.abs() <= MAX_DIM_SHIFT_MM)
        {
            return Err(DimensionEditError::BadShift);
        }
    }
    if let Some(lane) = edit.placement.and_then(|p| p.lane) {
        let max = super::layout::MAX_LANES;
        if lane == 0 || lane as usize > max {
            return Err(DimensionEditError::BadLane { max });
        }
    }
    if let Some(w) = &edit.suppressed {
        if w.by.trim().is_empty() {
            return Err(DimensionEditError::UnsignedWaiver);
        }
        check_text("name", &w.by)?;
        if w.reason.chars().count() > 200 || w.reason.chars().any(char::is_control) {
            return Err(DimensionEditError::BadText(
                "the reason must be one line of at most 200 characters".to_string(),
            ));
        }
    }
    let Some(tolerance) = &edit.tolerance else {
        return Ok(());
    };
    let Some(dim) = dim else {
        return Err(DimensionEditError::BadTolerance(
            "this dimension is not on the sheet; only its suppression can be recorded".to_string(),
        ));
    };
    let form = tolerance.kind().to_string();
    let unsupported = |why: &str| DimensionEditError::ToleranceNotSupported {
        form: form.clone(),
        kind: dim.kind.clone(),
        why: why.to_string(),
    };
    let finite = |v: f64| v.is_finite();
    match tolerance {
        DimTolerance::Basic | DimTolerance::Reference => return Ok(()),
        _ => {}
    }
    if !tolerable_kind(&dim.kind) {
        return Err(unsupported(
            "its label does not state one size a tolerance could follow",
        ));
    }
    let Some((nominal, _)) = label_value(&dim.label) else {
        return Err(unsupported("its label states no value"));
    };
    match tolerance {
        DimTolerance::Symmetric { pm } => {
            if !finite(*pm) || *pm <= 0.0 {
                return Err(DimensionEditError::BadTolerance(
                    "a symmetric tolerance is a positive number (\u{00B1}0.1)".to_string(),
                ));
            }
        }
        DimTolerance::Bilateral { upper, lower } => {
            if !finite(*upper) || !finite(*lower) || upper <= lower {
                return Err(DimensionEditError::BadTolerance(
                    "the upper deviation must be greater than the lower one (+0.05 / -0.10)"
                        .to_string(),
                ));
            }
        }
        DimTolerance::Limits { upper, lower } => {
            if !finite(*upper) || !finite(*lower) || upper <= lower || *lower < 0.0 {
                return Err(DimensionEditError::BadTolerance(
                    "limits of size are two sizes, the upper greater than the lower \
                     (25.15 / 24.85)"
                        .to_string(),
                ));
            }
        }
        DimTolerance::Fit { designation } => {
            if dim.kind == "angle" {
                return Err(unsupported("ISO 286 classes apply to linear sizes"));
            }
            if unit != LengthUnit::Millimetre {
                return Err(unsupported(
                    "ISO 286 classes are tabulated for millimetre sizes; \
                     this sheet is not in millimetres",
                ));
            }
            iso286::resolve_class(nominal, designation).map_err(DimensionEditError::Fit)?;
        }
        DimTolerance::Basic | DimTolerance::Reference => {}
    }
    Ok(())
}

/// The size cell of a hole-table row as inked: the row's diameter label with
/// the bore's tolerance and authored text. A tabled bore's diameter is stated
/// by its row, not by a dimension line, so what is authored on that
/// dimension (or on the bore face, in the GD&T sidecar) is inked here.
pub fn hole_row_ink(drawing: &Drawing, site: &super::hole_table::HoleSite) -> String {
    let dim = drawing
        .views
        .iter()
        .flat_map(|v| v.dimensions.iter())
        .find(|d| {
            d.kind == "diameter" && d.entities.iter().any(|e| site.face_entities.contains(e))
        });
    let row = Dimension2d {
        id: String::new(),
        kind: "diameter".to_string(),
        value: site.diameter_mm,
        unit: "mm".to_string(),
        label: site.dia_label.clone(),
        a: [0.0, 0.0],
        b: [0.0, 0.0],
        entities: site.face_entities.clone(),
        axis3: None,
        dir3: None,
        pid: None,
        datum: None,
        tolerance: dim
            .and_then(|d| d.tolerance.clone())
            .or_else(|| site.tolerance.clone()),
        key: dim.and_then(|d| d.key.clone()),
        members: Vec::new(),
    };
    let edit = row
        .key
        .as_ref()
        .and_then(|k| drawing.dimension_edits.get(k));
    dimension_ink(&row, edit, drawing.document_unit).label
}

/// Every dimension edit on a sheet, by dimension key.
pub type DimensionEdits = BTreeMap<String, DimensionEdit>;

impl Drawing {
    /// The dimension `reference` names: a dimension id on this sheet
    /// (`data-dim-id` in the SVG), or a dimension key.
    pub fn find_dimension(&self, reference: &str) -> Option<&Dimension2d> {
        let all = || self.views.iter().flat_map(|v| v.dimensions.iter());
        all()
            .find(|d| d.id == reference)
            .or_else(|| all().find(|d| d.key.as_deref() == Some(reference)))
    }

    /// The edit authored on the dimension `reference` names, if any.
    pub fn dimension_edit(&self, reference: &str) -> Option<&DimensionEdit> {
        let key = self.find_dimension(reference).and_then(|d| d.key.clone());
        self.dimension_edits
            .get(key.as_deref().unwrap_or(reference))
    }

    /// Author `edit` on the dimension `reference` names (an id or a key),
    /// replacing what was authored on it before. Returns the dimension's key:
    /// what the history records, and what replay hands to
    /// [`Self::apply_dimension_edit`].
    ///
    /// A `reference` that is on no view is accepted only as a key carrying
    /// nothing but a suppression (waiving a dimension the sheet already
    /// leaves off): `known_omitted_key` says whether the caller found it
    /// among the certificate's omissions.
    pub fn set_dimension_edit(
        &mut self,
        reference: &str,
        edit: DimensionEdit,
        known_omitted_key: bool,
    ) -> Result<String, DimensionEditError> {
        let unit = self.document_unit;
        let key = match self.find_dimension(reference) {
            Some(dim) => {
                validate_dimension_edit(Some(dim), &edit, unit)?;
                dim.key
                    .clone()
                    .ok_or_else(|| DimensionEditError::NotAddressable(reference.to_string()))?
            }
            None => {
                if !known_omitted_key && !self.dimension_edits.contains_key(reference) {
                    return Err(DimensionEditError::NoSuchDimension(reference.to_string()));
                }
                validate_dimension_edit(None, &edit, unit)?;
                reference.to_string()
            }
        };
        self.apply_dimension_edit(&key, edit);
        Ok(key)
    }

    /// Put a recorded edit back (history replay): no validation against the
    /// sheet, because a dimension that is gone must stay visible as a
    /// dangling edit in the certificate, not be dropped here.
    pub fn apply_dimension_edit(&mut self, key: &str, edit: DimensionEdit) {
        if edit.is_empty() {
            self.dimension_edits.remove(key);
        } else {
            self.dimension_edits.insert(key.to_string(), edit);
        }
    }
}
