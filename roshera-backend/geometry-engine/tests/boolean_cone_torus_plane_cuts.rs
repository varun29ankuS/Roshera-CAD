// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Face shapes found importing the robot-arm assembly (2026-11-05), made by
//! the KERNEL's own booleans rather than brought in by a STEP file.
//!
//! TWO-CIRCLE TORUS BAND — a torus cut by a plane perpendicular to its axis.
//! Before 2026-11-05 the boolean returned Ok with an UNSOUND result (no
//! planar cap, a tangled torus split): plane∘torus was routed to the general
//! plane tracer (a torus classifies as `Other`, so the torus arm was never
//! reached), the marched curve defeated every torus splitter, and the slab's
//! face never nested the two concentric cut circles into an annulus. Now: exact
//! latitude circles, a latitude-band splitter, planar island nesting. The
//! volume checks pin WHICH tube arc each band covers (a wrong arc is still
//! watertight). A cut exactly through the equator seam is refused (typed).
//!
//! CONE APEX CAP — a cone cut by planes parallel to its axis (a drill point
//! inside a square socket). Before 2026-11-05: Ok with 5 planes and NO cone
//! face (unsound). The planar arrangement never walks the region round the
//! apex (not a closed polygon in the cone's chart) — now recovered from the
//! cut edges only one walked region uses — and the hyperbola sections were
//! cubic fits 5.3e-3 off the cone — now exact rational quadratic NURBS.
//!
//! Each solid must certify sound where it is built AND after rigid placements:
//! a move is an isometry and cannot honestly change soundness.

use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::transform::{transform_solid, TransformOptions};
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(id) => id,
        o => panic!("expected a solid, got {o:?}"),
    }
}

/// Rotation by `angle` about the unit axis, then translation.
fn placement(axis: [f64; 3], angle: f64, t: [f64; 3]) -> Matrix4 {
    let [x, y, z] = axis;
    let (s, c) = angle.sin_cos();
    let k = 1.0 - c;
    Matrix4::from_cols(
        Vector3::new(c + x * x * k, y * x * k + z * s, z * x * k - y * s),
        Vector3::new(x * y * k - z * s, c + y * y * k, z * y * k + x * s),
        Vector3::new(x * z * k + y * s, y * z * k - x * s, c + z * z * k),
        Vector3::new(t[0], t[1], t[2]),
    )
}

fn assert_sound(model: &mut BRepModel, s: SolidId, what: &str) {
    let cert = model.certify_solid(s);
    assert!(
        cert.is_sound(),
        "{what}: must certify sound (brep_valid={} watertight={} oriented={} \
         self_x_free={}): {:?}",
        cert.brep_valid,
        cert.watertight,
        cert.oriented,
        cert.self_intersection_free,
        cert.errors
    );
}

/// Sound as built, then sound after each of three placements (applied in turn).
fn assert_sound_built_and_placed(model: &mut BRepModel, s: SolidId, what: &str) {
    assert_sound(model, s, &format!("{what} as built"));
    let moves = [
        (
            "rotate-x 90",
            placement([1.0, 0.0, 0.0], std::f64::consts::FRAC_PI_2, [0.0; 3]),
        ),
        (
            "oblique 1.1 rad + move",
            placement([0.3, 0.4, 0.866_025_403_784_438_6], 1.1, [40.0, -15.0, 7.0]),
        ),
        (
            "rotate-z 0.37 rad",
            placement([0.0, 0.0, 1.0], 0.37, [0.0; 3]),
        ),
    ];
    for (name, m) in moves {
        transform_solid(model, s, m, TransformOptions::default()).expect("transform");
        assert_sound(model, s, &format!("{what} after {name}"));
    }
}

/// A face of `s` on a surface of `kind` whose loops satisfy `shape`
/// (outer-loop edge count, inner-loop count) — proves the case is not vacuous.
fn has_face(
    model: &BRepModel,
    s: SolidId,
    kind: &str,
    shape: impl Fn(usize, usize) -> bool,
) -> bool {
    let Some(solid) = model.solids.get(s) else {
        return false;
    };
    let Some(shell) = model.shells.get(solid.outer_shell) else {
        return false;
    };
    shell.faces.iter().any(|&fid| {
        let Some(f) = model.faces.get(fid) else {
            return false;
        };
        let ty = model.surfaces.get(f.surface_id).map(|s| s.type_name());
        let outer = model
            .loops
            .get(f.outer_loop)
            .map(|l| l.edges.len())
            .unwrap_or(0);
        ty == Some(kind) && shape(outer, f.inner_loops.len())
    })
}

