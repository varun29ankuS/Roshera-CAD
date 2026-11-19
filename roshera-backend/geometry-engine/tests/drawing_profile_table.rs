// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! An aerofoil is defined on its drawing by a table of ordinates.
//!
//! Varun 2026-11-19, on the wing-rib sheet: "the drawing for the aerofoil has
//! no overall dimensions neither spline measurement". A chord and a depth do
//! not let a machinist make an aerofoil; the profile itself must be stated.
//! The sheet now carries a PROFILE ORDINATES table: the upper and lower
//! surface, measured from the solid, at the standard stations along the
//! chord, with the chord, the maximum thickness and where it occurs, and the
//! trailing-edge thickness. The certificate re-measures it.

use geometry_engine::drawing::certify_drawing;
use geometry_engine::drawing::dimensioning::standard_drawing_auto;
use geometry_engine::drawing::layout::{compute_layout, SheetItemKind};
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::extrude::{extrude_polygon, ExtrudeOptions};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

const CHORD: f64 = 100.0;

/// NACA 4-digit "2412" at x/c, open trailing edge (a4 = -0.1015): (upper, lower).
fn naca2412(xc: f64) -> (f64, f64) {
    let (m, p, t) = (0.02, 0.4, 0.12);
    let yt = 5.0
        * t
        * (0.2969 * xc.sqrt() - 0.1260 * xc - 0.3516 * xc.powi(2) + 0.2843 * xc.powi(3)
            - 0.1015 * xc.powi(4));
    let (yc, dyc) = if xc < p {
        (
            m / (p * p) * (2.0 * p * xc - xc * xc),
            2.0 * m / (p * p) * (p - xc),
        )
    } else {
        (
            m / ((1.0 - p).powi(2)) * ((1.0 - 2.0 * p) + 2.0 * p * xc - xc * xc),
            2.0 * m / ((1.0 - p).powi(2)) * (p - xc),
        )
    };
    let th = dyc.atan();
    let xu = xc - yt * th.sin();
    let yu = yc + yt * th.cos();
    let xl = xc + yt * th.sin();
    let yl = yc - yt * th.cos();
    // Return the surface points' heights; x is shifted by camber slope,
    // which the table's linear interpolation absorbs at these tolerances.
    let _ = (xu, xl);
    (yu, yl)
}

/// The profile as a closed polygon in the XY plane: upper surface from the
/// trailing edge to the leading edge, lower surface back.
fn profile_points(n: usize) -> Vec<(f64, f64)> {
    let (m, p, t) = (0.02, 0.4, 0.12);
    let surf = |xc: f64, upper: bool| -> (f64, f64) {
        let yt = 5.0
            * t
            * (0.2969 * xc.sqrt() - 0.1260 * xc - 0.3516 * xc.powi(2) + 0.2843 * xc.powi(3)
                - 0.1015 * xc.powi(4));
        let (yc, dyc) = if xc < p {
            (
                m / (p * p) * (2.0 * p * xc - xc * xc),
                2.0 * m / (p * p) * (p - xc),
            )
        } else {
            (
                m / ((1.0 - p).powi(2)) * ((1.0 - 2.0 * p) + 2.0 * p * xc - xc * xc),
                2.0 * m / ((1.0 - p).powi(2)) * (p - xc),
            )
        };
        let th = dyc.atan();
        if upper {
            ((xc - yt * th.sin()) * CHORD, (yc + yt * th.cos()) * CHORD)
        } else {
            ((xc + yt * th.sin()) * CHORD, (yc - yt * th.cos()) * CHORD)
        }
    };
    let station = |i: usize| 0.5 * (1.0 - (std::f64::consts::PI * i as f64 / n as f64).cos());
    let mut pts = Vec::new();
    for i in (0..=n).rev() {
        pts.push(surf(station(i), true));
    }
    for i in 1..=n {
        pts.push(surf(station(i), false));
    }
    pts
}

