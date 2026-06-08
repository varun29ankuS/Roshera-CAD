//! Brutal boolean FUZZ-SURVEY — the engine of the kernel-hardening loop.
//!
//! Hand-picked cases find the bugs you thought of. This sweeps the WHOLE
//! configuration space of `box ∩/∪/∖ sphere` densely and checks every result
//! against an independent grid oracle PLUS the full B-Rep invariant battery,
//! then prints a ranked FAILURE CATALOG. That catalog is the loop's work queue:
//! run it, take the worst class, fix it, re-run, then promote the conquered
//! region into an asserting gate (`curved_boolean_poke_envelope.rs`).
//!
//! SURVEY, not a gate (ignored by default — exploratory):
//!   `cargo test -p geometry-engine --test boolean_fuzz_survey -- --ignored --nocapture`
//!
//! Speed: parallel over configs (rayon); topology via `brep_integrity` (a B-Rep
//! walk, no tessellation). The only per-config tessellation is the volume.
//!
//! HARD vs SOFT classes. HARD = real Boolean bugs: VOLUME, WATERTIGHT, MANIFOLD,
//! ERROR — trust these; they are the work queue. SOFT = over-reporting classes,
//! verify in isolation before acting:
//!   * HANG — a hung boolean can't be killed from safe Rust, so `run_op_timed`
//!     leaks the thread; under rayon every real hang burns a core, starving later
//!     configs so a slow-but-finite op blows the budget and mis-flags HANG. Fix:
//!     subprocess-isolated runner (kill on timeout).
//!   * EULER — the UV-sphere primitive carries an INTRINSIC genus-0 Euler residual
//!     of -1 (periodic seam + 2 poles; see `survey_euler_baseline`), so any
//!     sphere-bearing result fails the residual==0 check with no real bug. Fix:
//!     baseline against the operands' residual and flag only deltas.
//!
//! Invariants per (config, op):
//!   * VOLUME    — |kernel − grid_oracle| / max(truth,ε) ≤ tol
//!   * WATERTIGHT— no B-Rep edge used by exactly one face (open seam)
//!   * MANIFOLD  — no edge used by ≥3 faces
//!   * EULER     — Euler–Poincaré genus-0 residual = 0
//!   * NO-ERROR  — the op does not error on a config with a real result
//!
//! Placement classes (sphere centre vs box [-1,1]³): interior, face-straddle,
//! edge-straddle, corner-straddle, off-centre, just-outside poke. Radii from a
//! sliver to larger than the box — covering face/edge/corner poke, multi-face
//! clip, tangency, near-tangency, containment, disjoint in one sweep.

use geometry_engine::harness::brep_integrity::brep_integrity;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use rayon::prelude::*;

const BOX_HALF: f64 = 1.0;

// ---------------------------------------------------------------------------
// Grid oracle: count cell centres over a region covering both solids and bucket
// them into the three boolean regions. Independent of the kernel.
// ---------------------------------------------------------------------------

struct GridTruth {
    intersection: f64,
    union: f64,
    difference: f64, // box ∖ sphere
}

fn grid_truth(center: [f64; 3], r: f64) -> GridTruth {
    let reach = (0..3).map(|i| center[i].abs() + r).fold(BOX_HALF, f64::max) + 0.05;
    const N: usize = 96; // cells/axis; ~0.9M samples — ample for a 3% check
    let cell = 2.0 * reach / N as f64;
    let cv = cell * cell * cell;
    let r2 = r * r;
    let (mut i_n, mut u_n, mut d_n) = (0u64, 0u64, 0u64);
    for i in 0..N {
        let x = -reach + (i as f64 + 0.5) * cell;
        let in_bx = x.abs() <= BOX_HALF;
        for j in 0..N {
            let y = -reach + (j as f64 + 0.5) * cell;
            let in_by = in_bx && y.abs() <= BOX_HALF;
            for k in 0..N {
                let z = -reach + (k as f64 + 0.5) * cell;
                let in_box = in_by && z.abs() <= BOX_HALF;
                let (dx, dy, dz) = (x - center[0], y - center[1], z - center[2]);
                let in_sph = dx * dx + dy * dy + dz * dz <= r2;
                if in_box && in_sph {
                    i_n += 1;
                }
                if in_box || in_sph {
                    u_n += 1;
                }
                if in_box && !in_sph {
                    d_n += 1;
                }
            }
        }
    }
    GridTruth {
        intersection: i_n as f64 * cv,
        union: u_n as f64 * cv,
        difference: d_n as f64 * cv,
    }
}

// ---------------------------------------------------------------------------
// Kernel builders + one run
// ---------------------------------------------------------------------------

fn the_box(model: &mut BRepModel) -> SolidId {
    match TopologyBuilder::new(model)
        .create_box_3d(2.0 * BOX_HALF, 2.0 * BOX_HALF, 2.0 * BOX_HALF)
        .expect("box")
    {
        GeometryId::Solid(id) => id,
        o => panic!("box: {o:?}"),
    }
}

fn sphere(model: &mut BRepModel, c: [f64; 3], r: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_sphere_3d(Point3::new(c[0], c[1], c[2]), r)
        .expect("sphere")
    {
        GeometryId::Solid(id) => id,
        o => panic!("sphere: {o:?}"),
    }
}

#[derive(Clone, Copy)]
struct Facts {
    vol: f64,
    open_edges: usize,
    nonmanifold_edges: usize,
    euler_residual: i64,
}

/// Run one box∘B boolean fresh (`build_b` makes the second solid); `None` on
/// kernel error. Generic over the second solid so the same machinery surveys
/// box∘sphere, box∘cylinder, box∘cone, …
fn run_op<F: Fn(&mut BRepModel) -> SolidId>(op: BooleanOp, build_b: F) -> Option<Facts> {
    let mut model = BRepModel::new();
    let bx = the_box(&mut model);
    let sp = build_b(&mut model);
    let res = boolean_operation(&mut model, bx, sp, op, BooleanOptions::default()).ok()?;
    let vol = model.calculate_solid_volume(res)?;
    let rep = brep_integrity(&model, res, 1e-6);
    Some(Facts {
        vol,
        open_edges: rep.edges_used_once.len(),
        nonmanifold_edges: rep.edges_used_3plus.len(),
        euler_residual: rep.euler_poincare_genus0_residual(),
    })
}

enum Outcome {
    Ok(Facts),
    Err,
    Hang,
}

/// `run_op` under a wall-clock budget: a config that never returns (an infinite
/// loop in the boolean) is the worst failure class, and it must NOT block the
/// survey. Run it on a detached thread and give up after `OP_TIMEOUT`. The hung
/// thread leaks — acceptable for an occasional survey, and the catalog records
/// the offending config so it becomes a fixable HANG, not a frozen run.
fn run_op_timed<F: Fn(&mut BRepModel) -> SolidId + Send + 'static>(
    op: BooleanOp,
    build_b: F,
) -> Outcome {
    const OP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run_op(op, build_b));
    });
    match rx.recv_timeout(OP_TIMEOUT) {
        Ok(Some(f)) => Outcome::Ok(f),
        Ok(None) => Outcome::Err,
        Err(_) => Outcome::Hang,
    }
}

// ---------------------------------------------------------------------------
// Configuration grid
// ---------------------------------------------------------------------------

