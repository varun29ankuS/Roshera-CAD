// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The ink of a dimension with a tolerance and authored text on it: every
//! tolerance form, on diameters AND linear dimensions, and what is refused.

use geometry_engine::drawing::dimension_edit::{
    dimension_ink, validate_dimension_edit, DimInk, DimStack, DimTolerance, DimensionEdit,
    DimensionEditError, Waiver, MAX_DIM_TEXT_CHARS,
};
use geometry_engine::drawing::dimensioning::Dimension2d;
use geometry_engine::drawing::types::ToleranceRef;
use geometry_engine::gdt::iso286::FitRefusal;
use geometry_engine::units::LengthUnit;

fn dim(kind: &str, label: &str, value: f64) -> Dimension2d {
    Dimension2d {
        id: "d0".to_string(),
        kind: kind.to_string(),
        value,
        unit: "mm".to_string(),
        label: label.to_string(),
        a: [0.0, 0.0],
        b: [value, 0.0],
        entities: vec![1],
        axis3: None,
        dir3: None,
        pid: Some("pid-0".to_string()),
        datum: None,
        tolerance: None,
        key: Some("pid-0".to_string()),
        members: Vec::new(),
        member_points: Vec::new(),
        chain_from: None,
    }
}

fn ink(d: &Dimension2d, tolerance: DimTolerance) -> DimInk {
    let edit = DimensionEdit {
        tolerance: Some(tolerance),
        ..DimensionEdit::default()
    };
    dimension_ink(d, Some(&edit), LengthUnit::Millimetre)
}

#[test]
fn every_tolerance_form_is_inked() {
    let bore = dim("diameter", "\u{00D8}25.00", 25.0);
    assert_eq!(
        ink(&bore, DimTolerance::Symmetric { pm: 0.15 }).label,
        "\u{00D8}25.00 \u{00B1}0.15"
    );
    let bi = ink(
        &bore,
        DimTolerance::Bilateral {
            upper: 0.05,
            lower: -0.1,
        },
    );
    assert_eq!(bi.label, "\u{00D8}25.00 +0.05/-0.10");
    assert_eq!(
        bi.stack,
        Some(DimStack {
            lead: "\u{00D8}25.00".to_string(),
            upper: "+0.05".to_string(),
            lower: "-0.10".to_string(),
            trail: String::new(),
            full_size: false,
        })
    );
    assert_eq!(
        ink(
            &bore,
            DimTolerance::Bilateral {
                upper: 0.0,
                lower: -0.2
            }
        )
        .label,
        "\u{00D8}25.00 0/-0.2",
        "a zero deviation is written 0, without a sign"
    );
    assert_eq!(
        ink(
            &bore,
            DimTolerance::Limits {
                upper: 25.15,
                lower: 24.85
            }
        )
        .label,
        "\u{00D8}25.15/24.85"
    );
    let h7 = ink(
        &bore,
        DimTolerance::Fit {
            designation: "H7".to_string(),
        },
    );
    assert_eq!(h7.label, "\u{00D8}25.00 H7 (+0.021/0)");
    assert_eq!(
        h7.tolerance.as_ref().and_then(|t| t.limits),
        Some([25.0, 25.021])
    );
    let g6 = ink(
        &bore,
        DimTolerance::Fit {
            designation: "g6".to_string(),
        },
    );
    assert_eq!(g6.label, "\u{00D8}25.00 g6 (-0.007/-0.020)");
    let basic = ink(&bore, DimTolerance::Basic);
    assert!(basic.boxed);
    assert_eq!(basic.label, "\u{00D8}25.00");
    assert_eq!(ink(&bore, DimTolerance::Reference).label, "(\u{00D8}25.00)");
}

#[test]
fn a_linear_dimension_takes_a_tolerance_too() {
    let width = dim("length", "120.00", 120.0);
    assert_eq!(
        ink(
            &width,
            DimTolerance::Bilateral {
                upper: 0.1,
                lower: 0.05
            }
        )
        .label,
        "120.00 +0.10/+0.05"
    );
    let angle = dim("angle", "\u{2220} 30.0\u{00B0}", 30.0);
    assert_eq!(
        ink(&angle, DimTolerance::Symmetric { pm: 0.5 }).label,
        "\u{2220} 30.0\u{00B0} \u{00B1}0.5"
    );
}

#[test]
fn the_tolerance_follows_the_size_not_the_count() {
    let callout = dim("hole_callout", "4\u{00D7} \u{00D8}6.60 THRU", 6.6);
    assert_eq!(
        ink(
            &callout,
            DimTolerance::Fit {
                designation: "H7".to_string()
            }
        )
        .label,
        "4\u{00D7} \u{00D8}6.60 H7 (+0.015/0) THRU"
    );
}

