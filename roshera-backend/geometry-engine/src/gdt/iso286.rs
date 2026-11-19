//! ISO 286 limits and fits, resolved from the nominal size.
//!
//! A fit class such as `H7` (a hole) or `g6` (a shaft) names two things: a
//! **fundamental deviation** (the letter: where the tolerance zone sits
//! relative to the nominal size) and a **standard tolerance grade** (the
//! number: how wide the zone is). Both depend on the nominal size band. This
//! module holds those tables and turns `(nominal, "H7")` into the two limit
//! deviations.
//!
//! ## Table-driven, never a formula
//!
//! Every number here is a tabulated value of ISO 286-1:2010 (Table 1 for the
//! standard tolerance grades, Tables 2 and 3 for the fundamental deviations).
//! Nothing is interpolated and nothing is computed from the tolerance-unit
//! formula: a class or size this module does not tabulate is refused with a
//! typed [`FitRefusal`], never approximated.
//!
//! ## Scope
//!
//! * Nominal sizes above 0 mm up to and including 500 mm.
//! * Grades IT3 to IT16.
//! * Shafts `c d e f g h js k m n p r s u`.
//! * Holes `C D E F G H JS` at every grade above, and `K M N` at grades 6 to 8
//!   and `P R S U` at grades 6 and 7, where the hole deviation follows from the
//!   shaft deviation by the standard's delta rule.
//!
//! Everything else (`a b j t v x y z za zb zc` and their holes, hole classes
//! K to U at other grades, sizes above 500 mm) is refused.

use std::fmt;

/// Why a fit class could not be resolved.
#[derive(Debug, Clone, PartialEq)]
pub enum FitRefusal {
    /// The designation is not `<letters><grade>` (e.g. `H7`, `js6`) or a
    /// `<hole>/<shaft>` pair of those.
    BadDesignation(String),
    /// The nominal size is not a finite size in `(0, 500]` mm.
    NominalOutOfRange {
        /// The nominal that was asked for, in millimetres.
        nominal_mm: f64,
    },
    /// The class is a real ISO 286 class this module does not tabulate.
    UnsupportedClass {
        /// The class as written, e.g. `"za6"`.
        class: String,
        /// What is missing, in words.
        reason: String,
    },
    /// A pair was given where one class was needed, or one where a pair was.
    WrongArity(String),
}

impl fmt::Display for FitRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FitRefusal::BadDesignation(d) => write!(
                f,
                "'{d}' is not an ISO 286 class: write a letter and a grade, e.g. H7 or g6"
            ),
            FitRefusal::NominalOutOfRange { nominal_mm } => write!(
                f,
                "ISO 286 limits are tabulated here for sizes above 0 up to 500 mm; \
                 {nominal_mm} mm is outside that range"
            ),
            FitRefusal::UnsupportedClass { class, reason } => {
                write!(f, "ISO 286 class {class} is not tabulated here: {reason}")
            }
            FitRefusal::WrongArity(d) => write!(f, "{d}"),
        }
    }
}

impl std::error::Error for FitRefusal {}

/// Upper bounds (inclusive) of the thirteen main size bands, in millimetres:
/// "above the previous bound, up to and including this one".
const BAND_UPPER_MM: [f64; 13] = [
    3.0, 6.0, 10.0, 18.0, 30.0, 50.0, 80.0, 120.0, 180.0, 250.0, 315.0, 400.0, 500.0,
];

/// Upper bounds (inclusive) of the intermediate size bands the standard uses
/// for the deviations that vary inside a main band (`c`, `r`, `s`, `u`).
const SUB_BAND_UPPER_MM: [f64; 25] = [
    3.0, 6.0, 10.0, 14.0, 18.0, 24.0, 30.0, 40.0, 50.0, 65.0, 80.0, 100.0, 120.0, 140.0, 160.0,
    180.0, 200.0, 225.0, 250.0, 280.0, 315.0, 355.0, 400.0, 450.0, 500.0,
];