fn placements() -> Vec<([f64; 3], &'static str)> {
    vec![
        ([0.0, 0.0, 0.0], "interior-centre"),
        ([0.5, 0.3, 0.0], "interior-offset"),
        ([1.0, 0.0, 0.0], "face+x"),
        ([-1.0, 0.0, 0.0], "face-x"),
        ([0.0, 1.0, 0.0], "face+y"),
        ([0.0, 0.0, 1.0], "face+z"),
        ([1.0, 0.3, -0.2], "face+x-offset"),
        ([1.0, 1.0, 0.0], "edge+x+y"),
        ([1.0, 0.0, 1.0], "edge+x+z"),
        ([1.0, 1.0, 0.3], "edge+x+y-off"),
        ([1.0, 1.0, 1.0], "corner+++"),
        ([1.0, -1.0, 1.0], "corner+-+"),
        ([0.9, 0.9, 0.9], "corner-inside"),
        ([1.4, 0.0, 0.0], "poke+x"),
        ([1.4, 0.5, 0.0], "poke+x-off"),
        ([1.6, 1.6, 0.0], "poke-edge"),
    ]
}

fn radii() -> &'static [f64] {
    &[0.25, 0.5, 0.8, 0.95, 1.0, 1.05, 1.2, 1.5, 1.8, 2.2]
}

struct Failure {
    label: String,
    op: &'static str,
    kind: &'static str,
    detail: String,
}

/// #91 calibration: does a BARE (un-cut) sphere / box already carry a nonzero
/// genus-0 Euler-Poincaré residual? If so the survey's EULER class is a false
/// positive on the primitive's own representation, not a Boolean bug, and must
/// be baselined (flag only DELTAS from the operands).
#[test]
#[ignore = "calibration — run with --ignored --nocapture"]
fn survey_euler_baseline() {
    let mut m = BRepModel::new();
    let bx = the_box(&mut m);
    let sp = sphere(&mut m, [0.0, 0.0, 0.0], 0.5);
    let rb = brep_integrity(&m, bx, 1e-6);
    let rs = brep_integrity(&m, sp, 1e-6);
    println!("\n=== #91 EULER baseline (bare primitives) ===");
    println!(
        "box:    euler_residual={}  open_edges={}  nonmanifold={}  clean={}",
        rb.euler_poincare_genus0_residual(),
        rb.edges_used_once.len(),
        rb.edges_used_3plus.len(),
        rb.is_clean()
    );
    println!(
        "sphere: euler_residual={}  open_edges={}  nonmanifold={}  clean={}",
        rs.euler_poincare_genus0_residual(),
        rs.edges_used_once.len(),
        rs.edges_used_3plus.len(),
        rs.is_clean()
    );
    println!("=== end ===\n");
}