#[test]
fn free_text_is_inked_and_listed_as_authored() {
    let tap = dim("diameter", "\u{00D8}10.20", 10.2);
    let edit = DimensionEdit {
        prefix: "6X".to_string(),
        suffix: "M12x1.75-6H".to_string(),
        ..DimensionEdit::default()
    };
    validate_dimension_edit(Some(&tap), &edit, LengthUnit::Millimetre)
        .expect("a thread designation is ordinary drafting text");
    let i = dimension_ink(&tap, Some(&edit), LengthUnit::Millimetre);
    assert_eq!(i.label, "6X \u{00D8}10.20 M12x1.75-6H");
    assert_eq!(
        i.authored,
        vec!["6X".to_string(), "M12x1.75-6H".to_string()]
    );
    for ok in ["Ra 3.2", "SEE NOTE 3", "DIN 74", "TYP", "THRU"] {
        let e = DimensionEdit {
            suffix: ok.to_string(),
            ..DimensionEdit::default()
        };
        assert!(
            validate_dimension_edit(Some(&tap), &e, LengthUnit::Millimetre).is_ok(),
            "'{ok}' must be accepted"
        );
    }
    let long = DimensionEdit {
        suffix: "x".repeat(MAX_DIM_TEXT_CHARS + 1),
        ..DimensionEdit::default()
    };
    assert!(matches!(
        validate_dimension_edit(Some(&tap), &long, LengthUnit::Millimetre),
        Err(DimensionEditError::BadText(_))
    ));
}

#[test]
fn bad_tolerances_are_refused_with_their_reason() {
    let bore = dim("diameter", "\u{00D8}25.00", 25.0);
    let v = |t: DimTolerance, unit: LengthUnit| {
        validate_dimension_edit(
            Some(&bore),
            &DimensionEdit {
                tolerance: Some(t),
                ..DimensionEdit::default()
            },
            unit,
        )
    };
    let mm = LengthUnit::Millimetre;
    assert!(matches!(
        v(DimTolerance::Symmetric { pm: 0.0 }, mm),
        Err(DimensionEditError::BadTolerance(_))
    ));
    assert!(matches!(
        v(
            DimTolerance::Bilateral {
                upper: -0.1,
                lower: 0.05
            },
            mm
        ),
        Err(DimensionEditError::BadTolerance(_))
    ));
    assert!(matches!(
        v(
            DimTolerance::Fit {
                designation: "za6".to_string()
            },
            mm
        ),
        Err(DimensionEditError::Fit(FitRefusal::UnsupportedClass { .. }))
    ));
    assert!(matches!(
        v(
            DimTolerance::Fit {
                designation: "H7".to_string()
            },
            LengthUnit::Inch
        ),
        Err(DimensionEditError::ToleranceNotSupported { .. })
    ));
    let chamfer = dim("chamfer", "0.50 \u{00D7} 45\u{00B0}", 0.5);
    assert!(matches!(
        validate_dimension_edit(
            Some(&chamfer),
            &DimensionEdit {
                tolerance: Some(DimTolerance::Symmetric { pm: 0.1 }),
                ..DimensionEdit::default()
            },
            mm
        ),
        Err(DimensionEditError::ToleranceNotSupported { .. })
    ));
    assert!(matches!(
        validate_dimension_edit(
            Some(&bore),
            &DimensionEdit {
                suppressed: Some(Waiver {
                    by: "  ".to_string(),
                    reason: String::new()
                }),
                ..DimensionEdit::default()
            },
            mm
        ),
        Err(DimensionEditError::UnsignedWaiver)
    ));
}

#[test]
fn a_feature_tolerance_is_inked_and_a_dimension_one_overrides_it() {
    let mut bore = dim("diameter", "\u{00D8}10.00", 10.0);
    bore.tolerance = Some(ToleranceRef {
        feature_pid: Some("ab".to_string()),
        annotation_index: 0,
        kind: "plus_minus".to_string(),
        nominal: 10.0,
        limits: Some([9.95, 10.05]),
        designation: None,
    });
    assert_eq!(
        dimension_ink(&bore, None, LengthUnit::Millimetre).label,
        "\u{00D8}10.00 \u{00B1}0.05"
    );
    assert_eq!(
        ink(
            &bore,
            DimTolerance::Fit {
                designation: "H7".to_string()
            }
        )
        .label,
        "\u{00D8}10.00 H7 (+0.015/0)"
    );
}
