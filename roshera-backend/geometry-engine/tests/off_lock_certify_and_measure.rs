// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The shared-borrow halves an import uses so the model stays READABLE while
//! a batch of parts certifies, and the per-solid display-mesh cache the scene
//! snapshot reads. Each must give exactly what the `&mut` path gives, and
//! must never keep a result computed from a shape the solid no longer has.

use std::sync::Arc;

use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

fn solid(id: GeometryId) -> SolidId {
    match id {
        GeometryId::Solid(s) => s,
        other => panic!("expected a solid, got {other:?}"),
    }
}

/// A box, a cylinder and a sphere: planar, curved and doubly-curved faces.
fn three_parts() -> (BRepModel, Vec<SolidId>) {
    let mut model = BRepModel::new();
    let mut ids = Vec::new();
    {
        let mut b = TopologyBuilder::new(&mut model);
        ids.push(solid(b.create_box_3d(4.0, 3.0, 2.0).expect("box")));
    }
    {
        let mut b = TopologyBuilder::new(&mut model);
        ids.push(solid(
            b.create_cylinder_3d(
                geometry_engine::math::Point3::new(10.0, 0.0, 0.0),
                geometry_engine::math::Vector3::new(0.0, 0.0, 1.0),
                1.5,
                5.0,
            )
            .expect("cylinder"),
        ));
    }
    {
        let mut b = TopologyBuilder::new(&mut model);
        ids.push(solid(
            b.create_sphere_3d(geometry_engine::math::Point3::new(-10.0, 0.0, 0.0), 2.0)
                .expect("sphere"),
        ));
    }
    (model, ids)
}

#[test]
fn shared_certification_gives_the_certificates_certify_solids_gives() {
    let (mut a, ids) = three_parts();
    let (b, ids_b) = three_parts();
    assert_eq!(ids, ids_b);
    let exclusive = a.certify_solids(&ids);
    let shared = b
        .certify_solids_shared(&ids)
        .expect("a model with no labels certifies through a shared borrow");
    assert_eq!(exclusive.len(), 3);
    assert_eq!(shared, exclusive, "one computation, two borrows");

    // And it is CACHED like the `&mut` path caches it: the next read is the
    // stored certificate, and the part reads as freshly verified.
    let mut b = b;
    for (id, cert) in &shared {
        assert_eq!(&b.certify_solid(*id), cert);
        assert!(
            !matches!(
                b.soundness_reading(*id),
                Some(geometry_engine::primitives::provenance::SoundnessReading::Stale { .. })
                    | None
            ),
            "solid {id} reads as verified"
        );
    }
}

#[test]
fn a_labelled_model_is_not_certified_through_a_shared_borrow() {
    let (mut model, ids) = three_parts();
    let face = {
        let s = model.solids.get(ids[0]).expect("box");
        model.shells.get(s.outer_shell).expect("shell").faces[0]
    };
    model.label_face(face, "datum A", None).expect("label");
    assert!(
        model.certify_solids_shared(&ids).is_none(),
        "the labels dimension needs `&mut`; a certificate without it is refused"
    );
}

#[test]
fn measuring_then_installing_gives_the_volumes_calculate_gives() {
    let (mut a, ids) = three_parts();
    let (mut b, _) = three_parts();
    let exclusive = a.calculate_solid_volumes(&ids);
    let measured = b.measure_solid_volumes(&ids);
    let installed = b.install_measured_volumes(measured);
    assert_eq!(installed.len(), 3, "every part measured and installed");
    for (id, volume) in &exclusive {
        assert_eq!(installed.get(id), Some(volume), "solid {id}");
        assert_eq!(b.calculate_solid_volume(*id), *volume, "solid {id} cached");
    }
}

#[test]
fn a_measurement_of_a_part_touched_since_is_not_installed() {
    let (mut model, ids) = three_parts();
    let measured = model.measure_solid_volumes(&ids);
    // Another request touches the box between the measuring and the install.
    let _ = model.solids.get_mut(ids[0]);
    let installed = model.install_measured_volumes(measured);
    assert!(
        !installed.contains_key(&ids[0]),
        "a measurement of a shape the part may no longer have is dropped"
    );
    assert!(installed.contains_key(&ids[1]) && installed.contains_key(&ids[2]));
}

#[test]
fn a_cached_display_mesh_is_cleared_by_any_touch() {
    let (mut model, ids) = three_parts();
    let params = TessellationParams::default();
    let s = model.solids.get(ids[1]).expect("cylinder");
    let epoch = s.mutation_epoch();
    let mesh = Arc::new(tessellate_solid(s, &model, &params));
    assert!(s.store_display_mesh(Arc::clone(&mesh), epoch));
    assert!(model
        .solids
        .get(ids[1])
        .expect("cylinder")
        .cached_display_mesh()
        .is_some());

    let _ = model.solids.get_mut(ids[1]);
    assert!(
        model
            .solids
            .get(ids[1])
            .expect("cylinder")
            .cached_display_mesh()
            .is_none(),
        "a touched part's mesh is gone with its certificate"
    );
}

#[test]
fn a_mesh_of_a_part_touched_while_it_was_built_is_not_stored() {
    let (mut model, ids) = three_parts();
    let params = TessellationParams::default();
    let (epoch, mesh) = {
        let s = model.solids.get(ids[2]).expect("sphere");
        (
            s.mutation_epoch(),
            Arc::new(tessellate_solid(s, &model, &params)),
        )
    };
    let _ = model.solids.get_mut(ids[2]);
    let s = model.solids.get(ids[2]).expect("sphere");
    assert!(
        !s.store_display_mesh(mesh, epoch),
        "a stale mesh is refused"
    );
    assert!(s.cached_display_mesh().is_none());
}

#[test]
fn an_operation_on_a_part_clears_its_display_mesh() {
    // The `record_operation` funnel — every kernel op — not only `get_mut`.
    let (model, ids) = three_parts();
    let params = TessellationParams::default();
    let s = model.solids.get(ids[0]).expect("box");
    let epoch = s.mutation_epoch();
    assert!(s.store_display_mesh(Arc::new(tessellate_solid(s, &model, &params)), epoch));
    model.record_operation(
        geometry_engine::operations::recorder::RecordedOperation::new("transform_solid")
            .with_input_solids([ids[0] as u64])
            .with_output_solids([ids[0] as u64]),
    );
    assert!(model
        .solids
        .get(ids[0])
        .expect("box")
        .cached_display_mesh()
        .is_none());
}