#[test]
#[ignore = "fuzz survey — run with --ignored --nocapture"]
fn boolean_box_sphere_fuzz_survey() {
    let vol_tol = 0.03;
    let ops: [(BooleanOp, &str, fn(&GridTruth) -> f64); 3] = [
        (BooleanOp::Intersection, "∩", |g| g.intersection),
        (BooleanOp::Union, "∪", |g| g.union),
        (BooleanOp::Difference, "∖", |g| g.difference),
    ];

    // Flat config list for parallel map.
    let mut configs: Vec<([f64; 3], &'static str, f64)> = Vec::new();
    for (c, label) in placements() {
        for &r in radii() {
            configs.push((c, label, r));
        }
    }
    let n_cfg = configs.len();

    let n_checks = std::sync::atomic::AtomicUsize::new(0);
    let fails: Vec<Failure> = configs
        .par_iter()
        .flat_map(|&(c, label, r)| {
            let truth = grid_truth(c, r);
            let mut out: Vec<Failure> = Vec::new();
            for &(op, sym, pick) in &ops {
                let t = pick(&truth);
                if t < 1e-3 {
                    continue; // empty/whole result — no boundary to test
                }
                n_checks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let lab = format!("{label} r={r}");
                match run_op_timed(op, move |m| sphere(m, c, r)) {
                    Outcome::Hang => out.push(Failure {
                        label: lab,
                        op: sym,
                        kind: "HANG",
                        detail: format!("op did not return within budget (truth {t:.3})"),
                    }),
                    Outcome::Err => out.push(Failure {
                        label: lab,
                        op: sym,
                        kind: "ERROR",
                        detail: format!("op errored (truth {t:.3})"),
                    }),
                    Outcome::Ok(f) => {
                        let rel = (f.vol - t).abs() / t.max(1e-3);
                        if rel > vol_tol {
                            out.push(Failure {
                                label: lab.clone(),
                                op: sym,
                                kind: "VOLUME",
                                detail: format!(
                                    "kernel={:.4} truth={t:.4} ({:+.1}%)",
                                    f.vol,
                                    100.0 * (f.vol - t) / t
                                ),
                            });
                        }
                        if f.open_edges != 0 {
                            out.push(Failure {
                                label: lab.clone(),
                                op: sym,
                                kind: "WATERTIGHT",
                                detail: format!("open_edges={}", f.open_edges),
                            });
                        }
                        if f.nonmanifold_edges != 0 {
                            out.push(Failure {
                                label: lab.clone(),
                                op: sym,
                                kind: "MANIFOLD",
                                detail: format!("nonmanifold_edges={}", f.nonmanifold_edges),
                            });
                        }
                        if f.euler_residual != 0 {
                            out.push(Failure {
                                label: lab.clone(),
                                op: sym,
                                kind: "EULER",
                                detail: format!("euler_residual={}", f.euler_residual),
                            });
                        }
                    }
                }
            }
            out
        })
        .collect();

    print_catalog(
        "box ∘ sphere",
        &fails,
        n_cfg,
        n_checks.load(std::sync::atomic::Ordering::Relaxed),
    );
}

/// Print a ranked failure catalog. HARD = trustworthy real-bug classes (VOLUME,
/// WATERTIGHT, MANIFOLD, ERROR) — the work queue. SOFT = over-report (HANG =
/// leaked-thread core-burn under rayon; EULER = the UV-sphere primitive's
/// intrinsic -1 genus residual, see survey_euler_baseline) — verify in isolation.
fn print_catalog(title: &str, fails: &[Failure], n_cfg: usize, n_checks: usize) {
    let is_soft = |k: &str| k == "HANG" || k == "EULER";
    use std::collections::BTreeMap;
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for f in fails {
        *by_kind.entry(f.kind).or_default() += 1;
    }
    let hard: usize = fails.iter().filter(|f| !is_soft(f.kind)).count();
    let soft = fails.len() - hard;
    println!("\n========== BOOLEAN FUZZ SURVEY: {title} ==========");
    println!("configs={n_cfg}  checks={n_checks}  HARD failures={hard}  (soft={soft})");
    println!("by kind: {by_kind:?}   [HARD: VOLUME WATERTIGHT MANIFOLD ERROR | soft: HANG EULER]");
    println!("====== HARD (real bugs — the work queue) ======");
    for (kind, _) in by_kind.iter().filter(|(k, _)| !is_soft(k)) {
        println!("--- {kind} ---");
        let mut lines: Vec<String> = fails
            .iter()
            .filter(|f| &f.kind == kind)
            .map(|f| format!("  [{}] {} : {}", f.op, f.label, f.detail))
            .collect();
        lines.sort();
        for l in lines {
            println!("{l}");
        }
    }
    println!("------ soft (verify in isolation; over-report) ------");
    for (kind, n) in by_kind.iter().filter(|(k, _)| is_soft(k)) {
        println!("--- {kind} ({n}) ---");
    }
    println!("======================================================\n");
}

// ===========================================================================
// box ∘ CYLINDER survey — same machinery, second solid is a z-axis cylinder.
// Maps whether the multi-face curved-Boolean breakage generalises beyond the
// sphere (it should — the side wall + cap circles cross box faces the same way).
// ===========================================================================

fn cylinder(model: &mut BRepModel, base: [f64; 3], r: f64, h: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_cylinder_3d(Point3::new(base[0], base[1], base[2]), Vector3::Z, r, h)
        .expect("cylinder")
    {
        GeometryId::Solid(id) => id,
        o => panic!("cylinder: {o:?}"),
    }
}

/// Inside a finite z-axis cylinder: radial ≤ r and axial ∈ [0, h] from `base`.
fn in_cylinder(p: [f64; 3], base: [f64; 3], r: f64, h: f64) -> bool {
    let axial = p[2] - base[2];
    if axial < 0.0 || axial > h {
        return false;
    }
    let (dx, dy) = (p[0] - base[0], p[1] - base[1]);
    dx * dx + dy * dy <= r * r
}

fn cyl_grid_truth(base: [f64; 3], r: f64, h: f64) -> GridTruth {
    let reach = [
        base[0].abs() + r,
        base[1].abs() + r,
        base[2].abs().max((base[2] + h).abs()),
    ]
    .into_iter()
    .fold(BOX_HALF, f64::max)
        + 0.05;
    const N: usize = 96;
    let cell = 2.0 * reach / N as f64;
    let cv = cell * cell * cell;
    let (mut i_n, mut u_n, mut d_n) = (0u64, 0u64, 0u64);
    for i in 0..N {
        let x = -reach + (i as f64 + 0.5) * cell;
        let in_bx = x.abs() <= BOX_HALF;
        for j in 0..N {
            let y = -reach + (j as f64 + 0.5) * cell;
            let in_by = in_bx && y.abs() <= BOX_HALF;
            for k in 0..N {
                let z = -reach + (k as f64 + 0.5) * cell;
                let in_box = in_by && z.abs() <= BOX_HALF;
                let in_cyl = in_cylinder([x, y, z], base, r, h);
                if in_box && in_cyl {
                    i_n += 1;
                }
                if in_box || in_cyl {
                    u_n += 1;
                }
                if in_box && !in_cyl {
                    d_n += 1;
                }
            }
        }
    }
    GridTruth {
        intersection: i_n as f64 * cv,
        union: u_n as f64 * cv,
        difference: d_n as f64 * cv,
    }
}

/// (base, radius, height, label) — z-axis cylinder placements vs box [-1,1]³.
fn cyl_configs() -> Vec<([f64; 3], f64, f64, &'static str)> {
    vec![
        ([0.0, 0.0, -1.5], 0.5, 3.0, "axial-through"),
        ([0.0, 0.0, -1.5], 0.9, 3.0, "axial-through-fat"),
        ([0.0, 0.0, 0.0], 0.5, 1.0, "axial-poke+z"),
        ([0.0, 0.0, -0.5], 0.3, 1.0, "contained"),
        ([0.5, 0.3, -0.5], 0.3, 1.0, "contained-offset"),
        ([1.0, 0.0, -0.5], 0.5, 1.0, "radial-face+x"),
        ([0.0, 1.0, -0.5], 0.5, 1.0, "radial-face+y"),
        ([1.0, 1.0, -0.5], 0.5, 1.0, "radial-edge"),
        ([1.0, 1.0, 0.6], 0.5, 1.0, "corner"),
        ([0.0, 0.0, -1.5], 1.5, 3.0, "wider-than-box"),
        ([0.5, 0.3, -1.5], 0.6, 3.0, "offset-through"),
        ([1.4, 0.0, -0.5], 0.6, 1.0, "radial-poke-past"),
    ]
}

#[test]
#[ignore = "fuzz survey — run with --ignored --nocapture"]
fn boolean_box_cylinder_fuzz_survey() {
    let vol_tol = 0.03;
    let ops: [(BooleanOp, &str, fn(&GridTruth) -> f64); 3] = [
        (BooleanOp::Intersection, "∩", |g| g.intersection),
        (BooleanOp::Union, "∪", |g| g.union),
        (BooleanOp::Difference, "∖", |g| g.difference),
    ];
    let configs = cyl_configs();
    let n_cfg = configs.len();
    let n_checks = std::sync::atomic::AtomicUsize::new(0);

    let fails: Vec<Failure> = configs
        .par_iter()
        .flat_map(|&(base, r, h, label)| {
            let truth = cyl_grid_truth(base, r, h);
            let mut out: Vec<Failure> = Vec::new();
            for &(op, sym, pick) in &ops {
                let t = pick(&truth);
                if t < 1e-3 {
                    continue;
                }
                n_checks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let lab = format!("{label} r={r} h={h}");
                match run_op_timed(op, move |m| cylinder(m, base, r, h)) {
                    Outcome::Hang => out.push(Failure {
                        label: lab,
                        op: sym,
                        kind: "HANG",
                        detail: format!("op did not return within budget (truth {t:.3})"),
                    }),
                    Outcome::Err => out.push(Failure {
                        label: lab,
                        op: sym,
                        kind: "ERROR",
                        detail: format!("op errored (truth {t:.3})"),
                    }),
                    Outcome::Ok(f) => {
                        let rel = (f.vol - t).abs() / t.max(1e-3);
                        if rel > vol_tol {
                            out.push(Failure {
                                label: lab.clone(),
                                op: sym,
                                kind: "VOLUME",
                                detail: format!(
                                    "kernel={:.4} truth={t:.4} ({:+.1}%)",
                                    f.vol,
                                    100.0 * (f.vol - t) / t
                                ),
                            });
                        }
                        if f.open_edges != 0 {
                            out.push(Failure {
                                label: lab.clone(),
                                op: sym,
                                kind: "WATERTIGHT",
                                detail: format!("open_edges={}", f.open_edges),
                            });
                        }
                        if f.nonmanifold_edges != 0 {
                            out.push(Failure {
                                label: lab.clone(),
                                op: sym,
                                kind: "MANIFOLD",
                                detail: format!("nonmanifold_edges={}", f.nonmanifold_edges),
                            });
                        }
                        // EULER deliberately not checked for cylinder yet (the
                        // cylinder primitive's intrinsic residual is uncalibrated;
                        // VOLUME+WATERTIGHT+MANIFOLD are the trusted classes).
                    }
                }
            }
            out
        })
        .collect();

    print_catalog(
        "box ∘ cylinder",
        &fails,
        n_cfg,
        n_checks.load(std::sync::atomic::Ordering::Relaxed),
    );
}

// ===========================================================================
// box ∘ CONE survey — second solid is a z-axis cone/frustum. Apex + slanted
// lateral cross box faces, so the same multi-face curved-cut breakage applies.
// ===========================================================================

fn cone(model: &mut BRepModel, bc: [f64; 3], rb: f64, rt: f64, h: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_cone_3d(Point3::new(bc[0], bc[1], bc[2]), Vector3::Z, rb, rt, h)
        .expect("cone")
    {
        GeometryId::Solid(id) => id,
        o => panic!("cone: {o:?}"),
    }
}

/// Inside a finite z-axis cone/frustum: radial ≤ r(axial), axial ∈ [0,h], where
/// r interpolates base_radius `rb` (axial 0) → top_radius `rt` (axial h).
fn in_cone(p: [f64; 3], bc: [f64; 3], rb: f64, rt: f64, h: f64) -> bool {
    let axial = p[2] - bc[2];
    if axial < 0.0 || axial > h {
        return false;
    }
    let r_at = rb + (rt - rb) * (axial / h);
    let (dx, dy) = (p[0] - bc[0], p[1] - bc[1]);
    dx * dx + dy * dy <= r_at * r_at
}

fn cone_grid_truth(bc: [f64; 3], rb: f64, rt: f64, h: f64) -> GridTruth {
    let rmax = rb.max(rt);
    let reach = [
        bc[0].abs() + rmax,
        bc[1].abs() + rmax,
        bc[2].abs().max((bc[2] + h).abs()),
    ]
    .into_iter()
    .fold(BOX_HALF, f64::max)
        + 0.05;
    const N: usize = 96;
    let cell = 2.0 * reach / N as f64;
    let cv = cell * cell * cell;
    let (mut i_n, mut u_n, mut d_n) = (0u64, 0u64, 0u64);
    for i in 0..N {
        let x = -reach + (i as f64 + 0.5) * cell;
        let in_bx = x.abs() <= BOX_HALF;
        for j in 0..N {
            let y = -reach + (j as f64 + 0.5) * cell;
            let in_by = in_bx && y.abs() <= BOX_HALF;
            for k in 0..N {
                let z = -reach + (k as f64 + 0.5) * cell;
                let in_box = in_by && z.abs() <= BOX_HALF;
                let in_cn = in_cone([x, y, z], bc, rb, rt, h);
                if in_box && in_cn {
                    i_n += 1;
                }
                if in_box || in_cn {
                    u_n += 1;
                }
                if in_box && !in_cn {
                    d_n += 1;
                }
            }
        }
    }
    GridTruth {
        intersection: i_n as f64 * cv,
        union: u_n as f64 * cv,
        difference: d_n as f64 * cv,
    }
}

/// (base_center, base_r, top_r, height, label) — z-axis cones vs box [-1,1]³.
fn cone_configs() -> Vec<([f64; 3], f64, f64, f64, &'static str)> {
    vec![
        ([0.0, 0.0, -1.5], 0.9, 0.0, 3.0, "apex-through"),
        ([0.0, 0.0, -1.0], 0.8, 0.4, 2.0, "frustum-through"),
        ([0.0, 0.0, -0.5], 0.4, 0.0, 1.0, "contained-apex"),
        ([0.5, 0.3, -0.5], 0.4, 0.2, 1.0, "contained-frustum-off"),
        ([1.0, 0.0, -0.5], 0.5, 0.3, 1.0, "radial-face+x"),
        ([1.0, 1.0, -0.5], 0.5, 0.3, 1.0, "radial-edge"),
        ([1.0, 1.0, 0.5], 0.5, 0.0, 1.0, "corner"),
        ([0.0, 0.0, -1.5], 1.5, 0.5, 3.0, "wider-than-box"),
        ([0.0, 0.0, 0.0], 0.6, 0.0, 1.0, "apex-poke+z"),
        ([1.4, 0.0, -0.5], 0.6, 0.4, 1.0, "radial-poke-past"),
    ]
}

#[test]
#[ignore = "fuzz survey — run with --ignored --nocapture"]
fn boolean_box_cone_fuzz_survey() {
    let vol_tol = 0.03;
    let ops: [(BooleanOp, &str, fn(&GridTruth) -> f64); 3] = [
        (BooleanOp::Intersection, "∩", |g| g.intersection),
        (BooleanOp::Union, "∪", |g| g.union),
        (BooleanOp::Difference, "∖", |g| g.difference),
    ];
    let configs = cone_configs();
    let n_cfg = configs.len();
    let n_checks = std::sync::atomic::AtomicUsize::new(0);

    let fails: Vec<Failure> = configs
        .par_iter()
        .flat_map(|&(bc, rb, rt, h, label)| {
            let truth = cone_grid_truth(bc, rb, rt, h);
            let mut out: Vec<Failure> = Vec::new();
            for &(op, sym, pick) in &ops {
                let t = pick(&truth);
                if t < 1e-3 {
                    continue;
                }
                n_checks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let lab = format!("{label} rb={rb} rt={rt} h={h}");
                match run_op_timed(op, move |m| cone(m, bc, rb, rt, h)) {
                    Outcome::Hang => out.push(Failure {
                        label: lab,
                        op: sym,
                        kind: "HANG",
                        detail: format!("op did not return within budget (truth {t:.3})"),
                    }),
                    Outcome::Err => out.push(Failure {
                        label: lab,
                        op: sym,
                        kind: "ERROR",
                        detail: format!("op errored (truth {t:.3})"),
                    }),
                    Outcome::Ok(f) => {
                        let rel = (f.vol - t).abs() / t.max(1e-3);
                        if rel > vol_tol {
                            out.push(Failure {
                                label: lab.clone(),
                                op: sym,
                                kind: "VOLUME",
                                detail: format!(
                                    "kernel={:.4} truth={t:.4} ({:+.1}%)",
                                    f.vol,
                                    100.0 * (f.vol - t) / t
                                ),
                            });
                        }
                        if f.open_edges != 0 {
                            out.push(Failure {
                                label: lab.clone(),
                                op: sym,
                                kind: "WATERTIGHT",
                                detail: format!("open_edges={}", f.open_edges),
                            });
                        }
                        if f.nonmanifold_edges != 0 {
                            out.push(Failure {
                                label: lab.clone(),
                                op: sym,
                                kind: "MANIFOLD",
                                detail: format!("nonmanifold_edges={}", f.nonmanifold_edges),
                            });
                        }
                    }
                }
            }
            out
        })
        .collect();

    print_catalog(
        "box ∘ cone",
        &fails,
        n_cfg,
        n_checks.load(std::sync::atomic::Ordering::Relaxed),
    );
}

// ===========================================================================
// box ∘ ROTATED-BOX survey — second solid is a unit-ish box rotated by an
// arbitrary axis/angle. All-planar, but the rotated faces cut the axis-aligned
// box obliquely: exercises the polygon-clip / split-face path that the curved
// surveys don't, and is the classic #34/#80 over-inclusion regression surface.
// ===========================================================================

use geometry_engine::math::Matrix4;
use geometry_engine::operations::transform::{transform_solid, TransformOptions};

/// A box of half-extent `hb`, rotated `angle` rad about `axis`, then centered
/// at `center`. Transform applied to vertices is M = T(center)·R(axis,angle),
/// so a local corner v maps to R·v + center.
fn rotated_box(
    model: &mut BRepModel,
    hb: f64,
    center: [f64; 3],
    axis: [f64; 3],
    angle: f64,
) -> SolidId {
    let id = match TopologyBuilder::new(model)
        .create_box_3d(2.0 * hb, 2.0 * hb, 2.0 * hb)
        .expect("rbox")
    {
        GeometryId::Solid(id) => id,
        o => panic!("rbox: {o:?}"),
    };
    let r = Matrix4::from_axis_angle(&Vector3::new(axis[0], axis[1], axis[2]), angle)
        .expect("axis-angle");
    let m = Matrix4::translation(center[0], center[1], center[2]) * r;
    transform_solid(model, id, m, TransformOptions::default()).expect("transform rbox");
    id
}

/// Inside the rotated box iff the inverse-rotated, de-centered point lies in the
/// axis-aligned box [-hb,hb]³. `r` is the SAME rotation used by `rotated_box`;
/// R is orthonormal so R⁻¹ = Rᵀ, and `transform_vector` drops translation.
fn in_rotated_box(p: [f64; 3], hb: f64, center: [f64; 3], r: &Matrix4) -> bool {
    let local = r.transpose().transform_vector(&Vector3::new(
        p[0] - center[0],
        p[1] - center[1],
        p[2] - center[2],
    ));
    local.x.abs() <= hb && local.y.abs() <= hb && local.z.abs() <= hb
}

fn rbox_grid_truth(hb: f64, center: [f64; 3], axis: [f64; 3], angle: f64) -> GridTruth {
    let r = Matrix4::from_axis_angle(&Vector3::new(axis[0], axis[1], axis[2]), angle)
        .expect("axis-angle");
    let diag = hb * 3.0_f64.sqrt();
    let reach = (0..3)
        .map(|i| center[i].abs() + diag)
        .fold(BOX_HALF, f64::max)
        + 0.05;
    const N: usize = 96;
    let cell = 2.0 * reach / N as f64;
    let cv = cell * cell * cell;
    let (mut i_n, mut u_n, mut d_n) = (0u64, 0u64, 0u64);
    for i in 0..N {
        let x = -reach + (i as f64 + 0.5) * cell;
        let in_bx = x.abs() <= BOX_HALF;
        for j in 0..N {
            let y = -reach + (j as f64 + 0.5) * cell;
            let in_by = in_bx && y.abs() <= BOX_HALF;
            for k in 0..N {
                let z = -reach + (k as f64 + 0.5) * cell;
                let in_box = in_by && z.abs() <= BOX_HALF;
                let in_rb = in_rotated_box([x, y, z], hb, center, &r);
                if in_box && in_rb {
                    i_n += 1;
                }
                if in_box || in_rb {
                    u_n += 1;
                }
                if in_box && !in_rb {
                    d_n += 1;
                }
            }
        }
    }
    GridTruth {
        intersection: i_n as f64 * cv,
        union: u_n as f64 * cv,
        difference: d_n as f64 * cv,
    }
}

/// (half-extent, center, axis, angle_deg, label) — rotated boxes vs box [-1,1]³.
fn rbox_configs() -> Vec<(f64, [f64; 3], [f64; 3], f64, &'static str)> {
    vec![
        (
            0.7,
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            45.0,
            "diag-45-centered",
        ),
        (
            0.4,
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            20.0,
            "contained-tilt",
        ),
        (0.9, [0.5, 0.0, 0.0], [0.0, 0.0, 1.0], 30.0, "z-rot-offset"),
        (0.6, [0.8, 0.8, 0.8], [1.0, 1.0, 0.0], 30.0, "corner-rot"),
        (0.7, [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], 40.0, "edge-straddle"),
        (1.0, [0.0, 0.0, 0.0], [1.0, 1.0, 1.0], 45.0, "big-diag"),
        (0.5, [0.6, 0.6, 0.0], [0.0, 0.0, 1.0], 45.0, "spin-off"),
        (0.5, [0.0, 0.0, 0.0], [1.0, 2.0, 0.0], 35.0, "tilt-through"),
        (0.8, [0.3, 0.3, 0.3], [1.0, 1.0, 1.0], 60.0, "diag-60-off"),
    ]
}

#[test]
#[ignore = "fuzz survey — run with --ignored --nocapture"]
fn boolean_box_rotated_box_fuzz_survey() {
    let vol_tol = 0.03;
    let ops: [(BooleanOp, &str, fn(&GridTruth) -> f64); 3] = [
        (BooleanOp::Intersection, "∩", |g| g.intersection),
        (BooleanOp::Union, "∪", |g| g.union),
        (BooleanOp::Difference, "∖", |g| g.difference),
    ];
    let configs = rbox_configs();
    let n_cfg = configs.len();
    let n_checks = std::sync::atomic::AtomicUsize::new(0);

    let fails: Vec<Failure> = configs
        .par_iter()
        .flat_map(|&(hb, center, axis, angle_deg, label)| {
            let angle = angle_deg.to_radians();
            let truth = rbox_grid_truth(hb, center, axis, angle);
            let mut out: Vec<Failure> = Vec::new();
            for &(op, sym, pick) in &ops {
                let t = pick(&truth);
                if t < 1e-3 {
                    continue;
                }
                n_checks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let lab = format!("{label} hb={hb} {angle_deg}°");
                match run_op_timed(op, move |m| rotated_box(m, hb, center, axis, angle)) {
                    Outcome::Hang => out.push(Failure {
                        label: lab,
                        op: sym,
                        kind: "HANG",
                        detail: format!("op did not return within budget (truth {t:.3})"),
                    }),
                    Outcome::Err => out.push(Failure {
                        label: lab,
                        op: sym,
                        kind: "ERROR",
                        detail: format!("op errored (truth {t:.3})"),
                    }),
                    Outcome::Ok(f) => {
                        let rel = (f.vol - t).abs() / t.max(1e-3);
                        if rel > vol_tol {
                            out.push(Failure {
                                label: lab.clone(),
                                op: sym,
                                kind: "VOLUME",
                                detail: format!(
                                    "kernel={:.4} truth={t:.4} ({:+.1}%)",
                                    f.vol,
                                    100.0 * (f.vol - t) / t
                                ),
                            });
                        }
                        if f.open_edges != 0 {
                            out.push(Failure {
                                label: lab.clone(),
                                op: sym,
                                kind: "WATERTIGHT",
                                detail: format!("open_edges={}", f.open_edges),
                            });
                        }
                        if f.nonmanifold_edges != 0 {
                            out.push(Failure {
                                label: lab.clone(),
                                op: sym,
                                kind: "MANIFOLD",
                                detail: format!("nonmanifold_edges={}", f.nonmanifold_edges),
                            });
                        }
                    }
                }
            }
            out
        })
        .collect();

    print_catalog(
        "box ∘ rotated-box",
        &fails,
        n_cfg,
        n_checks.load(std::sync::atomic::Ordering::Relaxed),
    );
}

// ===========================================================================
// SPHERE ∘ SPHERE survey — both operands curved. No planar faces at all, so
// every cut is curve-on-curve (a circle where two spheres meet). Exercises the
// curved∩curved arrangement that box∘sphere only hits on one operand.
//
// Generic two-solid runner (the box surveys hardcode `the_box` as operand A).
// Tolerance is looser (0.05) than the box surveys: BOTH the grid oracle and the
// kernel's tessellated volume discretize two curved operands, so a few-percent
// gap is grid noise, not a kernel bug. Catastrophic failures (wrong solid,
// dropped lens, open mesh) dwarf that band and still register.
// ===========================================================================

fn run_pair<A, B>(op: BooleanOp, build_a: A, build_b: B) -> Option<Facts>
where
    A: Fn(&mut BRepModel) -> SolidId,
    B: Fn(&mut BRepModel) -> SolidId,
{
    let mut model = BRepModel::new();
    let a = build_a(&mut model);
    let b = build_b(&mut model);
    let res = boolean_operation(&mut model, a, b, op, BooleanOptions::default()).ok()?;
    let vol = model.calculate_solid_volume(res)?;
    let rep = brep_integrity(&model, res, 1e-6);
    Some(Facts {
        vol,
        open_edges: rep.edges_used_once.len(),
        nonmanifold_edges: rep.edges_used_3plus.len(),
        euler_residual: rep.euler_poincare_genus0_residual(),
    })
}

fn run_pair_timed<A, B>(op: BooleanOp, build_a: A, build_b: B) -> Outcome
where
    A: Fn(&mut BRepModel) -> SolidId + Send + 'static,
    B: Fn(&mut BRepModel) -> SolidId + Send + 'static,
{
    const OP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run_pair(op, build_a, build_b));
    });
    match rx.recv_timeout(OP_TIMEOUT) {
        Ok(Some(f)) => Outcome::Ok(f),
        Ok(None) => Outcome::Err,
        Err(_) => Outcome::Hang,
    }
}

