// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! ISO 286 limits and fits: known vectors, band edges, and typed refusals
//! outside the tabulated range.

use geometry_engine::gdt::iso286::{resolve_class, resolve_fit, FitKind, FitRefusal};

fn dev(nominal: f64, class: &str) -> (f64, f64) {
    let r = resolve_class(nominal, class).unwrap_or_else(|e| panic!("{class}@{nominal}: {e}"));
    (r.upper_mm, r.lower_mm)
}

fn close(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
}

/// Known limit deviations (upper, lower) in millimetres, as printed in
/// every ISO 286-2 extract.
#[test]
fn known_vectors() {
    let cases: [(f64, &str, (f64, f64)); 22] = [
        (25.0, "H7", (0.021, 0.0)),
        (25.0, "g6", (-0.007, -0.020)),
        (10.0, "H7", (0.015, 0.0)),
        (10.0, "h6", (0.0, -0.009)),
        (20.0, "k6", (0.015, 0.002)),
        (50.0, "p6", (0.042, 0.026)),
        (25.0, "f7", (-0.020, -0.041)),
        (25.0, "H8", (0.033, 0.0)),
        (40.0, "e8", (-0.050, -0.089)),
        (40.0, "d9", (-0.080, -0.142)),
        (25.0, "c11", (-0.110, -0.240)),
        (25.0, "H11", (0.130, 0.0)),
        (25.0, "n6", (0.028, 0.015)),
        (25.0, "s6", (0.048, 0.035)),
        (25.0, "u6", (0.061, 0.048)),
        (60.0, "r6", (0.060, 0.041)),
        (25.0, "js6", (0.0065, -0.0065)),
        (25.0, "K7", (0.006, -0.015)),
        (25.0, "N7", (-0.007, -0.028)),
        (25.0, "P7", (-0.014, -0.035)),
        (25.0, "M6", (-0.004, -0.017)),
        (25.0, "F8", (0.053, 0.020)),
    ];
    for (nominal, class, want) in cases {
        let got = dev(nominal, class);
        assert!(
            close(got, want),
            "{class} at {nominal} mm: got {got:?}, want {want:?}"
        );
    }
}

/// A band is "above X up to and including Y": the upper bound belongs to
/// the lower band. Changing `<=` to `<` in `band_index` moves every one
/// of these to the next band's grade.
#[test]
fn band_edges_belong_to_the_lower_band() {
    let edges: [(f64, f64, f64); 12] = [
        (3.0, 0.010, 0.012),
        (6.0, 0.012, 0.015),
        (10.0, 0.015, 0.018),
        (18.0, 0.018, 0.021),
        (30.0, 0.021, 0.025),
        (50.0, 0.025, 0.030),
        (80.0, 0.030, 0.035),
        (120.0, 0.035, 0.040),
        (180.0, 0.040, 0.046),
        (250.0, 0.046, 0.052),
        (315.0, 0.052, 0.057),
        (400.0, 0.057, 0.063),
    ];
    for (edge, at, above) in edges {
        assert!(
            close(dev(edge, "H7"), (at, 0.0)),
            "H7 at exactly {edge} mm must take the lower band ({at})"
        );
        assert!(
            close(dev(edge + 0.001, "H7"), (above, 0.0)),
            "H7 just above {edge} mm must take the next band ({above})"
        );
    }
    assert!(close(dev(500.0, "H7"), (0.063, 0.0)));
}

#[test]
fn out_of_scope_is_refused_never_interpolated() {
    assert!(matches!(
        resolve_class(500.001, "H7"),
        Err(FitRefusal::NominalOutOfRange { .. })
    ));
    assert!(matches!(
        resolve_class(0.0, "H7"),
        Err(FitRefusal::NominalOutOfRange { .. })
    ));
    assert!(matches!(
        resolve_class(f64::NAN, "H7"),
        Err(FitRefusal::NominalOutOfRange { .. })
    ));
    assert!(matches!(
        resolve_class(25.0, "za6"),
        Err(FitRefusal::UnsupportedClass { .. })
    ));
    assert!(matches!(
        resolve_class(25.0, "H17"),
        Err(FitRefusal::UnsupportedClass { .. })
    ));
    assert!(matches!(
        resolve_class(25.0, "K9"),
        Err(FitRefusal::UnsupportedClass { .. })
    ));
    assert!(matches!(
        resolve_class(300.0, "M6"),
        Err(FitRefusal::UnsupportedClass { .. })
    ));
    for bad in ["", "7", "H", "Hh7", "H7x", "M12x1.75"] {
        assert!(
            matches!(resolve_class(25.0, bad), Err(FitRefusal::BadDesignation(_))),
            "'{bad}' must be refused as a designation"
        );
    }
    assert!(matches!(
        resolve_class(25.0, "H7/g6"),
        Err(FitRefusal::WrongArity(_))
    ));
}

#[test]
fn hole_shaft_pair_gives_the_clearances() {
    let fit = resolve_fit(25.0, "H7/g6").expect("H7/g6");
    assert_eq!(fit.kind, FitKind::Clearance);
    assert!((fit.min_clearance_mm - 0.007).abs() < 1e-9, "{fit:?}");
    assert!((fit.max_clearance_mm - 0.041).abs() < 1e-9, "{fit:?}");

    let press = resolve_fit(25.0, "H7/s6").expect("H7/s6");
    assert_eq!(press.kind, FitKind::Interference);
    assert!((press.min_clearance_mm + 0.048).abs() < 1e-9, "{press:?}");
    assert!((press.max_clearance_mm + 0.014).abs() < 1e-9, "{press:?}");

    let trans = resolve_fit(25.0, "H7/k6").expect("H7/k6");
    assert_eq!(trans.kind, FitKind::Transition);

    assert!(matches!(
        resolve_fit(25.0, "g6/H7"),
        Err(FitRefusal::BadDesignation(_))
    ));
    assert!(matches!(
        resolve_fit(25.0, "H7"),
        Err(FitRefusal::WrongArity(_))
    ));
}

#[test]
fn limits_are_absolute_sizes() {
    let c = resolve_class(25.0, "g6").expect("g6");
    let (lo, hi) = c.limits(25.0);
    assert!((lo - 24.980).abs() < 1e-9 && (hi - 24.993).abs() < 1e-9);
}
