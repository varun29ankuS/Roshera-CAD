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
