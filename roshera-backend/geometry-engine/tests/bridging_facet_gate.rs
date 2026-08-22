// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! **The bridging-facet gate had a blind spot exactly where it mattered most.**
//!
//! `mesh_quality`'s boundary-conformance rule is meant to catch a facet that
//! bridges across a periodic face — an edge cutting through the solid's
//! interior instead of stopping at a hole's trim curve. It tested
//! `periodic_coverage(..) > p * 0.5`.
//!
//! A bore drilled through a cylinder ON ITS AXIS breaks out at two exactly
//! antipodal `u`, so the facet spanning between them has coverage of precisely
//! `p/2` — and `p/2 > p/2` is false. Measured on a hollow skirt (outer R=43)
//! with one cross bore, from a dump of the real mesh:
//!
//! ```text
//!   uv = [(1.5707963, 18.0), (1.5751797, 18.0014804), (4.7123889, 18.0)]
//!   3D =  (0.000, +43.000, 18.000) -> (0.000, -43.000, 18.000)
//!   gaps = 0.0044, 3.1372, 3.1416  ->  coverage = 2pi - pi = pi EXACTLY
//! ```
//!
//! One straight 86mm chord from `y=+43` to `y=-43` through the middle of the
//! part, on a surface of radius 43. The certificate reported
//! `max_normal_deviation_deg = 90.0` for that same facet while this gate
//! reported ZERO bridging facets. The maximally-bridging facet was the single
//! case the strict inequality let through, and it is not a rare tie: every
//! axis-centred bore produces exactly-antipodal breakouts.
//!
//! ## Why the fix is not `>=`
//!
//! Coverage of exactly `p/2` means "some vertex pair is antipodal", and
//! antipodal pairs are not exclusively bridging. Split a cylinder lengthwise
//! into two pi-wide faces: a facet with vertices on both seam trim edges has
//! coverage `p/2` and crosses nothing. In UV that facet and the bore bridge are
//! structurally identical, so the discriminator cannot come from the facet. It
//! comes from the FACE — a face spanning only half a period legitimately owns
//! `p/2` boundary facets; a wider face does not.
//!
//! [`a_half_period_face_keeps_its_antipodal_facets`] is the control for that
//! second half: it feeds the IDENTICAL measured facet through the identical
//! predicate and must NOT flag it, which is what stops the fix from being a
//! blanket `>=` that trades one wrong answer for another.

use geometry_engine::harness::watertight::{is_bridging_facet, mesh_quality, periodic_coverage};
use geometry_engine::math::Point3;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

const TAU: f64 = std::f64::consts::TAU;

/// The three `u` values of the worst facet on the live piston's skirt, copied
/// from a diagnostic dump of the real tessellation rather than constructed.
const MEASURED_U: [f64; 3] = [1.5707963267948966, 1.5751797782219734, 4.71238898038469];

/// The premise this whole file rests on: the measured facet's coverage really
/// is exactly half the period. If this drifts, every assertion below is
/// testing a different situation than the one described in the header.
#[test]
fn the_measured_facet_covers_exactly_half_the_period() {
    let coverage = periodic_coverage(MEASURED_U[0], MEASURED_U[1], MEASURED_U[2], TAU);
    let half = 0.5 * TAU;
    assert!(
        (coverage - half).abs() < 1e-12,
        "the fixture must sit exactly on the boundary; got coverage {coverage} vs half {half}"
    );
    // And the OLD test really does miss it — this is the defect, in one line.
    assert!(
        !(coverage > half),
        "a strict `>` cannot catch this facet; that is the whole bug"
    );
}

/// RED before the fix: the bore bridge on a full-wrap face must be flagged.
#[test]
fn an_antipodal_facet_on_a_full_face_is_bridging() {
    assert!(
        is_bridging_facet(MEASURED_U[0], MEASURED_U[1], MEASURED_U[2], TAU, TAU),
        "the 86mm chord across the bore must be counted as bridging"
    );
}

