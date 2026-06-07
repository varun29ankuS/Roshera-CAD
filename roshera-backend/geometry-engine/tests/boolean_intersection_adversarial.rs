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

use geometry_engine::math::{Matrix4, Point3, Vector3};
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

// --- Curved intersections (sphere / cylinder vs the unit box) ------------

type Membership = Box<dyn Fn([f64; 3]) -> bool>;
type Builder = Box<dyn Fn(&mut BRepModel) -> SolidId>;

/// Kernel `A op B` where A is the unit box and B is built by `build_b`.
fn kernel_box_op(op: BooleanOp, build_b: &dyn Fn(&mut BRepModel) -> SolidId) -> Option<f64> {
    let mut model = BRepModel::new();
    let a = unit_cube(&mut model);
    let b = build_b(&mut model);
    let result = boolean_operation(&mut model, a, b, op, BooleanOptions::default()).ok()?;
    model.calculate_solid_volume(result)
}

fn sphere(model: &mut BRepModel, center: [f64; 3], r: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_sphere_3d(Point3::new(center[0], center[1], center[2]), r)
        .expect("sphere creation succeeds")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn z_cylinder(model: &mut BRepModel, r: f64, half_h: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_cylinder_3d(
            Point3::new(0.0, 0.0, -half_h),
            Vector3::new(0.0, 0.0, 1.0),
            r,
            2.0 * half_h,
        )
        .expect("cylinder creation succeeds")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

/// Curved ∩/∪ against the box, checked against analytic membership. Curved
/// results carry tessellation-volume error (~1%) on top of the grid's ~1%, so
/// the tolerance is looser than the polyhedral cases — still tight enough to
/// catch the dropped-face / open-mesh failures the curved boolean path is prone
/// to (those run to tens of percent).
#[test]
fn curved_intersection_matches_independent_oracle() {
    const TOL_C: f64 = 0.05;
    let rs = 1.2_f64; // sphere radius

    // Only the stable, correct curved cases run here. The offset sphere-poke and
    // the cylinder are pinned as #[ignore]'d known bugs below (#81, #82).
    let cases: Vec<(&str, f64, Membership, Builder)> = vec![(
        "sphere r1.2 ∩ box",
        1.8,
        Box::new(move |p: [f64; 3]| (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt() <= rs),
        Box::new(move |m: &mut BRepModel| sphere(m, [0.0, 0.0, 0.0], rs)),
    )];

    let mut failures: Vec<String> = Vec::new();
    eprintln!("  case                  | ∩ kernel  ∩ truth  err   | ∪ kernel  ∪ truth  err");
    for (name, region, in_b, build_b) in &cases {
        let (g_int, g_uni) = grid_core(*region, in_b.as_ref());
        let k_int = kernel_box_op(BooleanOp::Intersection, build_b.as_ref());
        let k_uni = kernel_box_op(BooleanOp::Union, build_b.as_ref());
        let k_int_s = k_int.map_or("ERR".to_string(), |v| format!("{v:7.3}"));
        let k_uni_s = k_uni.map_or("ERR".to_string(), |v| format!("{v:7.3}"));
        let e_int = k_int.map_or(f64::INFINITY, |v| rel_err(v, g_int));
        let e_uni = k_uni.map_or(f64::INFINITY, |v| rel_err(v, g_uni));
        eprintln!(
            "  {name:21} | {k_int_s}  {g_int:7.3}  {ei:5.1}% | {k_uni_s}  {g_uni:7.3}  {eu:5.1}%",
            ei = e_int * 100.0,
            eu = e_uni * 100.0
        );
        if e_int > TOL_C {
            failures.push(format!(
                "{name}: ∩ {k_int_s} vs {g_int:.3} ({:.1}%)",
                e_int * 100.0
            ));
        }
        if e_uni > TOL_C {
            failures.push(format!(
                "{name}: ∪ {k_uni_s} vs {g_uni:.3} ({:.1}%)",
                e_uni * 100.0
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "curved boolean disagreements with independent oracle:\n  {}",
        failures.join("\n  ")
    );
}

/// PINNED KNOWN BUG (#81): cylinder ∩/∪ box is wrong on the curved side-bulge
/// case. With cylinder radius 1.2 > box half-extent 1, the cylinder pokes past
/// all four box side faces. The kernel reports V(∪) < V(∩) — impossible — and
/// loses ~30% of the union, i.e. the four side-bulge regions are dropped or left
/// unstitched. Same class as the curved-union face-drop bugs (#50/#58), on a
/// config they didn't cover. Verified against an independent grid oracle whose
/// own ∩+∪ matches V(box)+V(cyl) to <0.1%, so the fault is the kernel.
/// Un-ignore when the cylinder-box boolean is fixed.
#[test]
fn cylinder_box_boolean_81() {
    let rc = 1.2_f64;
    let in_b = |p: [f64; 3]| (p[0] * p[0] + p[1] * p[1]).sqrt() <= rc && p[2].abs() <= 1.0;
    let (g_int, g_uni) = grid_core(1.8, &in_b);
    let k_int = kernel_box_op(BooleanOp::Intersection, &|m| z_cylinder(m, rc, 1.0)).expect("∩");
    let k_uni = kernel_box_op(BooleanOp::Union, &|m| z_cylinder(m, rc, 1.0)).expect("∪");
    assert!(
        k_uni >= k_int,
        "union must be ≥ intersection (got ∪={k_uni:.3} < ∩={k_int:.3})"
    );
    assert!(
        rel_err(k_int, g_int) <= 0.05,
        "∩ {k_int:.3} vs truth {g_int:.3}"
    );
    assert!(
        rel_err(k_uni, g_uni) <= 0.05,
        "∪ {k_uni:.3} vs truth {g_uni:.3}"
    );
}

/// KNOWN BUG (#82): box ∩ sphere with the sphere straddling a box face is both
/// non-deterministic and wrong. Sphere r=1 centred at (1,0,0) is exactly half
/// inside the box, so V(box ∩ sphere) = ½·(4/3·π) ≈ 2.094; the kernel sometimes
/// returns the *whole* sphere (~4.189) instead, failing to clip B by A.
///
/// This is asserted as a DETERMINISM test, not a flaky single-shot: it runs the
/// same boolean 8 times in one process (`std::HashMap` reseeds per map per
/// process, so each run shuffles internal iteration order) and requires every
/// run to agree — AND to match the independent grid oracle. A non-deterministic
/// boolean therefore fails *reliably* here, not 2-out-of-10. Four determinism
/// sources are already fixed (graph→BTreeMap etc., commit 5ee5845); a residual
/// source remains in the degenerate great-circle-on-cut-plane classification.
#[test]
fn sphere_poke_intersection_82() {
    let in_b = |p: [f64; 3]| ((p[0] - 1.0).powi(2) + p[1] * p[1] + p[2] * p[2]).sqrt() <= 1.0;
    let (g_int, _g_uni) = grid_core(2.1, &in_b);

    let runs: Vec<f64> = (0..8)
        .map(|_| {
            kernel_box_op(BooleanOp::Intersection, &|m| {
                sphere(m, [1.0, 0.0, 0.0], 1.0)
            })
            .expect("∩")
        })
        .collect();

    // Determinism: every run must produce the same volume.
    let first = runs[0];
    for (i, &v) in runs.iter().enumerate() {
        assert!(
            (v - first).abs() < 1e-9,
            "non-deterministic box∩sphere: run 0 = {first:.4}, run {i} = {v:.4} (runs = {runs:?})"
        );
    }
    // Correctness: the (now stable) value must be the clipped half, not the whole sphere.
    assert!(
        rel_err(first, g_int) <= 0.05,
        "box-sphere(poke) = {first:.3}, truth {g_int:.3} (kernel returns the unclipped sphere ~4.19)"
    );
}

type VolFn = Box<dyn Fn() -> f64>;

/// Determinism net for the boolean pipeline (P0). Each config is run 8 times in
/// ONE process — `std::HashMap` reseeds per map per process, so every run
/// shuffles internal iteration order — and every run must produce an identical
/// volume. This locks the determinism fixes (graph→BTreeMap, component/vertex/
/// sphere-split sorting; commit 5ee5845) across the configs that exercise the
/// rotated, offset, sphere, and cylinder paths. The degenerate face-straddle
/// poke has its own dedicated determinism test (`sphere_poke_intersection_82`).
#[test]
fn boolean_results_are_deterministic() {
    let r45 = std::f64::consts::FRAC_PI_4;
    let r30 = 30.0_f64 * std::f64::consts::PI / 180.0;
    let cases: Vec<(&str, VolFn)> = vec![
        (
            "rot45 ∩",
            Box::new(move || kernel_volume(BooleanOp::Intersection, r45).expect("v")),
        ),
        (
            "rot45 ∪",
            Box::new(move || kernel_volume(BooleanOp::Union, r45).expect("v")),
        ),
        (
            "offset30 ∩",
            Box::new(move || {
                kernel_volume_offset(BooleanOp::Intersection, r30, [0.6, 0.4, 0.0]).expect("v")
            }),
        ),
        (
            "offset30 ∪",
            Box::new(move || {
                kernel_volume_offset(BooleanOp::Union, r30, [0.6, 0.4, 0.0]).expect("v")
            }),
        ),
        (
            "sphere ∩",
            Box::new(|| {
                kernel_box_op(BooleanOp::Intersection, &|m| {
                    sphere(m, [0.0, 0.0, 0.0], 1.2)
                })
                .expect("v")
            }),
        ),
        (
            "sphere ∪",
            Box::new(|| {
                kernel_box_op(BooleanOp::Union, &|m| sphere(m, [0.0, 0.0, 0.0], 1.2)).expect("v")
            }),
        ),
    ];

    // Threshold catches *gross / topological* non-determinism — a dropped face,
    // a kept-vs-discarded fragment, the whole-vs-half flip — which moves the
    // volume by percent-level amounts. Sub-1e-6 drift is floating-point
    // summation-order noise (e.g. triangle order in the volume integral) and is
    // tolerated by design: making reordered float sums byte-identical everywhere
    // is a deep, low-value chase. Residual noted: `sphere ∩` drifts ~4e-8 from
    // order-dependent summation (P2 follow-up, not a logic fault).
    const REL: f64 = 1e-6;
    let mut nondet: Vec<String> = Vec::new();
    for (label, f) in &cases {
        let runs: Vec<f64> = (0..8).map(|_| f()).collect();
        let first = runs[0];
        if runs
            .iter()
            .any(|v| (v - first).abs() / first.abs().max(1.0) >= REL)
        {
            nondet.push(format!("{label}: {runs:?}"));
        }
    }
    assert!(
        nondet.is_empty(),
        "gross/topological boolean non-determinism (run-to-run volume drift):\n  {}",
        nondet.join("\n  ")
    );
}

/// Isolation of #81 from the coincident-cap degeneracy: a RADIAL poke where the
/// cylinder is SHORTER than the box (half-height 0.7 < box half-extent 1.0), so
/// the caps sit strictly inside the box and the box ±z faces never touch the
/// cylinder — only the four box SIDE planes cut it, in vertical generators,
/// producing the four θ-sector bulges. If the union still drops them here, the
/// bug is the radial sector partition itself, not the cap coincidence (#81/#85).
#[test]
fn cylinder_box_radial_poke_nondegenerate() {
    let rc = 1.2_f64;
    let hh = 0.7_f64;
    let in_b = |p: [f64; 3]| (p[0] * p[0] + p[1] * p[1]).sqrt() <= rc && p[2].abs() <= hh;
    let (g_int, g_uni) = grid_core(1.8, &in_b);
    let k_int = kernel_box_op(BooleanOp::Intersection, &|m| z_cylinder(m, rc, hh));
    let k_uni = kernel_box_op(BooleanOp::Union, &|m| z_cylinder(m, rc, hh));
    eprintln!("radial-nondegen: ∩ {k_int:?} (truth {g_int:.3}) | ∪ {k_uni:?} (truth {g_uni:.3})");
    // ISOLATED FINDINGS (cap degeneracy ruled out — caps interior to the box):
    // the radial θ-sector partition of the cylinder lateral corrupts BOTH ops.
    //   ∩ over-includes (returns ~box volume, exceeding even the cylinder's own
    //     volume — impossible, since A∩B ≤ min(|A|,|B|)): it barely clips the cyl.
    //   ∪ ERRORS (None): the four bulge sectors are not stitched into a closed
    //     shell, so build_shells_from_faces rejects the result.
    let mut fails: Vec<String> = Vec::new();
    let v_cyl = std::f64::consts::PI * rc * rc * (2.0 * hh);
    match k_int {
        None => fails.push("∩ errored".into()),
        Some(v) if v > v_cyl + 1e-6 => fails.push(format!(
            "∩ = {v:.3} EXCEEDS cylinder volume {v_cyl:.3} (A∩B must be ≤ min(|A|,|B|))"
        )),
        Some(v) if rel_err(v, g_int) > 0.05 => {
            fails.push(format!("∩ = {v:.3} vs truth {g_int:.3}"))
        }
        Some(_) => {}
    }
    match k_uni {
        None => fails.push("∪ ERRORED — 4 bulge sectors not stitched into a closed shell".into()),
        Some(v) if rel_err(v, g_uni) > 0.05 => {
            fails.push(format!("∪ = {v:.3} vs truth {g_uni:.3}"))
        }
        Some(_) => {}
    }
    assert!(
        fails.is_empty(),
        "radial fat-cylinder boolean broken — radial θ-sector partition (#81/#85):\n  {}",
        fails.join("\n  ")
    );
}
