// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Where material changed between two states (`queries::change`).
//!
//! Varun 2026-11-24: "why isnt agent using sdf to check". The kernel's exact
//! signed distance could tell an agent what an edit actually did -- "the cut
//! removed a cylinder here", "the boss landed there" -- and nothing asked it.
//! Each case builds two real states with kernel operations and checks the
//! regions found against the exact geometry.

use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::transform::{transform_solid, TransformOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::queries::change::{material_change, ChangeKind};

/// Grid resolution: coarse enough for debug-build test time, fine enough to
/// place a 4 mm hole in a 20 mm block.
const N: usize = 16;

fn solid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        other => panic!("built {other:?}"),
    }
}

/// A `w`-cube centred at `at`.
fn cube(m: &mut BRepModel, w: f64, at: [f64; 3]) -> SolidId {
    let s = solid(TopologyBuilder::new(m).create_box_3d(w, w, w).expect("box"));
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for (_, v) in m.vertices.iter() {
        for i in 0..3 {
            lo[i] = lo[i].min(v.position[i]);
            hi[i] = hi[i].max(v.position[i]);
        }
    }
    let c = [0, 1, 2].map(|i| (lo[i] + hi[i]) / 2.0);
    transform_solid(
        m,
        s,
        Matrix4::from_translation(&Vector3::new(at[0] - c[0], at[1] - c[1], at[2] - c[2])),
        TransformOptions::default(),
    )
    .expect("place");
    s
}

/// A 20 mm block at the origin.
fn block() -> BRepModel {
    let mut m = BRepModel::new();
    cube(&mut m, 20.0, [0.0, 0.0, 0.0]);
    m
}

/// The block with a vertical through-hole of radius `r` at (x, 0).
fn drilled(x: f64, r: f64) -> BRepModel {
    let mut m = BRepModel::new();
    let b = cube(&mut m, 20.0, [0.0, 0.0, 0.0]);
    let c = solid(
        TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(x, 0.0, -12.0), Vector3::Z, r, 24.0)
            .expect("cylinder"),
    );
    boolean_operation(
        &mut m,
        b,
        c,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("drill");
    m
}

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn a_drilled_hole_is_one_removed_cylinder_region_where_it_was_drilled() {
    let change = material_change(&block(), &drilled(0.0, 4.0), N);
    assert_eq!(change.regions.len(), 1, "one change: {:?}", change.regions);
    let r = &change.regions[0];
    assert_eq!(r.kind, ChangeKind::Removed);
    let h = change.cell;
    assert!(
        near(r.center.x, 0.0, h) && near(r.center.y, 0.0, h) && near(r.center.z, 0.0, h),
        "at the hole: {:?} (cell {h})",
        r.center
    );
    let exact = std::f64::consts::PI * 16.0 * 20.0; // r = 4 through 20 mm
    assert!(
        (r.volume_estimate - exact).abs() / exact < 0.35,
        "volume {} vs exact {exact} at cell {h}",
        r.volume_estimate
    );
    let kind = r.surface.unwrap_or("").to_lowercase();
    assert!(
        kind.contains("cylind"),
        "named by its surface: {:?}",
        r.surface
    );
}

#[test]
fn a_boss_on_top_is_one_added_region_on_top() {
    let mut after = BRepModel::new();
    let b = cube(&mut after, 20.0, [0.0, 0.0, 0.0]);
    let boss = cube(&mut after, 6.0, [0.0, 0.0, 13.0]);
    boolean_operation(
        &mut after,
        b,
        boss,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("union");
    let change = material_change(&block(), &after, N);
    assert_eq!(change.regions.len(), 1, "one change: {:?}", change.regions);
    let r = &change.regions[0];
    assert_eq!(r.kind, ChangeKind::Added);
    let h = change.cell;
    assert!(
        near(r.center.x, 0.0, h) && near(r.center.y, 0.0, h) && near(r.center.z, 13.0, h),
        "on top: {:?} (cell {h})",
        r.center
    );
}

#[test]
fn a_moved_hole_is_material_added_where_it_was_and_removed_where_it_went() {
    let change = material_change(&drilled(-5.0, 3.0), &drilled(5.0, 3.0), N);
    let added: Vec<_> = change
        .regions
        .iter()
        .filter(|r| r.kind == ChangeKind::Added)
        .collect();
    let removed: Vec<_> = change
        .regions
        .iter()
        .filter(|r| r.kind == ChangeKind::Removed)
        .collect();
    assert_eq!((added.len(), removed.len()), (1, 1), "{:?}", change.regions);
    let h = change.cell;
    assert!(
        near(added[0].center.x, -5.0, h),
        "refilled at x=-5: {:?}",
        added[0].center
    );
    assert!(
        near(removed[0].center.x, 5.0, h),
        "cut at x=+5: {:?}",
        removed[0].center
    );
}

#[test]
fn the_same_state_shows_no_change() {
    let change = material_change(&drilled(0.0, 4.0), &drilled(0.0, 4.0), N);
    assert!(change.regions.is_empty(), "{:?}", change.regions);
    assert!(change.cell > 0.0);
}

/// A change thinner than a cell falls between the samples -- the reason the
/// route pairs this query with the states' EXACT volumes and never reports
/// "unchanged" from the grid alone. Pinned so that limit stays visible.
#[test]
fn a_change_thinner_than_a_cell_is_not_seen_by_the_grid_alone() {
    let change = material_change(&block(), &drilled(0.0, 0.2), N);
    assert!(change.cell > 0.4, "the hole is thinner than a cell");
    assert!(change.regions.is_empty(), "{:?}", change.regions);
}

#[test]
fn regions_come_largest_first_and_identically_every_time() {
    let a = material_change(&drilled(-5.0, 3.0), &drilled(5.0, 2.0), N);
    let b = material_change(&drilled(-5.0, 3.0), &drilled(5.0, 2.0), N);
    let key = |c: &geometry_engine::queries::change::MaterialChange| {
        c.regions
            .iter()
            .map(|r| (r.kind == ChangeKind::Added, r.cells))
            .collect::<Vec<_>>()
    };
    assert_eq!(key(&a), key(&b), "deterministic");
    assert!(
        a.regions.windows(2).all(|w| w[0].cells >= w[1].cells),
        "largest first"
    );
}