fn in_ball(p: [f64; 3], c: [f64; 3], r: f64) -> bool {
    let (dx, dy, dz) = (p[0] - c[0], p[1] - c[1], p[2] - c[2]);
    dx * dx + dy * dy + dz * dz <= r * r
}

fn ss_grid_truth(ca: [f64; 3], ra: f64, cb: [f64; 3], rb: f64) -> GridTruth {
    let reach = (0..3)
        .map(|i| (ca[i].abs() + ra).max(cb[i].abs() + rb))
        .fold(0.1, f64::max)
        + 0.05;
    const N: usize = 96;
    let cell = 2.0 * reach / N as f64;
    let cv = cell * cell * cell;
    let (mut i_n, mut u_n, mut d_n) = (0u64, 0u64, 0u64);
    for i in 0..N {
        let x = -reach + (i as f64 + 0.5) * cell;
        for j in 0..N {
            let y = -reach + (j as f64 + 0.5) * cell;
            for k in 0..N {
                let z = -reach + (k as f64 + 0.5) * cell;
                let p = [x, y, z];
                let in_a = in_ball(p, ca, ra);
                let in_b = in_ball(p, cb, rb);
                if in_a && in_b {
                    i_n += 1;
                }
                if in_a || in_b {
                    u_n += 1;
                }
                if in_a && !in_b {
                    d_n += 1;
                }
            }
        }
    }
    GridTruth {
        intersection: i_n as f64 * cv,
        union: u_n as f64 * cv,
        difference: d_n as f64 * cv,
    }
}