fn rib() -> (BRepModel, u32) {
    let mut m = BRepModel::new();
    m.set_event_key(Some("rib".to_string()));
    let pts: Vec<Point3> = profile_points(60)
        .into_iter()
        .map(|(x, y)| Point3::new(x, y, 0.0))
        .collect();
    let opts = ExtrudeOptions {
        direction: Vector3::Z,
        distance: 6.0,
        ..ExtrudeOptions::default()
    };
    let s = extrude_polygon(&mut m, &pts, opts, Tolerance::default()).expect("rib");
    m.set_event_key(None);
    (m, s)
}

#[test]
fn an_aerofoil_rib_carries_its_profile_ordinates() {
    let (m, s) = rib();
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let t = d
        .profile_table
        .as_ref()
        .expect("the rib's sheet carries a profile table");

    assert!((t.chord_mm - CHORD).abs() < 0.05, "chord {}", t.chord_mm);
    // Standard stations, leading edge to trailing edge.
    let pcts: Vec<f64> = t.stations.iter().map(|s| s.percent).collect();
    assert_eq!(pcts.first().copied(), Some(0.0));
    assert_eq!(pcts.last().copied(), Some(100.0));
    assert!(pcts.contains(&30.0) && pcts.contains(&1.25));
    // Every interior ordinate matches the NACA 2412 section within the
    // faceting of a 60-point profile.
    for st in t
        .stations
        .iter()
        .filter(|s| s.percent > 0.5 && s.percent < 99.5)
    {
        let (yu, yl) = naca2412(st.percent / 100.0);
        assert!(
            (st.upper_mm - yu * CHORD).abs() < 0.15,
            "upper at {}%: table {} vs NACA {}",
            st.percent,
            st.upper_mm,
            yu * CHORD
        );
        assert!(
            (st.lower_mm - yl * CHORD).abs() < 0.15,
            "lower at {}%: table {} vs NACA {}",
            st.percent,
            st.lower_mm,
            yl * CHORD
        );
    }
    // 2412: 12 % thick, the thickest section near 30 % chord.
    assert!(
        (t.max_thickness_mm - 12.0).abs() < 0.2,
        "max t {}",
        t.max_thickness_mm
    );
    assert!(
        (t.max_thickness_at_percent - 30.0).abs() < 4.0,
        "at {}%",
        t.max_thickness_at_percent
    );
    // Open trailing edge: a small, finite thickness.
    assert!(
        t.te_thickness_mm > 0.05 && t.te_thickness_mm < 0.5,
        "te {}",
        t.te_thickness_mm
    );

    // On the sheet, and the sheet still certifies.
    let layout = compute_layout(&d);
    assert!(
        layout
            .items
            .iter()
            .any(|it| it.kind == SheetItemKind::HoleTableText
                && it.text.as_deref() == Some("% CHORD")),
        "the ordinate table is laid out on the sheet"
    );
    let c = certify_drawing(&m, &d);
    assert!(c.sound, "{:?}", c.counts);
}

#[test]
fn a_part_without_a_freeform_profile_has_no_ordinate_table() {
    let mut m = BRepModel::new();
    let s = match TopologyBuilder::new(&mut m)
        .create_box_3d(60.0, 40.0, 10.0)
        .unwrap()
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    assert!(d.profile_table.is_none());
}

#[test]
fn a_stale_ordinate_table_is_caught_by_the_certificate() {
    let (m, s) = rib();
    let mut d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let t = d.profile_table.as_mut().expect("table");
    // The sheet claims a profile 0.5 mm fuller at 30 % than the part has.
    let st = t
        .stations
        .iter_mut()
        .find(|s| s.percent == 30.0)
        .expect("30 %");
    st.upper_mm += 0.5;
    let c = certify_drawing(&m, &d);
    assert!(
        !c.sound,
        "a wrong ordinate makes the sheet unsound: {:?}",
        c.counts
    );
}
