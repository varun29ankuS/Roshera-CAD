// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Gate test for kernel-verified GD&T conformance.
//!
//! The whole point of "GD&T that can't lie" is that the kernel MEASURES the
//! actual geometry and the verdict actually flips when it should. This gate
//! proves both directions:
//!
//! * a PERFECT primitive measures form error ≈ 0 ≤ tolerance → `InSpec`;
//! * a tolerance tighter than the real (analytic-but-discretely-sampled) form
//!   error, and a deliberately wrong diameter, → `OutOfSpec` — a verifier that
//!   always passes is worthless, so the must-FAIL cases are the load-bearing
//!   ones;
//! * a dimensional ± on a cylinder diameter is measured correctly;
//! * a datum-referenced characteristic reports `NotYetVerified`, never a false
//!   pass.

use geometry_engine::gdt::model::{
    Annotation, DimensionalTolerance, FeatureControlFrame, GeometricCharacteristic,
};
use geometry_engine::gdt::verify::Conformance;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn build_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    let mut b = TopologyBuilder::new(model);
    match b.create_box_3d(w, h, d).expect("box") {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn build_cylinder(model: &mut BRepModel, radius: f64, height: f64) -> SolidId {
    let mut b = TopologyBuilder::new(model);
    match b
        .create_cylinder_3d(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            radius,
            height,
        )
        .expect("cylinder")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

/// First face of a solid whose surface type matches `type_name`.
fn first_face_of_kind(model: &BRepModel, solid: SolidId, type_name: &str) -> FaceId {
    let s = model.solids.get(solid).expect("solid");
    for sh in s.all_shells() {
        if let Some(shell) = model.shells.get(sh) {
            for &fid in &shell.faces {
                if let Some(f) = model.faces.get(fid) {
                    if let Some(surf) = model.surfaces.get(f.surface_id) {
                        if surf.type_name() == type_name {
                            return fid;
                        }
                    }
                }
            }
        }
    }
    panic!("no {type_name} face on solid {solid}");
}

#[test]
fn perfect_planar_face_is_flat_within_tolerance() {
    let mut model = BRepModel::new();
    let solid = build_box(&mut model, 20.0, 20.0, 20.0);
    let face = first_face_of_kind(&model, solid, "Plane");

    // A perfect box face has zero flatness error; any positive zone passes.
    let key = model.attach_face_annotation(
        face,
        Annotation::Geometric(FeatureControlFrame::form(
            GeometricCharacteristic::Flatness,
            0.01,
        )),
    );
    assert!(model.face_pid(face) == Some(key));

    let results = model.verify_face_conformance(face);
    assert_eq!(results.len(), 1);
    let r = &results[0];
    assert_eq!(
        r.verdict,
        Conformance::InSpec,
        "perfect plane must pass flatness; measured {:?} detail={}",
        r.actual,
        r.detail
    );
    let err = r.actual.expect("flatness measured");
    assert!(
        err < 1e-6,
        "perfect plane flatness error must be ~0, got {err}"
    );
}

#[test]
fn curved_face_fails_flatness() {
    // The must-FAIL direction for flatness: measure a cylinder's CURVED lateral
    // face against a FLATNESS callout. Its sampled points span the curvature, so
    // the best-fit-plane peak-to-valley band is large (≈ the radius scale) and a
    // realistic flatness zone must FAIL. This proves flatness flips to OutOfSpec
    // when the geometry is genuinely non-planar — the kernel measured it.
    let mut model = BRepModel::new();
    let radius = 5.0;
    let solid = build_cylinder(&mut model, radius, 30.0);
    let face = first_face_of_kind(&model, solid, "Cylinder");

    model.attach_face_annotation(
        face,
        Annotation::Geometric(FeatureControlFrame::form(
            GeometricCharacteristic::Flatness,
            0.1,
        )),
    );
    let r = &model.verify_face_conformance(face)[0];
    assert_eq!(
        r.verdict,
        Conformance::OutOfSpec,
        "a curved face must FAIL a 0.1 flatness zone; measured {:?} detail={}",
        r.actual,
        r.detail
    );
    let err = r.actual.expect("flatness measured");
    // Deviation of a half-cylinder band from its best-fit plane is on the order
    // of the radius; certainly well above the 0.1 zone.
    assert!(
        err > 0.1,
        "curved-face flatness error {err} must exceed the zone"
    );
}

#[test]
fn cylinder_diameter_measured_and_in_spec() {
    let mut model = BRepModel::new();
    let radius = 5.0;
    let solid = build_cylinder(&mut model, radius, 30.0);
    let face = first_face_of_kind(&model, solid, "Cylinder");

    // Nominal Ø10 ± 0.2; the true diameter is exactly 10.
    model.attach_face_annotation(
        face,
        Annotation::Dimensional(DimensionalTolerance::symmetric(10.0, 0.2)),
    );
    let results = model.verify_face_conformance(face);
    assert_eq!(results.len(), 1);
    let r = &results[0];
    assert_eq!(r.verdict, Conformance::InSpec, "detail={}", r.detail);
    let dia = r.actual.expect("diameter measured");
    assert!(
        (dia - 10.0).abs() < 1e-6,
        "measured diameter must be 10.0, got {dia}"
    );
    assert!((r.deviation.expect("deviation") - 0.0).abs() < 1e-6);
}

#[test]
fn wrong_nominal_diameter_fails() {
    // The must-FAIL direction for dimensional: the actual Ø10 cylinder against a
    // nominal Ø12 ± 0.1 callout must report OutOfSpec — the verdict flips
    // because the kernel measured the real diameter.
    let mut model = BRepModel::new();
    let solid = build_cylinder(&mut model, 5.0, 30.0);
    let face = first_face_of_kind(&model, solid, "Cylinder");

    model.attach_face_annotation(
        face,
        Annotation::Dimensional(DimensionalTolerance::symmetric(12.0, 0.1)),
    );
    let r = &model.verify_face_conformance(face)[0];
    assert_eq!(
        r.verdict,
        Conformance::OutOfSpec,
        "Ø10 actual vs Ø12±0.1 must FAIL; measured {:?} detail={}",
        r.actual,
        r.detail
    );
    let dia = r.actual.expect("diameter");
    assert!((dia - 10.0).abs() < 1e-6, "measured {dia}");
}

#[test]
fn perfect_cylinder_is_cylindrical_within_tolerance() {
    let mut model = BRepModel::new();
    let solid = build_cylinder(&mut model, 5.0, 30.0);
    let face = first_face_of_kind(&model, solid, "Cylinder");

    model.attach_face_annotation(
        face,
        Annotation::Geometric(FeatureControlFrame::form(
            GeometricCharacteristic::Cylindricity,
            0.05,
        )),
    );
    let r = &model.verify_face_conformance(face)[0];
    // The fine tessellation of a perfect cylinder lies on the ideal surface to
    // chord tolerance; cylindricity error must be well within 0.05.
    assert_eq!(
        r.verdict,
        Conformance::InSpec,
        "perfect cylinder must pass cylindricity; measured {:?} detail={}",
        r.actual,
        r.detail
    );
    let err = r.actual.expect("cylindricity measured");
    assert!(err < 0.05, "cylindricity error {err} should be small");
}

#[test]
fn coarse_cylinder_fails_tight_cylindricity() {
    // The decisive must-FAIL case: the SAME perfect cylinder, but a cylindricity
    // zone tighter than the worst radial deviation introduced by faceting. The
    // measured cylindricity of a faceted cylinder is the chord sagitta between
    // the polygon facets and the true circle — a real, non-zero number. Demand a
    // zone an order of magnitude below it and the verdict must flip to OutOfSpec.
    let mut model = BRepModel::new();
    let solid = build_cylinder(&mut model, 5.0, 30.0);
    let face = first_face_of_kind(&model, solid, "Cylinder");

    // Measure the true form error with a generous zone first.
    model.attach_face_annotation(
        face,
        Annotation::Geometric(FeatureControlFrame::form(
            GeometricCharacteristic::Cylindricity,
            10.0,
        )),
    );
    let measured = model.verify_face_conformance(face)[0]
        .actual
        .expect("measured cylindricity");
    assert!(
        measured > 0.0,
        "a discretely-sampled cylinder must have a non-zero measured form band"
    );

    // Now author a zone strictly below the measured band on a fresh feature key
    // and confirm the verdict flips. We attach a second annotation with the
    // tighter zone; verification returns both, the tight one must FAIL.
    let tight = measured * 0.5;
    model.attach_face_annotation(
        face,
        Annotation::Geometric(FeatureControlFrame::form(
            GeometricCharacteristic::Cylindricity,
            tight,
        )),
    );
    let results = model.verify_face_conformance(face);
    assert_eq!(results.len(), 2, "two cylindricity callouts attached");
    // The generous one passes; the tight one fails.
    assert_eq!(results[0].verdict, Conformance::InSpec);
    assert_eq!(
        results[1].verdict,
        Conformance::OutOfSpec,
        "zone {tight} below measured {measured} must FAIL; detail={}",
        results[1].detail
    );
}

#[test]
fn datum_referenced_characteristic_is_not_falsely_passed() {
    // Honesty contract: a characteristic that needs a datum reference frame must
    // report NotYetVerified, never a silent pass.
    let mut model = BRepModel::new();
    let solid = build_box(&mut model, 20.0, 20.0, 20.0);
    let face = first_face_of_kind(&model, solid, "Plane");

    let mut fcf = FeatureControlFrame::form(GeometricCharacteristic::Perpendicularity, 0.1);
    fcf.datum_refs
        .push(geometry_engine::gdt::model::DatumRef::new("A"));
    model.attach_face_annotation(face, Annotation::Geometric(fcf));

    let r = &model.verify_face_conformance(face)[0];
    assert_eq!(
        r.verdict,
        Conformance::NotYetVerified,
        "datum-referenced check must not fake a pass; detail={}",
        r.detail
    );
    assert!(r.in_spec().is_none(), "unverified must not look measured");
}

#[test]
fn fit_class_dimension_is_not_verified() {
    // A fit class (H7) has no resolved numeric envelope this phase → honest
    // NotYetVerified, not a pass.
    let mut model = BRepModel::new();
    let solid = build_cylinder(&mut model, 5.0, 30.0);
    let face = first_face_of_kind(&model, solid, "Cylinder");

    model.attach_face_annotation(
        face,
        Annotation::Dimensional(DimensionalTolerance::fit(10.0, "H7")),
    );
    let r = &model.verify_face_conformance(face)[0];
    assert_eq!(
        r.verdict,
        Conformance::NotYetVerified,
        "detail={}",
        r.detail
    );
}
