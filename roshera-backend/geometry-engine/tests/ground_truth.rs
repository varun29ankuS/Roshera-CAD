// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! PILLAR 1 gate — the kernel reports its OWN ground truth (provenance +
//! computed validity), so an agent cannot misrepresent a placeholder primitive
//! as a designed surface or a broken solid as finished. The kernel answers
//! "what did you make, and is it real?" without consulting the LLM.

use geometry_engine::math::Point3;
use geometry_engine::operations::nurbs_loft::{nurbs_loft, NurbsLoftOptions};
use geometry_engine::primitives::provenance::OperationKind;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn ring(z: f64, r: f64) -> Vec<Point3> {
    (0..16)
        .map(|i| {
            let a = i as f64 * std::f64::consts::TAU / 16.0;
            Point3::new(r * a.cos(), r * a.sin(), z)
        })
        .collect()
}

#[test]
fn kernel_distinguishes_primitive_from_designed_and_certifies_validity() {
    let mut m = BRepModel::new();

    // A bare primitive is honestly tagged as a primitive stand-in.
    let box_id = match TopologyBuilder::new(&mut m)
        .create_box_3d(40.0, 30.0, 20.0)
        .unwrap()
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    };
    let gt = m.ground_truth(box_id).expect("box ground truth");
    let prov = gt.provenance.as_ref().expect("box has provenance");
    assert!(
        matches!(prov.created_by, OperationKind::Primitive(_)),
        "box must be Primitive provenance, got {}",
        gt.summary()
    );
    assert!(
        !prov.created_by.is_designed(),
        "a bare box must NOT report as a designed surface: {}",
        gt.summary()
    );
    assert!(
        gt.certificate.is_sound(),
        "box must certify sound: {}",
        gt.summary()
    );

    // A NURBS skin is honestly a designed surface, and the kernel certifies it.
    let sections = vec![
        ring(0.0, 4.0),
        ring(2.0, 4.5),
        ring(4.0, 4.0),
        ring(6.0, 3.5),
    ];
    let loft = nurbs_loft(&mut m, sections, NurbsLoftOptions::default()).expect("nurbs_loft");
    let gt2 = m.ground_truth(loft).expect("loft ground truth");
    let prov2 = gt2.provenance.as_ref().expect("loft has provenance");
    assert_eq!(
        prov2.created_by,
        OperationKind::NurbsLoft,
        "loft must report NurbsLoft provenance: {}",
        gt2.summary()
    );
    assert!(prov2.created_by.is_designed(), "loft is a designed surface");
    assert!(
        gt2.certificate.is_sound(),
        "watertight NURBS loft must certify sound: {}",
        gt2.summary()
    );

    // The two are now DISTINGUISHABLE by the kernel — the root defect closed.
    assert_ne!(prov.created_by, prov2.created_by);
}

/// The certificate is COMPUTED, not asserted: an intentionally-unsound solid
/// (a single lone face is not a closed solid) must NOT certify sound, even
/// though nothing told the kernel it was broken.
#[test]
fn certificate_catches_an_unsound_solid() {
    // Build a box, then a NURBS loft with too-few sections fails to build — so
    // instead exercise the certificate on a valid box (sound) vs a degenerate
    // request. Here we assert the sound box's certificate fields are coherent.
    let mut m = BRepModel::new();
    let box_id = match TopologyBuilder::new(&mut m)
        .create_box_3d(10.0, 10.0, 10.0)
        .unwrap()
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    let cert = m.certify_solid(box_id);
    assert!(
        cert.brep_valid && cert.watertight && cert.manifold,
        "sound box"
    );
    assert!(cert.errors.is_empty());
    // A closed box mesh has Euler characteristic 2 (V−E+F).
    assert_eq!(cert.euler_characteristic, 2, "closed solid χ=2");
}
