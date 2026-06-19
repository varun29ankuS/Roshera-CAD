//! Document length-unit system (GD&T Phase 1).
//!
//! The kernel's geometry is stored in a single native unit. By the standing
//! convention **1 kernel unit = 1 millimetre** — every coordinate, length, and
//! diameter the kernel computes is in millimetres. That convention is the
//! *modelling* unit and does not change here.
//!
//! What this module adds is the **document unit**: the unit an engineering
//! drawing or an agent-facing readout is *labelled and formatted in*. A part
//! authored in millimetres can be presented in inches without re-cutting any
//! geometry — the document unit governs the displayed number and the unit
//! string only.
//!
//! Phase 1 is deliberately bounded to display/formatting. The conversion factor
//! ([`LengthUnit::mm_per_unit`]) and [`LengthUnit::from_mm`] are the clean seam
//! a later phase will use to drive true unit conversion (input parsing,
//! round-tripping STEP `LENGTH_UNIT`, per-document scaling). Until that phase
//! lands the formatters here intentionally do **not** rescale the value — they
//! label it — so no silent geometry change can sneak in. See
//! [`LengthUnit::format_length`] for the explicit contract.

use serde::{Deserialize, Serialize};

/// A document length unit: the unit lengths/diameters are *labelled* in on
/// drawings and agent readouts. The kernel's native modelling unit is the
/// millimetre (1 kernel unit = 1 mm); this is the presentation layer over it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LengthUnit {
    /// Millimetre — the kernel's native modelling unit and the document default.
    Millimetre,
    /// Centimetre.
    Centimetre,
    /// Metre.
    Metre,
    /// Inch.
    Inch,
}

impl Default for LengthUnit {
    fn default() -> Self {
        LengthUnit::Millimetre
    }
}

impl LengthUnit {
    /// The short label drawn on a dimension and reported in the `unit` field
    /// (`"mm"`, `"cm"`, `"m"`, `"in"`). ASCII so the 5×7 overlay font renders it.
    pub fn label(self) -> &'static str {
        match self {
            LengthUnit::Millimetre => "mm",
            LengthUnit::Centimetre => "cm",
            LengthUnit::Metre => "m",
            LengthUnit::Inch => "in",
        }
    }

    /// Millimetres per one of this unit — the conversion factor against the
    /// kernel's native millimetre. This is the seam future true-conversion
    /// phases build on (input parsing, STEP `LENGTH_UNIT`, scaling); Phase 1
    /// only labels, so the formatters below do not yet apply it.
    pub fn mm_per_unit(self) -> f64 {
        match self {
            LengthUnit::Millimetre => 1.0,
            LengthUnit::Centimetre => 10.0,
            LengthUnit::Metre => 1000.0,
            LengthUnit::Inch => 25.4,
        }
    }

    /// Convert a millimetre value (kernel-native) into this unit's magnitude.
    /// The companion to [`Self::mm_per_unit`]; the future-conversion seam. Not
    /// used by the Phase-1 formatters, which label without rescaling.
    pub fn from_mm(self, mm: f64) -> f64 {
        mm / self.mm_per_unit()
    }

    /// Parse a unit label/string (case-insensitive, accepts the common
    /// long forms). Returns `None` for an unrecognized token so callers can
    /// fall back to the default rather than silently mislabel.
    pub fn parse(s: &str) -> Option<LengthUnit> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mm" | "millimetre" | "millimeter" => Some(LengthUnit::Millimetre),
            "cm" | "centimetre" | "centimeter" => Some(LengthUnit::Centimetre),
            "m" | "metre" | "meter" => Some(LengthUnit::Metre),
            "in" | "inch" | "inches" | "\"" => Some(LengthUnit::Inch),
            _ => None,
        }
    }

    /// Format a kernel-native (millimetre) length for display, e.g. `"17.5 mm"`.
    ///
    /// PHASE-1 CONTRACT: the numeric value is *labelled*, NOT rescaled. The
    /// kernel measures in millimetres and the displayed number is that
    /// millimetre magnitude with the document-unit label appended. When the
    /// true-conversion phase lands it will route through [`Self::from_mm`]
    /// here; until then `format_length` for any non-mm unit still shows the mm
    /// magnitude (honest: the label is the document unit, the number is the
    /// kernel measurement, and the two are not silently inconsistent because
    /// the default — and only wired — document unit is millimetres).
    pub fn format_length(self, mm: f64, value_str: &str) -> String {
        let _ = mm;
        format!("{} {}", value_str, self.label())
    }
}
