//! Fix A wave-3 — filleting a boolean-derived concave PLANE-PLANE edge must
//! compute the correct BALL-SIDE. This routes through
//! `spine_solver::solve_plane_plane` (constant radius, plane∩plane).
//!
//! The concave edge's two supporting faces are tool-derived `Backward` faces
//! whose stored loop winding boolean Difference left inconsistent with their
//! flipped outward normals. Before wave-3, `solve_plane_plane` took the tangent
//! handedness from that stale flag, computed `dihedral > 0` (convex) for a
//! geometrically CONCAVE edge, and offset the rolling ball to the WRONG side —
//! producing a non-orientable (invalid) fillet. After routing the tangent sign
//! through `geometry_signed_edge_tangent`, the ball sits in the void wedge, the
//! fillet ADDS material (a concave blend), and the solid stays watertight.
//!
//! Ball-side is asserted DIRECTLY via volume: a correct concave fillet ADDS a
//! `r²(1−π/4)·L` sliver of material; a wrong (convex) ball-side removes material
//! or fails outright. The single concave edge has SIMPLE endpoints for this
//! feature (no multi-edge corner-patch synthesis is on the critical path — only
//! one edge is filleted).

use geometry_engine::math::Vector3;
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::edge_classification::{classify_edge, find_adjacent_faces};
use geometry_engine::operations::fillet::{fillet_edges, FilletOptions, FilletType};
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::primitives::mass_properties::integrate_solid;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::SurfaceType;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// A 40³ box (centred, spans −20..20) with the +X/+Z quadrant removed for the
/// full Y depth by a boolean Difference — an L-shaped prism. The single
/// re-entrant edge runs along Y at (x=0, z=0); both its faces are the
/// tool-derived step walls (planes). Volume = 40³ − 20·20·40 = 48000.
fn l_prism(m: &mut BRepModel) -> SolidId {
    let base = match TopologyBuilder::new(m)
        .create_box_3d(40.0, 40.0, 40.0)
        .expect("base")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid for base, got {o:?}"),
    };
    // Tool: 20×60×20 — full Y (60 ⊃ 40), shifted +X10/+Z10 → occupies
    // x∈[0,20], y∈[−30,30], z∈[0,20]. Difference removes the +X+Z quadrant.
    let tool = match TopologyBuilder::new(m)
        .create_box_3d(20.0, 60.0, 20.0)
        .expect("tool")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid for tool, got {o:?}"),
    };
    translate(m, vec![tool], Vector3::X, 10.0, TransformOptions::default()).expect("tx");
    translate(m, vec![tool], Vector3::Z, 10.0, TransformOptions::default()).expect("tz");
    boolean_operation(
        m,
        base,
        tool,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("cut")
}

#[test]
fn boolean_concave_step_fillet_is_on_material_side() {
    let mut m = BRepModel::new();
    let s = l_prism(&mut m);

    let cert = m.certify_solid(s);
    assert!(
        cert.brep_valid && cert.watertight && cert.manifold,
        "L-prism must be sound before filleting; got bv={} wt={} mf={} errors={:?}",
        cert.brep_valid,
        cert.watertight,
        cert.manifold,
        cert.errors
    );
    let vol0 = integrate_solid(s, &m, 1.0, 1e-6)
        .expect("volume before")
        .volume;

    // Exactly one concave edge (the re-entrant step corner), and it is
    // PLANE∩PLANE so a constant-radius fillet routes solve_plane_plane.
    let concave: Vec<u32> = m
        .edges
        .iter()
        .filter(|(eid, _)| {
            classify_edge(&m, *eid)
                .map(|c| c.convexity == -1)
                .unwrap_or(false)
        })
        .map(|(eid, _)| eid)
        .collect();
    assert_eq!(
        concave.len(),
        1,
        "the L-prism has exactly one concave edge; got {concave:?}"
    );
    let edge = concave[0];

    let faces = find_adjacent_faces(&m, edge);
    assert_eq!(
        faces.len(),
        2,
        "concave edge must be a manifold two-face edge"
    );
    for &fid in &faces {
        let face = m.faces.get(fid).expect("adjacent face present");
        let surf = m
            .surfaces
            .get(face.surface_id)
            .expect("supporting surface present");
        assert_eq!(
            surf.surface_type(),
            SurfaceType::Plane,
            "concave edge must be plane∩plane to route spine_solver::solve_plane_plane"
        );
    }

    // Fillet ONLY the concave edge. With the stale loop flag the ball-side is
    // wrong and this errors (non-orientable fillet) — the RED.
    let opts = FilletOptions {
        fillet_type: FilletType::Constant(3.0),
        radius: 3.0,
        ..Default::default()
    };
    let created = fillet_edges(&mut m, s, vec![edge], opts)
        .expect("filleting the concave plane∩plane edge must succeed with the correct ball-side");
    assert!(
        !created.is_empty(),
        "fillet must create at least one blend face"
    );

    let cert2 = m.certify_solid(s);
    assert!(
        cert2.brep_valid && cert2.watertight && cert2.manifold,
        "filleted solid must stay sound; got bv={} wt={} mf={} errors={:?}",
        cert2.brep_valid,
        cert2.watertight,
        cert2.manifold,
        cert2.errors
    );

    // BALL-SIDE proof: a concave fillet ADDS a r²(1−π/4)·L sliver of material
    // (fills the void wedge). A wrong (convex) ball-side removes material or
    // errors — so a strict volume INCREASE proves the ball sits on the concave
    // (material) side.
    let vol1 = integrate_solid(s, &m, 1.0, 1e-6)
        .expect("volume after")
        .volume;
    let expected_add = 9.0 * (1.0 - std::f64::consts::FRAC_PI_4) * 40.0; // ≈ 77.3
    assert!(
        vol1 > vol0 + 40.0,
        "concave-edge fillet must ADD material (correct ball-side): vol0={vol0}, vol1={vol1}, expected +~{expected_add:.1}"
    );
    assert!(
        vol1 < vol0 + 120.0,
        "fillet added implausibly much material (wrong topology?): vol0={vol0}, vol1={vol1}"
    );
}
