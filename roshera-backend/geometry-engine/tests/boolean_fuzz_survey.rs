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
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions, OperationError};
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
    // Box and sphere volumes are EXACT analytically; only their INTERSECTION is
    // Monte-Carlo'd. Gridding the whole box+sphere over a sphere-sized `reach`
    // biased the BOX estimate: a symmetric grid whose cell size rarely divides
    // the 2.0 box edge over-counts the box by up to ~4.5% (e.g. poke+x r=0.5
    // reported a UNION truth of 8.886, exceeding the true maximum box+sphere =
    // 8.524, false-flagging a CORRECT kernel result). Estimating only the
    // compact box∩sphere overlap keeps the discretised quantity small against
    // the exact box (8.0), so union/difference land well inside the survey's 3%.
    let box_vol = (2.0 * BOX_HALF).powi(3);
    let sphere_vol = 4.0 / 3.0 * std::f64::consts::PI * r * r * r;
    let intersection = box_sphere_intersection_grid(center, r, 96);
    GridTruth {
        intersection,
        union: box_vol + sphere_vol - intersection,
        difference: box_vol - intersection,
    }
}

/// Monte-Carlo estimate of V(box ∩ sphere) with `n` samples/axis over the
/// box∩sphere overlap AABB (every sample is in-box by construction, so only the
/// sphere test matters). Exposed for the convergence self-test
/// (`grid_oracle_converges`): a trustworthy oracle's estimate must stabilise as
/// `n` doubles; a drifting estimate would mean the truth it feeds the survey is
/// itself unreliable.
fn box_sphere_intersection_grid(center: [f64; 3], r: f64, n: usize) -> f64 {
    let mut lo = [0.0_f64; 3];
    let mut hi = [0.0_f64; 3];
    for a in 0..3 {
        lo[a] = (center[a] - r).max(-BOX_HALF);
        hi[a] = (center[a] + r).min(BOX_HALF);
    }
    if (0..3).any(|a| hi[a] <= lo[a]) {
        return 0.0; // disjoint on some axis
    }
    let cell = [
        (hi[0] - lo[0]) / n as f64,
        (hi[1] - lo[1]) / n as f64,
        (hi[2] - lo[2]) / n as f64,
    ];
    let cv = cell[0] * cell[1] * cell[2];
    let r2 = r * r;
    let mut count = 0u64;
    for i in 0..n {
        let x = lo[0] + (i as f64 + 0.5) * cell[0];
        for j in 0..n {
            let y = lo[1] + (j as f64 + 0.5) * cell[1];
            for k in 0..n {
                let z = lo[2] + (k as f64 + 0.5) * cell[2];
                let (dx, dy, dz) = (x - center[0], y - center[1], z - center[2]);
                if dx * dx + dy * dy + dz * dz <= r2 {
                    count += 1;
                }
            }
        }
    }
    count as f64 * cv
}

