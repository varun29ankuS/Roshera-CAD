//! VERIFICATION-LAYER gate: display-tessellation quality is a first-class
//! certificate dimension (Varun: "include the display tessellation in the
//! verification layer for the agent").
//!
//! A solid can be a valid, watertight, manifold B-Rep yet tessellate to a
//! degenerate / inverted-normal mesh (the inner-bore "scribble"): the mesh
//! closes and every edge borders two faces, so every TOPOLOGICAL check passes,
//! but the facets are zero-area slivers or face the wrong way. Before this
//! invariant the kernel certified such a solid `sound = true` — it could LIE
//! about a part that renders as garbage. These tests pin the new behaviour:
//!
//! 1. the pure scorer FLAGS an inverted-normal facet and a degenerate facet,
//! 2. it LOCALISES the worst face for the agent (deterministically),
//! 3. a clean primitive still certifies `tessellation.clean == true` (no false
//!    positive), and the dimension feeds `is_sound()`.

use geometry_engine::harness::watertight::score_mesh_tessellation;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::mesh::{MeshVertex, TriangleMesh};

/// Append one independent triangle (its own three vertices) lying in the XY
/// plane, with every vertex normal set to `normal`, tagged to face `face_id`.
/// Winding is CCW so the geometric (winding) normal is `+Z`.
fn push_flat_tri(mesh: &mut TriangleMesh, normal: Vector3, face_id: u32, offset: f64) {
    let n = normal;
    let a = mesh.add_vertex(MeshVertex {
        position: Point3::new(offset, 0.0, 0.0),
        normal: n,
        uv: None,
    });
    let b = mesh.add_vertex(MeshVertex {
        position: Point3::new(offset + 1.0, 0.0, 0.0),
        normal: n,
        uv: None,
    });
    let c = mesh.add_vertex(MeshVertex {
        position: Point3::new(offset + 1.0, 1.0, 0.0),
        normal: n,
        uv: None,
    });
    mesh.add_triangle(a, b, c);
    mesh.face_map.push(face_id);
}

#[test]
fn clean_mesh_scores_clean() {
    // Two facets, stored normals = +Z = their winding normal.
    let mut mesh = TriangleMesh::new();
    push_flat_tri(&mut mesh, Vector3::Z, 1, 0.0);
    push_flat_tri(&mut mesh, Vector3::Z, 1, 2.0);

    let q = score_mesh_tessellation(&mesh);
    assert!(q.clean, "all-consistent mesh must be clean: {q:?}");
    assert_eq!(q.degenerate_triangles, 0);
    assert_eq!(q.inconsistent_facets, 0);
    assert!((q.normal_agreement - 1.0).abs() < 1e-12);
    assert!(q.worst_face.is_none(), "clean mesh has no worst face");
}

#[test]
fn inverted_normal_facet_is_flagged_and_localised() {
    // Face 1 clean (+Z); face 7 has its stored normals inverted (−Z) while its
    // geometry still winds +Z — the bore-scribble class. Half the facets agree.
    let mut mesh = TriangleMesh::new();
    push_flat_tri(&mut mesh, Vector3::Z, 1, 0.0);
    push_flat_tri(&mut mesh, -Vector3::Z, 7, 2.0);

    let q = score_mesh_tessellation(&mesh);
    assert!(
        !q.clean,
        "inverted-normal facet must make the mesh not clean"
    );
    assert_eq!(q.inconsistent_facets, 1);
    assert!(
        (q.normal_agreement - 0.5).abs() < 1e-12,
        "got {}",
        q.normal_agreement
    );
    let worst = q.worst_face.expect("a defective face must be reported");
    assert_eq!(worst.face_id, 7, "the worst face must be the inverted one");
    assert!(worst.normal_agreement < 0.5 + 1e-9);
}

#[test]
fn degenerate_facet_is_counted_and_blocks_clean() {
    // One good facet + one zero-area sliver (three coincident-ish collinear pts).
    let mut mesh = TriangleMesh::new();
    push_flat_tri(&mut mesh, Vector3::Z, 1, 0.0);
    let a = mesh.add_vertex(MeshVertex {
        position: Point3::new(5.0, 0.0, 0.0),
        normal: Vector3::Z,
        uv: None,
    });
    let b = mesh.add_vertex(MeshVertex {
        position: Point3::new(6.0, 0.0, 0.0),
        normal: Vector3::Z,
        uv: None,
    });
    let c = mesh.add_vertex(MeshVertex {
        position: Point3::new(7.0, 0.0, 0.0), // collinear with a,b → zero area
        normal: Vector3::Z,
        uv: None,
    });
    mesh.add_triangle(a, b, c);
    mesh.face_map.push(9);

    let q = score_mesh_tessellation(&mesh);
    assert_eq!(q.degenerate_triangles, 1, "{q:?}");
    assert!(!q.clean, "a degenerate facet must block clean");
    assert_eq!(q.worst_face.expect("face 9 defective").face_id, 9);
}

#[test]
fn worst_face_selection_is_deterministic() {
    // Two defective faces with different severities; the worst (lowest agreement)
    // must be picked identically across runs regardless of HashMap order.
    let build = || {
        let mut mesh = TriangleMesh::new();
        // face 3: 1 of 2 inverted → agreement 0.5
        push_flat_tri(&mut mesh, Vector3::Z, 3, 0.0);
        push_flat_tri(&mut mesh, -Vector3::Z, 3, 2.0);
        // face 5: both inverted → agreement 0.0 (worst)
        push_flat_tri(&mut mesh, -Vector3::Z, 5, 4.0);
        push_flat_tri(&mut mesh, -Vector3::Z, 5, 6.0);
        mesh
    };
    let first = score_mesh_tessellation(&build())
        .worst_face
        .expect("defect");
    for _ in 0..8 {
        let again = score_mesh_tessellation(&build())
            .worst_face
            .expect("defect");
        assert_eq!(
            again.face_id, first.face_id,
            "worst-face pick must be stable"
        );
        assert_eq!(again.face_id, 5, "face 5 (0% agreement) is the worst");
    }
}

#[test]
fn clean_primitive_certifies_tessellation_clean_and_sound() {
    // A plain solid cylinder is a real curved primitive whose OD tessellates
    // clean — the no-false-positive guard for the live certificate path.
    let mut m = BRepModel::new();
    let g = TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 20.0, 40.0)
        .expect("cylinder");
    let sid = match g {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    };
    let cert = m.certify_solid(sid);
    assert!(
        cert.tessellation.clean,
        "a clean cylinder must certify tessellation-clean: {:?}",
        cert.tessellation
    );
    assert_eq!(cert.tessellation.degenerate_triangles, 0);
    assert!((cert.tessellation.normal_agreement - 1.0).abs() < 1e-9);
    assert!(cert.is_sound(), "a clean watertight cylinder must be sound");
}
