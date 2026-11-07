// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! IMP-2: `certify_solids` certifies a batch with ONE model-wide validation
//! sweep, and must give every solid exactly the certificate `certify_solid`
//! gives it alone — including a broken solid beside sound ones, so a wrong
//! partition of the model's errors (leaking one part's defect into another,
//! or dropping it) fails here.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn solid(id: GeometryId) -> SolidId {
    match id {
        GeometryId::Solid(s) => s,
        other => panic!("expected a solid, got {other:?}"),
    }
}

/// A box, a cylinder, a sphere, and a second box with one face torn out of
/// its shell (open, invalid, and the torn face left as orphan debris).
/// Deterministic, so two calls build identical models with identical ids.
fn scene() -> (BRepModel, Vec<SolidId>, SolidId) {
    let mut model = BRepModel::new();
    let ids = {
        let mut b = TopologyBuilder::new(&mut model);
        vec![
            solid(b.create_box_3d(10.0, 10.0, 10.0).expect("box")),
            solid(
                b.create_cylinder_3d(
                    Point3::new(30.0, 0.0, 0.0),
                    Vector3::new(0.0, 0.0, 1.0),
                    4.0,
                    12.0,
                )
                .expect("cylinder"),
            ),
            solid(
                b.create_sphere_3d(Point3::new(0.0, 30.0, 0.0), 5.0)
                    .expect("sphere"),
            ),
            solid(b.create_box_3d(4.0, 4.0, 4.0).expect("second box")),
        ]
    };
    let broken = ids[3];
    let shell = model.solids.get(broken).expect("broken box").outer_shell;
    let face = *model
        .shells
        .get(shell)
        .expect("shell")
        .faces
        .first()
        .expect("faces");
    assert!(model
        .shells
        .get_mut(shell)
        .expect("shell")
        .remove_face(face));
    (model, ids, broken)
}

#[test]
fn a_batch_certificate_equals_the_certificate_each_solid_gets_alone() {
    let (mut batch_model, ids, broken) = scene();
    let batch = batch_model.certify_solids(&ids);
    assert_eq!(
        batch.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        ids,
        "every id certified, in input order"
    );

    for (id, batch_cert) in &batch {
        let (mut alone_model, _, _) = scene();
        let alone = alone_model.certify_solid(*id);
        assert_eq!(
            batch_cert, &alone,
            "solid {id}: the batch certificate must equal the solo one"
        );
        assert_eq!(
            batch_cert.brep_valid,
            *id != broken,
            "solid {id}: only the torn box is B-Rep invalid; errors={:?}",
            batch_cert.errors
        );
        assert_eq!(
            batch_model.cached_soundness(*id),
            Some(batch_cert.is_sound()),
            "solid {id}: the batch stores its verdict in the cache"
        );
    }
    assert!(
        batch.iter().all(|(_, c)| c.model_debris_orphan_faces == 1),
        "the torn face is model debris, counted once for every part"
    );

    // A second batch on the unchanged model is served from the cache.
    let again = batch_model.certify_solids(&ids);
    assert_eq!(again, batch, "an unmutated model certifies identically");
}

/// IMP-2: `calculate_solid_volumes` computes a batch's mass properties in
/// parallel and must give every solid exactly the volume (and the cached
/// mass properties behind it) `calculate_solid_volume` gives it alone —
/// the box takes the exact-polyhedron path, the cylinder and sphere the
/// mesh path, and the torn box whatever the solo call does with it.
#[test]
fn a_batch_of_volumes_equals_each_solid_measured_alone() {
    let (mut batch_model, ids, _) = scene();
    let batch = batch_model.calculate_solid_volumes(&ids);
    assert_eq!(
        batch.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        ids,
        "every id measured, in input order"
    );
    for (id, volume) in &batch {
        let (mut alone_model, _, _) = scene();
        let alone = alone_model.calculate_solid_volume(*id);
        assert_eq!(*volume, alone, "solid {id}: batch volume equals solo");
        // The batch installs the same cache the solo call does: the next
        // read on the batch model is the solo model's cached value.
        assert_eq!(
            batch_model.calculate_solid_surface_area(*id),
            alone_model.calculate_solid_surface_area(*id),
            "solid {id}: the cached mass properties match"
        );
    }

    // Volumes first, then certificates: the certificates survive, so a
    // registration that warms volumes and then certifies keeps its verdicts.
    let certs = batch_model.certify_solids(&ids);
    for (id, cert) in &certs {
        assert_eq!(
            batch_model.cached_soundness(*id),
            Some(cert.is_sound()),
            "solid {id}: certificate still cached after the volume batch"
        );
    }
}

/// IMP-2: `tessellate_solids` meshes a batch in parallel and must hand back,
/// for every solid, exactly the mesh `tessellate_solid` builds alone, in
/// input order.
#[test]
fn a_batch_of_display_meshes_equals_each_solid_meshed_alone() {
    use geometry_engine::tessellation::{tessellate_solid, tessellate_solids, TessellationParams};
    let (model, ids, _) = scene();
    let params = TessellationParams::default();
    let batch = tessellate_solids(&model, &ids, &params);
    assert_eq!(
        batch.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        ids,
        "every id meshed, in input order"
    );
    for (id, mesh) in &batch {
        let alone = tessellate_solid(model.solids.get(*id).expect("solid"), &model, &params);
        assert!(!mesh.triangles.is_empty(), "solid {id} meshes to something");
        assert_eq!(mesh.triangles, alone.triangles, "solid {id}: triangles");
        assert_eq!(mesh.face_map, alone.face_map, "solid {id}: face map");
        let positions = |m: &geometry_engine::tessellation::TriangleMesh| {
            m.vertices
                .iter()
                .map(|v| [v.position.x, v.position.y, v.position.z])
                .collect::<Vec<_>>()
        };
        assert_eq!(positions(mesh), positions(&alone), "solid {id}: vertices");
    }
}
