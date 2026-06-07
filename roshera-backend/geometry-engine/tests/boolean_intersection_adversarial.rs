//! Adversarial boolean-INTERSECTION oracle harness (BOOL-∩-HARNESS, #78).
//!
//! The existing volume proptests measure the kernel against its *own* mesh
//! volume with a soft 10% inclusion-exclusion tolerance — loose enough that a
//! real over-inclusion can hide. This harness uses an **independent ground
//! truth**: a deterministic grid-occupancy oracle that knows nothing about the
//! kernel. A grid of cell centres over a region enclosing both solids is tested
//! for analytic membership in each operand; the occupied-cell count times the
//! cell volume is the truth for `V(A∩B)` and `V(A∪B)`.
//!
//! The adversarial input is the classic Parasolid-grade stressor: two identical
//! cubes sharing a centre, one rotated about Z. Axis-aligned (θ=0) is trivial;
//! every rotated pose forces the intersection pipeline to build a genuine
//! many-faced solid (a 45° pose is an octagonal prism). A correct kernel must
//! track the truth across the whole sweep and never violate inclusion-exclusion
//!   V(A∪B) + V(A∩B) = V(A) + V(B).
//!
//! Prior diagnosis (task #34): the kernel over-includes on rotated input. This
//! harness pins that quantitatively; the fix lands under #80.

use geometry_engine::math::{Matrix4, Vector3};
use geometry_engine::operations::{
    boolean_operation, transform_solid, BooleanOp, BooleanOptions, TransformOptions,
};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

// Unit cube: `create_box_3d(2,2,2)` is centred at the origin with half-extent 1.
const HALF: f64 = 1.0;
// Grid oracle cell size — fixed, so the oracle's accuracy is the same no matter
// how large a region a given case needs (the region must fully contain both
// solids or it silently clips one and lies). ~1-2% discretisation bias.
const CELL: f64 = 0.02;
// Agreement tolerance against the grid oracle: absorbs the grid's own bias while
// still catching the gross (>4%) errors real boolean bugs produce. Inclusion-
// exclusion is checked separately and exactly (it needs no oracle).
const TOL: f64 = 0.04;

/// Is `p` inside an origin-centred cube of half-extent `HALF` that has been
/// rotated by `angle` about Z? Expresses `p` in the cube's own frame (rotate by
/// −angle) and checks the axis-aligned bounds. Pure analytic truth — no kernel.
fn in_rotated_cube(p: [f64; 3], angle: f64) -> bool {
    let (s, c) = angle.sin_cos();
    let bx = c * p[0] + s * p[1];
    let by = -s * p[0] + c * p[1];
    bx.abs() <= HALF && by.abs() <= HALF && p[2].abs() <= HALF
}

/// Independent grid-occupancy truth for `V(A∩B)` and `V(A∪B)`, where A is the
/// axis-aligned unit cube and B's membership is `in_b`. `region` is the
/// half-width of the (cube-shaped) sampling domain and MUST fully contain both
/// solids — otherwise the far solid is clipped and the union is under-counted.
/// Deterministic: identical bytes on every run.
fn grid_core(region: f64, in_b: &dyn Fn([f64; 3]) -> bool) -> (f64, f64) {
    let n = (2.0 * region / CELL).ceil() as usize;
    let cell = 2.0 * region / n as f64;
    let cell_vol = cell * cell * cell;
    let (mut n_int, mut n_uni) = (0u64, 0u64);
    for i in 0..n {
        let x = -region + (i as f64 + 0.5) * cell;
        for j in 0..n {
            let y = -region + (j as f64 + 0.5) * cell;
            for k in 0..n {
                let z = -region + (k as f64 + 0.5) * cell;
                let p = [x, y, z];
                let in_a = in_rotated_cube(p, 0.0);
                let in_bb = in_b(p);
                if in_a && in_bb {
                    n_int += 1;
                }
                if in_a || in_bb {
                    n_uni += 1;
                }
            }
        }
    }
    (n_int as f64 * cell_vol, n_uni as f64 * cell_vol)
}

/// Truth for A ∩/∪ B with B rotated `angle_b` about Z (concentric). Region 1.5
/// contains a unit cube rotated to any angle (corner reach √2 ≈ 1.414).
fn grid_volumes(angle_b: f64) -> (f64, f64) {
    grid_core(1.5, &|p| in_rotated_cube(p, angle_b))
}