/// Standard tolerance grades IT3..=IT16 in micrometres, one row per grade,
/// one column per main size band (ISO 286-1:2010 Table 1).
const IT_UM: [[f64; 13]; 14] = [
    // IT3
    [
        2.0, 2.5, 2.5, 3.0, 4.0, 4.0, 5.0, 6.0, 8.0, 10.0, 12.0, 13.0, 15.0,
    ],
    // IT4
    [
        3.0, 4.0, 4.0, 5.0, 6.0, 7.0, 8.0, 10.0, 12.0, 14.0, 16.0, 18.0, 20.0,
    ],
    // IT5
    [
        4.0, 5.0, 6.0, 8.0, 9.0, 11.0, 13.0, 15.0, 18.0, 20.0, 23.0, 25.0, 27.0,
    ],
    // IT6
    [
        6.0, 8.0, 9.0, 11.0, 13.0, 16.0, 19.0, 22.0, 25.0, 29.0, 32.0, 36.0, 40.0,
    ],
    // IT7
    [
        10.0, 12.0, 15.0, 18.0, 21.0, 25.0, 30.0, 35.0, 40.0, 46.0, 52.0, 57.0, 63.0,
    ],
    // IT8
    [
        14.0, 18.0, 22.0, 27.0, 33.0, 39.0, 46.0, 54.0, 63.0, 72.0, 81.0, 89.0, 97.0,
    ],
    // IT9
    [
        25.0, 30.0, 36.0, 43.0, 52.0, 62.0, 74.0, 87.0, 100.0, 115.0, 130.0, 140.0, 155.0,
    ],
    // IT10
    [
        40.0, 48.0, 58.0, 70.0, 84.0, 100.0, 120.0, 140.0, 160.0, 185.0, 210.0, 230.0, 250.0,
    ],
    // IT11
    [
        60.0, 75.0, 90.0, 110.0, 130.0, 160.0, 190.0, 220.0, 250.0, 290.0, 320.0, 360.0, 400.0,
    ],
    // IT12
    [
        100.0, 120.0, 150.0, 180.0, 210.0, 250.0, 300.0, 350.0, 400.0, 460.0, 520.0, 570.0, 630.0,
    ],
    // IT13
    [
        140.0, 180.0, 220.0, 270.0, 330.0, 390.0, 460.0, 540.0, 630.0, 720.0, 810.0, 890.0, 970.0,
    ],
    // IT14
    [
        250.0, 300.0, 360.0, 430.0, 520.0, 620.0, 740.0, 870.0, 1000.0, 1150.0, 1300.0, 1400.0,
        1550.0,
    ],
    // IT15
    [
        400.0, 480.0, 580.0, 700.0, 840.0, 1000.0, 1200.0, 1400.0, 1600.0, 1850.0, 2100.0, 2300.0,
        2500.0,
    ],
    // IT16
    [
        600.0, 750.0, 900.0, 1100.0, 1300.0, 1600.0, 1900.0, 2200.0, 2500.0, 2900.0, 3200.0,
        3600.0, 4000.0,
    ],
];

/// Lowest and highest tabulated grade.
const GRADE_MIN: u8 = 3;
const GRADE_MAX: u8 = 16;

// Shaft fundamental deviations in micrometres per MAIN band (ISO 286-1:2010
// Table 2). For `d`..`g` this is the UPPER deviation `es` (negative: the zone
// lies below the nominal); for `k`..`p` it is the LOWER deviation `ei`.
const SHAFT_D_ES: [f64; 13] = [
    -20.0, -30.0, -40.0, -50.0, -65.0, -80.0, -100.0, -120.0, -145.0, -170.0, -190.0, -210.0,
    -230.0,
];
const SHAFT_E_ES: [f64; 13] = [
    -14.0, -20.0, -25.0, -32.0, -40.0, -50.0, -60.0, -72.0, -85.0, -100.0, -110.0, -125.0, -135.0,
];
const SHAFT_F_ES: [f64; 13] = [
    -6.0, -10.0, -13.0, -16.0, -20.0, -25.0, -30.0, -36.0, -43.0, -50.0, -56.0, -62.0, -68.0,
];
const SHAFT_G_ES: [f64; 13] = [
    -2.0, -4.0, -5.0, -6.0, -7.0, -9.0, -10.0, -12.0, -14.0, -15.0, -17.0, -18.0, -20.0,
];
/// `k` at grades 4 to 7; at every other grade the lower deviation is 0.
const SHAFT_K_EI_4_TO_7: [f64; 13] = [
    0.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 3.0, 3.0, 4.0, 4.0, 4.0, 5.0,
];
const SHAFT_M_EI: [f64; 13] = [
    2.0, 4.0, 6.0, 7.0, 8.0, 9.0, 11.0, 13.0, 15.0, 17.0, 20.0, 21.0, 23.0,
];
const SHAFT_N_EI: [f64; 13] = [
    4.0, 8.0, 10.0, 12.0, 15.0, 17.0, 20.0, 23.0, 27.0, 31.0, 34.0, 37.0, 40.0,
];
const SHAFT_P_EI: [f64; 13] = [
    6.0, 12.0, 15.0, 18.0, 22.0, 26.0, 32.0, 37.0, 43.0, 50.0, 56.0, 62.0, 68.0,
];

