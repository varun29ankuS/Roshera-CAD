//! Regression guard for #41 — "non-convex sketch hole with sharp re-entrant
//! corners is mis-triangulated as an interior loop".
//!
//! Reported symptom (AGENT-EVAL-α, 2026-07-15): a sparse 4-corner keyway notch,
//! correct as a *standalone* solid, produced a wildly wrong region/face area
//! when used as a HOLE — the reported area collapsed to the OUTER shape's area
//! (π·10² in a gear, the square's area in a square), as if the hole were never
//! subtracted; densifying the notch walls made it correct.
//!
//! Root cause (verified here): the defect belonged to the previous *bridged
//! ear-clipping* planar triangulator, which broke on N≥4 sharp/collinear hole
//! vertices (see the comment above `triangulate_planar_polygon` in
//! `tessellation/surface.rs`). That path was replaced by the set-based
//! `cdt`-crate constrained Delaunay triangulator, which is immune to the
//! collinear-bridge failure mode. Against the current kernel the extruded
//! volume — computed from the tessellated caps AND from the analytic
//! `audit_volume` — matches the exact polygonal cross-section for every
//! sparse/dense, circle/square, straddling/interior configuration below. These
//! assertions lock that in so the ear-clipping regression cannot return.
//!
//! Cross-section is polygonal on BOTH loops, so the extruded prism volume is
//! EXACTLY `(area(outer) - area(hole)) * height` — no faceting slack needed;
//! the tolerance only absorbs f64 rounding.

use geometry_engine::harness::watertight::{analytic_volume, mesh_volume};
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::extrude::{extrude_polygon_regions, PolygonRegion};
use geometry_engine::primitives::topology_builder::BRepModel;

fn shoelace(poly: &[[f64; 2]]) -> f64 {
    let n = poly.len();
    let mut a = 0.0;
    for i in 0..n {
        let p = poly[i];
        let q = poly[(i + 1) % n];
        a += p[0] * q[1] - q[0] * p[1];
    }
    (a * 0.5).abs()
}

fn circle(r: f64, n: usize) -> Vec<[f64; 2]> {
    (0..n)
        .map(|i| {
            let t = std::f64::consts::TAU * (i as f64) / (n as f64);
            [r * t.cos(), r * t.sin()]
        })
        .collect()
}

fn densify(poly: &[[f64; 2]], k: usize) -> Vec<[f64; 2]> {
    let n = poly.len();
    let mut out = Vec::new();
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        for j in 0..=k {
            let t = j as f64 / (k as f64 + 1.0);
            out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
        }
    }
    out
}

/// The exact eval hole: a standalone 4-corner keyway rectangle whose base edge
/// (y = 4.77) sits on the r=5 circle (x=±1.5 ⟹ y=√(25−2.25)=4.77).
fn keyway_rect() -> Vec<[f64; 2]> {
    vec![[1.5, 4.77], [1.5, 6.4], [-1.5, 6.4], [-1.5, 4.77]]
}

/// A keyed bore: circle radius `r` with a rectangular keyway tab poking OUTWARD
/// at the top (x ∈ [−hw, hw]); `arc_n` controls circle sampling density. This is
/// the "dense keyed bore" the eval runner used as its working representation.
fn keyed_bore(r: f64, hw: f64, tab_top: f64, arc_n: usize) -> Vec<[f64; 2]> {
    let y_on = (r * r - hw * hw).sqrt();
    let ang_r = y_on.atan2(hw);
    let ang_l = y_on.atan2(-hw);
    let mut poly = Vec::new();
    for i in 0..arc_n {
        let t = std::f64::consts::TAU * (i as f64) / (arc_n as f64);
        if t > ang_r && t < ang_l {
            continue;
        }
        poly.push([r * t.cos(), r * t.sin()]);
        let t_next = std::f64::consts::TAU * ((i + 1) as f64) / (arc_n as f64);
        if t <= ang_r && t_next > ang_r {
            poly.push([hw, y_on]);
            poly.push([hw, tab_top]);
            poly.push([-hw, tab_top]);
            poly.push([-hw, y_on]);
        }
    }
    poly
}

/// Extrude `outer` with a single `hole` by `h`, and assert BOTH the tessellated
/// mesh volume and the analytic audit volume equal the exact polygonal
/// cross-section prism volume.
fn assert_hole_subtracted(outer: Vec<[f64; 2]>, hole: Vec<[f64; 2]>, h: f64, label: &str) {
    let expect = (shoelace(&outer) - shoelace(&hole)) * h;
    // A dropped hole would read `shoelace(outer) * h` — guard the two apart.
    let outer_only = shoelace(&outer) * h;
    assert!(
        (expect - outer_only).abs() > 1.0,
        "{label}: test is not discriminating (hole area negligible)"
    );

    let mut m = BRepModel::new();
    let region = PolygonRegion {
        outer,
        holes: vec![hole],
    };
    let solid = extrude_polygon_regions(
        &mut m,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::X,
        Vector3::Y,
        &[region],
        h,
        None,
        Tolerance::default(),
    )
    .unwrap_or_else(|e| panic!("{label}: extrude failed: {e:?}"));

    let mesh =
        mesh_volume(&m, solid, 0.05).unwrap_or_else(|| panic!("{label}: mesh volume uncomputable"));
    let analytic = analytic_volume(&mut m, solid)
        .unwrap_or_else(|| panic!("{label}: analytic volume uncomputable"));

    let tol = 1e-6 * expect.abs().max(1.0);
    assert!(
        (mesh - expect).abs() <= tol,
        "{label}: MESH volume {mesh} != expected {expect} (hole not subtracted \
         from the cap triangulation → {outer_only} would be the dropped-hole value)"
    );
    assert!(
        (analytic - expect).abs() <= tol,
        "{label}: ANALYTIC volume {analytic} != expected {expect}"
    );
}

#[test]
fn sparse_keyway_hole_is_subtracted_in_a_round_disc() {
    // Bare 4-corner keyway notch as a hole in a round gear disc.
    assert_hole_subtracted(
        circle(10.0, 96),
        keyway_rect(),
        5.0,
        "disc + sparse 4-pt keyway",
    );
    // Densifying the notch must not change the answer (it did in the old path).
    assert_hole_subtracted(
        circle(10.0, 96),
        densify(&keyway_rect(), 3),
        5.0,
        "disc + densified keyway",
    );
}

#[test]
fn sparse_keyway_hole_is_subtracted_in_a_square() {
    // The eval saw the same drop with a square outer (a different wrong value).
    let square = vec![[-10.0, -10.0], [10.0, -10.0], [10.0, 10.0], [-10.0, 10.0]];
    assert_hole_subtracted(
        square.clone(),
        keyway_rect(),
        5.0,
        "square + straddling keyway",
    );
    let interior = vec![[1.5, 2.0], [1.5, 4.0], [-1.5, 4.0], [-1.5, 2.0]];
    assert_hole_subtracted(square, interior, 5.0, "square + interior keyway");
}

#[test]
fn keyed_bore_hole_subtracts_at_any_arc_density() {
    // The re-entrant keyed-bore (circle + outward keyway tab) as a single hole,
    // sampled sparse and dense — both must subtract exactly.
    for arc_n in [8usize, 12, 48] {
        assert_hole_subtracted(
            circle(10.0, 96),
            keyed_bore(5.0, 1.5, 6.4, arc_n),
            5.0,
            &format!("disc + keyed bore arc_n={arc_n}"),
        );
    }
}