fn unit_cube(model: &mut BRepModel) -> SolidId {
    match TopologyBuilder::new(model)
        .create_box_3d(2.0 * HALF, 2.0 * HALF, 2.0 * HALF)
        .expect("unit cube creation succeeds")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

/// Kernel volume of `A op B` where B is a unit cube rotated `angle_b` about Z.
/// Fresh model per call so neither operand is mutated across measurements.
/// Returns `None` if the boolean errors (itself a finding for a coincident or
/// rotated pose a production kernel should handle).
fn kernel_volume(op: BooleanOp, angle_b: f64) -> Option<f64> {
    let mut model = BRepModel::new();
    let a = unit_cube(&mut model);
    let b = unit_cube(&mut model);
    transform_solid(
        &mut model,
        b,
        Matrix4::rotation_z(angle_b),
        TransformOptions::default(),
    )
    .expect("rotation of a valid cube must succeed");
    let result = boolean_operation(&mut model, a, b, op, BooleanOptions::default()).ok()?;
    model.calculate_solid_volume(result)
}

fn rel_err(actual: f64, truth: f64) -> f64 {
    (actual - truth).abs() / truth.abs().max(1.0)
}

/// Membership in a unit cube rotated `angle` about Z then translated by `t`.
fn in_rotated_translated_cube(p: [f64; 3], angle: f64, t: [f64; 3]) -> bool {
    in_rotated_cube([p[0] - t[0], p[1] - t[1], p[2] - t[2]], angle)
}

/// Grid truth for A (axis-aligned) ∩/∪ B (rotated `angle_b` about Z, translated
/// `t`). Region grows with `t` so the translated cube is always fully contained
/// (1.5 covers the rotation reach; `max|t|` covers the offset).
fn grid_volumes_offset(angle_b: f64, t: [f64; 3]) -> (f64, f64) {
    let region = 1.5 + t.iter().fold(0.0_f64, |m, &v| m.max(v.abs()));
    grid_core(region, &|p| in_rotated_translated_cube(p, angle_b, t))
}

/// Kernel `A op B` with B rotated `angle_b` about Z then translated by `t`.
fn kernel_volume_offset(op: BooleanOp, angle_b: f64, t: [f64; 3]) -> Option<f64> {
    let mut model = BRepModel::new();
    let a = unit_cube(&mut model);
    let b = unit_cube(&mut model);
    transform_solid(
        &mut model,
        b,
        Matrix4::rotation_z(angle_b),
        TransformOptions::default(),
    )
    .expect("rotation must succeed");
    transform_solid(
        &mut model,
        b,
        Matrix4::from_translation(&Vector3::new(t[0], t[1], t[2])),
        TransformOptions::default(),
    )
    .expect("translation must succeed");
    let result = boolean_operation(&mut model, a, b, op, BooleanOptions::default()).ok()?;
    model.calculate_solid_volume(result)
}

/// Sweep Z-rotations and compare the kernel's intersection / union volume to the
/// independent grid oracle, and check inclusion-exclusion. Prints a full table
/// so a failure shows the whole curve, not just the first bad angle.
#[test]
fn rotated_cube_intersection_matches_independent_oracle() {
    // Degrees chosen to span trivial → maximally-rotated (45° square symmetry).
    let angles_deg = [0.0, 5.0, 10.0, 20.0, 30.0, 45.0];
    let v_a = 8.0; // unit cube, side 2
    let v_b = 8.0;

    let mut failures: Vec<String> = Vec::new();
    eprintln!("  θ°   | ∩ kernel  ∩ truth  err   | ∪ kernel  ∪ truth  err   | incl-excl");
    for deg in angles_deg {
        let ang = deg * std::f64::consts::PI / 180.0;
        let (g_int, g_uni) = grid_volumes(ang);
        let k_int = kernel_volume(BooleanOp::Intersection, ang);
        let k_uni = kernel_volume(BooleanOp::Union, ang);

        let k_int_s = k_int.map_or("ERR".to_string(), |v| format!("{v:7.3}"));
        let k_uni_s = k_uni.map_or("ERR".to_string(), |v| format!("{v:7.3}"));
        let e_int = k_int.map_or(f64::INFINITY, |v| rel_err(v, g_int));
        let e_uni = k_uni.map_or(f64::INFINITY, |v| rel_err(v, g_uni));
        let ie = match (k_int, k_uni) {
            (Some(i), Some(u)) => rel_err(i + u, v_a + v_b),
            _ => f64::INFINITY,
        };
        eprintln!(
            "  {deg:4.0} | {k_int_s}  {g_int:7.3}  {e_int:5.1}% | {k_uni_s}  {g_uni:7.3}  {e_uni:5.1}% | {ie:5.1}%",
            e_int = e_int * 100.0,
            e_uni = e_uni * 100.0,
            ie = ie * 100.0
        );

        if e_int > TOL {
            failures.push(format!(
                "θ={deg}°: intersection {k_int_s} vs truth {g_int:.3} ({:.1}% off)",
                e_int * 100.0
            ));
        }
        if e_uni > TOL {
            failures.push(format!(
                "θ={deg}°: union {k_uni_s} vs truth {g_uni:.3} ({:.1}% off)",
                e_uni * 100.0
            ));
        }
        if ie > TOL {
            failures.push(format!(
                "θ={deg}°: inclusion-exclusion off by {:.1}%",
                ie * 100.0
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "rotated-cube boolean disagreements with independent oracle:\n  {}",
        failures.join("\n  ")
    );
}

/// The harder case: B is rotated *and* offset, so the two cubes meet in a
/// partial, irregular overlap (a corner/edge engagement rather than a clean
/// concentric prism). This is where face-classification and trimming bugs hide.
/// Same independent-oracle contract: kernel ∩/∪ must track the grid, and
/// inclusion-exclusion must hold.
#[test]
fn offset_rotated_cube_intersection_matches_independent_oracle() {
    // (angle°, translation) — each keeps B overlapping A but off-centre.
    let cases = [
        (30.0, [0.6, 0.4, 0.0]),
        (20.0, [0.8, 0.0, 0.5]),
        (45.0, [0.5, 0.5, 0.3]),
        (15.0, [0.9, 0.6, 0.0]),
    ];
    let (v_a, v_b) = (8.0, 8.0);

    let mut failures: Vec<String> = Vec::new();
    eprintln!(
        "  θ°  t            | ∩ kernel  ∩ truth  err   | ∪ kernel  ∪ truth  err   | incl-excl"
    );
    for (deg, t) in cases {
        let ang = deg * std::f64::consts::PI / 180.0;
        let (g_int, g_uni) = grid_volumes_offset(ang, t);
        let k_int = kernel_volume_offset(BooleanOp::Intersection, ang, t);
        let k_uni = kernel_volume_offset(BooleanOp::Union, ang, t);

        let k_int_s = k_int.map_or("ERR".to_string(), |v| format!("{v:7.3}"));
        let k_uni_s = k_uni.map_or("ERR".to_string(), |v| format!("{v:7.3}"));
        let e_int = k_int.map_or(f64::INFINITY, |v| rel_err(v, g_int));
        let e_uni = k_uni.map_or(f64::INFINITY, |v| rel_err(v, g_uni));
        let ie = match (k_int, k_uni) {
            (Some(i), Some(u)) => rel_err(i + u, v_a + v_b),
            _ => f64::INFINITY,
        };
        eprintln!(
            "  {deg:4.0} {t:?} | {k_int_s}  {g_int:7.3}  {ei:5.1}% | {k_uni_s}  {g_uni:7.3}  {eu:5.1}% | {ie:5.1}%",
            ei = e_int * 100.0,
            eu = e_uni * 100.0,
            ie = ie * 100.0
        );

        if e_int > TOL {
            failures.push(format!(
                "θ={deg}° t={t:?}: ∩ {k_int_s} vs {g_int:.3} ({:.1}%)",
                e_int * 100.0
            ));
        }
        if e_uni > TOL {
            failures.push(format!(
                "θ={deg}° t={t:?}: ∪ {k_uni_s} vs {g_uni:.3} ({:.1}%)",
                e_uni * 100.0
            ));
        }
        if ie > TOL {
            failures.push(format!(
                "θ={deg}° t={t:?}: incl-excl off {:.1}%",
                ie * 100.0
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "offset+rotated boolean disagreements with independent oracle:\n  {}",
        failures.join("\n  ")
    );
}
