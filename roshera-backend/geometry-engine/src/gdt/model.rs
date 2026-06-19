//! GD&T tolerance data model — the production-grade Rust types for ASME Y14.5 /
//! ISO 1101 geometric dimensioning and tolerancing.
//!
//! This module is the *vocabulary*: dimensional tolerances, the fourteen
//! geometric characteristics, material-condition modifiers, datum references,
//! and the feature control frame that binds them. It is deliberately separate
//! from verification ([`super::verify`]) so the type model can be serialized,
//! round-tripped, and reasoned about independently of whether the kernel can
//! yet *measure* a given characteristic.
//!
//! ## Honesty contract
//!
//! Nothing in this module asserts conformance. A [`FeatureControlFrame`] carries
//! the *requirement*; the verdict is computed by the kernel against the actual
//! geometry. A characteristic whose verification is not yet implemented is
//! reported as [`super::verify::Conformance::NotYetVerified`] — never a false
//! pass.
//!
//! ## References
//! - ASME Y14.5-2018, *Dimensioning and Tolerancing*.
//! - ISO 1101:2017, *Geometrical product specifications (GPS) — Geometrical
//!   tolerancing*.

use serde::{Deserialize, Serialize};

use crate::primitives::persistent_id::PersistentId;

/// The fourteen geometric characteristics of ASME Y14.5 / ISO 1101, grouped by
/// type. The grouping is reflected in [`GeometricCharacteristic::needs_datum`]:
/// the form family (straightness/flatness/circularity/cylindricity) is
/// datum-free; orientation, location, and runout require a datum reference
/// frame; profile may or may not, depending on the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GeometricCharacteristic {
    // --- Form (no datum) ---
    /// Straightness — a line element must lie within a tolerance zone (ISO 1101 §18.2).
    Straightness,
    /// Flatness — a surface must lie between two parallel planes (ISO 1101 §18.3).
    Flatness,
    /// Circularity (roundness) — a circular element within two concentric circles.
    Circularity,
    /// Cylindricity — a cylindrical surface within two coaxial cylinders.
    Cylindricity,

    // --- Profile (datum optional) ---
    /// Profile of a line — a 2D profile within a tolerance band.
    ProfileLine,
    /// Profile of a surface — a 3D surface within a tolerance band.
    ProfileSurface,

    // --- Orientation (datum required) ---
    /// Parallelism — controlled feature parallel to a datum.
    Parallelism,
    /// Perpendicularity — controlled feature at 90° to a datum.
    Perpendicularity,
    /// Angularity — controlled feature at a specified angle to a datum.
    Angularity,

    // --- Location (datum required) ---
    /// Position — feature axis/center-plane within a positional zone about true position.
    Position,
    /// Concentricity — median points of a feature about a datum axis.
    Concentricity,
    /// Symmetry — median points of a feature symmetric about a datum.
    Symmetry,

    // --- Runout (datum required) ---
    /// Circular runout — single-revolution FIM about a datum axis.
    CircularRunout,
    /// Total runout — full-surface FIM about a datum axis.
    TotalRunout,
}

impl GeometricCharacteristic {
    /// The standard symbol's ASCII mnemonic (for logs / agent readouts; the
    /// drawing glyph layer maps these to the Y14.5 symbols).
    pub fn mnemonic(self) -> &'static str {
        match self {
            GeometricCharacteristic::Straightness => "STRAIGHTNESS",
            GeometricCharacteristic::Flatness => "FLATNESS",
            GeometricCharacteristic::Circularity => "CIRCULARITY",
            GeometricCharacteristic::Cylindricity => "CYLINDRICITY",
            GeometricCharacteristic::ProfileLine => "PROFILE_LINE",
            GeometricCharacteristic::ProfileSurface => "PROFILE_SURFACE",
            GeometricCharacteristic::Parallelism => "PARALLELISM",
            GeometricCharacteristic::Perpendicularity => "PERPENDICULARITY",
            GeometricCharacteristic::Angularity => "ANGULARITY",
            GeometricCharacteristic::Position => "POSITION",
            GeometricCharacteristic::Concentricity => "CONCENTRICITY",
            GeometricCharacteristic::Symmetry => "SYMMETRY",
            GeometricCharacteristic::CircularRunout => "CIRCULAR_RUNOUT",
            GeometricCharacteristic::TotalRunout => "TOTAL_RUNOUT",
        }
    }

    /// True when the characteristic is meaningless without a datum reference
    /// frame — orientation, location, and runout. The form family is datum-free
    /// and is the set this phase actually *verifies*. Profile is treated as
    /// datum-required here (datum-referenced profile is the common case and the
    /// datum-free profile-to-nominal compare needs a nominal model we don't
    /// carry yet).
    pub fn needs_datum(self) -> bool {
        matches!(
            self,
            GeometricCharacteristic::ProfileLine
                | GeometricCharacteristic::ProfileSurface
                | GeometricCharacteristic::Parallelism
                | GeometricCharacteristic::Perpendicularity
                | GeometricCharacteristic::Angularity
                | GeometricCharacteristic::Position
                | GeometricCharacteristic::Concentricity
                | GeometricCharacteristic::Symmetry
                | GeometricCharacteristic::CircularRunout
                | GeometricCharacteristic::TotalRunout
        )
    }

    /// True for the form family this phase computes a real measured value for.
    pub fn is_form(self) -> bool {
        matches!(
            self,
            GeometricCharacteristic::Straightness
                | GeometricCharacteristic::Flatness
                | GeometricCharacteristic::Circularity
                | GeometricCharacteristic::Cylindricity
        )
    }
}

