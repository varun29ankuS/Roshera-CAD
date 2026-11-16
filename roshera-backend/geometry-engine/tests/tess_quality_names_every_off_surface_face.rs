// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The display-mesh check fails a part on a single facet that points away
//! from its surface. It must also NAME that face: before, a face was listed
//! only once its whole agreement ratio fell under 0.999, so one bad facet
//! among thousands failed the part with no face to look at (CADGenBench
//! part 102: "1 off-surface facet", no face named).

use geometry_engine::harness::watertight::tessellation_quality_of_mesh;
use geometry_engine::math::Point3;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

#[test]
fn one_flipped_facet_among_thousands_is_named_on_its_face() {
    let mut model = BRepModel::new();
    let sid = match TopologyBuilder::new(&mut model)
        .create_sphere_3d(Point3::new(0.0, 0.0, 0.0), 10.0)
        .expect("sphere")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected a solid, got {other:?}"),
    };
    let params = TessellationParams {
        chord_tolerance: 0.002,
        ..TessellationParams::default()
    };
    let mut mesh = {
        let solid = model.solids.get(sid).expect("solid");
        tessellate_solid(&solid, &model, &params)
    };
    let clean = tessellation_quality_of_mesh(&model, &mesh).expect("quality");
    assert!(clean.clean, "the sphere's own mesh is clean: {clean:?}");

    // The face with the most facets, and one of its facets turned over.
    let mut counts = std::collections::HashMap::<u32, usize>::new();
    for &f in &mesh.face_map {
        *counts.entry(f).or_default() += 1;
    }
    let (&face, &n) = counts.iter().max_by_key(|(_, n)| **n).expect("faces");
    assert!(
        n > 1000,
        "need a face whose ratio hides one facet ({n} facets)"
    );
    let t = mesh
        .face_map
        .iter()
        .position(|&f| f == face)
        .expect("a facet on the face");
    mesh.triangles[t].swap(1, 2);

    let q = tessellation_quality_of_mesh(&model, &mesh).expect("quality");
    assert!(!q.clean, "one off-surface facet fails the check: {q:?}");
    let named = q
        .defects
        .iter()
        .find(|d| d.face_id == face as u64)
        .unwrap_or_else(|| panic!("face {face} is named: {:?}", q.defects));
    assert_eq!(named.off_surface_facets, 1, "{named:?}");
}