// Shaft fundamental deviations in micrometres per INTERMEDIATE band
// (`SUB_BAND_UPPER_MM`): `c` is `es`, `r` / `s` / `u` are `ei`.
const SHAFT_C_ES: [f64; 25] = [
    -60.0, -70.0, -80.0, -95.0, -95.0, -110.0, -110.0, -120.0, -130.0, -140.0, -150.0, -170.0,
    -180.0, -200.0, -210.0, -230.0, -240.0, -260.0, -280.0, -300.0, -330.0, -360.0, -400.0, -440.0,
    -480.0,
];
const SHAFT_R_EI: [f64; 25] = [
    10.0, 15.0, 19.0, 23.0, 23.0, 28.0, 28.0, 34.0, 34.0, 41.0, 43.0, 51.0, 54.0, 63.0, 65.0, 68.0,
    77.0, 80.0, 84.0, 94.0, 98.0, 108.0, 114.0, 126.0, 132.0,
];
const SHAFT_S_EI: [f64; 25] = [
    14.0, 19.0, 23.0, 28.0, 28.0, 35.0, 35.0, 43.0, 43.0, 53.0, 59.0, 71.0, 79.0, 92.0, 100.0,
    108.0, 122.0, 130.0, 140.0, 158.0, 170.0, 190.0, 208.0, 232.0, 252.0,
];
const SHAFT_U_EI: [f64; 25] = [
    18.0, 23.0, 28.0, 33.0, 33.0, 41.0, 48.0, 60.0, 70.0, 87.0, 102.0, 124.0, 144.0, 170.0, 190.0,
    210.0, 236.0, 258.0, 284.0, 315.0, 350.0, 390.0, 435.0, 490.0, 540.0,
];

/// One resolved class: the two limit deviations of a hole or a shaft.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedClass {
    /// The class as normalised, e.g. `"H7"`, `"js6"`.
    pub class: String,
    /// True for a hole (upper-case letter), false for a shaft.
    pub hole: bool,
    /// The tolerance grade number (the `7` of `H7`).
    pub grade: u8,
    /// Upper limit deviation from the nominal, millimetres (signed).
    pub upper_mm: f64,
    /// Lower limit deviation from the nominal, millimetres (signed).
    pub lower_mm: f64,
}

impl ResolvedClass {
    /// The absolute `(lower, upper)` limits of size for `nominal_mm`.
    pub fn limits(&self, nominal_mm: f64) -> (f64, f64) {
        (nominal_mm + self.lower_mm, nominal_mm + self.upper_mm)
    }
}

/// The character of a hole/shaft pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitKind {
    /// The shaft is always smaller than the hole.
    Clearance,
    /// Clearance or interference, depending on the actual sizes.
    Transition,
    /// The shaft is always larger than the hole.
    Interference,
}

impl FitKind {
    /// Lower-case name for readback.
    pub fn label(self) -> &'static str {
        match self {
            FitKind::Clearance => "clearance",
            FitKind::Transition => "transition",
            FitKind::Interference => "interference",
        }
    }
}

/// A resolved hole/shaft pair, e.g. `H7/g6`.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedFit {
    pub hole: ResolvedClass,
    pub shaft: ResolvedClass,
    /// Smallest hole minus largest shaft, millimetres (negative = interference).
    pub min_clearance_mm: f64,
    /// Largest hole minus smallest shaft, millimetres.
    pub max_clearance_mm: f64,
    pub kind: FitKind,
}

fn band_index(bounds: &[f64], nominal_mm: f64) -> Result<usize, FitRefusal> {
    if !nominal_mm.is_finite() || nominal_mm <= 0.0 {
        return Err(FitRefusal::NominalOutOfRange { nominal_mm });
    }
    bounds
        .iter()
        .position(|&upper| nominal_mm <= upper)
        .ok_or(FitRefusal::NominalOutOfRange { nominal_mm })
}