/// Material condition / boundary modifier applied to a tolerance or datum
/// reference (ASME Y14.5 §3, the circled M / L / nothing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MaterialModifier {
    /// Regardless of feature size (RFS) — the default; the tolerance is constant
    /// and does not grow as the feature departs from MMC.
    Rfs,
    /// Maximum material condition (MMC) — bonus tolerance accrues as the feature
    /// departs from its maximum-material limit.
    Mmc,
    /// Least material condition (LMC).
    Lmc,
}

impl Default for MaterialModifier {
    fn default() -> Self {
        MaterialModifier::Rfs
    }
}

impl MaterialModifier {
    pub fn symbol(self) -> &'static str {
        match self {
            MaterialModifier::Rfs => "RFS",
            MaterialModifier::Mmc => "MMC",
            MaterialModifier::Lmc => "LMC",
        }
    }
}

/// A standard ISO 286 fit class, e.g. `H7` (hole) or `g6` (shaft). Carried as
/// the authored text plus the nominal; this phase models the type and resolves
/// the *symmetric envelope is unknown* honestly rather than fabricating limits
/// from an incomplete table. A later phase will expand the ISO 286 grade tables
/// into explicit upper/lower deviations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FitClass {
    /// The fit designation as authored, e.g. `"H7"`, `"g6"`, `"H7/g6"`.
    pub designation: String,
}

/// How a [`DimensionalTolerance`] bounds the nominal size.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToleranceBound {
    /// Plus/minus about the nominal: `nominal + plus` (upper), `nominal - minus`
    /// (lower). `plus` and `minus` are non-negative magnitudes; an asymmetric
    /// tolerance is expressed with different values.
    PlusMinus { plus: f64, minus: f64 },
    /// Explicit upper and lower limits (already absolute sizes, not deltas).
    Limits { upper: f64, lower: f64 },
    /// An ISO 286 fit class. The numeric envelope is not yet resolved from the
    /// grade tables, so verification against a fit class reports
    /// [`super::verify::Conformance::NotYetVerified`] rather than guessing.
    Fit(FitClass),
}

/// A dimensional (size) tolerance: a nominal value with a bound. Native kernel
/// unit is the millimetre (1 kernel unit = 1 mm), matching [`crate::units`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimensionalTolerance {
    /// The nominal (basic) size in millimetres.
    pub nominal: f64,
    /// How the actual size is bounded around the nominal.
    pub bound: ToleranceBound,
}

impl DimensionalTolerance {
    /// Symmetric plus/minus tolerance: `nominal ± tol`.
    pub fn symmetric(nominal: f64, tol: f64) -> Self {
        let t = tol.abs();
        Self {
            nominal,
            bound: ToleranceBound::PlusMinus { plus: t, minus: t },
        }
    }

    /// Asymmetric plus/minus tolerance.
    pub fn plus_minus(nominal: f64, plus: f64, minus: f64) -> Self {
        Self {
            nominal,
            bound: ToleranceBound::PlusMinus {
                plus: plus.abs(),
                minus: minus.abs(),
            },
        }
    }

    /// Explicit absolute limits.
    pub fn limits(nominal: f64, lower: f64, upper: f64) -> Self {
        Self {
            nominal,
            bound: ToleranceBound::Limits { upper, lower },
        }
    }

    /// Fit-class tolerance (numeric envelope not yet resolved).
    pub fn fit(nominal: f64, designation: impl Into<String>) -> Self {
        Self {
            nominal,
            bound: ToleranceBound::Fit(FitClass {
                designation: designation.into(),
            }),
        }
    }

    /// The resolved `[lower, upper]` absolute size limits, when the bound is a
    /// numeric one. `None` for an unresolved fit class — the caller must report
    /// `NotYetVerified` rather than treat an absent envelope as in-spec.
    pub fn limit_range(&self) -> Option<(f64, f64)> {
        match &self.bound {
            ToleranceBound::PlusMinus { plus, minus } => {
                Some((self.nominal - minus, self.nominal + plus))
            }
            ToleranceBound::Limits { upper, lower } => Some((*lower, *upper)),
            ToleranceBound::Fit(_) => None,
        }
    }
}

/// A reference to a datum feature: the drawing label (`"A"`, `"B"`, `"C"`) bound
/// to a persistent topological id (a face / axis / plane on the model). The
/// persistent id is what survives regeneration; the label is the drawing-facing
/// name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatumRef {
    /// Drawing label, e.g. `"A"`. The precedence in a feature control frame is
    /// the *order* of the `datum_refs` vector, per Y14.5.
    pub label: String,
    /// Material modifier on the datum reference itself (datum shift, §3.3.2).
    #[serde(default)]
    pub modifier: MaterialModifier,
}