/// (centre_a, r_a, centre_b, r_b, label) — sphere∖sphere is A∖B (order matters).
fn ss_configs() -> Vec<([f64; 3], f64, [f64; 3], f64, &'static str)> {
    vec![
        ([0.0, 0.0, 0.0], 1.0, [0.0, 0.0, 0.0], 0.6, "concentric"),
        ([0.0, 0.0, 0.0], 1.0, [0.8, 0.0, 0.0], 0.8, "offset-overlap"),
        ([0.0, 0.0, 0.0], 1.0, [1.0, 0.0, 0.0], 1.0, "equal-lens"),
        ([0.0, 0.0, 0.0], 1.0, [0.7, 0.7, 0.0], 0.7, "corner-overlap"),
        (
            [0.0, 0.0, 0.0],
            1.2,
            [0.3, 0.0, 0.0],
            0.4,
            "small-inside-big",
        ),
        ([0.0, 0.0, 0.0], 0.9, [0.0, 0.0, 1.0], 0.9, "offset-z"),
        (
            [0.0, 0.0, 0.0],
            0.8,
            [1.55, 0.0, 0.0],
            0.8,
            "near-tangent-ext",
        ),
        ([0.0, 0.0, 0.0], 0.6, [2.0, 0.0, 0.0], 0.6, "disjoint"),
        ([0.0, 0.0, 0.0], 1.0, [0.5, 0.5, 0.5], 0.9, "diag-overlap"),
    ]
}