/// Round a micrometre sum to the nearest 0.5 um: every tabulated value is a
/// multiple of 0.5 um, so this only removes floating-point residue.
fn um_to_mm(um: f64) -> f64 {
    ((um * 2.0).round() / 2.0) / 1000.0
}

fn it_um(grade: u8, band: usize) -> Option<f64> {
    if !(GRADE_MIN..=GRADE_MAX).contains(&grade) {
        return None;
    }
    IT_UM
        .get(usize::from(grade - GRADE_MIN))
        .and_then(|row| row.get(band))
        .copied()
}

/// Split `"H7"` into (`"H"`, 7); `"js6"` into (`"js"`, 6).
fn split_class(class: &str) -> Result<(String, u8), FitRefusal> {
    let class = class.trim();
    let bad = || FitRefusal::BadDesignation(class.to_string());
    let letters: String = class
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    let digits = &class[letters.len()..];
    if letters.is_empty() || digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(bad());
    }
    let all_upper = letters.chars().all(|c| c.is_ascii_uppercase());
    let all_lower = letters.chars().all(|c| c.is_ascii_lowercase());
    if !(all_upper || all_lower) {
        return Err(bad());
    }
    let grade: u8 = digits.parse().map_err(|_| bad())?;
    Ok((letters, grade))
}

/// Shaft fundamental deviation: `(is_upper_deviation, micrometres)`.
fn shaft_fundamental_um(
    letter: &str,
    grade: u8,
    nominal_mm: f64,
    class: &str,
) -> Result<(bool, f64), FitRefusal> {
    let band = band_index(&BAND_UPPER_MM, nominal_mm)?;
    let sub = band_index(&SUB_BAND_UPPER_MM, nominal_mm)?;
    let main = |table: &[f64; 13]| table.get(band).copied();
    let inter = |table: &[f64; 25]| table.get(sub).copied();
    let found = match letter {
        "c" => inter(&SHAFT_C_ES).map(|v| (true, v)),
        "d" => main(&SHAFT_D_ES).map(|v| (true, v)),
        "e" => main(&SHAFT_E_ES).map(|v| (true, v)),
        "f" => main(&SHAFT_F_ES).map(|v| (true, v)),
        "g" => main(&SHAFT_G_ES).map(|v| (true, v)),
        "h" => Some((true, 0.0)),
        "k" => {
            if (4..=7).contains(&grade) {
                main(&SHAFT_K_EI_4_TO_7).map(|v| (false, v))
            } else {
                Some((false, 0.0))
            }
        }
        "m" => main(&SHAFT_M_EI).map(|v| (false, v)),
        "n" => main(&SHAFT_N_EI).map(|v| (false, v)),
        "p" => main(&SHAFT_P_EI).map(|v| (false, v)),
        "r" => inter(&SHAFT_R_EI).map(|v| (false, v)),
        "s" => inter(&SHAFT_S_EI).map(|v| (false, v)),
        "u" => inter(&SHAFT_U_EI).map(|v| (false, v)),
        _ => None,
    };
    found.ok_or_else(|| FitRefusal::UnsupportedClass {
        class: class.to_string(),
        reason: format!(
            "fundamental deviation '{letter}' is outside the tabulated set \
             (c d e f g h js k m n p r s u)"
        ),
    })
}

