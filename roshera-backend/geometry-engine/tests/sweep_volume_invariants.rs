// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Oracle-free volume + watertightness invariants for the sweep operation.
//!
//! Sweeping a closed planar profile of area `A` along a straight path of
//! length `L` perpendicular to the profile produces a prism of volume `A·L`
//! and surface area `2A + perimeter·L`. We sweep an axis-aligned `w×h`
//! rectangle (XY plane) along the +Z axis by `L`, so the result is a
//! `w×h×L` box: volume `w·h·L`.
//!
//! These are exactly the "swept-construction" paths that hid non-watertight /
//! non-manifold bugs in revolve and loft. The result is validated (so a
//! non-manifold sweep fails at construction), and its tessellated divergence
//! volume is asserted against both the analytic prism volume and the kernel's
//! reported mass-properties volume (the watertightness witness). A sweep that
//! returns a typed `Err` on this trivial known-good input is a regression, not
//! a skip — so the helper `.expect()`s success.

use geometry_engine::math::Point3;
use geometry_engine::operations::{sweep_profile, CommonOptions, SweepOptions};
use geometry_engine::primitives::curve::Line;
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::tessellation::{tessellate_solid, TessellationParams, TriangleMesh};

fn rel_close(a: f64, b: f64, tol: f64) -> bool {
    if b.abs() < 1e-9 {
        a.abs() <= tol
    } else {
        ((a - b) / b).abs() <= tol
    }
}

/// Divergence-theorem volume of a (watertight) tessellated solid:
/// `Σ (a · (b × c)) / 6` over triangles.
fn mesh_volume(mesh: &TriangleMesh) -> f64 {
    let mut v = 0.0;
    for t in &mesh.triangles {
        let a = mesh.vertices[t[0] as usize].position;
        let b = mesh.vertices[t[1] as usize].position;
        let c = mesh.vertices[t[2] as usize].position;
        v += (a.x * (b.y * c.z - b.z * c.y) - a.y * (b.x * c.z - b.z * c.x)
            + a.z * (b.x * c.y - b.y * c.x))
            / 6.0;
    }
    v.abs()
}

fn add_line_edge(model: &mut BRepModel, a: u32, b: u32) -> EdgeId {
    let s = model.vertices.get(a).expect("start vertex").position;
    let e = model.vertices.get(b).expect("end vertex").position;
    let curve_id = model
        .curves
        .add(Box::new(Line::new(Point3::from(s), Point3::from(e))));
    model.edges.add(Edge::new_auto_range(
        0,
        a,
        b,
        curve_id,
        EdgeOrientation::Forward,
    ))
}

/// Closed CCW `w×h` rectangle in the XY plane (z = 0).
fn rectangle_xy(model: &mut BRepModel, w: f64, h: f64) -> Vec<EdgeId> {
    let v0 = model.vertices.add(0.0, 0.0, 0.0);
    let v1 = model.vertices.add(w, 0.0, 0.0);
    let v2 = model.vertices.add(w, h, 0.0);
    let v3 = model.vertices.add(0.0, h, 0.0);
    vec![
        add_line_edge(model, v0, v1),
        add_line_edge(model, v1, v2),
        add_line_edge(model, v2, v3),
        add_line_edge(model, v3, v0),
    ]
}

/// Sweep a `w×h` rectangle along +Z by `length`. Returns the result solid's
/// (tessellated divergence volume, reported mass-properties volume). The
/// result is validated at construction; a typed `Err` on this trivial input
/// is treated as a regression, not a skip.
fn swept_prism(w: f64, h: f64, length: f64) -> (f64, f64) {
    let mut model = BRepModel::new();
    let profile = rectangle_xy(&mut model, w, h);
    let va = model.vertices.add(0.0, 0.0, 0.0);
    let vb = model.vertices.add(0.0, 0.0, length);
    let path = add_line_edge(&mut model, va, vb);
    let opts = SweepOptions {
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let solid_id: SolidId = sweep_profile(&mut model, profile, path, opts)
        .expect("sweep of a known-good rectangle along a straight path must succeed");
    let mp = model
        .mass_properties_for(solid_id)
        .expect("swept solid mass properties");
    let solid = model.solids.get(solid_id).expect("swept solid");
    let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
    (mesh_volume(&mesh), mp.volume)
}

macro_rules! sweep_prism_volume_test {
    ($name:ident, $w:expr, $h:expr, $l:expr) => {
        #[test]
        fn $name() {
            let (tess_vol, mass_vol) = swept_prism($w, $h, $l);
            let expected = ($w as f64) * ($h as f64) * ($l as f64);
            // Analytic prism oracle: tessellated divergence volume = w·h·L.
            assert!(
                rel_close(tess_vol, expected, 0.03),
                "sweep {}x{} along {}: tess volume {} vs prism {}",
                $w,
                $h,
                $l,
                tess_vol,
                expected
            );
            // Watertightness witness: divergence volume agrees with the
            // kernel's reported mass-properties volume.
            assert!(
                rel_close(tess_vol, mass_vol, 0.03),
                "sweep {}x{} along {}: tess {} vs mass-props {} (non-watertight?)",
                $w,
                $h,
                $l,
                tess_vol,
                mass_vol
            );
        }
    };
}

sweep_prism_volume_test!(sweep_unit_along_5, 1.0, 1.0, 5.0);
sweep_prism_volume_test!(sweep_2_3_along_4, 2.0, 3.0, 4.0);
sweep_prism_volume_test!(sweep_thin_along_10, 5.0, 0.5, 10.0);
sweep_prism_volume_test!(sweep_wide_along_2, 8.0, 6.0, 2.0);
sweep_prism_volume_test!(sweep_cube, 3.0, 3.0, 3.0);

#[test]
fn sweep_volume_scales_linearly_with_path_length() {
    let (v1, _) = swept_prism(2.0, 2.0, 3.0);
    let (v2, _) = swept_prism(2.0, 2.0, 6.0);
    assert!(
        rel_close(v2, 2.0 * v1, 0.03),
        "doubling path length must double swept volume: {v1} -> {v2}"
    );
}

use proptest::prelude::*;

proptest! {
    // 16 cases keeps wall-clock well under the nextest slow-timeout backstop:
    // each sweep case runs full result validation + a fine tessellation
    // (~2.5 s), so the case count is deliberately modest (the fixed-size cases
    // above already pin specific shapes; this fuzzes the space around them).
    #![proptest_config(ProptestConfig::with_cases(16))]

    /// Prism oracle over a wide, randomized range of profile sizes and path
    /// lengths — the comprehensive complement to the fixed-size cases above.
    /// Every case is validated at construction (via `swept_prism`), checked
    /// against the analytic prism volume, and checked for watertightness.
    #[test]
    fn prop_sweep_prism_volume(
        w in 0.3f64..8.0,
        h in 0.3f64..8.0,
        l in 0.5f64..12.0,
    ) {
        let (tess, mass) = swept_prism(w, h, l);
        let expected = w * h * l;
        prop_assert!(rel_close(tess, expected, 0.03), "tess {tess} vs prism {expected}");
        prop_assert!(rel_close(tess, mass, 0.03), "tess {tess} vs mass-props {mass}");
    }
}
