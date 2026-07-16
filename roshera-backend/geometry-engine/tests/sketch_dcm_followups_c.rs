// Reason: integration-test crate -- panicking (unwrap/expect/assert/index) is
// the test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::indexing_slicing)]

//! SKETCH-DCM #45 — Wave C follow-ups (item 3): seam alignment of
//! `try_build_cylinder_from_circles` (Slice-5 residual 9).
//!
//! A sketch-extruded full-circle wall collapses to an analytic
//! `Cylinder`. Its closed circle edge's start==end vertex sits at the
//! circle's t = 0 (CDT-γ.3), so the SURFACE's parametric seam
//! (`ref_dir`, u = 0) must sit there too — the invariant
//! `create_cylinder_topology` documents and enforces for the primitive
//! ("consumers that read `cylinder.ref_dir` to locate the seam …
//! otherwise compute positions π/2 out of phase with the real seam
//! vertex"). Pre-fix, the extrude path left `ref_dir` at
//! `Cylinder::new_finite`'s default `axis.perpendicular()`.

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::extrude::{extrude_profile_regions, ProfileLoop, ProfileRegion};
use geometry_engine::operations::fillet::{FilletType, PropagationMode};
use geometry_engine::operations::{fillet_edges, FilletOptions};
use geometry_engine::primitives::surface::{Cylinder, Surface};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::sketch2d::sketch_topology::ProfileEdge;

const DISK_R: f64 = 6.0;
const DISK_H: f64 = 10.0;

/// Extrude a full-circle profile (a disk) through the shared kernel
/// entry — the csketch/timeline route's exact path. Returns the model
/// and the solid id.
fn extrude_disk() -> (BRepModel, u32) {
    let region = ProfileRegion {
        outer: ProfileLoop::Edges(vec![ProfileEdge::Circle {
            center: [0.0, 0.0],
            radius: DISK_R,
        }]),
        holes: vec![],
    };
    let mut model = BRepModel::new();
    let solid = extrude_profile_regions(
        &mut model,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::X,
        Vector3::Y,
        &[region],
        DISK_H,
        None,
        Tolerance::default(),
    )
    .expect("disk extrude succeeds");
    (model, solid)
}

/// The lateral `Cylinder` face's surface, cloned out of the store.
fn wall_cylinder(model: &BRepModel) -> Cylinder {
    for (_fid, face) in model.faces.iter() {
        if let Some(surface) = model.surfaces.get(face.surface_id) {
            if let Some(cyl) = surface.as_any().downcast_ref::<Cylinder>() {
                return *cyl;
            }
        }
    }
    panic!("extruded disk must carry exactly one Cylinder wall face");
}

/// The closed circle edge's start==end seam vertex position.
fn seam_vertex_position(model: &BRepModel) -> Point3 {
    for (_eid, edge) in model.edges.iter() {
        if edge.start_vertex == edge.end_vertex {
            let p = model
                .vertices
                .get_position(edge.start_vertex)
                .expect("seam vertex has a position");
            return Point3::new(p[0], p[1], p[2]);
        }
    }
    panic!("extruded disk must carry a closed (seam-vertex) circle edge");
}

/// GATE (structural): the wall cylinder's `ref_dir` (parametric seam,
/// u = 0) points at the closed circle edge's seam vertex. Pre-fix:
/// `axis.perpendicular()` = −Y for a +Z extrude, while the seam vertex
/// sits at +X — π/2 out of phase.
#[test]
fn extruded_disk_wall_ref_dir_points_at_seam_vertex() {
    let (model, _solid) = extrude_disk();
    let cyl = wall_cylinder(&model);
    let seam = seam_vertex_position(&model);

    // Radial direction of the seam vertex from the cylinder axis.
    let d = seam - cyl.origin;
    let radial = (d - cyl.axis * d.dot(&cyl.axis))
        .normalize()
        .expect("seam vertex is off-axis");
    assert!(
        cyl.ref_dir.dot(&radial) > 1.0 - 1e-9,
        "wall cylinder ref_dir must be seam-aligned to the closed circle \
         edge's start==end vertex (create_cylinder_topology invariant): \
         ref_dir = {:?}, seam radial = {:?}",
        cyl.ref_dir,
        radial
    );

    // And the surface's (u, v) = (0, 0) evaluates ON that vertex.
    let at_origin = cyl.point_at(0.0, 0.0).expect("surface at (0,0)");
    assert!(
        at_origin.distance(&seam) < 1e-9,
        "surface parametric origin must coincide with the seam vertex: \
         {at_origin:?} vs {seam:?}"
    );
}

/// Behavioural companion: a bore-rim-style fillet on the sketch-extruded
/// disk's top rim must succeed AND validate (fillet_edges validates the
/// result by default — success IS the sound verdict). The rim fillet
/// derives its seam anchor from the rim edge's own vertex, so this pins
/// the whole seam-consistency chain (wall surface + cap edges + blend).
#[test]
fn rim_fillet_on_sketch_extruded_disk_is_sound() {
    let (mut model, solid) = extrude_disk();

    // Top rim = the closed edge whose vertex sits at z = DISK_H.
    let rim = model
        .edges
        .iter()
        .filter(|(_, e)| e.start_vertex == e.end_vertex)
        .find(|(_, e)| {
            model
                .vertices
                .get_position(e.start_vertex)
                .map(|p| (p[2] - DISK_H).abs() < 1e-9)
                .unwrap_or(false)
        })
        .map(|(id, _)| id)
        .expect("disk has a closed top rim edge");

    let opts = FilletOptions {
        fillet_type: FilletType::Constant(1.0),
        radius: 1.0,
        propagation: PropagationMode::None,
        ..Default::default()
    };
    let result = fillet_edges(&mut model, solid, vec![rim], opts);
    assert!(
        result.is_ok(),
        "rim fillet on a sketch-extruded disk must validate sound, got {:?}",
        result.err()
    );
}