/// Resolve one ISO 286 class (`"H7"`, `"g6"`, `"JS9"`) at `nominal_mm` into
/// its two limit deviations.
///
/// A hole is written with upper-case letters, a shaft with lower-case ones.
pub fn resolve_class(nominal_mm: f64, class: &str) -> Result<ResolvedClass, FitRefusal> {
    let class = class.trim();
    if class.contains('/') {
        return Err(FitRefusal::WrongArity(format!(
            "'{class}' names a hole/shaft pair; one feature takes one class (H7 or g6)"
        )));
    }
    let (letters, grade) = split_class(class)?;
    let hole = letters.chars().all(|c| c.is_ascii_uppercase());
    let letter = letters.to_ascii_lowercase();
    let normalised = format!("{letters}{grade}");
    let band = band_index(&BAND_UPPER_MM, nominal_mm)?;
    let it = it_um(grade, band).ok_or_else(|| FitRefusal::UnsupportedClass {
        class: normalised.clone(),
        reason: format!("grade IT{grade} is outside the tabulated IT{GRADE_MIN}..IT{GRADE_MAX}"),
    })?;

    // JS / js: the zone is symmetric about the nominal.
    if letter == "js" {
        let half = it / 2.0;
        return Ok(ResolvedClass {
            class: normalised,
            hole,
            grade,
            upper_mm: half / 1000.0,
            lower_mm: -half / 1000.0,
        });
    }

    let (is_upper, shaft_um) = shaft_fundamental_um(&letter, grade, nominal_mm, &normalised)?;
    let (upper_um, lower_um) = if !hole {
        if is_upper {
            (shaft_um, shaft_um - it)
        } else {
            (shaft_um + it, shaft_um)
        }
    } else if is_upper {
        // Holes A..H: the lower deviation EI mirrors the shaft's upper one.
        let ei = -shaft_um;
        (ei + it, ei)
    } else {
        // Holes K..ZC: ES = -ei + delta, where delta = IT(n) - IT(n-1), for
        // K M N up to IT8 and P..ZC up to IT7 (ISO 286-1 Table 3 notes). Only
        // those grades are tabulated here; other grades follow special rules
        // this module does not carry.
        let grades_with_delta = match letter.as_str() {
            "k" | "m" | "n" => 6..=8u8,
            _ => 6..=7u8,
        };
        if !grades_with_delta.contains(&grade) {
            return Err(FitRefusal::UnsupportedClass {
                class: normalised,
                reason: format!(
                    "hole class {}{grade}: only grades {}..{} are tabulated for this letter",
                    letters,
                    grades_with_delta.start(),
                    grades_with_delta.end()
                ),
            });
        }
        // The standard's own exception: M6 above 250 up to 315 mm is -9, not
        // the -11 the delta rule gives. Refused rather than special-cased
        // from memory.
        if letter == "m" && grade == 6 && band == 10 {
            return Err(FitRefusal::UnsupportedClass {
                class: normalised,
                reason: "M6 above 250 up to 315 mm is a tabulated exception to the delta rule"
                    .to_string(),
            });
        }
        // Up to 3 mm the delta is zero; the hole K value there is also zero
        // at every tabulated grade (the shaft k value at grades 4..7 is 0).
        let delta = if band == 0 {
            0.0
        } else {
            let below = it_um(grade - 1, band).ok_or_else(|| FitRefusal::UnsupportedClass {
                class: normalised.clone(),
                reason: format!("grade IT{} is not tabulated", grade - 1),
            })?;
            it - below
        };
        // The hole K rule uses the shaft k value of grades 4..7 at every
        // grade it applies to (6..8).
        let shaft_ei = if letter == "k" {
            SHAFT_K_EI_4_TO_7.get(band).copied().unwrap_or(0.0)
        } else {
            shaft_um
        };
        let es = -shaft_ei + delta;
        (es, es - it)
    };
    Ok(ResolvedClass {
        class: normalised,
        hole,
        grade,
        upper_mm: um_to_mm(upper_um),
        lower_mm: um_to_mm(lower_um),
    })
}

/// Resolve a hole/shaft pair (`"H7/g6"`) at `nominal_mm`: both classes, the
/// extreme clearances and the character of the fit.
pub fn resolve_fit(nominal_mm: f64, designation: &str) -> Result<ResolvedFit, FitRefusal> {
    let designation = designation.trim();
    let Some((hole_txt, shaft_txt)) = designation.split_once('/') else {
        return Err(FitRefusal::WrongArity(format!(
            "'{designation}' is one class; a fit is a hole/shaft pair such as H7/g6"
        )));
    };
    let hole = resolve_class(nominal_mm, hole_txt)?;
    let shaft = resolve_class(nominal_mm, shaft_txt)?;
    if !hole.hole || shaft.hole {
        return Err(FitRefusal::BadDesignation(format!(
            "{designation}: write the hole class first (upper case) and the shaft class \
             second (lower case), e.g. H7/g6"
        )));
    }
    let min_clearance_mm = um_to_mm((hole.lower_mm - shaft.upper_mm) * 1000.0);
    let max_clearance_mm = um_to_mm((hole.upper_mm - shaft.lower_mm) * 1000.0);
    let kind = if min_clearance_mm >= 0.0 {
        FitKind::Clearance
    } else if max_clearance_mm <= 0.0 {
        FitKind::Interference
    } else {
        FitKind::Transition
    };
    Ok(ResolvedFit {
        hole,
        shaft,
        min_clearance_mm,
        max_clearance_mm,
        kind,
    })
}