impl DatumRef {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            modifier: MaterialModifier::Rfs,
        }
    }
}

/// What kind of geometry a datum feature establishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DatumKind {
    /// A planar datum feature → a datum plane.
    Plane,
    /// A cylindrical datum feature → a datum axis.
    Axis,
    /// A point datum.
    Point,
}

/// A datum definition: the label, the kind of reference frame it establishes,
/// and the persistent id of the feature it is derived from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Datum {
    pub label: String,
    pub kind: DatumKind,
    /// The persistent id of the face/feature this datum is derived from.
    pub feature: PersistentId,
}

impl Datum {
    pub fn new(label: impl Into<String>, kind: DatumKind, feature: PersistentId) -> Self {
        Self {
            label: label.into(),
            kind,
            feature,
        }
    }
}

/// A feature control frame (FCF): the boxed sequence on a drawing that states a
/// geometric characteristic, its tolerance value, the material modifier, and the
/// ordered datum references. This is the *requirement*; the verdict is computed
/// elsewhere.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureControlFrame {
    pub characteristic: GeometricCharacteristic,
    /// The tolerance zone size in millimetres (the value in the second
    /// compartment of the FCF). For a diametral zone this is the diameter.
    pub tolerance_value: f64,
    /// `true` when the tolerance zone is diametral (the circled-Ø prefix), e.g.
    /// a positional zone for an axis.
    #[serde(default)]
    pub diametral_zone: bool,
    /// Material modifier on the tolerance value.
    #[serde(default)]
    pub modifier: MaterialModifier,
    /// Ordered datum references (primary, secondary, tertiary). Empty for the
    /// datum-free form family.
    #[serde(default)]
    pub datum_refs: Vec<DatumRef>,
}

impl FeatureControlFrame {
    /// A datum-free form-tolerance FCF (straightness/flatness/circularity/
    /// cylindricity): characteristic + value, no datum references.
    pub fn form(characteristic: GeometricCharacteristic, tolerance_value: f64) -> Self {
        Self {
            characteristic,
            tolerance_value,
            diametral_zone: false,
            modifier: MaterialModifier::Rfs,
            datum_refs: Vec::new(),
        }
    }

    /// True when this FCF is structurally complete for verification *as a form
    /// tolerance*: a form characteristic with no missing datum references.
    pub fn is_datum_free_form(&self) -> bool {
        self.characteristic.is_form() && self.datum_refs.is_empty()
    }
}

/// A single GD&T annotation attached to a feature on the model: either a
/// dimensional (size) tolerance or a geometric feature control frame. The
/// feature is named by a [`PersistentId`] (face / edge / axis) so the annotation
/// survives regeneration and parameter edits, mirroring the kernel's persistent
/// naming.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Annotation {
    /// A size tolerance on a feature (e.g. a hole/boss diameter, a slot width).
    Dimensional(DimensionalTolerance),
    /// A geometric tolerance (feature control frame).
    Geometric(FeatureControlFrame),
}

impl Annotation {
    /// A short human/agent-facing label for the annotation kind.
    pub fn kind_label(&self) -> &'static str {
        match self {
            Annotation::Dimensional(_) => "dimensional",
            Annotation::Geometric(_) => "geometric",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plus_minus_limit_range() {
        let t = DimensionalTolerance::symmetric(10.0, 0.1);
        assert_eq!(t.limit_range(), Some((9.9, 10.1)));
        let a = DimensionalTolerance::plus_minus(10.0, 0.2, 0.05);
        assert_eq!(a.limit_range(), Some((9.95, 10.2)));
        let l = DimensionalTolerance::limits(10.0, 9.8, 10.3);
        assert_eq!(l.limit_range(), Some((9.8, 10.3)));
    }

    #[test]
    fn fit_class_has_no_resolved_range() {
        let f = DimensionalTolerance::fit(10.0, "H7");
        assert_eq!(
            f.limit_range(),
            None,
            "unresolved fit must not fabricate limits"
        );
    }

    #[test]
    fn form_characteristics_are_datum_free() {
        for c in [
            GeometricCharacteristic::Straightness,
            GeometricCharacteristic::Flatness,
            GeometricCharacteristic::Circularity,
            GeometricCharacteristic::Cylindricity,
        ] {
            assert!(c.is_form());
            assert!(!c.needs_datum());
        }
        for c in [
            GeometricCharacteristic::Position,
            GeometricCharacteristic::Perpendicularity,
            GeometricCharacteristic::TotalRunout,
        ] {
            assert!(!c.is_form());
            assert!(c.needs_datum());
        }
    }

    #[test]
    fn annotation_round_trips_through_json() {
        let fcf = FeatureControlFrame::form(GeometricCharacteristic::Flatness, 0.05);
        let ann = Annotation::Geometric(fcf);
        let json = serde_json::to_string(&ann).expect("serialize");
        let back: Annotation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ann, back);
    }
}
