// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Regression: mass-properties on a multi-edge-corner fillet must TERMINATE.
//!
//! ## The bug (dogfood 2026-07-09)
//!
//! `MCP fillet_edges` on all 12 edges of a box wedged the api-server for >120 s
//! (restart required). Root-caused to the tessellation vertex weld
//! (`weld_mesh_watertight_range`): its spatial-hash cell size was
//! `tolerance × 1000`, so on a dense `fine()` mesh (vertex spacing
//! ~`max_edge_length` 0.01, tol 0.0001 → cells 0.1) every cell held ~1000
//! vertices and the 3×3×3 neighbourhood scan degraded to ~O(n²). A fillet
//! **corner-cap** (a `Sphere` face where ≥3 filleted edges meet) tessellates at
//! `fine()` to the ~150k-triangle fan-budget cap (~76k verts), which the
//! quadratic weld then chewed on for minutes. The ambient mass-properties
//! (`calculate_solid_volume` → `mesh_based_mass_properties`) tessellate at
//! `fine()` under the write lock, so this presented as a full-backend hang.
//!
//! Fix: size the weld's spatial-hash cells at the tolerance (`× 2`), keeping
//! buckets sparse (only bit-exact seam duplicates collide) → genuinely O(n).
//! The `tol_sq` membership test is unchanged, so the welded mesh is identical.
//!
//! These tests would HANG (time out) before the fix and complete in seconds
//! after it. The 3-edge-corner case (one sphere cap) is the minimal trigger;
//! it exercises exactly the corner-cap `Sphere` + dense-weld path.

use geometry_engine::operations::fillet::{FilletType, PropagationMode};
use geometry_engine::operations::{fillet_edges, FilletOptions};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

const BOX_W: f64 = 18.0;
const BOX_D: f64 = 25.0;
const BOX_H: f64 = 34.0;
const BOX_VOLUME: f64 = BOX_W * BOX_D * BOX_H; // 15_300

fn make_box(model: &mut BRepModel) -> SolidId {
    match TopologyBuilder::new(model)
        .create_box_3d(BOX_W, BOX_D, BOX_H)
        .expect("box creation")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn all_edges(model: &BRepModel) -> Vec<EdgeId> {
    model
        .edges
        .iter()
        .filter(|(_, e)| !e.is_loop())
        .map(|(id, _)| id)
        .collect()
}

/// The 3 edges incident to the (+w/2,+h/2,+d/2) corner — one convex 3-edge
/// corner, which synthesises exactly one `Sphere` corner-cap face.
fn corner_edges(model: &BRepModel) -> Vec<EdgeId> {
    let (cx, cy, cz) = (BOX_W / 2.0, BOX_D / 2.0, BOX_H / 2.0);
    let eps = 1e-6;
    let at =
        |p: [f64; 3]| (p[0] - cx).abs() < eps && (p[1] - cy).abs() < eps && (p[2] - cz).abs() < eps;
    model
        .edges
        .iter()
        .filter(|(_, e)| !e.is_loop())
        .filter(|(_, e)| {
            let v0 = model.vertices.get(e.start_vertex);
            let v1 = model.vertices.get(e.end_vertex);
            matches!((v0, v1), (Some(a), Some(b)) if at(a.position) || at(b.position))
        })
        .map(|(id, _)| id)
        .collect()
}

fn fillet_opts(r: f64) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(r),
        radius: r,
        propagation: PropagationMode::None,
        ..Default::default()
    }
}

/// The minimal wedge repro: a single 3-edge corner fillet (one `Sphere` cap).
/// `calculate_solid_volume` fine-tessellates + welds; before the weld fix this
/// hung indefinitely. It must now return a finite, physically-sane volume.
#[test]
fn corner_fillet_volume_terminates_and_is_sane() {
    let mut model = BRepModel::new();
    let solid = make_box(&mut model);
    let edges = corner_edges(&model);
    assert_eq!(edges.len(), 3, "a box corner has exactly 3 incident edges");
    fillet_edges(&mut model, solid, edges, fillet_opts(1.5)).expect("3-corner fillet");

    let vol = model
        .calculate_solid_volume(solid)
        .expect("volume must be computed (must not hang or fail)");

    assert!(
        vol.is_finite() && vol > 0.0,
        "volume must be finite and positive; got {vol}"
    );
    // A fillet only removes material at the corner, so the volume drops just
    // below the box and by well under 2%.
    assert!(
        vol < BOX_VOLUME && vol > 0.98 * BOX_VOLUME,
        "corner-filleted volume must be a hair under the box ({BOX_VOLUME}); got {vol}"
    );
}

/// The exact live trigger — all 12 edges (8 simultaneous `Sphere` corner caps).
/// Heavier (the caps over-tessellate at `fine()`), but it must TERMINATE with a
/// sound volume rather than wedge the backend.
#[test]
fn all_edges_fillet_volume_terminates() {
    let mut model = BRepModel::new();
    let solid = make_box(&mut model);
    let edges = all_edges(&model);
    assert_eq!(edges.len(), 12, "a box has 12 edges");
    fillet_edges(&mut model, solid, edges, fillet_opts(1.5)).expect("all-12 fillet");

    let vol = model
        .calculate_solid_volume(solid)
        .expect("all-12 volume must terminate");
    assert!(
        vol.is_finite() && vol > 0.95 * BOX_VOLUME && vol < BOX_VOLUME,
        "all-12 filleted volume must be finite and just under the box; got {vol}"
    );
}

/// Direct on the tessellator: `fine()` tessellation of a corner-cap solid must
/// terminate with a bounded, non-empty mesh (the weld no longer degrades to
/// quasi-quadratic on the dense corner-cap vertex set).
#[test]
fn fine_tessellation_of_corner_fillet_terminates() {
    let mut model = BRepModel::new();
    let solid = make_box(&mut model);
    let edges = corner_edges(&model);
    fillet_edges(&mut model, solid, edges, fillet_opts(1.5)).expect("3-corner fillet");

    let s = model.solids.get(solid).expect("solid stored");
    let mesh = tessellate_solid(s, &model, &TessellationParams::fine());
    assert!(
        !mesh.triangles.is_empty(),
        "fine tessellation must produce triangles"
    );
    // The fan budget caps a single sphere cap at ~150k triangles; the whole
    // solid stays comfortably under a few hundred k. Purely a termination /
    // no-runaway guard.
    assert!(
        mesh.triangles.len() < 400_000,
        "fine tessellation triangle count must stay bounded; got {}",
        mesh.triangles.len()
    );
}
