// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Task 43: the watertight weld's spatial-hash grid must ADDRESS every vertex
//! it is handed, or say so.
//!
//! `weld_mesh_watertight_range` buckets vertices by `(p * inv_grid).floor()`
//! and scans the 3x3x3 neighbourhood around each cell. The cast was
//! `as i32`, which SATURATES: any vertex whose scaled coordinate exceeds
//! `i32::MAX` landed on `i32::MAX`, and the neighbour scan's `cell + 1` then
//! overflowed - a panic under debug overflow checks (in a workspace that
//! denies `panic` in production code) and a wrapped, wrong bucket in release.
//!
//! Two reachable drivers, both exercised here:
//!   1. a LARGE part - the grid is sized off the weld tolerance, not the part,
//!      so cell indices grow with the coordinate;
//!   2. a caller-supplied chord tolerance of zero - `tessellate_solid`'s
//!      relative chord floor only applies to a POSITIVE chord, so the weld
//!      falls back to its 1e-9 tolerance floor and the grid gets 4e5 times
//!      finer, saturating on a coordinate of a few millimetres.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams, TriangleMesh};
use std::collections::HashSet;

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

/// Every triangle index must address a real vertex, and the mesh must exist.
fn assert_addressable(mesh: &TriangleMesh, label: &str) {
    assert!(
        !mesh.triangles.is_empty(),
        "{label}: the weld must not empty the mesh"
    );
    let n = mesh.vertices.len();
    for t in &mesh.triangles {
        for &i in t {
            assert!(
                (i as usize) < n,
                "{label}: triangle index {i} out of {n} vertices"
            );
        }
    }
}

/// A 40 m box at `fine()`. The weld tolerance is pinned to `MESH_WELD_CAP`
/// (4e-6) whatever the chord is, so the grid is 8e-6 across and the corner
/// coordinate 20000 scales to 2.5e9 cells - past `i32::MAX` (2.147e9).
#[test]
fn large_part_tessellation_addresses_the_weld_grid() {
    let mut m = BRepModel::new();
    let bx = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(40000.0, 40000.0, 40000.0)
        .expect("box"));
    let solid = m.solids.get(bx).expect("solid").clone();
    let mesh = tessellate_solid(&solid, &m, &TessellationParams::fine());
    assert_addressable(&mesh, "40 m box");
}

/// A 10 mm box at a chord tolerance of zero. `weld_distance = min(0, 4e-6)`
/// floors to 1e-9, so the grid is 2e-9 across and the corner coordinate 5
/// scales to 2.5e9 cells - again past `i32::MAX`.
#[test]
fn zero_chord_tolerance_addresses_the_weld_grid() {
    let mut m = BRepModel::new();
    let bx = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(10.0, 10.0, 10.0)
        .expect("box"));
    let solid = m.solids.get(bx).expect("solid").clone();
    let params = TessellationParams {
        chord_tolerance: 0.0,
        ..TessellationParams::fine()
    };
    let mesh = tessellate_solid(&solid, &m, &params);
    assert_addressable(&mesh, "zero-chord box");
}

/// Tessellate a cylinder of the given radius and equal height, and report
/// `(vertices emitted, vertices still referenced by a triangle, triangles)`.
/// Welding shows up as the gap between the first two: a merged vertex is
/// orphaned, since the weld remaps indices rather than compacting the array.
fn cylinder_weld_shape(radius: f64) -> (usize, usize, usize) {
    let mut m = BRepModel::new();
    let cy = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, radius, radius)
        .expect("cylinder"));
    let solid = m.solids.get(cy).expect("solid").clone();
    let mesh = tessellate_solid(&solid, &m, &TessellationParams::fine());
    assert_addressable(&mesh, "cylinder");
    let used: HashSet<u32> = mesh.triangles.iter().flatten().copied().collect();
    (mesh.vertices.len(), used.len(), mesh.triangles.len())
}

/// The welding itself must not change across the boundary the old cast
/// saturated at. A 20 m cylinder's lateral coordinates reach 2.5e9 cells,
/// past `i32::MAX`; a 20 mm one stays at 2.5e6 cells. Both meshes must come
/// out identical in shape, and both must actually weld their smooth lateral
/// seam (fewer vertices referenced than emitted).
///
/// The two are the same tessellation at two scales because `fine()`'s
/// density knobs are scale-free HERE: the sagitta and edge-length segment
/// counts both saturate `max_segments` (200) at either radius
/// (`tessellation/curve.rs::calculate_arc_segments` clamps last), and
/// `tessellate_solid`'s chord floor is a fraction of the part's own extent.
/// If a later change lifts that ceiling or makes density absolute, this
/// equality breaks for a reason that has nothing to do with the weld - read
/// the counts before assuming the addressing regressed.
#[test]
fn welding_is_unchanged_where_the_old_cast_saturated() {
    let large = cylinder_weld_shape(20000.0);
    let small = cylinder_weld_shape(20.0);
    assert!(
        large.1 < large.0,
        "the 20 m cylinder's smooth seam must still weld: {large:?}"
    );
    assert_eq!(
        large, small,
        concat!(
            "a part past the old i32 cell limit must weld exactly as the same ",
            "part inside it"
        )
    );
}