// ===========================================================================
// ORACLE SELF-TEST (non-ignored gate). The survey trusts `grid_truth`; this
// proves the trust is earned. For three configs with a CLOSED-FORM box∩sphere
// volume, the grid estimate must (a) land within 1% of the exact value at the
// production resolution and (b) CONVERGE — doubling samples/axis must not move
// it away from exact. A drifting or biased oracle would silently corrupt every
// VOLUME verdict in the survey; this catches that at the source.
// ===========================================================================
#[test]
fn grid_oracle_converges() {
    let pi = std::f64::consts::PI;
    // (centre, r, exact V(box∩sphere), name)
    let cases: [([f64; 3], f64, f64, &str); 3] = [
        // sphere fully inside the box → ∩ = the whole sphere
        (
            [0.0, 0.0, 0.0],
            0.5,
            4.0 / 3.0 * pi * 0.5_f64.powi(3),
            "contained (full sphere)",
        ),
        // centre ON the +x face plane → ∩ = inside-box hemisphere = half sphere
        (
            [1.0, 0.0, 0.0],
            0.5,
            2.0 / 3.0 * pi * 0.5_f64.powi(3),
            "face (hemisphere)",
        ),
        // centre ON the +++ corner vertex → ∩ = one octant = 1/8 sphere
        (
            [1.0, 1.0, 1.0],
            0.8,
            (4.0 / 3.0 * pi * 0.8_f64.powi(3)) / 8.0,
            "corner (octant)",
        ),
    ];
    for (c, r, exact, name) in cases {
        let e48 = box_sphere_intersection_grid(c, r, 48);
        let e96 = box_sphere_intersection_grid(c, r, 96);
        let e192 = box_sphere_intersection_grid(c, r, 192);
        // (a) accuracy at production resolution.
        let rel96 = (e96 - exact).abs() / exact;
        assert!(
            rel96 <= 0.01,
            "oracle inaccurate for {name}: grid(96)={e96:.5} exact={exact:.5} ({:.2}%)",
            100.0 * rel96
        );
        // (b) convergence: the finest grid is at least as close to exact as the
        // coarsest (a 1e-9 slack absorbs floating-point noise at the boundary).
        let d48 = (e48 - exact).abs();
        let d192 = (e192 - exact).abs();
        assert!(
            d192 <= d48 + 1e-9,
            "oracle NOT converging for {name}: |grid(48)-exact|={d48:.6} |grid(192)-exact|={d192:.6}"
        );
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
    let res = match boolean_operation(&mut model, bx, sp, op, BooleanOptions::default()) {
        Ok(res) => res,
        // A typed empty result is the volume-0 solid (A∖B with A⊆B, or a disjoint
        // A∩B). Report it as such so the VOLUME oracle ACCEPTS it when truth≈0
        // (engulf / disjoint — legitimately empty) and still flags it HARD when
        // truth>0 (faces wrongly dropped, e.g. the tangent A∖B bug). This keeps
        // real bugs loud while no longer counting a correct empty as a failure.
        Err(OperationError::EmptyResult) => {
            return Some(Facts {
                vol: 0.0,
                open_edges: 0,
                nonmanifold_edges: 0,
                euler_residual: 0,
            })
        }
        Err(_) => return None,
    };
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

/// Oracle-free Boolean invariants from the kernel's OWN (∩, ∪, ∖) volumes.
/// `v_a` = box (8.0), `v_b` = the second solid's analytic volume. These are
/// exact algebraic facts for ANY valid Boolean, so a violation is a guaranteed
/// kernel bug — no grid oracle is consulted, making them immune to oracle error
/// and sharper than a volume-vs-oracle tolerance band. One `Failure` per
/// violated invariant; empty when any op did not return (HANG/ERROR).
fn boolean_invariant_failures(
    lab: &str,
    v_a: f64,
    v_b: f64,
    kvol: [Option<f64>; 3],
) -> Vec<Failure> {
    let mut out: Vec<Failure> = Vec::new();
    let [Some(vi), Some(vu), Some(vd)] = kvol else {
        return out;
    };
    // Inclusion–exclusion: V(A∩B) + V(A∪B) = V(A) + V(B), exactly. Loosened to
    // 5% only to absorb the curved-cap tessellation discretisation shared by ∩
    // and ∪; a real petal-drop breaks it by 20–90%.
    let ie_rhs = v_a + v_b;
    if (vi + vu - ie_rhs).abs() / ie_rhs > 0.05 {
        out.push(Failure {
            label: lab.to_string(),
            op: "∩∪",
            kind: "INCL-EXCL",
            detail: format!(
                "V(∩)+V(∪)={:.4} ≠ V(A)+V(B)={:.4} ({:+.1}%)",
                vi + vu,
                ie_rhs,
                100.0 * (vi + vu - ie_rhs) / ie_rhs
            ),
        });
    }
    // Difference identity: V(A∖B) = V(A) − V(A∩B).
    if (vd - (v_a - vi)).abs() / v_a > 0.03 {
        out.push(Failure {
            label: lab.to_string(),
            op: "∖",
            kind: "DIFF-ID",
            detail: format!("V(∖)={vd:.4} ≠ V(A)−V(∩)={:.4}", v_a - vi),
        });
    }
    // Hard bounds — inequalities that cannot false-positive from small
    // discretisation unless a result is grossly wrong.
    let eps = 0.02 * v_a;
    if vi < -eps || vi > v_a.min(v_b) + eps {
        out.push(Failure {
            label: lab.to_string(),
            op: "∩",
            kind: "BOUNDS",
            detail: format!("V(∩)={vi:.4} ∉ [0, min(A,B)={:.4}]", v_a.min(v_b)),
        });
    }
    if vu < v_a.max(v_b) - eps || vu > v_a + v_b + eps {
        out.push(Failure {
            label: lab.to_string(),
            op: "∪",
            kind: "BOUNDS",
            detail: format!(
                "V(∪)={vu:.4} ∉ [max(A,B)={:.4}, A+B={:.4}]",
                v_a.max(v_b),
                v_a + v_b
            ),
        });
    }
    if vd < -eps || vd > v_a + eps {
        out.push(Failure {
            label: lab.to_string(),
            op: "∖",
            kind: "BOUNDS",
            detail: format!("V(∖)={vd:.4} ∉ [0, A={v_a:.4}]"),
        });
    }
    out
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

    let v_box = (2.0 * BOX_HALF).powi(3);
    let n_checks = std::sync::atomic::AtomicUsize::new(0);
    let fails: Vec<Failure> = configs
        .par_iter()
        .flat_map(|&(c, label, r)| {
            let truth = grid_truth(c, r);
            let v_sph = 4.0 / 3.0 * std::f64::consts::PI * r * r * r;
            let lab = format!("{label} r={r}");
            let mut out: Vec<Failure> = Vec::new();
            // Kernel volumes per op (∩, ∪, ∖), in `ops` order — for the
            // oracle-free cross-op invariants below.
            let mut kvol: [Option<f64>; 3] = [None; 3];
            for (idx, &(op, sym, pick)) in ops.iter().enumerate() {
                let t = pick(&truth);
                n_checks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                match run_op_timed(op, move |m| sphere(m, c, r)) {
                    Outcome::Hang => out.push(Failure {
                        label: lab.clone(),
                        op: sym,
                        kind: "HANG",
                        detail: format!("op did not return within budget (truth {t:.3})"),
                    }),
                    Outcome::Err => out.push(Failure {
                        label: lab.clone(),
                        op: sym,
                        kind: "ERROR",
                        detail: format!("op errored (truth {t:.3})"),
                    }),
                    Outcome::Ok(f) => {
                        kvol[idx] = Some(f.vol);
                        // Volume-vs-oracle only when the result has a boundary;
                        // an empty/whole region (t≈0) carries no volume signal.
                        if t >= 1e-3 {
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

            // ─── ORACLE-FREE INVARIANTS (kernel's own three volumes) ───
            // These need NO grid: they are exact algebraic facts about ANY
            // valid Boolean. They catch wrong-volume results the grid oracle's
            // tolerance might tolerate, and are immune to oracle error.
            out.extend(boolean_invariant_failures(&lab, v_box, v_sph, kvol));
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
    let is_soft = |k: &str| k == "HANG" || k == "EULER" || k == "SLOW";
    use std::collections::BTreeMap;
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for f in fails {
        *by_kind.entry(f.kind).or_default() += 1;
    }
    let hard: usize = fails.iter().filter(|f| !is_soft(f.kind)).count();
    let soft = fails.len() - hard;
    println!("\n========== BOOLEAN FUZZ SURVEY: {title} ==========");
    println!("configs={n_cfg}  checks={n_checks}  HARD failures={hard}  (soft={soft})");
    println!(
        "by kind: {by_kind:?}   [HARD: VOLUME WATERTIGHT MANIFOLD ERROR | soft: HANG SLOW EULER]"
    );
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
        // HANG and SLOW identities ARE the work queue (#91 true-hang
        // investigation, L4 profiling targets) — print them; EULER stays
        // count-only (seam baseline noise).
        if *kind == "HANG" || *kind == "SLOW" {
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
/// Honest box∘shape grid truth: box (8.0) and the shape (`shape_vol`) are exact;
/// only their intersection is Monte-Carlo'd over the box∩shape-AABB overlap
/// (clipped to the box, so every sample is in-box — only `in_shape` is tested).
/// Eliminates the box-over-count bias of gridding the whole box+shape over a
/// shape-sized reach (see `grid_truth` for the full rationale). `shape_lo/hi` is
/// any AABB that CONTAINS the shape; a loose one only adds zero-weight samples.
fn box_shape_grid_truth(
    shape_vol: f64,
    shape_lo: [f64; 3],
    shape_hi: [f64; 3],
    in_shape: impl Fn([f64; 3]) -> bool,
) -> GridTruth {
    let box_vol = (2.0 * BOX_HALF).powi(3);
    let mut lo = [0.0_f64; 3];
    let mut hi = [0.0_f64; 3];
    for a in 0..3 {
        lo[a] = shape_lo[a].max(-BOX_HALF);
        hi[a] = shape_hi[a].min(BOX_HALF);
    }
    let intersection = if (0..3).any(|a| hi[a] <= lo[a]) {
        0.0
    } else {
        const N: usize = 96;
        let cell = [
            (hi[0] - lo[0]) / N as f64,
            (hi[1] - lo[1]) / N as f64,
            (hi[2] - lo[2]) / N as f64,
        ];
        let cv = cell[0] * cell[1] * cell[2];
        let mut count = 0u64;
        for i in 0..N {
            let x = lo[0] + (i as f64 + 0.5) * cell[0];
            for j in 0..N {
                let y = lo[1] + (j as f64 + 0.5) * cell[1];
                for k in 0..N {
                    let z = lo[2] + (k as f64 + 0.5) * cell[2];
                    if in_shape([x, y, z]) {
                        count += 1;
                    }
                }
            }
        }
        count as f64 * cv
    };
    GridTruth {
        intersection,
        union: box_vol + shape_vol - intersection,
        difference: box_vol - intersection,
    }
}

fn in_cylinder(p: [f64; 3], base: [f64; 3], r: f64, h: f64) -> bool {
    let axial = p[2] - base[2];
    if axial < 0.0 || axial > h {
        return false;
    }
    let (dx, dy) = (p[0] - base[0], p[1] - base[1]);
    dx * dx + dy * dy <= r * r
}

fn cyl_grid_truth(base: [f64; 3], r: f64, h: f64) -> GridTruth {
    // Finite z-axis cylinder → V = π r² h exact; AABB = disk±r in xy, [base_z, +h].
    let vol = std::f64::consts::PI * r * r * h;
    let lo = [base[0] - r, base[1] - r, base[2].min(base[2] + h)];
    let hi = [base[0] + r, base[1] + r, base[2].max(base[2] + h)];
    box_shape_grid_truth(vol, lo, hi, |p| in_cylinder(p, base, r, h))
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
    // Truncated z-cone → V = (1/3)π h (rb²+rb·rt+rt²) exact; AABB = max-radius
    // disk in xy over [bc_z, bc_z+h].
    let rmax = rb.max(rt);
    let vol = std::f64::consts::PI * h * (rb * rb + rb * rt + rt * rt) / 3.0;
    let lo = [bc[0] - rmax, bc[1] - rmax, bc[2].min(bc[2] + h)];
    let hi = [bc[0] + rmax, bc[1] + rmax, bc[2].max(bc[2] + h)];
    box_shape_grid_truth(vol, lo, hi, |p| in_cone(p, bc, rb, rt, h))
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

/// PIN (task #1): the cone-radial conic-cut worst class — the deepest open
/// boolean-core defect. A z-axis cone whose base is shifted to x=1.0 so its
/// slanted LATERAL surface pierces the box's +X wall (plane x=1). A cone
/// lateral × plane intersection is a CONIC — here a HYPERBOLA (cutting plane
/// parallel to the cone axis) — not a circle or a line. The split/classify/
/// stitch pipeline cannot form that hyperbolic boundary curve, so the conic
/// patch is dropped or mis-stitched.
///
/// Characterized failure signature (2026-06-15, ROSHERA_BOOL_TRACE):
///   ∩ → InvalidBRep "component 0 has only 3 planar faces" (the hyperbolic
///       conic patch is dropped entirely → can't close a manifold).
///   ∪ → vol −1.2%, open=6, nonmanifold=2, odd Euler (3 faces share the conic
///       boundary edge + boundary gaps).
///   ∖ → vol −1.3%, open=8, odd Euler (gaps along the conic cut).
///
/// This asserts the CORRECT outcome (watertight + valid + volume within tol);
/// it FAILS today, so it is #[ignore]'d. Flip on when #1 (analytic cone×plane
/// conic SSI + conic-patch stitching, ties #7) lands. Run the live signature:
///   `cargo test -p geometry-engine --test boolean_fuzz_survey \
///        cone_radial_conic_cut_pin_1 -- --ignored --nocapture`
#[test]
#[ignore = "task #1: cone-radial conic-cut (hyperbola) not yet stitched — flip on when #1 lands"]
fn cone_radial_conic_cut_pin_1() {
    use geometry_engine::math::Tolerance;
    use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};

    let (bc, rb, rt, h) = ([1.0, 0.0, -0.5], 0.5, 0.3, 1.0); // "radial-face+x"
    let truth = cone_grid_truth(bc, rb, rt, h);
    let ops: [(BooleanOp, &str, f64); 3] = [
        (BooleanOp::Intersection, "∩", truth.intersection),
        (BooleanOp::Union, "∪", truth.union),
        (BooleanOp::Difference, "∖", truth.difference),
    ];
    eprintln!(
        "[cone-radial #1] cone base={bc:?} rb={rb} rt={rt} h={h} (lateral pierces plane x=1 → hyperbola)"
    );
    let mut failures = Vec::new();
    for (op, sym, t) in ops {
        let mut model = BRepModel::new();
        let bx = the_box(&mut model);
        let cn = cone(&mut model, bc, rb, rt, h);
        match boolean_operation(&mut model, bx, cn, op, BooleanOptions::default()) {
            Ok(res) => {
                let vol = model.calculate_solid_volume(res).unwrap_or(f64::NAN);
                let rep = brep_integrity(&model, res, 1e-6);
                let v = validate_solid_scoped(
                    &model,
                    res,
                    Tolerance::default(),
                    ValidationLevel::Standard,
                );
                let (open, nm) = (rep.edges_used_once.len(), rep.edges_used_3plus.len());
                let rel = (vol - t).abs() / t.max(1e-9);
                eprintln!(
                    "  {sym}: vol={vol:.4} truth={t:.4} ({:+.1}%)  open={open} nonmanifold={nm}  valid={}",
                    100.0 * (vol - t) / t.max(1e-9),
                    v.is_valid,
                );
                if !v.is_valid {
                    eprintln!("      B-Rep errors: {:?}", v.errors);
                }
                if open != 0 || nm != 0 {
                    failures.push(format!("{sym}: open={open} nonmanifold={nm}"));
                }
                if !v.is_valid {
                    failures.push(format!("{sym}: invalid B-Rep"));
                }
                if rel > 0.03 {
                    failures.push(format!("{sym}: vol {vol:.4} vs truth {t:.4}"));
                }
            }
            Err(e) => {
                eprintln!("  {sym}: ERROR {e:?} (truth {t:.4})");
                failures.push(format!("{sym}: ERROR {e:?}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "cone-radial conic-cut (#1) still broken: {failures:?}"
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
    // Rotated cube → V = (2·hb)³ exact (rotation-invariant). Conservative AABB
    // uses the cube circumradius hb·√3; loose-but-containing is fine (extra empty
    // samples weigh 0).
    let r = Matrix4::from_axis_angle(&Vector3::new(axis[0], axis[1], axis[2]), angle)
        .expect("axis-angle");
    let diag = hb * 3.0_f64.sqrt();
    let vol = (2.0 * hb).powi(3);
    let lo = [center[0] - diag, center[1] - diag, center[2] - diag];
    let hi = [center[0] + diag, center[1] + diag, center[2] + diag];
    box_shape_grid_truth(vol, lo, hi, |p| in_rotated_box(p, hb, center, &r))
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

/// One matrix config in its own process. Env: FUZZ_FAMILY (sphere|cyl|cone|
/// rbox|torus|tcyl|ss; default sphere — the original box∘sphere protocol),
/// FUZZ_CFG (index into that family's config table), FUZZ_OP (0=∩,1=∪,2=∖).
/// No timeout — hangs if the op hangs.
#[test]
#[ignore = "internal single-shot spawned by the isolated survey drivers"]
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
    let family = std::env::var("FUZZ_FAMILY").unwrap_or_else(|_| "sphere".to_string());
    let op = [
        BooleanOp::Intersection,
        BooleanOp::Union,
        BooleanOp::Difference,
    ][opi];

    // Direct call — no timeout thread. The parent owns the wall-clock budget.
    let facts = match run_matrix_cell(&family, cfg, op) {
        Some(f) => f,
        None => {
            println!("fuzz_single_shot: unknown family/cfg {family}/{cfg} — skipping");
            return;
        }
    };
    // When the driver passes FUZZ_OUT, write the full facts so the parent can
    // run the oracle + invariants on an ISOLATED result (no leaked-thread
    // starvation). Format: "OK <vol> <open> <nonmanifold> <euler>" or "ERR".
    if let Ok(path) = std::env::var("FUZZ_OUT") {
        let line = match facts {
            Some(f) => format!(
                "OK {} {} {} {}",
                f.vol, f.open_edges, f.nonmanifold_edges, f.euler_residual
            ),
            None => "ERR".to_string(),
        };
        let _ = std::fs::write(path, line);
    }
    println!(
        "SINGLE_SHOT_DONE family={family} cfg={cfg} op={opi} ok={}",
        facts.is_some()
    );
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
// HONEST (HANG-isolated) box∘sphere survey — the trustworthy variant.
//
// The in-process `boolean_box_sphere_fuzz_survey` runs configs under rayon and
// budgets each op on a LEAKED detached thread; a few true hangs burn cores and
// starve healthy configs, so their ops also miss the budget — inflating the HANG
// class AND masking the volume/invariant failures of the starved configs (a hung
// op yields no volume, so its checks are skipped). This driver runs every
// (cfg,op) in its OWN process: a hung child is KILLED (not leaked), so it cannot
// starve its siblings, and every healthy config's full facts are collected and
// checked. Same grid oracle + same oracle-free invariants, but no masking.
// Slow (spawns 3·|cfg| processes); #[ignore], never part of the green gate.
// ===========================================================================
#[test]
#[ignore = "fuzz survey — subprocess-isolated, HANG-honest (slow; spawns processes)"]
fn boolean_box_sphere_survey_isolated() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let exe = std::env::current_exe().expect("current_exe");
    let mut configs: Vec<([f64; 3], &'static str, f64)> = Vec::new();
    for (c, label) in placements() {
        for &r in radii() {
            configs.push((c, label, r));
        }
    }
    let budget = Duration::from_secs(6);
    let tmp = std::env::temp_dir();
    let v_box = (2.0 * BOX_HALF).powi(3);
    let vol_tol = 0.03;
    let sym = ["∩", "∪", "∖"];

    let mut fails: Vec<Failure> = Vec::new();
    let mut n_checks = 0usize;
    let mut true_hangs = 0usize;

    for (cfg, &(center, label, r)) in configs.iter().enumerate() {
        let truth = grid_truth(center, r);
        let v_sph = 4.0 / 3.0 * std::f64::consts::PI * r * r * r;
        let t_for = [truth.intersection, truth.union, truth.difference];
        let lab = format!("{label} r={r}");
        let mut kvol: [Option<f64>; 3] = [None; 3];

        for opi in 0..3usize {
            let out_path = tmp.join(format!("rosh_fuzz_{cfg}_{opi}.txt"));
            let _ = std::fs::remove_file(&out_path);
            let mut child = Command::new(&exe)
                .args(["fuzz_single_shot", "--exact", "--ignored"])
                .env("FUZZ_CFG", cfg.to_string())
                .env("FUZZ_OP", opi.to_string())
                .env("FUZZ_OUT", out_path.to_string_lossy().to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn single-shot");
            let start = Instant::now();
            // Two-tier timing (see isolated_matrix_survey): kill at 5× the
            // slow threshold so slow cells are MEASURED, not binned HANG.
            let kill_budget = budget * 5;
            let mut hung = false;
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => {
                        if start.elapsed() > kill_budget {
                            let _ = child.kill();
                            let _ = child.wait();
                            hung = true;
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => break,
                }
            }
            let elapsed = start.elapsed();
            n_checks += 1;
            if hung {
                true_hangs += 1;
                fails.push(Failure {
                    label: lab.clone(),
                    op: sym[opi],
                    kind: "HANG",
                    detail: format!(
                        "TRUE hang (isolated process, killed >{}s)",
                        kill_budget.as_secs()
                    ),
                });
                continue;
            }
            if elapsed > budget {
                fails.push(Failure {
                    label: lab.clone(),
                    op: sym[opi],
                    kind: "SLOW",
                    detail: format!(
                        "finished in {:.1}s (slow threshold {}s; incl. process spawn)",
                        elapsed.as_secs_f64(),
                        budget.as_secs()
                    ),
                });
            }
            let content = std::fs::read_to_string(&out_path).unwrap_or_default();
            let _ = std::fs::remove_file(&out_path);
            let parts: Vec<&str> = content.split_whitespace().collect();
            if parts.first() == Some(&"OK") && parts.len() >= 5 {
                let vol: f64 = parts[1].parse().unwrap_or(f64::NAN);
                let open: usize = parts[2].parse().unwrap_or(0);
                let nonman: usize = parts[3].parse().unwrap_or(0);
                let euler: i64 = parts[4].parse().unwrap_or(0);
                kvol[opi] = Some(vol);
                let t = t_for[opi];
                if t >= 1e-3 && (vol - t).abs() / t.max(1e-3) > vol_tol {
                    fails.push(Failure {
                        label: lab.clone(),
                        op: sym[opi],
                        kind: "VOLUME",
                        detail: format!(
                            "kernel={vol:.4} truth={t:.4} ({:+.1}%)",
                            100.0 * (vol - t) / t
                        ),
                    });
                }
                if open != 0 {
                    fails.push(Failure {
                        label: lab.clone(),
                        op: sym[opi],
                        kind: "WATERTIGHT",
                        detail: format!("open_edges={open}"),
                    });
                }
                if nonman != 0 {
                    fails.push(Failure {
                        label: lab.clone(),
                        op: sym[opi],
                        kind: "MANIFOLD",
                        detail: format!("nonmanifold_edges={nonman}"),
                    });
                }
                if euler != 0 {
                    fails.push(Failure {
                        label: lab.clone(),
                        op: sym[opi],
                        kind: "EULER",
                        detail: format!("euler_residual={euler}"),
                    });
                }
            } else {
                fails.push(Failure {
                    label: lab.clone(),
                    op: sym[opi],
                    kind: "ERROR",
                    detail: "op errored (isolated)".to_string(),
                });
            }
        }
        fails.extend(boolean_invariant_failures(&lab, v_box, v_sph, kvol));
    }

    print_catalog(
        "box ∘ sphere (subprocess-isolated)",
        &fails,
        configs.len(),
        n_checks,
    );
    println!("TRUE HANGS (isolated) = {true_hangs}  — vs the in-process survey's HANG≈330 (leaked-thread starvation)\n");
}

// ===========================================================================
// #91 GENERALIZATION — subprocess isolation for the WHOLE primitive matrix.
//
// The in-process pair surveys share `run_op_timed`/`run_pair_timed`'s
// leaked-thread weakness: every true hang burns a core for the rest of the
// run, starving healthy configs into false HANGs AND masking their volume /
// invariant failures (a hung op yields no volume, so its checks are skipped —
// the 2026-06-10 face-great-circle ∩ +35% bug hid under a HANG bin exactly
// this way; the 2026-06-10 re-run flagged 463/480 box∘sphere checks HANG on a
// loaded host while the serial gates were green). These drivers run every
// (config, op) in its own KILLED-on-timeout process: a hung child cannot
// starve siblings, and every healthy config's full facts are checked.
//
// Child protocol (`fuzz_single_shot`): FUZZ_FAMILY selects the config table,
// FUZZ_CFG the cell, FUZZ_OP the op, FUZZ_OUT the facts file. The parent
// (`isolated_matrix_survey`) owns the wall-clock budget and the oracle.
// `matrix_cells` (parent metadata) and `run_matrix_cell` (child build+run)
// MUST walk the same config fns in the same order — that shared iteration is
// what keeps the indices in sync. The box∘sphere driver above predates this
// machinery and is kept verbatim (same child protocol, FUZZ_FAMILY unset).
// Slow (spawns 3·|cfg| processes); all #[ignore], never part of the green gate.
//
// RUN THE DRIVERS ONE AT A TIME (or with --test-threads=1). A wall-clock
// budget is only meaningful when the child owns the machine: running several
// drivers as concurrent libtest threads multiplies child processes and the
// CPU contention pushed legitimate ~9s cells (cyl axial-through ∩, torus
// corner ∩ — both verified FINITE in solo runs) over the old 10s budget,
// recreating the false-HANG class subprocess isolation exists to kill.
// Budget is 30s for the same reason: TRUE hang must mean "not slow".
// ===========================================================================

/// `run_pair` with `run_op`'s EmptyResult honesty: a typed empty result is the
/// volume-0 solid, so the VOLUME oracle accepts it when truth≈0 (disjoint ∩,
/// engulfed ∖) and still flags it HARD when truth>0 (faces wrongly dropped).
fn run_pair_empty_ok<A, B>(op: BooleanOp, build_a: A, build_b: B) -> Option<Facts>
where
    A: Fn(&mut BRepModel) -> SolidId,
    B: Fn(&mut BRepModel) -> SolidId,
{
    let mut model = BRepModel::new();
    let a = build_a(&mut model);
    let b = build_b(&mut model);
    let res = match boolean_operation(&mut model, a, b, op, BooleanOptions::default()) {
        Ok(res) => res,
        Err(OperationError::EmptyResult) => {
            return Some(Facts {
                vol: 0.0,
                open_edges: 0,
                nonmanifold_edges: 0,
                euler_residual: 0,
            })
        }
        Err(_) => return None,
    };
    let vol = model.calculate_solid_volume(res)?;
    let rep = brep_integrity(&model, res, 1e-6);
    Some(Facts {
        vol,
        open_edges: rep.edges_used_once.len(),
        nonmanifold_edges: rep.edges_used_3plus.len(),
        euler_residual: rep.euler_poincare_genus0_residual(),
    })
}

/// Parent-side cell metadata for `family`: display label, grid truth, exact
/// operand volumes (for the oracle-free INCL-EXCL/BOUNDS/DIFF-ID invariants).
struct MatrixCell {
    label: String,
    truth: GridTruth,
    v_a: f64,
    v_b: f64,
}

fn matrix_cells(family: &str) -> Vec<MatrixCell> {
    use std::f64::consts::PI;
    let v_box = (2.0 * BOX_HALF).powi(3);
    match family {
        "sphere" => {
            let mut out = Vec::new();
            for (c, label) in placements() {
                for &r in radii() {
                    out.push(MatrixCell {
                        label: format!("{label} r={r}"),
                        truth: grid_truth(c, r),
                        v_a: v_box,
                        v_b: 4.0 / 3.0 * PI * r * r * r,
                    });
                }
            }
            out
        }
        "cyl" => cyl_configs()
            .into_iter()
            .map(|(base, r, h, label)| MatrixCell {
                label: format!("{label} r={r} h={h}"),
                truth: cyl_grid_truth(base, r, h),
                v_a: v_box,
                v_b: PI * r * r * h,
            })
            .collect(),
        "cone" => cone_configs()
            .into_iter()
            .map(|(bc, rb, rt, h, label)| MatrixCell {
                label: format!("{label} rb={rb} rt={rt} h={h}"),
                truth: cone_grid_truth(bc, rb, rt, h),
                v_a: v_box,
                v_b: PI * h * (rb * rb + rb * rt + rt * rt) / 3.0,
            })
            .collect(),
        "rbox" => rbox_configs()
            .into_iter()
            .map(|(hb, center, axis, angle_deg, label)| MatrixCell {
                label: format!("{label} hb={hb} {angle_deg}°"),
                truth: rbox_grid_truth(hb, center, axis, angle_deg.to_radians()),
                v_a: v_box,
                v_b: (2.0 * hb).powi(3),
            })
            .collect(),
        "torus" => torus_configs()
            .into_iter()
            .map(|(c, rmaj, rmin, label)| MatrixCell {
                label: format!("{label} R={rmaj} r={rmin}"),
                truth: torus_grid_truth(c, rmaj, rmin),
                v_a: v_box,
                v_b: 2.0 * PI * PI * rmaj * rmin * rmin,
            })
            .collect(),
        "tcyl" => tcyl_configs()
            .into_iter()
            .map(|(base, axis, r, h, label)| MatrixCell {
                label: format!("{label} r={r} h={h}"),
                truth: tcyl_grid_truth(base, axis, r, h),
                v_a: v_box,
                v_b: PI * r * r * h,
            })
            .collect(),
        "ss" => ss_configs()
            .into_iter()
            .map(|(ca, ra, cb, rb, label)| MatrixCell {
                label: format!("{label} ra={ra} rb={rb}"),
                truth: ss_grid_truth(ca, ra, cb, rb),
                v_a: 4.0 / 3.0 * PI * ra * ra * ra,
                v_b: 4.0 / 3.0 * PI * rb * rb * rb,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Child-side: build and run `family` cell `idx` under `op`, serially with no
/// timeout thread (the parent owns the wall-clock budget). Outer `None` =
/// unknown family / out-of-range index; inner Option is the op outcome.
fn run_matrix_cell(family: &str, idx: usize, op: BooleanOp) -> Option<Option<Facts>> {
    match family {
        "sphere" => {
            let mut cfgs = Vec::new();
            for (c, _label) in placements() {
                for &r in radii() {
                    cfgs.push((c, r));
                }
            }
            let &(c, r) = cfgs.get(idx)?;
            Some(run_pair_empty_ok(op, the_box, move |m| sphere(m, c, r)))
        }
        "cyl" => {
            let (base, r, h, _) = *cyl_configs().get(idx)?;
            Some(run_pair_empty_ok(op, the_box, move |m| {
                cylinder(m, base, r, h)
            }))
        }
        "cone" => {
            let (bc, rb, rt, h, _) = *cone_configs().get(idx)?;
            Some(run_pair_empty_ok(op, the_box, move |m| {
                cone(m, bc, rb, rt, h)
            }))
        }
        "rbox" => {
            let (hb, center, axis, angle_deg, _) = *rbox_configs().get(idx)?;
            Some(run_pair_empty_ok(op, the_box, move |m| {
                rotated_box(m, hb, center, axis, angle_deg.to_radians())
            }))
        }
        "torus" => {
            let (c, rmaj, rmin, _) = *torus_configs().get(idx)?;
            Some(run_pair_empty_ok(op, the_box, move |m| {
                torus(m, c, rmaj, rmin)
            }))
        }
        "tcyl" => {
            let (base, axis, r, h, _) = *tcyl_configs().get(idx)?;
            Some(run_pair_empty_ok(op, the_box, move |m| {
                tilted_cylinder(m, base, axis, r, h)
            }))
        }
        "ss" => {
            let (ca, ra, cb, rb, _) = *ss_configs().get(idx)?;
            Some(run_pair_empty_ok(
                op,
                move |m| sphere(m, ca, ra),
                move |m| sphere(m, cb, rb),
            ))
        }
        _ => None,
    }
}

/// The generic HANG-honest driver: every (config, op) of `family` in its own
/// killed-on-timeout process; full oracle + invariant battery on the survivors.
fn isolated_matrix_survey(family: &str, title: &str, vol_tol: f64, budget_secs: u64) {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let exe = std::env::current_exe().expect("current_exe");
    let cells = matrix_cells(family);
    let budget = Duration::from_secs(budget_secs);
    let tmp = std::env::temp_dir();
    let sym = ["∩", "∪", "∖"];

    let mut fails: Vec<Failure> = Vec::new();
    let mut n_checks = 0usize;
    let mut hang_cells: Vec<(usize, String, &str)> = Vec::new();

    for (cfg, cell) in cells.iter().enumerate() {
        let t_for = [
            cell.truth.intersection,
            cell.truth.union,
            cell.truth.difference,
        ];
        let mut kvol: [Option<f64>; 3] = [None; 3];

        for opi in 0..3usize {
            let out_path = tmp.join(format!("rosh_fuzz_{family}_{cfg}_{opi}.txt"));
            let _ = std::fs::remove_file(&out_path);
            let mut child = Command::new(&exe)
                .args(["fuzz_single_shot", "--exact", "--ignored"])
                .env("FUZZ_FAMILY", family)
                .env("FUZZ_CFG", cfg.to_string())
                .env("FUZZ_OP", opi.to_string())
                .env("FUZZ_OUT", out_path.to_string_lossy().to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn single-shot");
            let start = Instant::now();
            // Two-tier timing: `budget` is the SLOW threshold (a finished
            // shot past it is reported soft-SLOW with its elapsed time —
            // L4 input); the KILL budget is 5× that, so genuinely slow
            // cells get MEASURED instead of binned as HANG. The isolated
            // box∘sphere run that motivated this read HANG=183 at a 6s
            // kill while cyl/cone read ≈0 at the same budget — those were
            // slow arrangement cells plus ~seconds of process-spawn
            // overhead, not infinite loops, and every one of them was an
            // unmeasured oracle hole.
            let kill_budget = budget * 5;
            let mut hung = false;
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => {
                        if start.elapsed() > kill_budget {
                            let _ = child.kill();
                            let _ = child.wait();
                            hung = true;
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => break,
                }
            }
            let elapsed = start.elapsed();
            n_checks += 1;
            if hung {
                hang_cells.push((cfg, cell.label.clone(), sym[opi]));
                fails.push(Failure {
                    label: cell.label.clone(),
                    op: sym[opi],
                    kind: "HANG",
                    detail: format!(
                        "TRUE hang (isolated process, killed >{}s)",
                        kill_budget.as_secs()
                    ),
                });
                continue;
            }
            if elapsed > budget {
                fails.push(Failure {
                    label: cell.label.clone(),
                    op: sym[opi],
                    kind: "SLOW",
                    detail: format!(
                        "finished in {:.1}s (slow threshold {}s; incl. process spawn)",
                        elapsed.as_secs_f64(),
                        budget.as_secs()
                    ),
                });
            }
            let content = std::fs::read_to_string(&out_path).unwrap_or_default();
            let _ = std::fs::remove_file(&out_path);
            let parts: Vec<&str> = content.split_whitespace().collect();
            if parts.first() == Some(&"OK") && parts.len() >= 5 {
                let vol: f64 = parts[1].parse().unwrap_or(f64::NAN);
                let open: usize = parts[2].parse().unwrap_or(0);
                let nonman: usize = parts[3].parse().unwrap_or(0);
                let euler: i64 = parts[4].parse().unwrap_or(0);
                kvol[opi] = Some(vol);
                let t = t_for[opi];
                if t >= 1e-3 && (vol - t).abs() / t.max(1e-3) > vol_tol {
                    fails.push(Failure {
                        label: cell.label.clone(),
                        op: sym[opi],
                        kind: "VOLUME",
                        detail: format!(
                            "kernel={vol:.4} truth={t:.4} ({:+.1}%)",
                            100.0 * (vol - t) / t
                        ),
                    });
                }
                if open != 0 {
                    fails.push(Failure {
                        label: cell.label.clone(),
                        op: sym[opi],
                        kind: "WATERTIGHT",
                        detail: format!("open_edges={open}"),
                    });
                }
                if nonman != 0 {
                    fails.push(Failure {
                        label: cell.label.clone(),
                        op: sym[opi],
                        kind: "MANIFOLD",
                        detail: format!("nonmanifold_edges={nonman}"),
                    });
                }
                if euler != 0 {
                    fails.push(Failure {
                        label: cell.label.clone(),
                        op: sym[opi],
                        kind: "EULER",
                        detail: format!("euler_residual={euler}"),
                    });
                }
            } else {
                fails.push(Failure {
                    label: cell.label.clone(),
                    op: sym[opi],
                    kind: "ERROR",
                    detail: "op errored (isolated)".to_string(),
                });
            }
        }
        fails.extend(boolean_invariant_failures(
            &cell.label,
            cell.v_a,
            cell.v_b,
            kvol,
        ));
    }

    print_catalog(title, &fails, cells.len(), n_checks);
    // TRUE hangs are verified-real here (a killed child, not starvation), so
    // name every cell — the repro is `fuzz_single_shot` with FUZZ_FAMILY=
    // {family} FUZZ_CFG=<idx> FUZZ_OP={0|1|2}. print_catalog keeps soft kinds
    // count-only because the IN-PROCESS surveys over-report them; this list is
    // the trustworthy variant.
    println!("TRUE HANGS (isolated) = {}", hang_cells.len());
    for (cfg, label, op) in &hang_cells {
        println!("  HANG [{op}] {label}   (FUZZ_FAMILY={family} FUZZ_CFG={cfg})");
    }
    println!();
}

#[test]
#[ignore = "fuzz survey — subprocess-isolated, HANG-honest (slow; spawns processes)"]
fn boolean_box_cyl_survey_isolated() {
    isolated_matrix_survey("cyl", "box ∘ cylinder (subprocess-isolated)", 0.03, 30);
}

#[test]
#[ignore = "fuzz survey — subprocess-isolated, HANG-honest (slow; spawns processes)"]
fn boolean_box_cone_survey_isolated() {
    isolated_matrix_survey("cone", "box ∘ cone (subprocess-isolated)", 0.03, 30);
}

#[test]
#[ignore = "fuzz survey — subprocess-isolated, HANG-honest (slow; spawns processes)"]
fn boolean_box_rbox_survey_isolated() {
    isolated_matrix_survey("rbox", "box ∘ rotated-box (subprocess-isolated)", 0.03, 30);
}

#[test]
#[ignore = "fuzz survey — subprocess-isolated, HANG-honest (slow; spawns processes)"]
fn boolean_box_torus_survey_isolated() {
    isolated_matrix_survey("torus", "box ∘ torus (subprocess-isolated)", 0.03, 30);
}

#[test]
#[ignore = "fuzz survey — subprocess-isolated, HANG-honest (slow; spawns processes)"]
fn boolean_box_tcyl_survey_isolated() {
    isolated_matrix_survey(
        "tcyl",
        "box ∘ tilted-cylinder (subprocess-isolated)",
        0.03,
        30,
    );
}

#[test]
#[ignore = "fuzz survey — subprocess-isolated, HANG-honest (slow; spawns processes)"]
fn boolean_sphere_sphere_survey_isolated() {
    isolated_matrix_survey("ss", "sphere ∘ sphere (subprocess-isolated)", 0.05, 30);
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
    // z-axis torus → V = 2π²·rmaj·rmin² exact; AABB = (rmaj+rmin) disk in xy,
    // ±rmin in z.
    let vol = 2.0 * std::f64::consts::PI * std::f64::consts::PI * rmaj * rmin * rmin;
    let ro = rmaj + rmin;
    let lo = [c[0] - ro, c[1] - ro, c[2] - rmin];
    let hi = [c[0] + ro, c[1] + ro, c[2] + rmin];
    box_shape_grid_truth(vol, lo, hi, |p| in_torus(p, c, rmaj, rmin))
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

// ===========================================================================
// CLEAN-CELL reporter for box∘rotated-box (#91 ratchet). Planar∘planar is the
// healthy path (the survey found only volume noise, zero topology breakage), so
// most rotated-box cells are lockable — a far bigger conquered region than the
// curved surveys. This prints the cells passing all 3 ops cleanly so the gate is
// built from measured fact.
// ===========================================================================

#[test]
#[ignore = "fuzz survey — prints box∘rotated-box cells that pass all 3 ops cleanly"]
fn survey_box_rbox_clean_cells() {
    let vol_tol = 0.03;
    let ops = [
        BooleanOp::Intersection,
        BooleanOp::Union,
        BooleanOp::Difference,
    ];
    let picks: [fn(&GridTruth) -> f64; 3] = [|g| g.intersection, |g| g.union, |g| g.difference];

    let clean: Vec<String> = rbox_configs()
        .par_iter()
        .filter_map(|&(hb, center, axis, angle_deg, label)| {
            let angle = angle_deg.to_radians();
            let truth = rbox_grid_truth(hb, center, axis, angle);
            let mut all_clean = true;
            let mut any_checked = false;
            for (oi, &op) in ops.iter().enumerate() {
                let t = picks[oi](&truth);
                if t < 1e-3 {
                    continue;
                }
                any_checked = true;
                match run_op_timed(op, move |m| rotated_box(m, hb, center, axis, angle)) {
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
                Some(format!("{label} hb={hb} {angle_deg}"))
            } else {
                None
            }
        })
        .collect();

    let mut clean = clean;
    clean.sort();
    println!(
        "\n=== #91 box∘rotated-box CLEAN cells (pass ∩/∪/∖: vol≤3%, watertight, manifold) ==="
    );
    println!("clean_cells={}", clean.len());
    for c in &clean {
        println!("  OK {c}");
    }
    println!("=== end ===\n");
}

// ===========================================================================
// RATCHET GATE (#91) — NON-ignored. Locks the box∘rotated-box cells that pass
// all three booleans cleanly, per survey_box_rbox_clean_cells. The planar
// oblique-clip path is the kernel's healthiest cut path; these four centered
// rotations are the conquered floor for it and must never regress to the #34/#80
// over-inclusion class. Oracle = 96³ grid truth at 5% tol + watertight + manifold.
// ===========================================================================

#[test]
fn box_rbox_conquered_band_gate() {
    // (half-extent, centre, axis, angle_deg) — exactly the cells the reporter
    // measured clean (all origin-centred).
    let cells: [(f64, [f64; 3], [f64; 3], f64); 4] = [
        (0.7, [0.0, 0.0, 0.0], [1.0, 1.0, 1.0], 45.0), // diag-45-centered
        (0.4, [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], 20.0), // contained-tilt
        (1.0, [0.0, 0.0, 0.0], [1.0, 1.0, 1.0], 45.0), // big-diag
        (0.5, [0.0, 0.0, 0.0], [1.0, 2.0, 0.0], 35.0), // tilt-through
    ];
    let ops: [(BooleanOp, &str, fn(&GridTruth) -> f64); 3] = [
        (BooleanOp::Intersection, "∩", |g| g.intersection),
        (BooleanOp::Union, "∪", |g| g.union),
        (BooleanOp::Difference, "∖", |g| g.difference),
    ];
    let tol = 0.05;
    for (hb, center, axis, angle_deg) in cells {
        let angle = angle_deg.to_radians();
        let truth = rbox_grid_truth(hb, center, axis, angle);
        for &(op, sym, pick) in &ops {
            let t = pick(&truth);
            if t < 1e-3 {
                continue;
            }
            // Serial run_op (not run_op_timed): conquered cells never hang, and a
            // wall-clock thread budget flakes under full-suite CPU load.
            let f = run_op(op, move |m| rotated_box(m, hb, center, axis, angle))
                .unwrap_or_else(|| panic!("box∘rbox {sym} hb={hb} {angle_deg}°: no result solid"));
            let rel = (f.vol - t).abs() / t.max(1e-3);
            assert!(
                rel <= tol,
                "REGRESSION: box∘rbox {sym} hb={hb} {angle_deg}°: vol={:.4} truth={t:.4} ({:+.1}%, tol {:.0}%)",
                f.vol,
                100.0 * (f.vol - t) / t,
                100.0 * tol
            );
            assert_eq!(
                f.open_edges, 0,
                "REGRESSION: box∘rbox {sym} hb={hb} {angle_deg}°: {} open edges",
                f.open_edges
            );
            assert_eq!(
                f.nonmanifold_edges, 0,
                "REGRESSION: box∘rbox {sym} hb={hb} {angle_deg}°: {} non-manifold edges",
                f.nonmanifold_edges
            );
        }
    }
}

// ===========================================================================
// CLEAN-CELL reporter for box∘cylinder (#91 ratchet). box∘cylinder had the
// fewest HARD failures of the curved surveys (8 of 35), so its conquered region
// is the largest unlocked one. Prints cells passing all 3 ops cleanly.
// ===========================================================================

#[test]
#[ignore = "fuzz survey — prints box∘cylinder cells that pass all 3 ops cleanly"]
fn survey_box_cyl_clean_cells() {
    let vol_tol = 0.03;
    let ops = [
        BooleanOp::Intersection,
        BooleanOp::Union,
        BooleanOp::Difference,
    ];
    let picks: [fn(&GridTruth) -> f64; 3] = [|g| g.intersection, |g| g.union, |g| g.difference];

    let clean: Vec<String> = cyl_configs()
        .par_iter()
        .filter_map(|&(base, r, h, label)| {
            let truth = cyl_grid_truth(base, r, h);
            let mut all_clean = true;
            let mut any_checked = false;
            for (oi, &op) in ops.iter().enumerate() {
                let t = picks[oi](&truth);
                if t < 1e-3 {
                    continue;
                }
                any_checked = true;
                match run_op_timed(op, move |m| cylinder(m, base, r, h)) {
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
                Some(format!("{label} r={r} h={h}"))
            } else {
                None
            }
        })
        .collect();

    let mut clean = clean;
    clean.sort();
    println!("\n=== #91 box∘cylinder CLEAN cells (pass ∩/∪/∖: vol≤3%, watertight, manifold) ===");
    println!("clean_cells={}", clean.len());
    for c in &clean {
        println!("  OK {c}");
    }
    println!("=== end ===\n");
}

// ===========================================================================
// RATCHET GATE (#91) — NON-ignored. Locks the box∘cylinder cells that pass all
// three booleans cleanly, per survey_box_cyl_clean_cells. Cylinder has the
// fewest curved-survey failures; these four (contained / offset / edge / corner)
// are its conquered floor. Oracle = 96³ grid truth at 5% + watertight + manifold.
// ===========================================================================

#[test]
fn box_cyl_conquered_band_gate() {
    // (base, r, h) — exactly the cells survey_box_cyl_clean_cells reported clean.
    let cells: [([f64; 3], f64, f64); 4] = [
        ([0.0, 0.0, -0.5], 0.3, 1.0), // contained
        ([0.5, 0.3, -0.5], 0.3, 1.0), // contained-offset
        ([1.0, 1.0, -0.5], 0.5, 1.0), // radial-edge
        ([1.0, 1.0, 0.6], 0.5, 1.0),  // corner
    ];
    let ops: [(BooleanOp, &str, fn(&GridTruth) -> f64); 3] = [
        (BooleanOp::Intersection, "∩", |g| g.intersection),
        (BooleanOp::Union, "∪", |g| g.union),
        (BooleanOp::Difference, "∖", |g| g.difference),
    ];
    let tol = 0.05;
    for (base, r, h) in cells {
        let truth = cyl_grid_truth(base, r, h);
        for &(op, sym, pick) in &ops {
            let t = pick(&truth);
            if t < 1e-3 {
                continue;
            }
            // Serial run_op (conquered cells never hang; a thread budget flakes under load).
            let f = run_op(op, move |m| cylinder(m, base, r, h)).unwrap_or_else(|| {
                panic!("box∘cyl {sym} base={base:?} r={r} h={h}: no result solid")
            });
            let rel = (f.vol - t).abs() / t.max(1e-3);
            assert!(
                rel <= tol,
                "REGRESSION: box∘cyl {sym} base={base:?} r={r} h={h}: vol={:.4} truth={t:.4} ({:+.1}%, tol {:.0}%)",
                f.vol,
                100.0 * (f.vol - t) / t,
                100.0 * tol
            );
            assert_eq!(
                f.open_edges, 0,
                "REGRESSION: box∘cyl {sym} base={base:?} r={r} h={h}: {} open edges",
                f.open_edges
            );
            assert_eq!(
                f.nonmanifold_edges, 0,
                "REGRESSION: box∘cyl {sym} base={base:?} r={r} h={h}: {} non-manifold edges",
                f.nonmanifold_edges
            );
        }
    }
}

// ===========================================================================
// CLEAN-CELL reporter for box∘cone (#91 ratchet). Cone is the most broken
// curved survey (24/30 HARD), so the clean set is small — but whatever passes
// is conquered ground worth locking against further regression.
// ===========================================================================

#[test]
#[ignore = "fuzz survey — prints box∘cone cells that pass all 3 ops cleanly"]
fn survey_box_cone_clean_cells() {
    let vol_tol = 0.03;
    let ops = [
        BooleanOp::Intersection,
        BooleanOp::Union,
        BooleanOp::Difference,
    ];
    let picks: [fn(&GridTruth) -> f64; 3] = [|g| g.intersection, |g| g.union, |g| g.difference];

    let clean: Vec<String> = cone_configs()
        .par_iter()
        .filter_map(|&(bc, rb, rt, h, label)| {
            let truth = cone_grid_truth(bc, rb, rt, h);
            let mut all_clean = true;
            let mut any_checked = false;
            for (oi, &op) in ops.iter().enumerate() {
                let t = picks[oi](&truth);
                if t < 1e-3 {
                    continue;
                }
                any_checked = true;
                match run_op_timed(op, move |m| cone(m, bc, rb, rt, h)) {
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
                Some(format!("{label} rb={rb} rt={rt} h={h}"))
            } else {
                None
            }
        })
        .collect();

    let mut clean = clean;
    clean.sort();
    println!("\n=== #91 box∘cone CLEAN cells (pass ∩/∪/∖: vol≤3%, watertight, manifold) ===");
    println!("clean_cells={}", clean.len());
    for c in &clean {
        println!("  OK {c}");
    }
    println!("=== end ===\n");
}

// ===========================================================================
// RATCHET GATE (#91) — NON-ignored. Cone is the most broken curved survey
// (24/30); only 3 cells survive all three booleans cleanly. Lock them so the
// conquered apex/contained cases can't regress. 96³ grid truth, 5% + topology.
// ===========================================================================

#[test]
fn box_cone_conquered_band_gate() {
    // (base_centre, base_r, top_r, h) — the cells survey_box_cone_clean_cells found clean.
    let cells: [([f64; 3], f64, f64, f64); 3] = [
        ([0.0, 0.0, -1.5], 0.9, 0.0, 3.0), // apex-through
        ([0.0, 0.0, -0.5], 0.4, 0.0, 1.0), // contained-apex
        ([0.5, 0.3, -0.5], 0.4, 0.2, 1.0), // contained-frustum-off
    ];
    let ops: [(BooleanOp, &str, fn(&GridTruth) -> f64); 3] = [
        (BooleanOp::Intersection, "∩", |g| g.intersection),
        (BooleanOp::Union, "∪", |g| g.union),
        (BooleanOp::Difference, "∖", |g| g.difference),
    ];
    let tol = 0.05;
    for (bc, rb, rt, h) in cells {
        let truth = cone_grid_truth(bc, rb, rt, h);
        for &(op, sym, pick) in &ops {
            let t = pick(&truth);
            if t < 1e-3 {
                continue;
            }
            // Serial run_op (conquered cells never hang; a thread budget flakes under load).
            let f = run_op(op, move |m| cone(m, bc, rb, rt, h)).unwrap_or_else(|| {
                panic!("box∘cone {sym} bc={bc:?} rb={rb} rt={rt} h={h}: no result solid")
            });
            let rel = (f.vol - t).abs() / t.max(1e-3);
            assert!(
                rel <= tol,
                "REGRESSION: box∘cone {sym} bc={bc:?} rb={rb} rt={rt} h={h}: vol={:.4} truth={t:.4} ({:+.1}%, tol {:.0}%)",
                f.vol,
                100.0 * (f.vol - t) / t,
                100.0 * tol
            );
            assert_eq!(
                f.open_edges, 0,
                "REGRESSION: box∘cone {sym} bc={bc:?} rb={rb} rt={rt} h={h}: {} open edges",
                f.open_edges
            );
            assert_eq!(
                f.nonmanifold_edges, 0,
                "REGRESSION: box∘cone {sym} bc={bc:?} rb={rb} rt={rt} h={h}: {} non-manifold edges",
                f.nonmanifold_edges
            );
        }
    }
}

// ===========================================================================
// CLEAN-CELL reporter for box∘torus (#91 ratchet). Torus had only 6/27 HARD;
// the thin/contained tori should mostly pass. Prints the clean set for the gate.
// ===========================================================================

#[test]
#[ignore = "fuzz survey — prints box∘torus cells that pass all 3 ops cleanly"]
fn survey_box_torus_clean_cells() {
    let vol_tol = 0.03;
    let ops = [
        BooleanOp::Intersection,
        BooleanOp::Union,
        BooleanOp::Difference,
    ];
    let picks: [fn(&GridTruth) -> f64; 3] = [|g| g.intersection, |g| g.union, |g| g.difference];

    let clean: Vec<String> = torus_configs()
        .par_iter()
        .filter_map(|&(c, rmaj, rmin, label)| {
            let truth = torus_grid_truth(c, rmaj, rmin);
            let mut all_clean = true;
            let mut any_checked = false;
            for (oi, &op) in ops.iter().enumerate() {
                let t = picks[oi](&truth);
                if t < 1e-3 {
                    continue;
                }
                any_checked = true;
                match run_op_timed(op, move |m| torus(m, c, rmaj, rmin)) {
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
                Some(format!("{label} R={rmaj} r={rmin}"))
            } else {
                None
            }
        })
        .collect();

    let mut clean = clean;
    clean.sort();
    println!("\n=== #91 box∘torus CLEAN cells (pass ∩/∪/∖: vol≤3%, watertight, manifold) ===");
    println!("clean_cells={}", clean.len());
    for c in &clean {
        println!("  OK {c}");
    }
    println!("=== end ===\n");
}

// ===========================================================================
// RATCHET GATE (#91) — NON-ignored. Locks the box∘torus cells that pass all
// three booleans cleanly (the thin/contained genus-1 tori). 96³ grid truth at
// 5% + watertight + manifold. Fifth and final per-primitive ratchet gate.
// ===========================================================================

#[test]
fn box_torus_conquered_band_gate() {
    // (centre, major_r, minor_r) — the cells survey_box_torus_clean_cells found clean.
    let cells: [([f64; 3], f64, f64); 3] = [
        ([0.0, 0.0, 0.0], 0.6, 0.25), // centered
        ([0.0, 0.0, 0.0], 0.7, 0.2),  // ring-hole
        ([0.0, 0.0, 0.0], 0.5, 0.15), // thin-contained
    ];
    let ops: [(BooleanOp, &str, fn(&GridTruth) -> f64); 3] = [
        (BooleanOp::Intersection, "∩", |g| g.intersection),
        (BooleanOp::Union, "∪", |g| g.union),
        (BooleanOp::Difference, "∖", |g| g.difference),
    ];
    let tol = 0.05;
    for (c, rmaj, rmin) in cells {
        let truth = torus_grid_truth(c, rmaj, rmin);
        for &(op, sym, pick) in &ops {
            let t = pick(&truth);
            if t < 1e-3 {
                continue;
            }
            // Serial run_op (conquered cells never hang; a thread budget flakes under load).
            let f = run_op(op, move |m| torus(m, c, rmaj, rmin)).unwrap_or_else(|| {
                panic!("box∘torus {sym} c={c:?} R={rmaj} r={rmin}: no result solid")
            });
            let rel = (f.vol - t).abs() / t.max(1e-3);
            assert!(
                rel <= tol,
                "REGRESSION: box∘torus {sym} c={c:?} R={rmaj} r={rmin}: vol={:.4} truth={t:.4} ({:+.1}%, tol {:.0}%)",
                f.vol,
                100.0 * (f.vol - t) / t,
                100.0 * tol
            );
            assert_eq!(
                f.open_edges, 0,
                "REGRESSION: box∘torus {sym} c={c:?} R={rmaj} r={rmin}: {} open edges",
                f.open_edges
            );
            assert_eq!(
                f.nonmanifold_edges, 0,
                "REGRESSION: box∘torus {sym} c={c:?} R={rmaj} r={rmin}: {} non-manifold edges",
                f.nonmanifold_edges
            );
        }
    }
}

// ===========================================================================
// DETERMINISM GATE (#91) — NON-ignored. The survey runs booleans in parallel
// (rayon) and a recurring kernel lesson is "a flaky test is a determinism bug".
// This locks that the boolean pipeline is BIT-reproducible: the same op on the
// same operands yields f64-identical volume and identical topology counts across
// two independent runs. A regression that introduced order-dependence (a HashMap
// iteration, an unsorted merge) would flip a low bit here and fail CI before it
// could surface as an intermittent survey flake.
// ===========================================================================

#[test]
fn boolean_pipeline_determinism_gate() {
    // Representative conquered cells across primitives — clean + fast + varied.
    let ops = [
        BooleanOp::Intersection,
        BooleanOp::Union,
        BooleanOp::Difference,
    ];
    // Each entry runs both passes through `run_op` and must agree bit-for-bit.
    let builders: [(&str, Box<dyn Fn(&mut BRepModel) -> SolidId>); 4] = [
        (
            "sphere-contained",
            Box::new(|m: &mut BRepModel| sphere(m, [0.0, 0.0, 0.0], 0.8)),
        ),
        (
            "cyl-contained",
            Box::new(|m: &mut BRepModel| cylinder(m, [0.0, 0.0, -0.5], 0.3, 1.0)),
        ),
        (
            "cone-contained-apex",
            Box::new(|m: &mut BRepModel| cone(m, [0.0, 0.0, -0.5], 0.4, 0.0, 1.0)),
        ),
        (
            "rbox-diag45",
            Box::new(|m: &mut BRepModel| {
                rotated_box(
                    m,
                    0.7,
                    [0.0, 0.0, 0.0],
                    [1.0, 1.0, 1.0],
                    45.0_f64.to_radians(),
                )
            }),
        ),
    ];

    for (name, build) in &builders {
        for &op in &ops {
            let a = run_op(op, |m| build(m)).unwrap_or_else(|| {
                panic!("determinism gate: {name} {op:?} pass 1 returned no solid")
            });
            let b = run_op(op, |m| build(m)).unwrap_or_else(|| {
                panic!("determinism gate: {name} {op:?} pass 2 returned no solid")
            });
            assert_eq!(
                a.vol.to_bits(),
                b.vol.to_bits(),
                "NON-DETERMINISTIC: {name} {op:?} volume differs across runs: {} vs {}",
                a.vol,
                b.vol
            );
            assert_eq!(
                a.open_edges, b.open_edges,
                "NON-DETERMINISTIC: {name} {op:?} open_edges {} vs {}",
                a.open_edges, b.open_edges
            );
            assert_eq!(
                a.nonmanifold_edges, b.nonmanifold_edges,
                "NON-DETERMINISTIC: {name} {op:?} nonmanifold_edges {} vs {}",
                a.nonmanifold_edges, b.nonmanifold_edges
            );
        }
    }
}

// ===========================================================================
// box ∘ TILTED-CYLINDER survey — every curved second-operand above uses the Z
// axis. An arbitrary-axis cylinder drives the curved cut against the box faces
// at an oblique angle to the surface's own parameterisation, exercising the
// axis-handling path (frame construction, seam placement) the Z-aligned surveys
// never reach. Predicate projects onto the axis instead of reading p.z.
// ===========================================================================

fn norm3(v: [f64; 3]) -> [f64; 3] {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / n, v[1] / n, v[2] / n]
}

fn tilted_cylinder(
    model: &mut BRepModel,
    base: [f64; 3],
    axis: [f64; 3],
    r: f64,
    h: f64,
) -> SolidId {
    match TopologyBuilder::new(model)
        .create_cylinder_3d(
            Point3::new(base[0], base[1], base[2]),
            Vector3::new(axis[0], axis[1], axis[2]),
            r,
            h,
        )
        .expect("tilted cylinder")
    {
        GeometryId::Solid(id) => id,
        o => panic!("tilted cylinder: {o:?}"),
    }
}

/// Inside a finite cylinder of arbitrary axis: project (p−base) onto the unit
/// axis for the axial coord t∈[0,h]; the perpendicular residual must be ≤ r.
fn in_tilted_cyl(p: [f64; 3], base: [f64; 3], axis_unit: [f64; 3], r: f64, h: f64) -> bool {
    let d = [p[0] - base[0], p[1] - base[1], p[2] - base[2]];
    let t = d[0] * axis_unit[0] + d[1] * axis_unit[1] + d[2] * axis_unit[2];
    if t < 0.0 || t > h {
        return false;
    }
    let perp = [
        d[0] - t * axis_unit[0],
        d[1] - t * axis_unit[1],
        d[2] - t * axis_unit[2],
    ];
    perp[0] * perp[0] + perp[1] * perp[1] + perp[2] * perp[2] <= r * r
}

fn tcyl_grid_truth(base: [f64; 3], axis: [f64; 3], r: f64, h: f64) -> GridTruth {
    // Tilted finite cylinder → V = π r² h exact. Conservative AABB: the two cap
    // centres (base, base+u·h) expanded by r on every axis contain the cylinder.
    let u = norm3(axis);
    let end = [base[0] + u[0] * h, base[1] + u[1] * h, base[2] + u[2] * h];
    let vol = std::f64::consts::PI * r * r * h;
    let lo = [
        base[0].min(end[0]) - r,
        base[1].min(end[1]) - r,
        base[2].min(end[2]) - r,
    ];
    let hi = [
        base[0].max(end[0]) + r,
        base[1].max(end[1]) + r,
        base[2].max(end[2]) + r,
    ];
    box_shape_grid_truth(vol, lo, hi, |p| in_tilted_cyl(p, base, u, r, h))
}

/// (base, axis, r, h, label) — arbitrary-axis cylinders vs box [-1,1]³.
fn tcyl_configs() -> Vec<([f64; 3], [f64; 3], f64, f64, &'static str)> {
    vec![
        (
            [-0.7, -0.7, -0.7],
            [1.0, 1.0, 1.0],
            0.3,
            2.4,
            "diag-through",
        ),
        (
            [-1.0, -0.6, 0.0],
            [1.0, 1.0, 0.0],
            0.3,
            2.0,
            "tilt-xy-horizontal",
        ),
        (
            [-0.3, 0.0, -0.3],
            [1.0, 0.0, 1.0],
            0.2,
            0.85,
            "tilt-contained",
        ),
        ([0.0, 0.0, -1.0], [0.3, 0.0, 1.0], 0.3, 2.0, "tilt-poke+z"),
        ([0.0, -1.0, -0.5], [0.0, 1.0, 1.0], 0.3, 2.0, "tilt-edge-yz"),
        ([-0.5, -0.5, -1.2], [0.5, 0.5, 1.0], 0.25, 2.0, "tilt-skew"),
    ]
}

#[test]
#[ignore = "fuzz survey — run with --ignored --nocapture"]
fn boolean_box_tilted_cylinder_fuzz_survey() {
    let vol_tol = 0.03;
    let ops: [(BooleanOp, &str, fn(&GridTruth) -> f64); 3] = [
        (BooleanOp::Intersection, "∩", |g| g.intersection),
        (BooleanOp::Union, "∪", |g| g.union),
        (BooleanOp::Difference, "∖", |g| g.difference),
    ];
    let configs = tcyl_configs();
    let n_cfg = configs.len();
    let n_checks = std::sync::atomic::AtomicUsize::new(0);

    let fails: Vec<Failure> = configs
        .par_iter()
        .flat_map(|&(base, axis, r, h, label)| {
            let truth = tcyl_grid_truth(base, axis, r, h);
            let mut out: Vec<Failure> = Vec::new();
            for &(op, sym, pick) in &ops {
                let t = pick(&truth);
                if t < 1e-3 {
                    continue;
                }
                n_checks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let lab = format!("{label} r={r} h={h}");
                match run_op_timed(op, move |m| tilted_cylinder(m, base, axis, r, h)) {
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
        "box ∘ tilted-cylinder",
        &fails,
        n_cfg,
        n_checks.load(std::sync::atomic::Ordering::Relaxed),
    );
}

// ===========================================================================
// PARASOLID-GRADE ORACLES (#91 / harness rigor). The grid oracle measures one
// thing (voxel volume). Production kernels are validated against MANY independent
// oracles + algebraic laws that need no mesh. This block adds the mesh-free layer.
//
// `ShapeSpec` is a Copy descriptor of any operand — the foundation both for these
// algebraic surveys and for the seeded random fuzzer (later iteration). It carries
// an exact closed-form volume, so booleans can be cross-checked against ALGEBRA,
// not just a discretised grid.
// ===========================================================================

#[derive(Clone, Copy, Debug)]
enum ShapeSpec {
    Box,
    Sphere {
        c: [f64; 3],
        r: f64,
    },
    Cyl {
        base: [f64; 3],
        r: f64,
        h: f64,
    },
    Cone {
        bc: [f64; 3],
        rb: f64,
        rt: f64,
        h: f64,
    },
    Torus {
        c: [f64; 3],
        rmaj: f64,
        rmin: f64,
    },
}

fn build_shape(model: &mut BRepModel, s: ShapeSpec) -> SolidId {
    match s {
        ShapeSpec::Box => the_box(model),
        ShapeSpec::Sphere { c, r } => sphere(model, c, r),
        ShapeSpec::Cyl { base, r, h } => cylinder(model, base, r, h),
        ShapeSpec::Cone { bc, rb, rt, h } => cone(model, bc, rb, rt, h),
        ShapeSpec::Torus { c, rmaj, rmin } => torus(model, c, rmaj, rmin),
    }
}

/// Exact closed-form volume of the primitive — the mesh-free truth for one operand.
fn analytic_vol(s: ShapeSpec) -> f64 {
    use std::f64::consts::PI;
    match s {
        ShapeSpec::Box => 8.0, // (2·BOX_HALF)³
        ShapeSpec::Sphere { r, .. } => 4.0 / 3.0 * PI * r * r * r,
        ShapeSpec::Cyl { r, h, .. } => PI * r * r * h,
        ShapeSpec::Cone { rb, rt, h, .. } => PI * h / 3.0 * (rb * rb + rb * rt + rt * rt),
        ShapeSpec::Torus { rmaj, rmin, .. } => 2.0 * PI * PI * rmaj * rmin * rmin,
    }
}

/// IDEMPOTENCE + COMMUTATIVITY survey (#91 Parasolid-grade). These are EXACT laws,
/// independent of any grid:
///   A∩A = A,  A∪A = A,  A∖A = ∅   (and the result must stay watertight+manifold)
///   A∩B = B∩A,  A∪B = B∪A          (volume-equal under operand swap)
/// A∩A is the worst coincident-face degeneracy a kernel faces — two solids sharing
/// every face. A kernel that mis-handles it is unsound at the most basic level.
#[test]
#[ignore = "fuzz survey — run with --ignored --nocapture"]
fn boolean_idempotence_commutativity_survey() {
    let tol = 0.03;
    let shapes: [(&str, ShapeSpec); 5] = [
        ("box", ShapeSpec::Box),
        (
            "sphere",
            ShapeSpec::Sphere {
                c: [0.0, 0.0, 0.0],
                r: 0.8,
            },
        ),
        (
            "cyl",
            ShapeSpec::Cyl {
                base: [0.0, 0.0, -0.5],
                r: 0.5,
                h: 1.0,
            },
        ),
        (
            "cone",
            ShapeSpec::Cone {
                bc: [0.0, 0.0, -0.5],
                rb: 0.5,
                rt: 0.0,
                h: 1.0,
            },
        ),
        (
            "torus",
            ShapeSpec::Torus {
                c: [0.0, 0.0, 0.0],
                rmaj: 0.6,
                rmin: 0.25,
            },
        ),
    ];
    let mut fails: Vec<Failure> = Vec::new();
    let mut n_checks = 0usize;

    // --- self-operation idempotence (coincident-face stress) ---
    for (name, spec) in shapes {
        let va = analytic_vol(spec);
        let cases: [(BooleanOp, &str, f64); 3] = [
            (BooleanOp::Intersection, "∩", va),
            (BooleanOp::Union, "∪", va),
            (BooleanOp::Difference, "∖", 0.0),
        ];
        for (op, sym, exp) in cases {
            n_checks += 1;
            match run_pair_timed(
                op,
                move |m| build_shape(m, spec),
                move |m| build_shape(m, spec),
            ) {
                Outcome::Ok(f) => {
                    if (f.vol - exp).abs() / va.max(1e-3) > tol {
                        fails.push(Failure {
                            label: format!("{name} A{sym}A"),
                            op: sym,
                            kind: "VOLUME",
                            detail: format!("IDEMPOTENCE vol={:.4} expected={exp:.4}", f.vol),
                        });
                    }
                    if f.open_edges != 0 {
                        fails.push(Failure {
                            label: format!("{name} A{sym}A"),
                            op: sym,
                            kind: "WATERTIGHT",
                            detail: format!("open_edges={}", f.open_edges),
                        });
                    }
                    if f.nonmanifold_edges != 0 {
                        fails.push(Failure {
                            label: format!("{name} A{sym}A"),
                            op: sym,
                            kind: "MANIFOLD",
                            detail: format!("nonmanifold_edges={}", f.nonmanifold_edges),
                        });
                    }
                }
                Outcome::Err => fails.push(Failure {
                    label: format!("{name} A{sym}A"),
                    op: sym,
                    kind: "ERROR",
                    detail: "op errored".into(),
                }),
                Outcome::Hang => fails.push(Failure {
                    label: format!("{name} A{sym}A"),
                    op: sym,
                    kind: "HANG",
                    detail: "op did not return".into(),
                }),
            }
        }
    }

    // --- commutativity of ∩ and ∪ over box∘sphere placements ---
    let comm_cfgs: [([f64; 3], f64, &str); 4] = [
        ([0.0, 0.0, 0.0], 0.5, "interior"),
        ([1.0, 0.0, 0.0], 0.5, "face"),
        ([1.0, 1.0, 0.0], 0.5, "edge"),
        ([0.5, 0.3, 0.0], 0.5, "offset"),
    ];
    for (c, r, label) in comm_cfgs {
        for (op, sym) in [(BooleanOp::Intersection, "∩"), (BooleanOp::Union, "∪")] {
            n_checks += 1;
            let ab = run_pair_timed(op, the_box, move |m| sphere(m, c, r));
            let ba = run_pair_timed(op, move |m| sphere(m, c, r), the_box);
            if let (Outcome::Ok(a), Outcome::Ok(b)) = (&ab, &ba) {
                let denom = a.vol.abs().max(1e-3);
                if (a.vol - b.vol).abs() / denom > tol {
                    fails.push(Failure {
                        label: format!("{label} r={r}"),
                        op: sym,
                        kind: "VOLUME",
                        detail: format!(
                            "COMMUTATIVITY box{sym}sph={:.4} sph{sym}box={:.4}",
                            a.vol, b.vol
                        ),
                    });
                }
            } else {
                fails.push(Failure {
                    label: format!("{label} r={r}"),
                    op: sym,
                    kind: "ERROR",
                    detail: "COMMUTATIVITY one order errored/hung".into(),
                });
            }
        }
    }

    print_catalog(
        "idempotence + commutativity (mesh-free algebraic laws)",
        &fails,
        shapes.len() + comm_cfgs.len(),
        n_checks,
    );
}

// ===========================================================================
// SEEDED RANDOM FUZZER (#91 Parasolid-grade). The per-pair surveys above use
// hand-picked configs (~10 each). A production kernel is fuzzed over the
// CONTINUOUS parameter space with thousands of randomized cases. This drives
// random ShapeSpec pairs (any primitive x any primitive, random position/size)
// through the grid oracle. Every case is reproducible: case #N is derived
// deterministically from BASE_SEED via splitmix64, so a failing case#N
// regenerates byte-identically — no flaky, non-reproducible fuzz.
// ===========================================================================

/// splitmix64 — a deterministic, seedable PRNG (no external dep, no nondeterminism).
struct Rng {
    state: u64,
}
impl Rng {
    fn new(seed: u64) -> Self {
        Rng { state: seed }
    }
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
    fn pick(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

fn rand_center(rng: &mut Rng) -> [f64; 3] {
    [
        rng.range(-1.0, 1.0),
        rng.range(-1.0, 1.0),
        rng.range(-1.0, 1.0),
    ]
}
fn rand_base(rng: &mut Rng) -> [f64; 3] {
    // axial shapes: bias the base downward so the body straddles the box.
    [
        rng.range(-1.0, 1.0),
        rng.range(-1.0, 1.0),
        rng.range(-1.5, 0.3),
    ]
}

/// A random primitive with valid (non-degenerate, non-self-intersecting) params.
fn rand_shape(rng: &mut Rng) -> ShapeSpec {
    match rng.pick(5) {
        0 => ShapeSpec::Box,
        1 => ShapeSpec::Sphere {
            c: rand_center(rng),
            r: rng.range(0.3, 1.2),
        },
        2 => ShapeSpec::Cyl {
            base: rand_base(rng),
            r: rng.range(0.3, 1.0),
            h: rng.range(0.8, 2.5),
        },
        3 => ShapeSpec::Cone {
            bc: rand_base(rng),
            rb: rng.range(0.35, 1.0),
            rt: rng.range(0.0, 0.6),
            h: rng.range(0.8, 2.5),
        },
        // torus: rmin < rmaj guaranteed (no self-intersection).
        _ => ShapeSpec::Torus {
            c: rand_center(rng),
            rmaj: rng.range(0.45, 0.9),
            rmin: rng.range(0.12, 0.35),
        },
    }
}

fn in_shape(p: [f64; 3], s: ShapeSpec) -> bool {
    match s {
        ShapeSpec::Box => {
            p[0].abs() <= BOX_HALF && p[1].abs() <= BOX_HALF && p[2].abs() <= BOX_HALF
        }
        ShapeSpec::Sphere { c, r } => in_ball(p, c, r),
        ShapeSpec::Cyl { base, r, h } => in_cylinder(p, base, r, h),
        ShapeSpec::Cone { bc, rb, rt, h } => in_cone(p, bc, rb, rt, h),
        ShapeSpec::Torus { c, rmaj, rmin } => in_torus(p, c, rmaj, rmin),
    }
}

fn shape_aabb(s: ShapeSpec) -> ([f64; 3], [f64; 3]) {
    match s {
        ShapeSpec::Box => ([-BOX_HALF; 3], [BOX_HALF; 3]),
        ShapeSpec::Sphere { c, r } => (
            [c[0] - r, c[1] - r, c[2] - r],
            [c[0] + r, c[1] + r, c[2] + r],
        ),
        ShapeSpec::Cyl { base, r, h } => (
            [base[0] - r, base[1] - r, base[2]],
            [base[0] + r, base[1] + r, base[2] + h],
        ),
        ShapeSpec::Cone { bc, rb, rt, h } => {
            let rr = rb.max(rt);
            (
                [bc[0] - rr, bc[1] - rr, bc[2]],
                [bc[0] + rr, bc[1] + rr, bc[2] + h],
            )
        }
        ShapeSpec::Torus { c, rmaj, rmin } => {
            let rr = rmaj + rmin;
            (
                [c[0] - rr, c[1] - rr, c[2] - rmin],
                [c[0] + rr, c[1] + rr, c[2] + rmin],
            )
        }
    }
}

fn shape_name(s: ShapeSpec) -> &'static str {
    match s {
        ShapeSpec::Box => "box",
        ShapeSpec::Sphere { .. } => "sphere",
        ShapeSpec::Cyl { .. } => "cyl",
        ShapeSpec::Cone { .. } => "cone",
        ShapeSpec::Torus { .. } => "torus",
    }
}

/// Generic grid oracle for an arbitrary pair (coarser N for fuzzing throughput).
fn grid_truth_pair(a: ShapeSpec, b: ShapeSpec) -> GridTruth {
    let (amin, amax) = shape_aabb(a);
    let (bmin, bmax) = shape_aabb(b);
    let reach = (0..3)
        .map(|i| {
            amin[i]
                .abs()
                .max(amax[i].abs())
                .max(bmin[i].abs())
                .max(bmax[i].abs())
        })
        .fold(0.1, f64::max)
        + 0.05;
    const N: usize = 48;
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
                let ina = in_shape(p, a);
                let inb = in_shape(p, b);
                if ina && inb {
                    i_n += 1;
                }
                if ina || inb {
                    u_n += 1;
                }
                if ina && !inb {
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

#[test]
#[ignore = "fuzz survey — seeded random pairs; run with --ignored --nocapture"]
fn boolean_random_fuzz_survey() {
    const N_CASES: usize = 400; // tunable; each case reproducible from BASE_SEED
    const BASE_SEED: u64 = 0x00C0_FFEE_1234_5678;
    // coarse N=48 grid + two arbitrary operands -> loose tol; only catastrophes register.
    let vol_tol = 0.06;
    let ops: [(BooleanOp, &str, fn(&GridTruth) -> f64); 3] = [
        (BooleanOp::Intersection, "I", |g| g.intersection),
        (BooleanOp::Union, "U", |g| g.union),
        (BooleanOp::Difference, "D", |g| g.difference),
    ];
    let n_checks = std::sync::atomic::AtomicUsize::new(0);

    let fails: Vec<Failure> = (0..N_CASES)
        .into_par_iter()
        .flat_map(|case| {
            let mut rng = Rng::new(BASE_SEED ^ (case as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let a = rand_shape(&mut rng);
            let b = rand_shape(&mut rng);
            let truth = grid_truth_pair(a, b);
            let mut out: Vec<Failure> = Vec::new();
            for &(op, sym, pick) in &ops {
                let t = pick(&truth);
                if t < 1e-3 {
                    continue;
                }
                n_checks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let lab = format!("case#{case} {}-{}", shape_name(a), shape_name(b));
                match run_pair_timed(op, move |m| build_shape(m, a), move |m| build_shape(m, b)) {
                    Outcome::Hang => out.push(Failure {
                        label: lab,
                        op: sym,
                        kind: "HANG",
                        detail: "op did not return".into(),
                    }),
                    Outcome::Err => out.push(Failure {
                        label: lab,
                        op: sym,
                        kind: "ERROR",
                        detail: format!("op errored (truth {t:.3})"),
                    }),
                    Outcome::Ok(f) => {
                        if (f.vol - t).abs() / t.max(1e-3) > vol_tol {
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

    println!(
        "\n[random fuzz] BASE_SEED={BASE_SEED:#018x} — reproduce a failure by rebuilding case#N from this seed"
    );
    print_catalog(
        &format!("random fuzz (seeded, N_CASES={N_CASES})"),
        &fails,
        N_CASES,
        n_checks.load(std::sync::atomic::Ordering::Relaxed),
    );
}

// ===========================================================================
// #89 LENS DISSECTION — dumps the geometry of the equal-lens ∩ RESULT so we can
// decide WHERE the 0.785-vs-1.309 volume error lives. For two unit spheres at
// distance d=1 the lens is two spherical caps of height h=0.5. Each cap's curved
// area is 2πrh = π, so a CORRECT result has total surface area ≈ 2π ≈ 6.2832 and
// volume = 5π/12 ≈ 1.30900.
//
// We tessellate the result, bucket triangles by their B-Rep FaceId (face_map),
// and per face report: surface type, mesh area, area-weighted outward-normal
// sample, and the centroid. Then total mesh area + kernel analytic area + volume.
//
// DECISION RULE:
//   total area ≈ 6.28  → the two caps are the RIGHT SIZE; the wrong volume is an
//                        ORIENTATION/assembly defect (caps face wrong way, or the
//                        divergence integral signs/flat-disk closure is off).
//   total area ≠ 6.28  → the caps are the WRONG SIZE; tessellation is filling the
//                        wrong UV region of the sphere (e.g. the far spherical
//                        cap instead of the near lens cap), so the surface itself
//                        does not bound the lens.
// ===========================================================================
#[test]
#[ignore = "diagnostic — dissect the equal-lens INT result geometry (#89)"]
fn diag_equal_lens_geometry_dump() {
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

    // Build sphere([0,0,0],1) ∩ sphere([1,0,0],1) once, keep the model alive so
    // we can interrogate the RESULT solid's faces / surfaces / mesh.
    let mut model = BRepModel::new();
    let a = sphere(&mut model, [0.0, 0.0, 0.0], 1.0);
    let b = sphere(&mut model, [1.0, 0.0, 0.0], 1.0);
    let res = boolean_operation(
        &mut model,
        a,
        b,
        BooleanOp::Intersection,
        BooleanOptions::default(),
    )
    .expect("equal-lens INT must produce a result");

    // (1) number of faces + the face id list of the result's outer shell.
    let face_ids: Vec<u32> = {
        let solid = model.get_solid(res).expect("result solid");
        let shell = model.shells.get(solid.outer_shell).expect("outer shell");
        shell.faces.clone()
    };
    println!("\n========== #89 EQUAL-LENS ∩ RESULT GEOMETRY DUMP ==========");
    println!(
        "truth: volume = 5π/12 = {:.5} ; total surface area (two h=0.5 caps) = 2π = {:.5}",
        5.0 * std::f64::consts::PI / 12.0,
        2.0 * std::f64::consts::PI
    );
    println!(
        "(1) result outer-shell faces = {} : ids {:?}",
        face_ids.len(),
        face_ids
    );

    // (2) per-face surface type.
    println!("(2) per-face surface type:");
    for &fid in &face_ids {
        let ty = model
            .faces
            .get(fid)
            .and_then(|f| model.surfaces.get(f.surface_id))
            .map(|s| s.type_name())
            .unwrap_or("?");
        println!("    face {fid}: {ty}");
    }

    // Tessellate the WHOLE result (welded mesh, fine params so areas converge),
    // then bucket triangles by FaceId via face_map.
    let params = TessellationParams::fine();
    let mesh = {
        let solid = model.get_solid(res).expect("result solid");
        tessellate_solid(solid, &model, &params)
    };
    assert_eq!(
        mesh.face_map.len(),
        mesh.triangles.len(),
        "face_map must be parallel to triangles"
    );

    // Per-face accumulators: area, area-weighted normal, area-weighted centroid.
    use std::collections::BTreeMap;
    let mut per_face: BTreeMap<u32, (f64, [f64; 3], [f64; 3])> = BTreeMap::new();
    let mut total_area = 0.0_f64;
    for (tri, &fid) in mesh.triangles.iter().zip(mesh.face_map.iter()) {
        let p0 = mesh.vertices[tri[0] as usize].position;
        let p1 = mesh.vertices[tri[1] as usize].position;
        let p2 = mesh.vertices[tri[2] as usize].position;
        let e1 = [p1.x - p0.x, p1.y - p0.y, p1.z - p0.z];
        let e2 = [p2.x - p0.x, p2.y - p0.y, p2.z - p0.z];
        // cross(e1,e2)
        let cx = e1[1] * e2[2] - e1[2] * e2[1];
        let cy = e1[2] * e2[0] - e1[0] * e2[2];
        let cz = e1[0] * e2[1] - e1[1] * e2[0];
        let twice = (cx * cx + cy * cy + cz * cz).sqrt();
        let area = 0.5 * twice;
        total_area += area;
        // geometric (winding) outward normal of this triangle, unit.
        let n = if twice > 1e-15 {
            [cx / twice, cy / twice, cz / twice]
        } else {
            [0.0, 0.0, 0.0]
        };
        let centroid = [
            (p0.x + p1.x + p2.x) / 3.0,
            (p0.y + p1.y + p2.y) / 3.0,
            (p0.z + p1.z + p2.z) / 3.0,
        ];
        let e = per_face.entry(fid).or_insert((0.0, [0.0; 3], [0.0; 3]));
        e.0 += area;
        for k in 0..3 {
            e.1[k] += n[k] * area;
            e.2[k] += centroid[k] * area;
        }
    }

    // (3) per-face tessellated area + outward-normal sample (+ centroid).
    println!("(3) per-face tessellated area + area-weighted outward-normal winding sample:");
    for (fid, (area, nsum, csum)) in &per_face {
        let nlen = (nsum[0] * nsum[0] + nsum[1] * nsum[1] + nsum[2] * nsum[2]).sqrt();
        let n = if nlen > 1e-12 {
            [nsum[0] / nlen, nsum[1] / nlen, nsum[2] / nlen]
        } else {
            [0.0; 3]
        };
        let c = if *area > 1e-12 {
            [csum[0] / area, csum[1] / area, csum[2] / area]
        } else {
            [0.0; 3]
        };
        let ty = model
            .faces
            .get(*fid)
            .and_then(|f| model.surfaces.get(f.surface_id))
            .map(|s| s.type_name())
            .unwrap_or("?");
        println!(
            "    face {fid} ({ty}): area={:.5}  winding_normal=({:+.3},{:+.3},{:+.3})  centroid=({:+.3},{:+.3},{:+.3})",
            area, n[0], n[1], n[2], c[0], c[1], c[2]
        );
    }

    // (4) total surface area: mesh sum vs kernel analytic.
    let analytic_area = model
        .calculate_solid_surface_area(res)
        .expect("analytic surface area");
    println!(
        "(4) TOTAL surface area: mesh_sum={:.5}  kernel_analytic={:.5}  (expected 2π={:.5})",
        total_area,
        analytic_area,
        2.0 * std::f64::consts::PI
    );

    // (5) volume.
    let vol = model.calculate_solid_volume(res).expect("volume");
    println!(
        "(5) VOLUME: kernel={:.5}  (truth 5π/12={:.5})",
        vol,
        5.0 * std::f64::consts::PI / 12.0
    );

    // VERDICT line — compare mesh area to 2π.
    let two_pi = 2.0 * std::f64::consts::PI;
    let area_err = (total_area - two_pi) / two_pi;
    println!(
        "VERDICT: mesh_area={:.4} vs 2π={:.4} ({:+.1}%) → caps are {}",
        total_area,
        two_pi,
        100.0 * area_err,
        if area_err.abs() < 0.05 {
            "RIGHT SIZE  ⇒ volume error is ORIENTATION/ASSEMBLY"
        } else {
            "WRONG SIZE  ⇒ tessellation fills the wrong UV region"
        }
    );
    println!("===========================================================\n");
}

// ===========================================================================
// RATCHET GATE (#89 lens) — NON-ignored. Pins the two-part sphere∘sphere fix:
// the closed-form (Sphere,Sphere) intersection arm (db170f5) + the cap mesh
// winding fix (tessellate_spherical_cap apex-side handedness). A proper-overlap
// sphere∩sphere must produce the analytic lens volume, NOT the whole operand
// (4.19) and NOT the half-wound π/4 (0.785). Uses the exact analytic lens
// formula as an independent oracle (no grid).
// ===========================================================================

#[test]
fn sphere_sphere_lens_gate() {
    // Exact lens (intersection) volume of two spheres r0,r1 at centre distance d.
    fn lens_vol(r0: f64, r1: f64, d: f64) -> f64 {
        use std::f64::consts::PI;
        // proper overlap assumed: |r0-r1| < d < r0+r1
        PI * (r0 + r1 - d).powi(2) * (d * d + 2.0 * d * (r0 + r1) - 3.0 * (r0 - r1).powi(2))
            / (12.0 * d)
    }
    // (ca, ra, cb, rb, d) proper-overlap lenses.
    let cases: [([f64; 3], f64, [f64; 3], f64); 3] = [
        ([0.0, 0.0, 0.0], 1.0, [1.0, 0.0, 0.0], 1.0), // equal-lens d=1 → 5π/12
        ([0.0, 0.0, 0.0], 1.0, [0.8, 0.0, 0.0], 0.8), // offset-overlap
        ([0.0, 0.0, 0.0], 1.0, [0.5, 0.5, 0.5], 0.9), // diag-overlap
    ];
    for (ca, ra, cb, rb) in cases {
        let d =
            ((cb[0] - ca[0]).powi(2) + (cb[1] - ca[1]).powi(2) + (cb[2] - ca[2]).powi(2)).sqrt();
        let truth = lens_vol(ra, rb, d);
        // Serial run_pair (not run_pair_timed): these clean proper-overlap lenses
        // never hang, and the 4s thread budget of run_pair_timed produced false
        // "did not return" failures when this gate ran under full-suite CPU load.
        let f = run_pair(
            BooleanOp::Intersection,
            move |m| sphere(m, ca, ra),
            move |m| sphere(m, cb, rb),
        )
        .unwrap_or_else(|| panic!("sphere∩sphere a={ca:?}/{ra} b={cb:?}/{rb}: no result solid"));
        let rel = (f.vol - truth).abs() / truth;
        assert!(
            rel <= 0.02,
            "REGRESSION (#89 lens): sphere∩sphere a={ca:?}/{ra} b={cb:?}/{rb}: vol={:.5} analytic-lens={:.5} ({:+.1}%)",
            f.vol,
            truth,
            100.0 * (f.vol - truth) / truth
        );
        // Guard the two specific historical wrong answers.
        assert!(
            (f.vol - 4.18879).abs() > 0.1,
            "REGRESSION: returned a whole sphere (4.19) — #89 whole-operand bug is back"
        );
        assert!(
            (f.vol - 0.78540).abs() > 0.05 || rel <= 0.02,
            "REGRESSION: returned π/4 (0.785) — cap mesh winding bug is back"
        );
        assert_eq!(
            f.open_edges, 0,
            "lens not watertight: {} open edges",
            f.open_edges
        );
        assert_eq!(
            f.nonmanifold_edges, 0,
            "lens non-manifold: {} edges",
            f.nonmanifold_edges
        );
    }
}

// ===========================================================================
// POINT-MEMBERSHIP ORACLE (#91 Parasolid-grade). Volume/grid oracles are BLIND
// to a result with the right volume but the WRONG geometry — e.g. box∩sphere
// building the wrong hemisphere (both hemispheres have equal volume, so the
// volume oracle passes while the solid is geometrically wrong; that bug actually
// shipped during the cap-side work and the volume oracle could not see it). This
// oracle is INDEPENDENT of volume: sample random points and compare the result
// solid's ACTUAL membership (winding-number Shell::contains_point — a separate
// code path from the boolean) against the analytic EXPECTED membership. Points
// within ε of either operand boundary are skipped (contains_point triangle-fan-
// approximates curved faces; analytic membership is ambiguous at the boundary).
// ===========================================================================

/// `in_shape`, but `None` if the point is within `eps` of the shape boundary
/// (an ±eps corner perturbation flips membership) — classification is unstable
/// there, so the point must be skipped.
fn stable_in_shape(p: [f64; 3], s: ShapeSpec, eps: f64) -> Option<bool> {
    let base = in_shape(p, s);
    for &dx in &[-eps, eps] {
        for &dy in &[-eps, eps] {
            for &dz in &[-eps, eps] {
                if in_shape([p[0] + dx, p[1] + dy, p[2] + dz], s) != base {
                    return None;
                }
            }
        }
    }
    Some(base)
}

/// Möller–Trumbore ray–triangle intersection; `Some(t)` for a forward hit (t>ε).
fn ray_tri(o: Point3, d: Vector3, tri: &[Point3; 3]) -> Option<f64> {
    let e1 = tri[1] - tri[0];
    let e2 = tri[2] - tri[0];
    let h = d.cross(&e2);
    let a = e1.dot(&h);
    if a.abs() < 1e-12 {
        return None; // ray parallel to triangle
    }
    let f = 1.0 / a;
    let s = o - tri[0];
    let u = f * s.dot(&h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(&e1);
    let v = f * d.dot(&q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = f * e2.dot(&q);
    (t > 1e-9).then_some(t)
}

/// Inside-result test by RAY PARITY against the result's triangle mesh — exact
/// for the actual result geometry (unlike Shell::contains_point's winding-number
/// fan, which is only accurate for planar faces and misclassifies ~20-50% of
/// points near curved surfaces). Odd crossings of a fixed oblique ray = inside.
fn inside_mesh(p: [f64; 3], dir: Vector3, tris: &[[Point3; 3]]) -> bool {
    let o = Point3::new(p[0], p[1], p[2]);
    let crossings = tris.iter().filter(|t| ray_tri(o, dir, t).is_some()).count();
    crossings % 2 == 1
}

/// (points_checked, mismatches) for box∘B membership, or None on kernel error.
fn membership_check(
    op: BooleanOp,
    b: ShapeSpec,
    seed: u64,
    k: usize,
    eps: f64,
) -> Option<(usize, usize)> {
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    let mut model = BRepModel::new();
    let bx = the_box(&mut model);
    let bsolid = build_shape(&mut model, b);
    let res = boolean_operation(&mut model, bx, bsolid, op, BooleanOptions::default()).ok()?;

    // Tessellate the result once; classify points by ray parity against its mesh.
    let mesh = {
        let solid = model.get_solid(res)?;
        tessellate_solid(solid, &model, &TessellationParams::fine())
    };
    let tris: Vec<[Point3; 3]> = mesh
        .triangles
        .iter()
        .map(|t| {
            [
                mesh.vertices[t[0] as usize].position,
                mesh.vertices[t[1] as usize].position,
                mesh.vertices[t[2] as usize].position,
            ]
        })
        .collect();
    if tris.is_empty() {
        return None;
    }
    // Fixed oblique ray — avoids axis-aligned edge-on degeneracies.
    let dir = Vector3::new(0.273_1, 0.512_7, 0.814_3);

    let (bmin, bmax) = shape_aabb(b);
    let reach = (0..3)
        .map(|i| bmin[i].abs().max(bmax[i].abs()).max(BOX_HALF))
        .fold(0.1, f64::max)
        + 0.1;

    let mut rng = Rng::new(seed);
    let (mut checked, mut mism) = (0usize, 0usize);
    for _ in 0..k {
        let p = [
            rng.range(-reach, reach),
            rng.range(-reach, reach),
            rng.range(-reach, reach),
        ];
        let (Some(in_a), Some(in_b)) = (
            stable_in_shape(p, ShapeSpec::Box, eps),
            stable_in_shape(p, b, eps),
        ) else {
            continue; // near a boundary — skip
        };
        let expected = match op {
            BooleanOp::Intersection => in_a && in_b,
            BooleanOp::Union => in_a || in_b,
            BooleanOp::Difference => in_a && !in_b,
        };
        checked += 1;
        if inside_mesh(p, dir, &tris) != expected {
            mism += 1;
        }
    }
    Some((checked, mism))
}

#[test]
#[ignore = "fuzz survey — point-membership oracle (mesh-free correctness; catches wrong-geometry-right-volume)"]
fn boolean_box_membership_survey() {
    const K: usize = 4000;
    const SEED: u64 = 0xBEEF_FACE_1234_5678;
    let eps = 0.04;
    let configs: [(ShapeSpec, &str); 8] = [
        (
            ShapeSpec::Sphere {
                c: [0.0, 0.0, 0.0],
                r: 0.8,
            },
            "sphere-contained",
        ),
        (
            ShapeSpec::Sphere {
                c: [1.0, 0.0, 0.0],
                r: 0.5,
            },
            "sphere-face-poke",
        ),
        (
            ShapeSpec::Sphere {
                c: [1.0, 1.0, 1.0],
                r: 0.8,
            },
            "sphere-corner",
        ),
        (
            ShapeSpec::Cyl {
                base: [0.0, 0.0, -0.5],
                r: 0.5,
                h: 1.0,
            },
            "cyl-contained",
        ),
        (
            ShapeSpec::Cyl {
                base: [1.0, 0.0, -0.5],
                r: 0.5,
                h: 1.0,
            },
            "cyl-face",
        ),
        (
            ShapeSpec::Cone {
                bc: [0.0, 0.0, -0.5],
                rb: 0.5,
                rt: 0.0,
                h: 1.0,
            },
            "cone-contained",
        ),
        (
            ShapeSpec::Torus {
                c: [0.0, 0.0, 0.0],
                rmaj: 0.6,
                rmin: 0.25,
            },
            "torus-centered",
        ),
        (
            ShapeSpec::Sphere {
                c: [0.5, 0.0, 0.0],
                r: 0.8,
            },
            "sphere-straddle",
        ),
    ];
    let ops = [
        (BooleanOp::Intersection, "I"),
        (BooleanOp::Union, "U"),
        (BooleanOp::Difference, "D"),
    ];
    println!("\n=== #91 box∘B POINT-MEMBERSHIP oracle (K={K} pts/cfg, eps={eps}) ===");
    let mut worst = 0.0f64;
    let mut total_mism = 0usize;
    for (i, (b, name)) in configs.iter().enumerate() {
        for (op, sym) in ops {
            let seed = SEED ^ ((i as u64) << 16) ^ (sym.len() as u64);
            match membership_check(op, *b, seed, K, eps) {
                Some((checked, mism)) if checked > 0 => {
                    let rate = mism as f64 / checked as f64;
                    worst = worst.max(rate);
                    total_mism += mism;
                    if mism > 0 {
                        println!(
                            "  [{sym}] {name}: {mism}/{checked} mismatches ({:.1}%)",
                            100.0 * rate
                        );
                    }
                }
                Some(_) => println!("  [{sym}] {name}: 0 points checked"),
                None => println!("  [{sym}] {name}: kernel error / non-closed result"),
            }
        }
    }
    println!(
        "total mismatches={total_mism}  worst rate={:.1}%",
        100.0 * worst
    );
    println!("=== end ===\n");
}

// ===========================================================================
// MEMBERSHIP RATCHET GATE (#91) — NON-ignored. Locks the point-membership
// oracle's verdict on the CONQUERED-correct cells: their result geometry is
// right, so ray-parity membership must match the analytic expectation. This
// catches a wrong-geometry-RIGHT-volume regression (e.g. the cap-side hemisphere
// bug) on conquered ground — which the volume gates are structurally blind to.
// ===========================================================================

#[test]
fn box_membership_conquered_gate() {
    let cells: [(ShapeSpec, &str); 4] = [
        (
            ShapeSpec::Sphere {
                c: [0.0, 0.0, 0.0],
                r: 0.8,
            },
            "sphere-contained",
        ),
        (
            ShapeSpec::Cyl {
                base: [0.0, 0.0, -0.5],
                r: 0.5,
                h: 1.0,
            },
            "cyl-contained",
        ),
        (
            ShapeSpec::Cone {
                bc: [0.0, 0.0, -0.5],
                rb: 0.5,
                rt: 0.0,
                h: 1.0,
            },
            "cone-contained",
        ),
        (
            ShapeSpec::Torus {
                c: [0.0, 0.0, 0.0],
                rmaj: 0.6,
                rmin: 0.25,
            },
            "torus-centered",
        ),
    ];
    let ops = [
        BooleanOp::Intersection,
        BooleanOp::Union,
        BooleanOp::Difference,
    ];
    const K: usize = 1500;
    let eps = 0.04;
    for (i, (b, name)) in cells.iter().enumerate() {
        for (j, op) in ops.iter().enumerate() {
            let seed = 0x6A7E_5EED_0000_0000 ^ ((i as u64) << 8) ^ (j as u64);
            let (checked, mism) = membership_check(*op, *b, seed, K, eps)
                .unwrap_or_else(|| panic!("box∘{name} op#{j}: membership_check returned None"));
            assert!(
                checked > 100,
                "box∘{name} op#{j}: too few points checked ({checked})"
            );
            let rate = mism as f64 / checked as f64;
            assert!(
                rate <= 0.015,
                "REGRESSION (membership): box∘{name} op#{j}: {mism}/{checked} point-membership \
                 mismatches ({:.1}%) — the result GEOMETRY is wrong (a right volume can hide this)",
                100.0 * rate
            );
        }
    }
}

// ===========================================================================
// BOOL-90 RATCHET GATE — NON-ignored. Pins the box∖sphere single-face-poke
// (BOOL-90-FIX): sphere(center=[1,0,0], r=0.5) pokes the +x box face, so the
// sphere surface inside the box is exactly a hemisphere of volume
// (2/3)π r³ = 0.2618. Before the cap-winding fix the spherical-cap apex/winding
// desynced at the great circle (h=0) and the cap flux cancelled to 0, so DIFF
// reported 8.262 (the UNION value) instead of 7.738. Locks all three ops + the
// watertight invariant on this exact config. Serial run_op (correctness gates
// do NOT run under a wall-clock thread budget — that races and false-fails).
// ===========================================================================

#[test]
fn box_minus_sphere_face_poke_gate() {
    let c = [1.0, 0.0, 0.0];
    let r = 0.5;
    let cap = 2.0 / 3.0 * std::f64::consts::PI * r * r * r; // inside hemisphere ≈ 0.2618
    let box_vol = (2.0 * BOX_HALF).powi(3); // 8.0
    let tol = 0.05; // tessellation discretization; the bug swings a full ±cap (0.26 ≫ tol)
    let cases: [(BooleanOp, &str, f64); 3] = [
        (BooleanOp::Intersection, "∩ (inside hemisphere)", cap),
        (
            BooleanOp::Union,
            "∪ (box + outside hemisphere)",
            box_vol + cap,
        ),
        (
            BooleanOp::Difference,
            "∖ (box − inside hemisphere)",
            box_vol - cap,
        ),
    ];
    for (op, name, expect) in cases {
        let f = run_op(op, |m| sphere(m, c, r))
            .unwrap_or_else(|| panic!("box {name}: kernel returned None (boolean error)"));
        assert!(
            (f.vol - expect).abs() <= tol,
            "REGRESSION (BOOL-90): box {name}: vol={:.4}, expected {:.4} (±{tol}) — \
             spherical-cap apex/winding desync (great-circle h=0) collapses the cap flux",
            f.vol,
            expect
        );
        assert!(
            f.open_edges == 0 && f.nonmanifold_edges == 0,
            "REGRESSION (BOOL-90): box {name}: not watertight ({} open, {} non-manifold edges)",
            f.open_edges,
            f.nonmanifold_edges
        );
    }
}

// ===========================================================================
// SPHERE-CORNER RATCHET GATE — NON-ignored. Pins box ∘ sphere([1,1,1], r=0.8):
// the sphere sits EXACTLY on the +++ corner vertex, so all 3 cut circles are
// great circles through the centre — the degenerate 3-great-circle arrangement
// that (a) only splits if `split_sphere_face_by_circles` accepts `Arc` cuts,
// (b) needs a complement-aware interior point so the 7/8 petal classifies
// Outside, and (c) needs `tessellate_spherical_polygon` to fan from the region
// centroid (not the rim centroid) so the >hemisphere petal meshes outward. Locks
// ∩≈0.268 / ∪≈9.877 / ∖≈7.732 + watertight. Serial `run_op` (correctness gates
// never run under a wall-clock thread budget).
// ===========================================================================
#[test]
fn sphere_corner_union_gate() {
    let c = [1.0, 1.0, 1.0];
    let r = 0.8_f64;
    let cap_in = (4.0 / 3.0 * std::f64::consts::PI * r * r * r) / 8.0; // inside octant
    let box_vol = (2.0 * BOX_HALF).powi(3);
    let tol = 0.05;
    let cases: [(BooleanOp, &str, f64); 3] = [
        (BooleanOp::Intersection, "∩ (inside octant)", cap_in),
        (
            BooleanOp::Union,
            "∪ (box + 7/8 sphere)",
            box_vol + 7.0 * cap_in,
        ),
        (
            BooleanOp::Difference,
            "∖ (box − inside octant)",
            box_vol - cap_in,
        ),
    ];
    for (op, name, expect) in cases {
        let f = run_op(op, |m| sphere(m, c, r))
            .unwrap_or_else(|| panic!("sphere-corner {name}: kernel returned None"));
        assert!(
            (f.vol - expect).abs() <= tol,
            "REGRESSION (sphere-corner): {name}: vol={:.4}, expected {:.4} (±{tol}) — the \
             3-great-circle corner arrangement dropped/mis-meshed the 7/8 petal",
            f.vol,
            expect
        );
        assert!(
            f.open_edges == 0 && f.nonmanifold_edges == 0,
            "REGRESSION (sphere-corner): {name}: not watertight ({} open, {} non-manifold)",
            f.open_edges,
            f.nonmanifold_edges
        );
    }
}

// ===========================================================================
// SPHERE-NEAR-VERTEX RATCHET GATE — NON-ignored. BOOL-CORNER-FIX generalised
// beyond the exact corner vertex: a sphere just INSIDE the corner ([0.9,0.9,0.9])
// or straddling an edge with an offset ([1,1,0.3]) cuts 3 NON-great circles that
// still mutually intersect, and the complement-aware interior point + region-
// centroid fan handle them too. These cells were broken pre-fix (∪ dropped the
// petal) and are now exact — lock them at grid truth (5% tol) + watertight.
// ===========================================================================
#[test]
fn sphere_near_vertex_union_gate() {
    let cells: [([f64; 3], f64, &str); 3] = [
        ([0.9, 0.9, 0.9], 0.8, "corner-inside"),
        ([1.0, 1.0, 0.3], 0.8, "edge+x+y-off"),
        // BOOL-EDGE-POKE-MESH (#92): sphere pokes a box EDGE — the kept ∪ region is
        // most of the sphere ringed by a small two-arc lens (sphere-minus-lens). The
        // centroid fan rejects it (rim near-antipodal to the region centroid); the
        // large-region grid+winding+stitch path meshes it watertight + exact. Was
        // 7.91 vs 11.59 (−32%) before the fix.
        ([1.6, 1.6, 0.0], 0.95, "poke-edge"),
    ];
    let ops: [(BooleanOp, &str, fn(&GridTruth) -> f64); 3] = [
        (BooleanOp::Intersection, "∩", |g| g.intersection),
        (BooleanOp::Union, "∪", |g| g.union),
        (BooleanOp::Difference, "∖", |g| g.difference),
    ];
    let tol = 0.05;
    for (c, r, name) in cells {
        let truth = grid_truth(c, r);
        for &(op, sym, pick) in &ops {
            let t = pick(&truth);
            let f = run_op(op, move |m| sphere(m, c, r))
                .unwrap_or_else(|| panic!("near-vertex {name} {sym}: kernel None"));
            let rel = (f.vol - t).abs() / t.max(1e-3);
            assert!(
                rel <= tol,
                "REGRESSION (near-vertex): {name} {sym}: vol={:.4} truth={t:.4} ({:+.1}%, tol {:.0}%)",
                f.vol,
                100.0 * (f.vol - t) / t,
                100.0 * tol
            );
            assert!(
                f.open_edges == 0 && f.nonmanifold_edges == 0,
                "REGRESSION (near-vertex): {name} {sym}: not watertight ({} open)",
                f.open_edges
            );
        }
    }
}

// ===========================================================================
// SPHERE-CONTAINMENT RATCHET GATE — NON-ignored. Locks the #90 tangent/contained
// fix: a sphere strictly INSIDE the box (r=0.8) and a sphere TANGENT to all 6
// faces (r=1.0) must give exact ∩/∪/∖ + watertight. The tangent case was the
// face-drop bug (box∩sphere → box 8.0 not 4.19; box∖sphere → dropped faces),
// fixed by the isolated-contact multi-sample classifier (50f0480). The contained
// case guards the un-touched strict-interior path. Inclusion-exclusion
// (V(∩)+V(∪)=V(A)+V(B)) is implied by all three matching grid truth.
// ===========================================================================
#[test]
fn sphere_containment_gate() {
    let cells: [([f64; 3], f64, &str); 2] = [
        ([0.0, 0.0, 0.0], 0.8, "contained"),
        ([0.0, 0.0, 0.0], 1.0, "tangent-6face"),
    ];
    let ops: [(BooleanOp, &str, fn(&GridTruth) -> f64); 3] = [
        (BooleanOp::Intersection, "∩", |g| g.intersection),
        (BooleanOp::Union, "∪", |g| g.union),
        (BooleanOp::Difference, "∖", |g| g.difference),
    ];
    let tol = 0.05;
    for (c, r, name) in cells {
        let truth = grid_truth(c, r);
        for &(op, sym, pick) in &ops {
            let t = pick(&truth);
            let f = run_op(op, move |m| sphere(m, c, r))
                .unwrap_or_else(|| panic!("containment {name} {sym}: kernel None"));
            let rel = (f.vol - t).abs() / t.max(1e-3);
            assert!(
                rel <= tol,
                "REGRESSION (containment): {name} {sym}: vol={:.4} truth={t:.4} ({:+.1}%, tol {:.0}%)",
                f.vol,
                100.0 * (f.vol - t) / t,
                100.0 * tol
            );
            assert!(
                f.open_edges == 0 && f.nonmanifold_edges == 0,
                "REGRESSION (containment): {name} {sym}: not watertight ({} open)",
                f.open_edges
            );
        }
    }
}

// ===========================================================================
// MULTI-COMPONENT POKE-THROUGH RATCHET GATE — NON-ignored. Locks the #88
// interior-offset fix (`assemble_multi_component_sphere_regions` +
// `tessellate_spherical_holed_region`): sphere c=[0.5,0.3,0] r=1.05 pokes both
// z faces (two DISJOINT cut circles) AND the x=1/y=1 edge (a 2-arc lens) — a
// 3-component cut graph whose regions must be nested as holes, deduped, and
// tessellated with hint-signed half-space membership. Before the fix: ∩
// hard-errored ("component 0 has only 1 planar face"), ∪ had 6 non-manifold
// edges at +52% volume. All three ops must match grid truth + be watertight.
// ===========================================================================
#[test]
fn sphere_multi_component_poke_gate() {
    // r=1.05: 3-component cut graph (2 lone z circles + an edge lens) —
    // pins the multi-component region assembly + holed tessellation.
    // r=1.2: single-component hub-and-satellites web (x=1 hub circle crossed
    // by 3 satellite arcs, 5 regions) — pins the verified-interior fallback
    // (the inside-box region's mean-of-midpoints lands outside the region
    // and classified Outside before the fix: ∩/∖ open=8, ∪ nonman=8).
    let cells: [([f64; 3], f64, &str); 3] = [
        ([0.5, 0.3, 0.0], 1.05, "multi-component"),
        ([0.5, 0.3, 0.0], 1.2, "hub-satellites"),
        // Sphere centred ON the +x face: great-circle cut + a chord-chain
        // bite — a UNION-type region ((y>1 ∨ z<−1) ∧ x<1) whose mean interior
        // lands in-box and whose hemisphere twin's mean lands ON the cut
        // plane. Pins the parity-membership interior verification.
        ([1.0, 0.3, -0.2], 1.2, "face-great-circle"),
    ];
    let ops: [(BooleanOp, &str, fn(&GridTruth) -> f64); 3] = [
        (BooleanOp::Intersection, "∩", |g| g.intersection),
        (BooleanOp::Union, "∪", |g| g.union),
        (BooleanOp::Difference, "∖", |g| g.difference),
    ];
    let tol = 0.05;
    for (c, r, name) in cells {
        let truth = grid_truth(c, r);
        for &(op, sym, pick) in &ops {
            let t = pick(&truth);
            let f = run_op(op, move |m| sphere(m, c, r))
                .unwrap_or_else(|| panic!("poke-through {name} {sym}: kernel None"));
            let rel = (f.vol - t).abs() / t.max(1e-3);
            assert!(
                rel <= tol,
                "REGRESSION (poke-through): {name} {sym}: vol={:.4} truth={t:.4} ({:+.1}%, tol {:.0}%)",
                f.vol,
                100.0 * (f.vol - t) / t,
                100.0 * tol
            );
            assert!(
                f.open_edges == 0 && f.nonmanifold_edges == 0,
                "REGRESSION (poke-through): {name} {sym}: not watertight ({} open, {} nonmanifold)",
                f.open_edges,
                f.nonmanifold_edges
            );
        }
    }
}

// ===========================================================================
// SPHERE-CORNER ∪ DIAGNOSTIC — next-worst cap-side target after BOOL-90-FIX.
// Sphere centred EXACTLY on the +++ box corner vertex (1,1,1), r=0.8: by
// symmetry exactly 1/8 of the sphere sits inside the box, so
//   box ∪ sphere = 8 + 7/8·(4/3·π·0.8³) ≈ 9.877
//   box ∩ sphere =       1/8·(4/3·π·0.8³) ≈ 0.268
//   box ∖ sphere = 8   − 1/8·(...)        ≈ 7.732
// The membership oracle flags this ∪ cell at 7.6% point-mismatch — right-ish
// volume, WRONG geometry (the sphere pokes 3 faces at once → a 3-cut corner
// cap arrangement; a wrong-hemisphere / dropped-petal cap mis-meshes). Run
// with `--ignored --nocapture` to read volume + watertight + membership.
// ===========================================================================
#[test]
#[ignore = "diagnostic — sphere-corner ∪ membership 7.6% (run with --ignored --nocapture)"]
fn diag_sphere_corner_union() {
    let c = [1.0, 1.0, 1.0];
    let r = 0.8_f64;
    let cap_in = (4.0 / 3.0 * std::f64::consts::PI * r * r * r) / 8.0; // inside octant
    let expect = [
        (BooleanOp::Intersection, "∩", cap_in),
        (BooleanOp::Union, "∪", 8.0 + 7.0 * cap_in),
        (BooleanOp::Difference, "∖", 8.0 - cap_in),
    ];
    for (op, sym, want) in expect {
        let facts = run_op(op, |m| sphere(m, c, r));
        match facts {
            Some(f) => println!(
                "sphere-corner {sym}: vol={:.4} (want {:.4}, Δ={:+.4})  open={}  nonmanifold={}  euler_res={}",
                f.vol,
                want,
                f.vol - want,
                f.open_edges,
                f.nonmanifold_edges,
                f.euler_residual
            ),
            None => println!("sphere-corner {sym}: kernel ERROR (None)"),
        }
    }
    // Point-membership: independent of volume — catches wrong-hemisphere.
    let b = ShapeSpec::Sphere { c, r };
    for (op, sym) in [
        (BooleanOp::Intersection, "∩"),
        (BooleanOp::Union, "∪"),
        (BooleanOp::Difference, "∖"),
    ] {
        match membership_check(op, b, 0x5125_E000 ^ sym.len() as u64, 4000, 0.04) {
            Some((checked, mism)) => println!(
                "sphere-corner {sym}: membership {mism}/{checked} mismatches ({:.1}%)",
                100.0 * mism as f64 / checked.max(1) as f64
            ),
            None => println!("sphere-corner {sym}: membership None"),
        }
    }
}

// NEXT-WORST after BOOL-CORNER-FIX: the NEAR-vertex union cells the corner-vertex
// fix does NOT cover (cuts are non-great circles). Reports vol vs grid truth +
// watertight + membership for each (centre, r) across the 3 ops.
#[test]
#[ignore = "diagnostic — near-vertex union (run with --ignored --nocapture)"]
fn diag_sphere_near_vertex() {
    let cases: [([f64; 3], f64, &str); 3] = [
        ([0.9, 0.9, 0.9], 0.8, "corner-inside"),
        ([1.6, 1.6, 0.0], 0.95, "poke-edge"),
        ([1.0, 1.0, 0.3], 0.8, "edge+x+y-off"),
    ];
    for (c, r, name) in cases {
        let truth = grid_truth(c, r);
        let b = ShapeSpec::Sphere { c, r };
        for (op, sym, t) in [
            (BooleanOp::Intersection, "∩", truth.intersection),
            (BooleanOp::Union, "∪", truth.union),
            (BooleanOp::Difference, "∖", truth.difference),
        ] {
            let v = run_op(op, |m| sphere(m, c, r));
            let mem = membership_check(op, b, 0x9A7E ^ sym.len() as u64, 3000, 0.04);
            match v {
                Some(f) => println!(
                    "{name} {sym}: vol={:.4} truth={t:.4} ({:+.1}%) open={} nonman={} | membership={}",
                    f.vol,
                    100.0 * (f.vol - t) / t.max(1e-3),
                    f.open_edges,
                    f.nonmanifold_edges,
                    mem.map(|(ch, mi)| format!("{:.1}%", 100.0 * mi as f64 / ch.max(1) as f64))
                        .unwrap_or_else(|| "None".into())
                ),
                None => println!("{name} {sym}: kernel ERROR (None)"),
            }
        }
    }
}

// Instrument the 3 OPEN (single-use) edges of the sphere-corner UNION: dump
// each open edge's endpoints, midpoint (arc vs chord ⇒ Circle vs Line), curve
// id, and param range. Confirms whether the unwelded edges are great-circle
// arcs and whether two of them are coincident-but-distinct (subdivision
// mismatch) vs genuinely single-use. READ-ONLY (no kernel mutation).
#[test]
#[ignore = "diagnostic — dump sphere-corner ∪ open edges (run with --ignored --nocapture)"]
fn diag_sphere_corner_union_open_edges() {
    let mut model = BRepModel::new();
    let bx = the_box(&mut model);
    let sp = sphere(&mut model, [1.0, 1.0, 1.0], 0.8);
    let res = match boolean_operation(
        &mut model,
        bx,
        sp,
        BooleanOp::Union,
        BooleanOptions::default(),
    ) {
        Ok(r) => r,
        Err(e) => {
            println!("union errored: {e:?}");
            return;
        }
    };
    let rep = brep_integrity(&model, res, 1e-6);
    println!(
        "\n=== sphere-corner ∪ open edges: {} single-use, {} non-manifold ===",
        rep.edges_used_once.len(),
        rep.edges_used_3plus.len()
    );
    let pos = |m: &BRepModel, vid| {
        m.vertices
            .get_position(vid)
            .map(|p| [p[0], p[1], p[2]])
            .unwrap_or([f64::NAN; 3])
    };
    for &eid in &rep.edges_used_once {
        let Some(edge) = model.edges.get(eid) else {
            continue;
        };
        let s = pos(&model, edge.start_vertex);
        let e = pos(&model, edge.end_vertex);
        let chord_mid = [
            0.5 * (s[0] + e[0]),
            0.5 * (s[1] + e[1]),
            0.5 * (s[2] + e[2]),
        ];
        let (kind, mid) = match model.curves.get(edge.curve_id) {
            Some(curve) => {
                let t = 0.5 * (edge.param_range.start + edge.param_range.end);
                match curve.evaluate(t) {
                    Ok(p) => {
                        let m = [p.position.x, p.position.y, p.position.z];
                        let bow = ((m[0] - chord_mid[0]).powi(2)
                            + (m[1] - chord_mid[1]).powi(2)
                            + (m[2] - chord_mid[2]).powi(2))
                        .sqrt();
                        (if bow > 1e-6 { "ARC" } else { "LINE" }, m)
                    }
                    Err(_) => ("?", [f64::NAN; 3]),
                }
            }
            None => ("?", [f64::NAN; 3]),
        };
        println!(
            "  edge {:?} curve {:?} {kind}: start={:.3?} end={:.3?} mid={:.3?} range=[{:.3},{:.3}]",
            eid, edge.curve_id, s, e, mid, edge.param_range.start, edge.param_range.end
        );
    }

    // Face walk: for every face touching the corner triangle (any edge endpoint
    // near A=(0.2,1,1), B=(1,0.2,1), C=(1,1,0.2)), print its surface type + the
    // edges it owns. This shows whether a box PLANE face carries a partner bite
    // arc at A-B-C (so the weld merely failed) or the box faces were never bitten.
    let corners = [[0.2, 1.0, 1.0], [1.0, 0.2, 1.0], [1.0, 1.0, 0.2]];
    let near_corner = |p: [f64; 3]| {
        corners.iter().any(|q| {
            (p[0] - q[0]).abs() < 0.02 && (p[1] - q[1]).abs() < 0.02 && (p[2] - q[2]).abs() < 0.02
        })
    };
    let Some(solid) = model.solids.get(res) else {
        return;
    };
    let mut shells = vec![solid.outer_shell];
    shells.extend(solid.inner_shells.iter().copied());
    // Surface-type tally: 7 external octants expected on the sphere; fewer ⇒
    // corner-adjacent octants were dropped (classification), equal ⇒ unwelded.
    {
        let mut by_ty: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for &sh in &shells {
            if let Some(shell) = model.shells.get(sh) {
                for &fid in &shell.faces {
                    if let Some(face) = model.faces.get(fid) {
                        let ty = model
                            .surfaces
                            .get(face.surface_id)
                            .map(|s| s.type_name())
                            .unwrap_or("?");
                        *by_ty.entry(ty).or_default() += 1;
                    }
                }
            }
        }
        println!("--- result faces by surface type: {by_ty:?} ---");
    }
    println!("--- faces touching the A/B/C corner triangle ---");
    for sh in shells {
        let Some(shell) = model.shells.get(sh) else {
            continue;
        };
        for &fid in &shell.faces {
            let Some(face) = model.faces.get(fid) else {
                continue;
            };
            let sty = model
                .surfaces
                .get(face.surface_id)
                .map(|s| s.type_name())
                .unwrap_or("?");
            let mut lids = vec![face.outer_loop];
            lids.extend(face.inner_loops.iter().copied());
            let mut touches = false;
            let mut edge_descs: Vec<String> = Vec::new();
            for lid in &lids {
                let Some(lp) = model.loops.get(*lid) else {
                    continue;
                };
                for &eid in &lp.edges {
                    let Some(edge) = model.edges.get(eid) else {
                        continue;
                    };
                    let s = pos(&model, edge.start_vertex);
                    let e = pos(&model, edge.end_vertex);
                    if near_corner(s) || near_corner(e) {
                        touches = true;
                        edge_descs.push(format!(
                            "e{:?}(c{:?}) {:.2?}->{:.2?}",
                            eid, edge.curve_id, s, e
                        ));
                    }
                }
            }
            if touches {
                println!(
                    "  face {:?} surf={sty} loops={} corner_edges=[{}]",
                    fid,
                    lids.len(),
                    edge_descs.join(", ")
                );
            }
        }
    }
}

// #90 BOOL-∩-CONTAINMENT diag: the interior-sphere HARD cluster. Surfaces the
// ACTUAL error string (run_op swallows it to None) + the wrong volume per op for
// each interior/engulf cell, so the root cause is read from data, not guessed.
// Box is [-1,1]^3 (vol 8). r=1 → sphere tangent to all 6 faces (contained);
// r=1.8/2.2 → sphere engulfs the box (corner dist √3≈1.73); r=1.05/1.2 →
// poke-through. Run with `--ignored --nocapture` (set ROSHERA_BOOL_TRACE=1 for the
// pipeline stage trace on the cell of interest).
#[test]
#[ignore = "diagnostic — interior/containment ∩/∖ errors (run with --ignored --nocapture)"]
fn diag_sphere_interior_containment() {
    let cases: [([f64; 3], f64, &str); 5] = [
        ([0.0, 0.0, 0.0], 1.0, "interior-centre-r1-tangent"),
        ([0.0, 0.0, 0.0], 1.8, "interior-centre-r1.8-engulf"),
        ([0.0, 0.0, 0.0], 2.2, "interior-centre-r2.2-engulf"),
        ([0.5, 0.3, 0.0], 1.05, "interior-offset-r1.05-poke"),
        ([0.5, 0.3, 0.0], 1.2, "interior-offset-r1.2-poke"),
    ];
    let pi = std::f64::consts::PI;
    for (c, r, name) in cases {
        let vsphere = 4.0 / 3.0 * pi * r * r * r;
        println!("\n=== {name}  c={c:?} r={r}  (V_sphere={vsphere:.4}) ===");
        for (op, sym) in [
            (BooleanOp::Intersection, "∩"),
            (BooleanOp::Union, "∪"),
            (BooleanOp::Difference, "∖"),
        ] {
            let mut model = BRepModel::new();
            let bx = the_box(&mut model);
            let sp = sphere(&mut model, c, r);
            match boolean_operation(&mut model, bx, sp, op, BooleanOptions::default()) {
                Ok(res) => {
                    let vol = model.calculate_solid_volume(res).unwrap_or(f64::NAN);
                    let rep = brep_integrity(&model, res, 1e-6);
                    println!(
                        "  {sym}: OK vol={vol:.4} open={} nonman={}",
                        rep.edges_used_once.len(),
                        rep.edges_used_3plus.len()
                    );
                }
                Err(e) => println!("  {sym}: ERR {e:?}"),
            }
        }
    }
}