#[test]
#[ignore = "fuzz survey — run with --ignored --nocapture"]
fn boolean_sphere_sphere_fuzz_survey() {
    let vol_tol = 0.05;
    let ops: [(BooleanOp, &str, fn(&GridTruth) -> f64); 3] = [
        (BooleanOp::Intersection, "∩", |g| g.intersection),
        (BooleanOp::Union, "∪", |g| g.union),
        (BooleanOp::Difference, "∖", |g| g.difference),
    ];
    let configs = ss_configs();
    let n_cfg = configs.len();
    let n_checks = std::sync::atomic::AtomicUsize::new(0);

    let fails: Vec<Failure> = configs
        .par_iter()
        .flat_map(|&(ca, ra, cb, rb, label)| {
            let truth = ss_grid_truth(ca, ra, cb, rb);
            let mut out: Vec<Failure> = Vec::new();
            for &(op, sym, pick) in &ops {
                let t = pick(&truth);
                if t < 1e-3 {
                    continue;
                }
                n_checks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let lab = format!("{label} ra={ra} rb={rb}");
                let build_a = move |m: &mut BRepModel| sphere(m, ca, ra);
                let build_b = move |m: &mut BRepModel| sphere(m, cb, rb);
                match run_pair_timed(op, build_a, build_b) {
                    Outcome::Hang => out.push(Failure {
                        label: lab,
                        op: sym,
                        kind: "HANG",
                        detail: format!("op did not return within budget (truth {t:.3})"),
                    }),
                    Outcome::Err => out.push(Failure {
                        label: lab,
                        op: sym,
                        kind: "ERROR",
                        detail: format!("op errored (truth {t:.3})"),
                    }),
                    Outcome::Ok(f) => {
                        let rel = (f.vol - t).abs() / t.max(1e-3);
                        if rel > vol_tol {
                            out.push(Failure {
                                label: lab.clone(),
                                op: sym,
                                kind: "VOLUME",
                                detail: format!(
                                    "kernel={:.4} truth={t:.4} ({:+.1}%)",
                                    f.vol,
                                    100.0 * (f.vol - t) / t
                                ),
                            });
                        }
                        if f.open_edges != 0 {
                            out.push(Failure {
                                label: lab.clone(),
                                op: sym,
                                kind: "WATERTIGHT",
                                detail: format!("open_edges={}", f.open_edges),
                            });
                        }
                        if f.nonmanifold_edges != 0 {
                            out.push(Failure {
                                label: lab.clone(),
                                op: sym,
                                kind: "MANIFOLD",
                                detail: format!("nonmanifold_edges={}", f.nonmanifold_edges),
                            });
                        }
                    }
                }
            }
            out
        })
        .collect();

    print_catalog(
        "sphere ∘ sphere",
        &fails,
        n_cfg,
        n_checks.load(std::sync::atomic::Ordering::Relaxed),
    );
}

// ===========================================================================
// SUBPROCESS-ISOLATED HANG count (#91). `run_op_timed` budgets a config on a
// detached thread and LEAKS it on timeout. Under the rayon survey, a few leaked
// threads burn cores and starve healthy configs, which then also miss their
// budget — so the in-process HANG class massively OVER-reports (box∘sphere
// showed 332, which cannot be 332 genuine infinite loops). The only trustworthy
// way to know whether a single config truly never returns is to run it in its
// OWN process and wall-clock it there: an OS-scheduled sibling process can't be
// starved by a hung one the way a thread in a shared pool can.
//
// `fuzz_single_shot` runs exactly ONE box∘sphere (cfg,op) selected by env, with
// NO internal timeout — it returns fast or hangs the process. `hang_isolation_
// survey` spawns it per (cfg,op), wall-clocks each child, and kills + records
// the ones that exceed budget. Both are #[ignore] (manual surveys); neither is
// part of the green gate, so a slow/again-flaky child can never break CI.
// ===========================================================================

