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
use geometry_engine::math::Point3;
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

/// Run one box∘sphere boolean fresh; `None` on kernel error.
fn run_op(op: BooleanOp, c: [f64; 3], r: f64) -> Option<Facts> {
    let mut model = BRepModel::new();
    let bx = the_box(&mut model);
    let sp = sphere(&mut model, c, r);
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
fn run_op_timed(op: BooleanOp, c: [f64; 3], r: f64) -> Outcome {
    const OP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run_op(op, c, r));
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
                match run_op_timed(op, c, r) {
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

    // HARD = trustworthy classes that flag a real Boolean bug. SOFT = classes
    // that over-report and must be verified in isolation before acting:
    //   HANG  — leaked-thread core-burn under rayon → false timeouts.
    //   EULER — the UV-sphere primitive carries an intrinsic genus-0 residual of
    //           -1 (periodic seam + 2 poles), so ANY sphere-bearing result fails
    //           the residual==0 check without a real bug (confirmed by
    //           survey_euler_baseline: bare sphere = -1, bare box = 0).
    let is_soft = |k: &str| k == "HANG" || k == "EULER";

    use std::collections::BTreeMap;
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for f in &fails {
        *by_kind.entry(f.kind).or_default() += 1;
    }
    let hard: usize = fails.iter().filter(|f| !is_soft(f.kind)).count();
    let soft: usize = fails.len() - hard;

    println!("\n========== BOOLEAN FUZZ SURVEY: box ∘ sphere ==========");
    println!(
        "configs={n_cfg}  checks={}  HARD failures={hard}  (soft={soft})",
        n_checks.load(std::sync::atomic::Ordering::Relaxed),
    );
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
