//! The built-in material library: linear elastic values for a static study,
//! each with where it comes from and -- for the yield strength, which a
//! safety factor is measured against -- whether it is a SPECIFIED MINIMUM
//! (a standard's guaranteed floor, fit for a design check) or a TYPICAL
//! value (a datasheet average, "not for design"). A study on a typical
//! yield says so in its result; it is never passed off as a minimum.
//!
//! Sources were checked 2026-11-23 against the standards named and the ASM /
//! Aluminum Association data sheets for the aluminium alloys. A customer's
//! own values override these in the study itself.

use serde::{Deserialize, Serialize};

/// What a yield value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum YieldBasis {
    /// The floor a standard guarantees for the product and size named.
    SpecifiedMinimum,
    /// A typical (average) value from a data sheet; not a design minimum.
    Typical,
}

/// One library material.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryMaterial {
    pub id: &'static str,
    pub name: &'static str,
    pub youngs_modulus_mpa: f64,
    pub poisson_ratio: f64,
    pub yield_strength_mpa: f64,
    pub yield_basis: YieldBasis,
    /// Where the yield strength comes from.
    pub yield_source: &'static str,
    /// Where the stiffness (E, nu) comes from.
    pub stiffness_source: &'static str,
}

/// The library, in the order it is offered.
pub fn library() -> Vec<LibraryMaterial> {
    vec![
        LibraryMaterial {
            id: "steel_s355",
            name: "Structural steel S355, up to 16 mm thick",
            youngs_modulus_mpa: 210_000.0,
            poisson_ratio: 0.3,
            yield_strength_mpa: 355.0,
            yield_basis: YieldBasis::SpecifiedMinimum,
            yield_source: "EN 10025-2, minimum yield strength for thickness up to 16 mm",
            stiffness_source: "EN 1993-1-1, 3.2.6 (E = 210 000 N/mm2, nu = 0.3)",
        },
        LibraryMaterial {
            id: "stainless_304",
            name: "Stainless steel 304, annealed",
            youngs_modulus_mpa: 193_000.0,
            poisson_ratio: 0.29,
            yield_strength_mpa: 205.0,
            yield_basis: YieldBasis::SpecifiedMinimum,
            yield_source: "ASTM A240 / ASME SA-240, annealed: minimum 0.2% yield 205 MPa (30 ksi)",
            stiffness_source: "typical data-sheet values (E 193 GPa; 193-200 GPa quoted), nu 0.29",
        },
        LibraryMaterial {
            id: "titanium_grade5",
            name: "Titanium Ti-6Al-4V, Grade 5",
            youngs_modulus_mpa: 114_000.0,
            poisson_ratio: 0.34,
            yield_strength_mpa: 828.0,
            yield_basis: YieldBasis::SpecifiedMinimum,
            yield_source: "ASTM B265 Grade 5: minimum 0.2% yield 828 MPa (120 ksi)",
            stiffness_source: "typical data-sheet values (E about 114 GPa, nu 0.34)",
        },
        LibraryMaterial {
            id: "aluminium_6061_t6",
            name: "Aluminium 6061-T6",
            youngs_modulus_mpa: 68_900.0,
            poisson_ratio: 0.33,
            yield_strength_mpa: 276.0,
            yield_basis: YieldBasis::Typical,
            yield_source: "ASM / Aluminum Association TYPICAL value, not a design minimum",
            stiffness_source: "ASM data sheet (E 68.9 GPa; nu 0.33 estimated from similar alloys)",
        },
        LibraryMaterial {
            id: "aluminium_7075_t6",
            name: "Aluminium 7075-T6",
            youngs_modulus_mpa: 71_700.0,
            poisson_ratio: 0.33,
            yield_strength_mpa: 503.0,
            yield_basis: YieldBasis::Typical,
            yield_source: "ASM / Aluminum Association TYPICAL value, not a design minimum",
            stiffness_source: "ASM data sheet (E 71.7 GPa, nu 0.33)",
        },
    ]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// Every entry is physical and names its sources; ids are unique; a
    /// typical yield says so in its source -- so a result quoting the source
    /// can never present a typical value as a minimum.
    #[test]
    fn every_material_is_physical_sourced_and_honest_about_its_yield() {
        let lib = library();
        let mut ids = std::collections::HashSet::new();
        for m in &lib {
            assert!(ids.insert(m.id), "duplicate id {}", m.id);
            assert!(
                m.youngs_modulus_mpa > 0.0 && m.yield_strength_mpa > 0.0,
                "{}",
                m.id
            );
            assert!(m.poisson_ratio > 0.0 && m.poisson_ratio < 0.5, "{}", m.id);
            assert!(
                !m.yield_source.is_empty() && !m.stiffness_source.is_empty(),
                "{}",
                m.id
            );
            if m.yield_basis == YieldBasis::Typical {
                assert!(
                    m.yield_source.contains("TYPICAL"),
                    "{} must say its yield is typical",
                    m.id
                );
            }
        }
    }
}