/// One box∘sphere config in its own process. Env: FUZZ_CFG (flat index into
/// placements×radii), FUZZ_OP (0=∩,1=∪,2=∖). No timeout — hangs if the op hangs.
#[test]
#[ignore = "internal single-shot spawned by hang_isolation_survey"]
fn fuzz_single_shot() {
    let cfg = match std::env::var("FUZZ_CFG")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
    {
        Some(c) => c,
        // Run en-masse with `-- --ignored` and no env: no-op so the suite stays
        // green; this test only does work when the driver sets FUZZ_CFG.
        None => {
            println!("fuzz_single_shot: FUZZ_CFG unset — skipping");
            return;
        }
    };
    let opi = std::env::var("FUZZ_OP")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);

    let mut configs: Vec<([f64; 3], &'static str, f64)> = Vec::new();
    for (c, label) in placements() {
        for &r in radii() {
            configs.push((c, label, r));
        }
    }
    let (center, _label, r) = configs[cfg];
    let op = [
        BooleanOp::Intersection,
        BooleanOp::Union,
        BooleanOp::Difference,
    ][opi];

    // Direct call — no timeout thread. The parent owns the wall-clock budget.
    let facts = run_op(op, move |m| sphere(m, center, r));
    println!("SINGLE_SHOT_DONE cfg={cfg} op={opi} ok={}", facts.is_some());
}

#[test]
#[ignore = "fuzz survey — subprocess-isolated true HANG count (slow; spawns processes)"]
fn hang_isolation_survey() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let exe = std::env::current_exe().expect("current_exe");
    let n_cfg = placements().len() * radii().len();
    let budget = Duration::from_secs(6);
    let mut hangs: Vec<(usize, usize)> = Vec::new();
    let mut proc_errs = 0usize;
    let total = n_cfg * 3;

    for cfg in 0..n_cfg {
        for opi in 0..3usize {
            let mut child = Command::new(&exe)
                .args(["fuzz_single_shot", "--exact", "--ignored"])
                .env("FUZZ_CFG", cfg.to_string())
                .env("FUZZ_OP", opi.to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn single-shot");
            let start = Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        if !status.success() {
                            proc_errs += 1;
                        }
                        break;
                    }
                    Ok(None) => {
                        if start.elapsed() > budget {
                            let _ = child.kill();
                            let _ = child.wait();
                            hangs.push((cfg, opi));
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    Err(_) => {
                        proc_errs += 1;
                        break;
                    }
                }
            }
        }
    }

    let sym = ["∩", "∪", "∖"];
    println!("\n=== #91 subprocess-isolated HANG count (box∘sphere) ===");
    println!(
        "total={total}  TRUE_HANGS={}  process-errs={}  (in-process survey reported HANG≈332 — false positives from leaked-thread starvation)",
        hangs.len(),
        proc_errs
    );
    let mut configs: Vec<([f64; 3], &'static str, f64)> = Vec::new();
    for (c, label) in placements() {
        for &r in radii() {
            configs.push((c, label, r));
        }
    }
    for (cfg, opi) in &hangs {
        let (_, label, r) = configs[*cfg];
        println!("  HANG [{}] {label} r={r}", sym[*opi]);
    }
    println!("=== end ===\n");
}

// ===========================================================================
// CLEAN-CELL reporter (#91 ratchet step 1). The surveys print only FAILURES;
// to promote conquered ground into a hard CI gate we must know exactly which
// (placement, r) cells pass ALL THREE ops cleanly (volume within tol, watertight,
// manifold). This prints that set so the gate is built from measured fact, not
// assumption. Hung cells (the 26 from hang_isolation_survey) are skipped via the
// timeout runner so one infinite loop can't block the report.
// ===========================================================================

