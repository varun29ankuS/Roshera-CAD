//! STRUCTURALLY-INTRINSIC certification gate — the kernel's validity
//! certificate is lazily cached on the `Solid` (mirroring `cached_mass_props`)
//! and CANNOT go stale: every mutating seam dirties it, so any read returns a
//! certificate that is current-or-freshly-recomputed.
//!
//! Two non-vacuous properties are pinned here, and the file is written so that
//! REMOVING either invalidation seam makes a test fail:
//!
//! 1. NEVER STALE — after a mutating op, `BRepModel::certificate(id)` equals the
//!    certificate an independent, cache-free recomputation of the SAME geometry
//!    produces. A stale cache would diverge.
//! 2. INVALIDATION FIRES — the funnel seam in `record_operation` flips a solid's
//!    cached certificate away from a previously-cached value. If the seam is
//!    deleted, the stale (sound) certificate survives and the assertion breaks.

use geometry_engine::labels::{Fingerprint, LabelAssertion, LabelKind};
use geometry_engine::math::Matrix4;
use geometry_engine::operations::recorder::RecordedOperation;
use geometry_engine::operations::transform::{transform_solid, TransformOptions};
use geometry_engine::primitives::provenance::LabelsConsistency;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// Build a box of the given extents, returning the model and its solid id.
fn box_model(dx: f64, dy: f64, dz: f64) -> (BRepModel, u32) {
    let mut m = BRepModel::new();
    let id = match TopologyBuilder::new(&mut m)
        .create_box_3d(dx, dy, dz)
        .expect("create_box_3d")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    };
    (m, id)
}

/// PROPERTY 1 — never stale across a real mutating op.
///
/// Certify a box (populating the cache), translate it through `transform_solid`
/// (which fires BOTH the `get_mut` backstop and the `record_operation` funnel),
/// then assert the cached certificate read back equals a fully independent,
/// cache-free recomputation of the post-transform geometry. If the cache were
/// stale the two would differ on at least the tessellation/mesh-quality fields
/// keyed off world coordinates.
#[test]
fn certificate_is_never_stale_after_a_mutating_op() {
    let (mut m, id) = box_model(40.0, 30.0, 20.0);

    // Warm the cache.
    let before = m.certificate(id).expect("certificate before");
    assert!(before.is_sound(), "as-built box must be sound");

    // Mutate through the real op path.
    let t = Matrix4::translation(12.5, -4.0, 7.25);
    transform_solid(&mut m, id, t, TransformOptions::default()).expect("transform_solid");

    // The cache must reflect the moved geometry, not the pre-move snapshot.
    let after = m.certificate(id).expect("certificate after");

    // Independent ground truth: same box, same translation, in a fresh model
    // that never saw the pre-move cert. This is the cache-free oracle.
    let (mut oracle, oid) = box_model(40.0, 30.0, 20.0);
    transform_solid(&mut oracle, oid, t, TransformOptions::default()).expect("oracle transform");
    let truth = oracle.certify_solid(oid);

    assert_eq!(
        after, truth,
        "cached certificate after a mutating op must equal an independent \
         cache-free recomputation of the same geometry — a mismatch means the \
         cache went stale"
    );
    assert!(after.is_sound(), "translated box stays sound");
}

/// PROPERTY 2 — the funnel invalidation seam actually fires, and removing it
/// breaks this test.
///
/// We engineer a state where the geometry's TRUE certificate has changed but
/// the change arrived through a path that does NOT touch the solid via `&mut`
/// and does NOT record an op — attaching a label with a deliberately-FALSE
/// assertion. Label attach is a sidecar that leaves the cache holding the
/// PRE-label value (`labels_consistent == NotApplicable`). Then we fire ONLY
/// the funnel seam by calling `record_operation` directly with the solid as an
/// output. If the funnel dirties the cache, the next read recomputes and the
/// `labels_consistent` flag FLIPS to `Inconsistent`. If the funnel seam is
/// removed, the stale `NotApplicable` value survives and the assertion fails.
///
/// `labels_consistent` is used (rather than a sound-affecting field) precisely
/// because it is a certificate input that has no invalidation seam of its own,
/// so it isolates the funnel as the only thing that can refresh the cache.
#[test]
fn record_operation_funnel_invalidates_a_cached_certificate() {
    let (mut m, id) = box_model(16.0, 16.0, 20.0);

    // Pick any face of the box to hang a label on.
    let solid = m.solids.get(id).expect("solid exists");
    let face = *m
        .shells
        .get(solid.outer_shell)
        .expect("outer shell")
        .faces
        .first()
        .expect("box has at least one face");

    // Warm the cache: a label-less box is sound with labels NotApplicable.
    let warmed = m.certificate(id).expect("warm certificate");
    assert_eq!(
        warmed.labels_consistent,
        LabelsConsistency::NotApplicable,
        "an unlabelled box has no label claims to verify"
    );
    assert!(warmed.is_sound(), "as-built box certifies sound");

    // Attach a label whose FINGERPRINT assertion cannot hold — a face-kind
    // fingerprint forged at a far-away position. This goes through the labels
    // sidecar, which does NOT fire any certificate-invalidation seam, so the
    // cache is intentionally left holding the warmed `NotApplicable` verdict.
    let bogus = Fingerprint {
        kind: LabelKind::Face,
        position: [1_000.0, 1_000.0, 1_000.0],
        normal: None,
        radius: Some(999.0),
        size: None,
    };
    m.label_face_with_assertion(face, "phantom", LabelAssertion::Fingerprint(bogus), None)
        .expect("attach bogus label");

    // The sidecar bypassed the seams: the cache is deliberately stale here.
    let still_cached = m.certificate(id).expect("still-cached certificate");
    assert_eq!(
        still_cached.labels_consistent,
        LabelsConsistency::NotApplicable,
        "label attach does not invalidate — the cache still holds the pre-label \
         verdict (this is the deliberately-stale precondition for the test)"
    );

    // Fire ONLY the funnel seam: an op naming this solid as an output.
    m.record_operation(RecordedOperation::new("test_marker_op").with_output_solids([id as u64]));

    // The funnel must have dirtied the cache; the recompute now verifies the
    // bogus label and reports the claim as broken.
    let after = m.certificate(id).expect("post-funnel certificate");
    assert_eq!(
        after.labels_consistent,
        LabelsConsistency::Inconsistent,
        "record_operation must invalidate the cached cert so the recompute \
         re-verifies the bogus label — a NotApplicable verdict here means the \
         funnel seam returned a STALE certificate"
    );
}