/// Every face of `s`: surface type, outer-loop edge count, inner-loop count.
fn inventory(model: &BRepModel, s: SolidId) -> Vec<(String, usize, usize)> {
    let Some(solid) = model.solids.get(s) else {
        return Vec::new();
    };
    let Some(shell) = model.shells.get(solid.outer_shell) else {
        return Vec::new();
    };
    shell
        .faces
        .iter()
        .filter_map(|&fid| {
            let f = model.faces.get(fid)?;
            let ty = model.surfaces.get(f.surface_id)?.type_name().to_string();
            let outer = model.loops.get(f.outer_loop).map(|l| l.edges.len())?;
            Some((ty, outer, f.inner_loops.len()))
        })
        .collect()
}

/// cone(base r8 at z=0, apex z=10) ∩ square prism 6×6: the cone survives only
/// as the cap above the four curves where the prism walls cut it — the apex
/// inside, one 4-edge loop round the axis (the robot-arm drill point, square).
/// Before 2026-11-05: 5 planes, NO cone face, brep_valid=false — the planar
/// arrangement never walks the region round the apex, and the traced
/// hyperbolas were cubic fits 5.3e-3 off the cone.
#[test]
fn cone_cut_by_a_square_prism_keeps_a_sound_apex_cap() {
    let mut m = BRepModel::new();
    let cone = sid(TopologyBuilder::new(&mut m)
        .create_cone_3d(Point3::ORIGIN, Vector3::Z, 8.0, 0.0, 10.0)
        .expect("cone"));
    let prism = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(6.0, 6.0, 30.0)
        .expect("prism"));
    let res = boolean_operation(
        &mut m,
        cone,
        prism,
        BooleanOp::Intersection,
        BooleanOptions::default(),
    )
    .expect("cone ∩ square prism");
    assert!(
        has_face(&m, res, "Cone", |outer, inner| outer >= 2 && inner == 0),
        "the result must carry the cone CAP (multi-edge outer loop, no holes); faces: {:?}; cert: {:?}",
        inventory(&m, res),
        m.certify_solid(res)
    );
    // Exact: height 10(1 − r/8) over the square [−3, 3]², so
    // V = 360 − (10/8)·∬r dA with ∬r dA = 4·3³·(√2 + ln(1 + √2))/3.
    let sqrt2 = std::f64::consts::SQRT_2;
    let int_r = 4.0 * 27.0 * (sqrt2 + (1.0 + sqrt2).ln()) / 3.0;
    assert_volume(&mut m, res, 360.0 - 1.25 * int_r, "cone ∩ square prism");
    assert_sound_built_and_placed(&mut m, res, "cone ∩ square prism");
}

/// cone minus the same square prism: the complement — the eight regions
/// outside the walls kept, the apex cap removed, a square hole through the
/// base. Volume = cone − (cone ∩ prism).
/// RED 2026-11-05 (mesh quality only): brep_valid / watertight / oriented /
/// self-x-free all TRUE and the volume exact, but mesh_quality_clean=false —
/// curved-CDT slivers (min angle ~0.5°) in the side strips just above the base
/// rim: a row of interior Steiner points meets one side-hyperbola sample.
/// Passed with the old cubic-FIT hyperbola (luck of sample positions); exact
/// conics (single-span, multi-span, arc-length spans) all leave the sliver.
#[test]
fn cone_minus_a_square_prism_is_sound() {
    let mut m = BRepModel::new();
    let cone = sid(TopologyBuilder::new(&mut m)
        .create_cone_3d(Point3::ORIGIN, Vector3::Z, 8.0, 0.0, 10.0)
        .expect("cone"));
    let prism = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(6.0, 6.0, 30.0)
        .expect("prism"));
    let res = boolean_operation(
        &mut m,
        cone,
        prism,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("cone minus square prism");
    let sqrt2 = std::f64::consts::SQRT_2;
    let int_r = 4.0 * 27.0 * (sqrt2 + (1.0 + sqrt2).ln()) / 3.0;
    let cone_v = std::f64::consts::PI * 64.0 * 10.0 / 3.0;
    assert_volume(
        &mut m,
        res,
        cone_v - (360.0 - 1.25 * int_r),
        "cone minus square prism",
    );
    assert_sound_built_and_placed(&mut m, res, "cone minus square prism");
}

/// torus(R10, r3, axis Z) minus everything above z = 1: the torus face left
/// is bounded by two major circles at z = 1 and runs the LONG way round the
/// tube (≈219°) — the arc the parameter-bounds grid could get backwards.
#[test]
fn torus_with_its_top_sliced_off_keeps_a_sound_two_circle_band() {
    let mut m = BRepModel::new();
    let torus = sid(TopologyBuilder::new(&mut m)
        .create_torus_3d(Point3::ORIGIN, Vector3::Z, 10.0, 3.0)
        .expect("torus"));
    let slab = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(40.0, 40.0, 10.0)
        .expect("slab"));
    transform_solid(
        &mut m,
        slab,
        placement([0.0, 0.0, 1.0], 0.0, [0.0, 0.0, 6.0]),
        TransformOptions::default(),
    )
    .expect("lift the slab to z in [1, 11]");
    let res = boolean_operation(
        &mut m,
        torus,
        slab,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("torus minus slab");
    assert!(
        // One band: an outer and ONE inner loop (each a closed circle, as one
        // edge or as the arcs the boolean pre-splits it into).
        has_face(&m, res, "Torus", |outer, inner| outer >= 1 && inner == 1),
        "the result must carry the two-circle torus band; faces: {:?}; cert: {:?}",
        inventory(&m, res),
        m.certify_solid(res)
    );
    // The LONG arc below z = 1, not its complement (watertight either way).
    assert_volume(&mut m, res, torus_volume_below(1.0), "torus minus slab z>1");
    assert_sound_built_and_placed(&mut m, res, "torus minus slab");
}