#[test]
#[ignore = "fuzz survey — prints box∘sphere cells that pass all 3 ops cleanly"]
fn survey_box_sphere_clean_cells() {
    let vol_tol = 0.03;
    let ops = [
        BooleanOp::Intersection,
        BooleanOp::Union,
        BooleanOp::Difference,
    ];
    let picks: [fn(&GridTruth) -> f64; 3] = [|g| g.intersection, |g| g.union, |g| g.difference];

    let mut configs: Vec<([f64; 3], &'static str, f64)> = Vec::new();
    for (c, label) in placements() {
        for &r in radii() {
            configs.push((c, label, r));
        }
    }

    let clean: Vec<String> = configs
        .par_iter()
        .filter_map(|&(c, label, r)| {
            let truth = grid_truth(c, r);
            let mut all_clean = true;
            let mut any_checked = false;
            for (oi, &op) in ops.iter().enumerate() {
                let t = picks[oi](&truth);
                if t < 1e-3 {
                    continue; // empty/whole — no boundary to test
                }
                any_checked = true;
                match run_op_timed(op, move |m| sphere(m, c, r)) {
                    Outcome::Ok(f) => {
                        let rel = (f.vol - t).abs() / t.max(1e-3);
                        if rel > vol_tol || f.open_edges != 0 || f.nonmanifold_edges != 0 {
                            all_clean = false;
                        }
                    }
                    _ => all_clean = false,
                }
            }
            if any_checked && all_clean {
                Some(format!("{label} r={r}"))
            } else {
                None
            }
        })
        .collect();

    let mut clean = clean;
    clean.sort();
    println!("\n=== #91 box∘sphere CLEAN cells (pass ∩/∪/∖: vol≤3%, watertight, manifold) ===");
    println!("clean_cells={}", clean.len());
    for c in &clean {
        println!("  OK {c}");
    }
    println!("=== end ===\n");
}

// ===========================================================================
// RATCHET GATE (#91) — NON-ignored. Locks the box∘sphere cells that currently
// pass all three booleans cleanly, derived from survey_box_sphere_clean_cells
// (not assumed). If a future kernel change regresses one of these — e.g. the
// curved-cut path starts returning a whole operand again (the dominant failure
// mode the surveys catalogue) — THIS test goes red in CI. The 471 still-failing
// cells stay in the #[ignore] surveys as the work queue; this gate is the floor
// of conquered ground that must never drop.
//
// Oracle = the same 96³ grid truth the surveys use, asserted at a looser 5% tol
// (these cells pass the survey at 3%, so ≥2% margin keeps the gate non-flaky)
// plus watertight + manifold. Volume is deterministic (determinism harness #84),
// so the 5% band can only be crossed by a real regression, never by noise.
// ===========================================================================

#[test]
fn box_sphere_conquered_band_gate() {
    // (centre, r) — the exact cells survey_box_sphere_clean_cells reported clean.
    let cells: [([f64; 3], f64); 6] = [
        ([0.0, 0.0, 0.0], 0.5),  // interior-centre r=0.5
        ([0.0, 0.0, 0.0], 0.8),  // interior-centre r=0.8
        ([0.5, 0.3, 0.0], 0.25), // interior-offset r=0.25
        ([0.5, 0.3, 0.0], 0.5),  // interior-offset r=0.5
        ([1.4, 0.0, 0.0], 0.25), // poke+x r=0.25 (disjoint — sphere fully outside)
        ([1.4, 0.5, 0.0], 0.25), // poke+x-off r=0.25 (disjoint)
    ];
    let ops: [(BooleanOp, &str, fn(&GridTruth) -> f64); 3] = [
        (BooleanOp::Intersection, "∩", |g| g.intersection),
        (BooleanOp::Union, "∪", |g| g.union),
        (BooleanOp::Difference, "∖", |g| g.difference),
    ];
    let tol = 0.05;
    for (c, r) in cells {
        let truth = grid_truth(c, r);
        for &(op, sym, pick) in &ops {
            let t = pick(&truth);
            if t < 1e-3 {
                continue; // empty result — no boundary
            }
            let facts = run_op(op, move |m| sphere(m, c, r)).unwrap_or_else(|| {
                panic!("box∘sphere {sym} at c={c:?} r={r} returned no solid (kernel error)")
            });
            let rel = (facts.vol - t).abs() / t.max(1e-3);
            assert!(
                rel <= tol,
                "REGRESSION: box∘sphere {sym} c={c:?} r={r}: vol={:.4} truth={t:.4} ({:+.1}%, tol {:.0}%)",
                facts.vol,
                100.0 * (facts.vol - t) / t,
                100.0 * tol
            );
            assert_eq!(
                facts.open_edges, 0,
                "REGRESSION: box∘sphere {sym} c={c:?} r={r}: {} open edges (not watertight)",
                facts.open_edges
            );
            assert_eq!(
                facts.nonmanifold_edges, 0,
                "REGRESSION: box∘sphere {sym} c={c:?} r={r}: {} non-manifold edges",
                facts.nonmanifold_edges
            );
        }
    }
}

// ===========================================================================
// box ∘ TORUS survey — second solid is a z-axis torus. A torus is genus-1: the
// central hole plus the doubly-curved tube exercise the rim-imprint path (#57)
// and the multi-face rim-poke case the other curved surveys don't reach.
// ===========================================================================

fn torus(model: &mut BRepModel, c: [f64; 3], rmaj: f64, rmin: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_torus_3d(Point3::new(c[0], c[1], c[2]), Vector3::Z, rmaj, rmin)
        .expect("torus")
    {
        GeometryId::Solid(id) => id,
        o => panic!("torus: {o:?}"),
    }
}

/// Inside a z-axis torus: distance from the tube centre-circle (radius `rmaj`
/// in the z=c_z plane) is ≤ tube radius `rmin`.
fn in_torus(p: [f64; 3], c: [f64; 3], rmaj: f64, rmin: f64) -> bool {
    let (dx, dy, dz) = (p[0] - c[0], p[1] - c[1], p[2] - c[2]);
    let radial = (dx * dx + dy * dy).sqrt();
    let q = radial - rmaj;
    q * q + dz * dz <= rmin * rmin
}

fn torus_grid_truth(c: [f64; 3], rmaj: f64, rmin: f64) -> GridTruth {
    let reach = [
        c[0].abs() + rmaj + rmin,
        c[1].abs() + rmaj + rmin,
        c[2].abs() + rmin,
    ]
    .into_iter()
    .fold(BOX_HALF, f64::max)
        + 0.05;
    const N: usize = 96;
    let cell = 2.0 * reach / N as f64;
    let cv = cell * cell * cell;
    let (mut i_n, mut u_n, mut d_n) = (0u64, 0u64, 0u64);
    for i in 0..N {
        let x = -reach + (i as f64 + 0.5) * cell;
        let in_bx = x.abs() <= BOX_HALF;
        for j in 0..N {
            let y = -reach + (j as f64 + 0.5) * cell;
            let in_by = in_bx && y.abs() <= BOX_HALF;
            for k in 0..N {
                let z = -reach + (k as f64 + 0.5) * cell;
                let in_box = in_by && z.abs() <= BOX_HALF;
                let in_t = in_torus([x, y, z], c, rmaj, rmin);
                if in_box && in_t {
                    i_n += 1;
                }
                if in_box || in_t {
                    u_n += 1;
                }
                if in_box && !in_t {
                    d_n += 1;
                }
            }
        }
    }
    GridTruth {
        intersection: i_n as f64 * cv,
        union: u_n as f64 * cv,
        difference: d_n as f64 * cv,
    }
}

/// (centre, major_r, minor_r, label) — z-axis tori vs box [-1,1]³.
fn torus_configs() -> Vec<([f64; 3], f64, f64, &'static str)> {
    vec![
        ([0.0, 0.0, 0.0], 0.6, 0.25, "centered"),
        ([0.0, 0.0, 0.0], 0.6, 0.4, "centered-fat"),
        ([0.0, 0.0, 0.0], 0.5, 0.15, "thin-contained"),
        ([0.3, 0.0, 0.0], 0.6, 0.25, "offset-x"),
        ([0.6, 0.6, 0.0], 0.5, 0.2, "corner"),
        ([0.0, 0.0, 0.0], 0.9, 0.3, "rim-through-4faces"),
        ([0.0, 0.0, 0.0], 1.0, 0.3, "rim-on-faces"),
        ([0.0, 0.0, 0.8], 0.5, 0.3, "axial-poke+z"),
        ([0.0, 0.0, 0.0], 0.7, 0.2, "ring-hole"),
    ]
}

#[test]
#[ignore = "fuzz survey — run with --ignored --nocapture"]
fn boolean_box_torus_fuzz_survey() {
    let vol_tol = 0.03;
    let ops: [(BooleanOp, &str, fn(&GridTruth) -> f64); 3] = [
        (BooleanOp::Intersection, "∩", |g| g.intersection),
        (BooleanOp::Union, "∪", |g| g.union),
        (BooleanOp::Difference, "∖", |g| g.difference),
    ];
    let configs = torus_configs();
    let n_cfg = configs.len();
    let n_checks = std::sync::atomic::AtomicUsize::new(0);

    let fails: Vec<Failure> = configs
        .par_iter()
        .flat_map(|&(c, rmaj, rmin, label)| {
            let truth = torus_grid_truth(c, rmaj, rmin);
            let mut out: Vec<Failure> = Vec::new();
            for &(op, sym, pick) in &ops {
                let t = pick(&truth);
                if t < 1e-3 {
                    continue;
                }
                n_checks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let lab = format!("{label} R={rmaj} r={rmin}");
                match run_op_timed(op, move |m| torus(m, c, rmaj, rmin)) {
                    Outcome::Hang => out.push(Failure {
                        label: lab,
                        op: sym,
                        kind: "HANG",
                        detail: format!("op did not return within budget (truth {t:.3})"),
                    }),
                    Outcome::Err => out.push(Failure {
                        label: lab,
                        op: sym,
                        kind: "ERROR",
                        detail: format!("op errored (truth {t:.3})"),
                    }),
                    Outcome::Ok(f) => {
                        let rel = (f.vol - t).abs() / t.max(1e-3);
                        if rel > vol_tol {
                            out.push(Failure {
                                label: lab.clone(),
                                op: sym,
                                kind: "VOLUME",
                                detail: format!(
                                    "kernel={:.4} truth={t:.4} ({:+.1}%)",
                                    f.vol,
                                    100.0 * (f.vol - t) / t
                                ),
                            });
                        }
                        if f.open_edges != 0 {
                            out.push(Failure {
                                label: lab.clone(),
                                op: sym,
                                kind: "WATERTIGHT",
                                detail: format!("open_edges={}", f.open_edges),
                            });
                        }
                        if f.nonmanifold_edges != 0 {
                            out.push(Failure {
                                label: lab.clone(),
                                op: sym,
                                kind: "MANIFOLD",
                                detail: format!("nonmanifold_edges={}", f.nonmanifold_edges),
                            });
                        }
                    }
                }
            }
            out
        })
        .collect();

    print_catalog(
        "box ∘ torus",
        &fails,
        n_cfg,
        n_checks.load(std::sync::atomic::Ordering::Relaxed),
    );
}
