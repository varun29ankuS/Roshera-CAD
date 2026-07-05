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
//! ## Canonical formatters (Phase 2)
//!
//! [`LengthUnit::format_len`], [`LengthUnit::format_area`], and
//! [`LengthUnit::format_vol`] are the **one** path a millimetre value takes to
//! become a string. Any code that converts a raw `f64` to display text without
//! routing through these formatters is a defect: the model stays mm-native
//! forever, and only the *moment a value becomes text* applies conversion.
//!
//! Supporting helpers: [`LengthUnit::suffix`] (the unit abbreviation appended
//! to formatted values), [`LengthUnit::per_mm`] (mm per one of this unit —
//! the conversion factor), [`LengthUnit::precision`] (decimal places per
//! drafting convention).
//!
//! Precision table (drafting convention):
//! - mm → 2 dp   (`"25.40mm"`)
//! - cm → 3 dp   (`"2.540cm"`)
//! - m  → 4 dp   (`"1.0000m"`)
//! - in → 3 dp   (`"1.000in"`)
//! - ft → 4 dp   (`"1.0000ft"`)

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
    /// Foot.
    Foot,
}

impl Default for LengthUnit {
    fn default() -> Self {
        LengthUnit::Millimetre
    }
}

impl LengthUnit {
    /// The short label drawn on a dimension and reported in the `unit` field
    /// (`"mm"`, `"cm"`, `"m"`, `"in"`, `"ft"`). ASCII so the 5×7 overlay
    /// font renders it. Kept for backwards compatibility; prefer
    /// [`Self::suffix`] in new code.
    pub fn label(self) -> &'static str {
        match self {
            LengthUnit::Millimetre => "mm",
            LengthUnit::Centimetre => "cm",
            LengthUnit::Metre => "m",
            LengthUnit::Inch => "in",
            LengthUnit::Foot => "ft",
        }
    }

    /// The unit suffix appended to formatted values, e.g. `"mm"`, `"in"`,
    /// `"ft"`. Identical to [`Self::label`] but named for clarity at
    /// call sites that build formatted strings.
    pub fn suffix(self) -> &'static str {
        self.label()
    }

    /// Millimetres per one of this unit — the conversion factor against the
    /// kernel's native millimetre.
    pub fn per_mm(self) -> f64 {
        match self {
            LengthUnit::Millimetre => 1.0,
            LengthUnit::Centimetre => 10.0,
            LengthUnit::Metre => 1000.0,
            LengthUnit::Inch => 25.4,
            LengthUnit::Foot => 304.8,
        }
    }

    /// Millimetres per one of this unit — the conversion factor against the
    /// kernel's native millimetre. This is the seam future true-conversion
    /// phases build on (input parsing, STEP `LENGTH_UNIT`, scaling).
    ///
    /// Alias kept for callers that depend on the `mm_per_unit` name.
    pub fn mm_per_unit(self) -> f64 {
        self.per_mm()
    }

    /// Decimal places to show when formatting a value in this unit, per
    /// drafting convention:
    ///
    /// | unit | dp |
    /// |------|----|
    /// | mm   |  2 |
    /// | cm   |  3 |
    /// | m    |  4 |
    /// | in   |  3 |
    /// | ft   |  4 |
    pub fn precision(self) -> usize {
        match self {
            LengthUnit::Millimetre => 2,
            LengthUnit::Centimetre => 3,
            LengthUnit::Metre => 4,
            LengthUnit::Inch => 3,
            LengthUnit::Foot => 4,
        }
    }

    /// Convert a millimetre value (kernel-native) into this unit's magnitude.
    /// The companion to [`Self::per_mm`]; the future-conversion seam. Not
    /// used by the Phase-1 formatters, which label without rescaling.
    pub fn from_mm(self, mm: f64) -> f64 {
        mm / self.per_mm()
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
            "ft" | "foot" | "feet" | "'" => Some(LengthUnit::Foot),
            _ => None,
        }
    }

    /// Format a kernel-native (millimetre) **length** for display.
    ///
    /// Converts `mm` to the target unit via [`Self::per_mm`] and formats to
    /// the unit's drafting precision, appending the suffix. Example outputs:
    /// `"25.40mm"`, `"1.000in"`, `"1.0000ft"`.
    ///
    /// This is the **one canonical path** for length → string; every text
    /// surface must route through here. Any raw `f64 → text` that bypasses
    /// this method is a defect.
    pub fn format_len(self, mm: f64) -> String {
        let converted = mm / self.per_mm();
        format!(
            "{:.prec$}{}",
            converted,
            self.suffix(),
            prec = self.precision()
        )
    }

    /// Format a kernel-native (mm²) **area** for display. Converts by
    /// `per_mm²` (i.e. `per_mm()²`) and appends `"²"` after the suffix.
    /// Example: `"645.160mm²"` (1 in² in mm²).
    pub fn format_area(self, mm2: f64) -> String {
        let factor = self.per_mm() * self.per_mm();
        let converted = mm2 / factor;
        format!(
            "{:.prec$}{}²",
            converted,
            self.suffix(),
            prec = self.precision()
        )
    }

    /// Format a kernel-native (mm³) **volume** for display. Converts by
    /// `per_mm³` and appends `"³"` after the suffix.
    /// Example: `"16387.064mm³"` (1 in³ in mm³ — displayed as `"1.000in³"`).
    pub fn format_vol(self, mm3: f64) -> String {
        let factor = self.per_mm() * self.per_mm() * self.per_mm();
        let converted = mm3 / factor;
        format!(
            "{:.prec$}{}³",
            converted,
            self.suffix(),
            prec = self.precision()
        )
    }

    /// Format a kernel-native (millimetre) tolerance value for use inside a
    /// **GD&T Feature Control Frame** — suffix omitted.
    ///
    /// ISO 1101 §7.2: the numeric tolerance is written bare inside the FCF
    /// cell; the unit is declared once in the drawing's title-block notes note
    /// (set via `Drawing::set_unit_notes`). Appending a suffix inside every
    /// FCF frame violates the standard and wastes horizontal space.
    ///
    /// The conversion and precision core is **identical** to [`Self::format_len`]:
    /// the value is divided by `per_mm()` and formatted to `precision()` decimal
    /// places. The only difference is that no suffix is appended. This sharing
    /// guarantees that a tolerance expressed in the document unit always
    /// matches the numeric part of the corresponding length callout — no
    /// independent rounding path.
    ///
    /// # Example
    ///
    /// ```
    /// # use geometry_engine::units::LengthUnit;
    /// // 0.05 mm in a mm document: "0.05"
    /// assert_eq!(LengthUnit::Millimetre.format_gdt_tolerance(0.05), "0.05");
    /// // 25.4 mm in an inch document: "1.000" (same as format_len minus "in")
    /// assert_eq!(LengthUnit::Inch.format_gdt_tolerance(25.4), "1.000");
    /// ```
    pub fn format_gdt_tolerance(self, mm: f64) -> String {
        let converted = mm / self.per_mm();
        format!("{:.prec$}", converted, prec = self.precision())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Foot variant ──────────────────────────────────────────────────────────

    #[test]
    fn foot_is_a_variant() {
        let u = LengthUnit::Foot;
        assert_eq!(u.suffix(), "ft");
        assert_eq!(u.per_mm(), 304.8);
        assert_eq!(u.precision(), 4);
    }

    #[test]
    fn foot_parse_round_trips() {
        assert_eq!(LengthUnit::parse("ft"), Some(LengthUnit::Foot));
        assert_eq!(LengthUnit::parse("foot"), Some(LengthUnit::Foot));
        assert_eq!(LengthUnit::parse("feet"), Some(LengthUnit::Foot));
        assert_eq!(LengthUnit::parse("'"), Some(LengthUnit::Foot));
        assert_eq!(LengthUnit::parse("FT"), Some(LengthUnit::Foot));
    }

    // ── format_len exact table ────────────────────────────────────────────────

    #[test]
    fn format_len_25_4mm_is_1_000in() {
        assert_eq!(LengthUnit::Inch.format_len(25.4), "1.000in");
    }

    #[test]
    fn format_len_304_8mm_is_1_0000ft() {
        assert_eq!(LengthUnit::Foot.format_len(304.8), "1.0000ft");
    }

    #[test]
    fn format_len_1000mm_is_1_0000m() {
        assert_eq!(LengthUnit::Metre.format_len(1000.0), "1.0000m");
    }

    #[test]
    fn format_len_25_4mm_cm_is_2_540cm() {
        assert_eq!(LengthUnit::Centimetre.format_len(25.4), "2.540cm");
    }

    #[test]
    fn format_len_mm_native_round_trips() {
        // 40 mm should format as "40.00mm".
        assert_eq!(LengthUnit::Millimetre.format_len(40.0), "40.00mm");
    }

    // ── format_area ──────────────────────────────────────────────────────────

    #[test]
    fn format_area_645_16mm2_is_1_000in2() {
        // 1 in² = 25.4² = 645.16 mm². Should format as "1.000in²".
        let mm2 = 25.4 * 25.4;
        assert_eq!(LengthUnit::Inch.format_area(mm2), "1.000in²");
    }

    #[test]
    fn format_area_100mm2_in_mm_is_100_00mm2() {
        assert_eq!(LengthUnit::Millimetre.format_area(100.0), "100.00mm²");
    }

    // ── format_vol ───────────────────────────────────────────────────────────

    #[test]
    fn format_vol_16387_064mm3_is_1_000in3() {
        // 1 in³ = 25.4³ ≈ 16387.064 mm³. Should format as "1.000in³".
        let mm3 = 25.4_f64.powi(3);
        assert_eq!(LengthUnit::Inch.format_vol(mm3), "1.000in³");
    }

    #[test]
    fn format_vol_1e9mm3_is_1_0000m3() {
        // 1 m³ = 1_000³ mm³ = 1e9 mm³.
        assert_eq!(LengthUnit::Metre.format_vol(1e9), "1.0000m³");
    }

    // ── suffix / per_mm / precision table ────────────────────────────────────

    #[test]
    fn suffix_table_exhaustive() {
        assert_eq!(LengthUnit::Millimetre.suffix(), "mm");
        assert_eq!(LengthUnit::Centimetre.suffix(), "cm");
        assert_eq!(LengthUnit::Metre.suffix(), "m");
        assert_eq!(LengthUnit::Inch.suffix(), "in");
        assert_eq!(LengthUnit::Foot.suffix(), "ft");
    }

    #[test]
    fn per_mm_table_exact() {
        assert_eq!(LengthUnit::Millimetre.per_mm(), 1.0);
        assert_eq!(LengthUnit::Centimetre.per_mm(), 10.0);
        assert_eq!(LengthUnit::Metre.per_mm(), 1000.0);
        assert_eq!(LengthUnit::Inch.per_mm(), 25.4);
        assert_eq!(LengthUnit::Foot.per_mm(), 304.8);
    }

    #[test]
    fn precision_table_exact() {
        assert_eq!(LengthUnit::Millimetre.precision(), 2);
        assert_eq!(LengthUnit::Centimetre.precision(), 3);
        assert_eq!(LengthUnit::Metre.precision(), 4);
        assert_eq!(LengthUnit::Inch.precision(), 3);
        assert_eq!(LengthUnit::Foot.precision(), 4);
    }

    // ── Backwards compat: mm_per_unit alias still works ──────────────────────

    #[test]
    fn mm_per_unit_alias_works() {
        assert_eq!(LengthUnit::Foot.mm_per_unit(), 304.8);
        assert_eq!(LengthUnit::Inch.mm_per_unit(), 25.4);
    }

    // ── format_gdt_tolerance: suffix-free, same numeric core as format_len ────

    /// GDT tolerance: mm document — bare number, same precision as format_len.
    #[test]
    fn format_gdt_tolerance_mm_no_suffix() {
        // "0.05mm" from format_len → "0.05" from format_gdt_tolerance.
        let mm_val = 0.05;
        let with_suffix = LengthUnit::Millimetre.format_len(mm_val);
        let bare = LengthUnit::Millimetre.format_gdt_tolerance(mm_val);
        assert_eq!(
            with_suffix,
            format!("{}mm", bare),
            "format_gdt_tolerance must be format_len minus the suffix (mm document)"
        );
        assert_eq!(bare, "0.05");
    }

    /// GDT tolerance: inch document — same numeric conversion as format_len,
    /// no suffix.  0.05 mm = 0.001968… in → formatted to 3 dp = "0.002".
    #[test]
    fn format_gdt_tolerance_inch_matches_format_len_numeric_part() {
        let mm_val = 0.05;
        let with_suffix = LengthUnit::Inch.format_len(mm_val);
        // Strip the "in" suffix to get the numeric part.
        let numeric_part = with_suffix
            .strip_suffix("in")
            .expect("format_len must produce an 'in' suffix");
        let bare = LengthUnit::Inch.format_gdt_tolerance(mm_val);
        assert_eq!(
            bare, numeric_part,
            "format_gdt_tolerance must equal the numeric part of format_len (inch document)"
        );
    }

    /// GDT tolerance: 25.4 mm in inch document → "1.000" (1 in, 3 dp, no suffix).
    #[test]
    fn format_gdt_tolerance_25_4mm_is_1_000_inch_no_suffix() {
        assert_eq!(LengthUnit::Inch.format_gdt_tolerance(25.4), "1.000");
    }

    /// GDT tolerance: mm document, value ≥ 1.0, trailing zeros preserved
    /// (GDT frames use the full precision — no trailing-zero stripping).
    #[test]
    fn format_gdt_tolerance_mm_preserves_trailing_zeros() {
        // 2.50 mm → "2.50" (2 dp for mm), NOT "2.5".
        assert_eq!(LengthUnit::Millimetre.format_gdt_tolerance(2.5), "2.50");
    }
}
