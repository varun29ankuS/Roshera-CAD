// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The certificate's mesh-quality gate fails a solid for a FOLD: a facet that
//! bridges away from the true surface (the bore "wing"). It judged a fold by
//! normal alone, so a near-degenerate sliver lying ON the surface -- corners a
//! hair apart, its plane meaningless, its normal 75 degrees off -- failed four
//! imported CADGenBench parts whose meshes were faithful within the chord. A
//! fold must also LEAVE the surface (its centroid farther off than the chord);
//! a sliver on the surface is a shape-quality readout, like aspect ratio.

use geometry_engine::harness::watertight::mesh_quality_with_normals;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::face::{Face, FaceOrientation};
use geometry_engine::primitives::surface::Cylinder;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::tessellation::{MeshVertex, TriangleMesh};

const R: f64 = 2.0;

/// A model holding one face on a radius-2 cylinder about +Z; its face id.
fn cylinder_face() -> (BRepModel, u32) {
    let mut model = BRepModel::new();
    let cyl = Cylinder::new(Point3::ORIGIN, Vector3::Z, R).expect("cylinder");
    let sid = model.surfaces.add(Box::new(cyl));
    let fid = model
        .faces
        .add(Face::new(0, sid, 0, FaceOrientation::Forward));
    (model, fid)
}

fn on_cylinder(theta: f64, z: f64) -> Point3 {
    Point3::new(R * theta.cos(), R * theta.sin(), z)
}

/// One triangle on face `fid`, with the true (radial) normal at its centroid
/// projected onto the cylinder.
fn one_triangle(fid: u32, a: Point3, b: Point3, c: Point3) -> (TriangleMesh, Vec<Option<Vector3>>) {
    let mut mesh = TriangleMesh::new();
    let ids: Vec<u32> = [a, b, c]
        .iter()
        .map(|&p| {
            mesh.add_vertex(MeshVertex {
                position: p,
                normal: Vector3::Z,
                uv: None,
            })
        })
        .collect();
    mesh.add_triangle(ids[0], ids[1], ids[2]);
    mesh.face_map = vec![fid];
    let m = Point3::new((a.x + b.x + c.x) / 3.0, (a.y + b.y + c.y) / 3.0, 0.0);
    let radial = Vector3::new(m.x, m.y, 0.0)
        .normalize()
        .unwrap_or(Vector3::X);
    (mesh, vec![Some(radial)])
}

#[test]
fn a_sliver_lying_on_the_surface_is_not_a_fold() {
    let (model, fid) = cylinder_face();
    // Three points 0.001 rad apart along a helix: on the surface, but the
    // plane through them is the helix's osculating plane, 90 degrees off.
    let (mesh, normals) = one_triangle(
        fid,
        on_cylinder(0.0, 0.0),
        on_cylinder(0.001, 0.0005),
        on_cylinder(0.002, 0.001),
    );
    let q = mesh_quality_with_normals(&model, &mesh, &normals).expect("quality");
    assert!(
        q.clean,
        "a sliver on the surface must not fail the gate: deviation {:.1} deg",
        q.max_normal_deviation_deg
    );
}

#[test]
fn a_facet_bridging_across_the_bore_is_still_a_fold() {
    let (model, fid) = cylinder_face();
    // A facet from one side of the bore to the other: its centroid sits
    // near the axis, a radius off the surface.
    let (mesh, normals) = one_triangle(
        fid,
        on_cylinder(0.0, 0.0),
        on_cylinder(std::f64::consts::PI, 0.0),
        on_cylinder(std::f64::consts::FRAC_PI_2, 1.0),
    );
    let q = mesh_quality_with_normals(&model, &mesh, &normals).expect("quality");
    assert!(!q.clean, "a bridge across the bore must fail the gate");
}

/// One triangle on face `fid` whose vertices carry the given stored (u, v).
fn triangle_with_uv(
    fid: u32,
    pts: [Point3; 3],
    uvs: [Option<(f64, f64)>; 3],
) -> (TriangleMesh, Vec<Option<Vector3>>) {
    let mut mesh = TriangleMesh::new();
    let ids: Vec<u32> = (0..3)
        .map(|k| {
            mesh.add_vertex(MeshVertex {
                position: pts[k],
                normal: Vector3::Z,
                uv: uvs[k],
            })
        })
        .collect();
    mesh.add_triangle(ids[0], ids[1], ids[2]);
    mesh.face_map = vec![fid];
    let c = Point3::new(
        (pts[0].x + pts[1].x + pts[2].x) / 3.0,
        (pts[0].y + pts[1].y + pts[2].y) / 3.0,
        0.0,
    );
    let radial = Vector3::new(c.x, c.y, 0.0)
        .normalize()
        .unwrap_or(Vector3::X);
    (mesh, vec![Some(radial)])
}

/// A vertex shared by two faces keeps ONE face's stored (u, v) when the shell
/// is welded -- a torus fillet's rim vertex carried its neighbouring
/// cylinder's (u = -0.013, v = -45). Read as this face's chart, a tiny facet
/// looked like a bridge across the period. Whether a facet crosses the face
/// is decided from where its corners ARE on this face's own surface.
#[test]
fn a_tiny_facet_is_not_a_crossing_whatever_its_stored_uv() {
    let (model, fid) = cylinder_face();
    let pts = [
        on_cylinder(0.01, 0.0),
        on_cylinder(0.02, 0.0),
        on_cylinder(0.015, 0.1),
    ];
    // Stored u's from another face's chart: antipodal on this one.
    let uvs = [Some((0.01, 0.0)), Some((2.2, -45.0)), Some((4.4, 0.1))];
    let (mesh, normals) = triangle_with_uv(fid, pts, uvs);
    let q = mesh_quality_with_normals(&model, &mesh, &normals).expect("quality");
    assert_eq!(
        q.boundary_crossing_facets, 0,
        "a 0.02 mm facet crosses nothing"
    );
}

/// The other side: a facet bridging the bore crosses the face even when its
/// vertices carry no stored (u, v) at all.
#[test]
fn a_bridge_is_a_crossing_without_stored_uv() {
    let (model, fid) = cylinder_face();
    let pts = [
        on_cylinder(0.0, 0.0),
        on_cylinder(std::f64::consts::PI + 0.3, 0.0),
        on_cylinder(std::f64::consts::PI - 0.3, 0.5),
    ];
    let (mesh, normals) = triangle_with_uv(fid, pts, [None, None, None]);
    let q = mesh_quality_with_normals(&model, &mesh, &normals).expect("quality");
    assert!(
        q.boundary_crossing_facets > 0,
        "a bore bridge is a crossing"
    );
}

/// Kernel Lens: the quality verdict lists EVERY defective face, not only the
/// worst, so each one can be shown and named.
#[test]
fn every_defective_face_is_listed() {
    let (model, fid) = cylinder_face();
    let (mesh, normals) = one_triangle(
        fid,
        on_cylinder(0.0, 0.0),
        on_cylinder(std::f64::consts::PI, 0.0),
        on_cylinder(std::f64::consts::FRAC_PI_2, 1.0),
    );
    let q = mesh_quality_with_normals(&model, &mesh, &normals).expect("quality");
    assert!(
        q.defects.iter().any(|d| d.face_id == fid as u64),
        "the bridging face is listed: {:#?}",
        q.defects
    );
}