/// R10 r3 torus about +Z: exact volume of the part below the plane z = h
/// (Pappus: 2πR × the area of the tube section below h).
fn torus_volume_below(h: f64) -> f64 {
    let (big_r, r) = (10.0_f64, 3.0_f64);
    let area = r * r * (-h / r).acos() + h * (r * r - h * h).sqrt();
    2.0 * std::f64::consts::PI * big_r * area
}

fn assert_volume(m: &mut BRepModel, s: SolidId, want: f64, what: &str) {
    let got = m.mass_properties_for(s).expect("mass properties").volume;
    assert!(
        (got - want).abs() <= 0.05 * want,
        "{what}: volume {got:.1}, exact {want:.1} (the wrong tube arc would be off by far more)"
    );
}

/// torus(R10, r3) and a 40×40×10 slab whose bottom face is the plane z = h.
fn torus_and_slab(h: f64) -> (BRepModel, SolidId, SolidId) {
    let mut m = BRepModel::new();
    let torus = sid(TopologyBuilder::new(&mut m)
        .create_torus_3d(Point3::ORIGIN, Vector3::Z, 10.0, 3.0)
        .expect("torus"));
    let slab = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(40.0, 40.0, 10.0)
        .expect("slab"));
    transform_solid(
        &mut m,
        slab,
        placement([0.0, 0.0, 1.0], 0.0, [0.0, 0.0, h + 5.0]),
        TransformOptions::default(),
    )
    .expect("place the slab");
    (m, torus, slab)
}

/// torus ∩ slab above z = 1 keeps the SHORT band over the top of the tube.
#[test]
fn torus_intersect_slab_keeps_the_short_band() {
    let (mut m, torus, slab) = torus_and_slab(1.0);
    let res = boolean_operation(
        &mut m,
        torus,
        slab,
        BooleanOp::Intersection,
        BooleanOptions::default(),
    )
    .expect("torus ∩ slab");
    let full = 2.0 * std::f64::consts::PI.powi(2) * 10.0 * 9.0;
    assert_volume(
        &mut m,
        res,
        full - torus_volume_below(1.0),
        "torus ∩ slab z>1",
    );
    assert_sound_built_and_placed(&mut m, res, "torus ∩ slab");
}

/// A cut through the equator (h = 0): the outer cut circle lies ON the
/// torus's own equator seam, and the pipeline merges the cut into that seam
/// edge. The two copies of the circle (seam on the torus, cut on the slab) are
/// split at different points and do not weld — measured 2026-11-05: banding
/// with the seam's edges gave an open solid. Until coincident seam/cut edges
/// are corefined, the boolean must REFUSE with a typed error (model rolled
/// back), never return an unsound solid; a sound lower half is also accepted.
#[test]
fn torus_cut_at_its_equator_is_sound_or_refused() {
    let (mut m, torus, slab) = torus_and_slab(0.0);
    match boolean_operation(
        &mut m,
        torus,
        slab,
        BooleanOp::Difference,
        BooleanOptions::default(),
    ) {
        Ok(res) => {
            assert_volume(&mut m, res, torus_volume_below(0.0), "lower half torus");
            assert_sound_built_and_placed(&mut m, res, "lower half torus");
        }
        Err(e) => eprintln!("equator cut refused (typed): {e}"),
    }
}

/// A slab TOUCHING the top of the tube (h = r) cuts nothing: the torus is
/// unchanged, or the boolean refuses the tangency with a typed error — never
/// an unsound solid.
#[test]
fn a_slab_touching_the_top_of_the_torus_cuts_nothing() {
    let (mut m, torus, slab) = torus_and_slab(3.0);
    match boolean_operation(
        &mut m,
        torus,
        slab,
        BooleanOp::Difference,
        BooleanOptions::default(),
    ) {
        Ok(res) => {
            let full = 2.0 * std::f64::consts::PI.powi(2) * 10.0 * 9.0;
            assert_volume(&mut m, res, full, "torus touched by a slab");
            assert_sound(&mut m, res, "torus touched by a slab");
        }
        Err(e) => eprintln!("tangent slab refused (typed): {e}"),
    }
}