/// CONTROL, and it must pass: the SAME facet on a face that legitimately spans
/// only half the period is not bridging.
///
/// This is what makes the fix a discriminator rather than a blanket `>=`. If
/// this test ever goes red, the gate has started flagging honest geometry on
/// lengthwise-split cylinders.
#[test]
fn a_half_period_face_keeps_its_antipodal_facets() {
    assert!(
        !is_bridging_facet(MEASURED_U[0], MEASURED_U[1], MEASURED_U[2], TAU, 0.5 * TAU),
        "a face spanning exactly half the period legitimately owns its \
         boundary-to-boundary facets"
    );
}

/// An ordinary small facet is not bridging, on either kind of face. Without
/// this the two tests above could both be satisfied by a predicate keyed only
/// on `face_u_span`, which would flag every facet on every full cylinder.
#[test]
fn an_ordinary_facet_is_not_bridging() {
    let u = [0.10, 0.12, 0.14];
    assert!(
        !is_bridging_facet(u[0], u[1], u[2], TAU, TAU),
        "a small facet on a full face must be clean"
    );
    assert!(
        !is_bridging_facet(u[0], u[1], u[2], TAU, 0.5 * TAU),
        "a small facet on a half face must be clean"
    );
}

/// A facet unambiguously wider than half the period is bridging regardless of
/// the face — no face of any width owns one of these without an edge leaving
/// the surface.
#[test]
fn an_unambiguously_wide_facet_is_bridging_on_any_face() {
    // Coverage ~ 3pi/2: gaps are pi/2 (wrap) and two smaller ones.
    let u = [0.0, 2.2, 4.4];
    let coverage = periodic_coverage(u[0], u[1], u[2], TAU);
    assert!(
        coverage > 0.5 * TAU + 1e-6,
        "fixture premise: this facet must be strictly wider than half; got {coverage}"
    );
    assert!(is_bridging_facet(u[0], u[1], u[2], TAU, TAU));
    assert!(is_bridging_facet(u[0], u[1], u[2], TAU, 0.5 * TAU));
}

/// A non-periodic surface has no seam to bridge, and must never be flagged.
#[test]
fn a_non_periodic_surface_is_never_bridging() {
    assert!(!is_bridging_facet(0.0, 1.0, 2.0, 0.0, 0.0));
    assert!(!is_bridging_facet(0.0, 1.0, 2.0, -1.0, 5.0));
}

/// CONTROL for the other half of the design: a sphere's poles must not be
/// mistaken for bridges.
///
/// A pole is one 3D point that every `u` maps to, so the pole vertex carries
/// whatever `u` the tessellator happened to write. On a radius-10 sphere the
/// four pole facets record `u = (pi, 0, 0.063)` — coverage of exactly half the
/// period, structurally identical in UV to the bore bridge above — while their
/// real 3D edges are 0.3mm. Judged on `u` alone they are indistinguishable;
/// judged on whether the surface actually moves with `u`, trivially separable.
///
/// This went red the moment the gate started catching the antipodal case,
/// which is how the false positive was found rather than shipped.
#[test]
fn a_spheres_poles_are_not_bridges() {
    let mut model = BRepModel::new();
    let sphere =
        match TopologyBuilder::new(&mut model).create_sphere_3d(Point3::new(0.0, 0.0, 0.0), 10.0) {
            Ok(GeometryId::Solid(id)) => id,
            other => panic!("expected a solid; got {other:?}"),
        };
    let q = mesh_quality(&model, sphere).expect("a sphere must measure");
    assert_eq!(
        q.boundary_crossing_facets, 0,
        "a sphere's pole facets span half the period in u only because u is          arbitrary at a parametric singularity; their 3D edges are 0.3mm on a          radius-10 sphere"
    );
    assert!(
        q.clean,
        "a plain sphere must certify clean (aspect {:.1}, dev {:.1}deg, wings {})",
        q.worst_aspect_ratio, q.max_normal_deviation_deg, q.boundary_crossing_facets
    );
}
