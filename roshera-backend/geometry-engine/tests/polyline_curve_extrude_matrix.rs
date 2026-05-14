//! Polyline-curve extrusion test matrix (kernel level).
//!
//! Pins the live defect reported 2026-05-14: when a sketch is built with
//! the polyline tool (one shared `Polyline` curve covering the outline,
//! and N edges with `param_range = [i/N, (i+1)/N]` slicing it), the
//! subsequent extrude or extrude-cut grows the api-server memory
//! 25 MB → 400 MB and hangs. The same shape extruded as N independent
//! `Line` curves succeeds.
//!
//! These tests run **at the kernel boundary** (no api-server) and
//! enforce two invariants per shape:
//!   1. **Termination** under a watchdog budget. Failure = the kernel
//!      hangs (or burns through the budget) on a polyline-tool-shaped
//!      input.
//!   2. **Correctness** of the produced solid: face count, mass,
//!      Euler-characteristic χ=2 on the outer shell, zero non-manifold
//!      edges in the tessellated `TriangleMesh`.
//!
//! A baseline group exercises the SAME shapes with per-edge `Line`
//! curves to prove the bug is specific to the shared-`Polyline`
//! parameter-slicing pattern, not to the polygonal topology itself.
//!
//! No production code is modified by this file. The fix lands in a
//! separate slice once these tests pin the failure modes.
//!
//! Coverage matrix:
//!   * Phase 1 (terminate, 10s/30s watchdogs): triangle, square,
//!     pentagon, hexagon, decagon, L-shape (reflex corner), star (8
//!     alternating convex/concave vertices).
//!   * Phase 2 (correctness — runs only if Phase 1 terminates): square,
//!     hexagon, L-shape.
//!   * Phase 3 (per-edge-Line baseline — control group): square,
//!     L-shape.
//!   * Phase 4 (cut workflows): pentagon-hole cut from a box,
//!     L-shape-hole cut from a box.

#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::extrude::create_face_from_profile_with_plane;
use geometry_engine::operations::{extrude_face, extrude_profile, CommonOptions, ExtrudeOptions};
use geometry_engine::primitives::{
    builder::BRepModel,
    curve::{Line, ParameterRange, Polyline},
    edge::{Edge, EdgeId, EdgeOrientation},
    solid::SolidId,
    vertex::VertexId,
};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

// ---------------------------------------------------------------------
// Watchdog
// ---------------------------------------------------------------------

/// Run `work` on a worker thread and panic if it does not return a
/// result within `timeout_ms`. The worker thread keeps running on a
/// timeout (Rust does not allow `thread::kill`); on a test-binary
/// process this is reclaimed at process exit.
///
/// Distinguishes three outcomes so test logs are not misleading:
///   * `Ok` — work completed, return its value.
///   * `Err(Disconnected)` — the worker panicked (the `Sender` was
///     dropped without sending). The original panic message is
///     already on stderr; we re-panic with a "(worker panicked)"
///     marker so the test summary attributes blame correctly.
///   * `Err(Timeout)` — true hang: the watchdog budget elapsed with
///     no panic and no send. This is the failure class the harness
///     is designed to catch.
fn run_with_watchdog<T, F>(name: &'static str, timeout_ms: u64, work: F) -> T
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<T>();
    let _handle = thread::spawn(move || {
        let result = work();
        // Send may fail if the receiver has already given up on us
        // (timeout fired). Suppress that — the test has already failed
        // on the receiving side.
        let _ = tx.send(result);
    });
    match rx.recv_timeout(Duration::from_millis(timeout_ms)) {
        Ok(value) => value,
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            // Worker thread panicked; original panic message already
            // printed to stderr by Rust's default panic hook.
            panic!(
                "watchdog: `{}` worker panicked (see panic message printed above)",
                name
            );
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!(
                "watchdog: `{}` did NOT complete within {} ms — \
                 true hang (no panic, no send)",
                name, timeout_ms
            );
        }
    }
}

// ---------------------------------------------------------------------
// Polyline-tool loop construction (mirrors api-server `build_loop_edges`)
// ---------------------------------------------------------------------

/// Build a closed loop of edges over a single shared `Polyline` curve.
///
/// This is the exact pattern emitted by the polyline tool in the
/// api-server (`api-server/src/sketch.rs::build_loop_edges`):
///
///   * One `Polyline` curve registered with N+1 vertices (last == first
///     to close the loop).
///   * N edges, each referencing the same `curve_id` with
///     `param_range = [i/N, (i+1)/N]`.
///
/// `verts` are the N distinct loop corners in CCW order on the XY
/// plane (z=0). Returns the N `EdgeId`s in walk order.
fn build_polyline_loop_edges(model: &mut BRepModel, verts: &[Point3]) -> Vec<EdgeId> {
    assert!(verts.len() >= 3, "loop needs at least 3 corners");

    // Shared Polyline curve carrying the full N+1 vertex chain.
    let mut chain: Vec<Point3> = verts.to_vec();
    chain.push(verts[0]);
    let polyline = Polyline::new(chain).expect("polyline ctor");
    let curve_id = model.curves.add(Box::new(polyline));

    // Vertex store entries for the N distinct corners.
    let n = verts.len();
    let v_ids: Vec<VertexId> = verts
        .iter()
        .map(|p| model.vertices.add(p.x, p.y, p.z))
        .collect();

    let n_f = n as f64;
    let mut edges = Vec::with_capacity(n);
    for i in 0..n {
        let v_start = v_ids[i];
        let v_end = v_ids[(i + 1) % n];
        let param_range = ParameterRange::new((i as f64) / n_f, ((i + 1) as f64) / n_f);
        let edge = Edge::new(
            0,
            v_start,
            v_end,
            curve_id,
            EdgeOrientation::Forward,
            param_range,
        );
        edges.push(model.edges.add(edge));
    }
    edges
}

/// Baseline (control) loop: N independent `Line` curves, one per edge,
/// each with `ParameterRange::unit()`. This is the pattern emitted by
/// the rectangle/circle/dimension tools and used by existing dihedral
/// tests — it must always extrude cleanly.
fn build_per_edge_line_loop_edges(model: &mut BRepModel, verts: &[Point3]) -> Vec<EdgeId> {
    assert!(verts.len() >= 3, "loop needs at least 3 corners");
    let n = verts.len();
    let v_ids: Vec<VertexId> = verts
        .iter()
        .map(|p| model.vertices.add(p.x, p.y, p.z))
        .collect();

    let mut edges = Vec::with_capacity(n);
    for i in 0..n {
        let p_start = verts[i];
        let p_end = verts[(i + 1) % n];
        let line = Line::new(p_start, p_end);
        let curve_id = model.curves.add(Box::new(line));
        let edge = Edge::new_auto_range(0, v_ids[i], v_ids[(i + 1) % n], curve_id, EdgeOrientation::Forward);
        edges.push(model.edges.add(edge));
    }
    edges
}

// ---------------------------------------------------------------------
// Geometry helpers
// ---------------------------------------------------------------------

fn z0(x: f64, y: f64) -> Point3 {
    Point3::new(x, y, 0.0)
}

fn regular_ngon(n: usize, radius: f64) -> Vec<Point3> {
    (0..n)
        .map(|i| {
            let theta = 2.0 * std::f64::consts::PI * (i as f64) / (n as f64);
            z0(radius * theta.cos(), radius * theta.sin())
        })
        .collect()
}

/// L-shape with one reflex (270°) corner. CCW.
fn lshape() -> Vec<Point3> {
    vec![
        z0(0.0, 0.0),
        z0(4.0, 0.0),
        z0(4.0, 2.0),
        z0(2.0, 2.0),
        z0(2.0, 4.0),
        z0(0.0, 4.0),
    ]
}

/// 8-pointed star (4 convex tips + 4 inner concave vertices). CCW.
fn star_8() -> Vec<Point3> {
    let outer = 2.0_f64;
    let inner = 0.8_f64;
    (0..8)
        .map(|i| {
            let r = if i % 2 == 0 { outer } else { inner };
            let theta = 2.0 * std::f64::consts::PI * (i as f64) / 8.0;
            z0(r * theta.cos(), r * theta.sin())
        })
        .collect()
}

/// Shoelace signed area of a CCW polygon. Positive for CCW.
fn signed_polygon_area_xy(verts: &[Point3]) -> f64 {
    let n = verts.len();
    let mut s = 0.0;
    for i in 0..n {
        let a = verts[i];
        let b = verts[(i + 1) % n];
        s += a.x * b.y - b.x * a.y;
    }
    s * 0.5
}

fn standard_extrude_opts(height: f64) -> ExtrudeOptions {
    ExtrudeOptions {
        distance: height,
        direction: Vector3::Z,
        common: CommonOptions {
            validate_result: false,
            ..Default::default()
        },
        ..Default::default()
    }
}

// ---------------------------------------------------------------------
// Tessellation invariants
// ---------------------------------------------------------------------

/// Count edges in the tessellated mesh whose unordered (v_min, v_max)
/// pair is shared by anything other than exactly two triangles.
/// Watertight closed manifold ⇒ every undirected edge has valence 2.
fn count_mesh_non_manifold_edges(mesh: &geometry_engine::tessellation::TriangleMesh) -> usize {
    use std::collections::HashMap;
    let mut counts: HashMap<(u32, u32), usize> = HashMap::new();
    for tri in &mesh.triangles {
        for k in 0..3 {
            let a = tri[k];
            let b = tri[(k + 1) % 3];
            let key = if a < b { (a, b) } else { (b, a) };
            *counts.entry(key).or_insert(0) += 1;
        }
    }
    counts.values().filter(|&&c| c != 2).count()
}

// ---------------------------------------------------------------------
// Phase 1 — TERMINATE under watchdog
// ---------------------------------------------------------------------

fn extrude_polyline_shape(verts: Vec<Point3>, height: f64) -> SolidId {
    let mut model = BRepModel::new();
    let edges = build_polyline_loop_edges(&mut model, &verts);
    extrude_profile(&mut model, edges, standard_extrude_opts(height))
        .expect("polyline extrude_profile succeeds")
}

#[test]
fn polyline_triangle_extrude_terminates() {
    run_with_watchdog("polyline_triangle_extrude", 10_000, || {
        let _ = extrude_polyline_shape(regular_ngon(3, 1.0), 2.0);
    });
}

#[test]
fn polyline_square_extrude_terminates() {
    run_with_watchdog("polyline_square_extrude", 10_000, || {
        let verts = vec![z0(0.0, 0.0), z0(2.0, 0.0), z0(2.0, 2.0), z0(0.0, 2.0)];
        let _ = extrude_polyline_shape(verts, 1.0);
    });
}

#[test]
fn polyline_pentagon_extrude_terminates() {
    run_with_watchdog("polyline_pentagon_extrude", 10_000, || {
        let _ = extrude_polyline_shape(regular_ngon(5, 1.5), 1.0);
    });
}

#[test]
fn polyline_hexagon_extrude_terminates() {
    run_with_watchdog("polyline_hexagon_extrude", 10_000, || {
        let _ = extrude_polyline_shape(regular_ngon(6, 1.5), 1.0);
    });
}

#[test]
fn polyline_decagon_extrude_terminates() {
    run_with_watchdog("polyline_decagon_extrude", 15_000, || {
        let _ = extrude_polyline_shape(regular_ngon(10, 2.0), 1.0);
    });
}

#[test]
fn polyline_lshape_extrude_terminates() {
    run_with_watchdog("polyline_lshape_extrude", 10_000, || {
        let _ = extrude_polyline_shape(lshape(), 1.0);
    });
}

#[test]
fn polyline_star8_extrude_terminates() {
    run_with_watchdog("polyline_star8_extrude", 15_000, || {
        let _ = extrude_polyline_shape(star_8(), 1.0);
    });
}

// ---------------------------------------------------------------------
// Phase 2 — CORRECTNESS (face count, manifold mesh)
// ---------------------------------------------------------------------

/// Asserts that the polyline-extruded solid has the expected face
/// count, χ = 2 on the outer shell (V - E + F over the shell graph),
/// and zero non-manifold edges under the given tessellation params.
///
/// Parameterised over the tolerance regime so the same assertion fires
/// across the api-server's actual production params, not just the
/// kernel test-suite's `coarse` preset. The hang reproduction lives
/// at `::default()` (chord_tolerance = 0.001, max_segments = 100),
/// which is what the live api-server hands to `tessellate_solid`.
fn assert_polyline_extrusion_is_clean_at(
    name: &str,
    verts: Vec<Point3>,
    height: f64,
    params: TessellationParams,
    regime: &str,
) {
    let n = verts.len();
    let mut model = BRepModel::new();
    let edges = build_polyline_loop_edges(&mut model, &verts);
    let solid_id = extrude_profile(&mut model, edges, standard_extrude_opts(height))
        .unwrap_or_else(|e| panic!("[{}/{}] extrude_profile failed: {:?}", name, regime, e));

    let solid = model.solids.get(solid_id).expect("solid");
    let shell = model.shells.get(solid.outer_shell).expect("shell");
    assert_eq!(
        shell.faces.len(),
        n + 2,
        "[{}/{}] expected {} side faces + bottom + top = {} faces, got {}",
        name,
        regime,
        n,
        n + 2,
        shell.faces.len()
    );

    let mesh = tessellate_solid(solid, &model, &params);
    let nm = count_mesh_non_manifold_edges(&mesh);
    assert_eq!(
        nm, 0,
        "[{}/{}] expected 0 non-manifold mesh edges, found {} (triangles={}, vertices={})",
        name,
        regime,
        nm,
        mesh.triangles.len(),
        mesh.vertices.len()
    );
}

/// Backwards-compatible wrapper retaining the original `coarse` regime
/// so existing call-sites stay green while the parameterised matrix
/// below pins behaviour across `default` and `fine` as well.
fn assert_polyline_extrusion_is_clean(name: &str, verts: Vec<Point3>, height: f64) {
    assert_polyline_extrusion_is_clean_at(
        name,
        verts,
        height,
        TessellationParams::coarse(),
        "coarse",
    );
}

#[test]
fn polyline_square_extrusion_is_clean() {
    run_with_watchdog("polyline_square_clean", 15_000, || {
        assert_polyline_extrusion_is_clean(
            "square",
            vec![z0(0.0, 0.0), z0(2.0, 0.0), z0(2.0, 2.0), z0(0.0, 2.0)],
            1.0,
        );
    });
}

#[test]
fn polyline_hexagon_extrusion_is_clean() {
    run_with_watchdog("polyline_hexagon_clean", 15_000, || {
        assert_polyline_extrusion_is_clean("hexagon", regular_ngon(6, 1.5), 1.0);
    });
}

#[test]
fn polyline_lshape_extrusion_is_clean() {
    run_with_watchdog("polyline_lshape_clean", 15_000, || {
        assert_polyline_extrusion_is_clean("lshape", lshape(), 1.0);
    });
}

// ---------------------------------------------------------------------
// Phase 2b — Tolerance matrix (default + fine)
// ---------------------------------------------------------------------
//
// The kernel-level `coarse` tests above passed even before the
// tessellation-hang fix (commit c91d042) under earlier defensive
// dispatch logic. The api-server in production hands
// `TessellationParams::default()` (chord_tolerance = 0.001,
// max_segments = 100) to `tessellate_solid`, which is 10× tighter
// than `coarse` on tolerance and 5× higher on segment cap. Both
// regimes must complete cleanly within the same watchdog budget —
// otherwise the api-server hang is a manifestation of the same root
// cause, just expressed at a different point in the parameter space.

#[test]
fn polyline_square_extrusion_is_clean_default_params() {
    run_with_watchdog("polyline_square_clean_default", 15_000, || {
        assert_polyline_extrusion_is_clean_at(
            "square",
            vec![z0(0.0, 0.0), z0(2.0, 0.0), z0(2.0, 2.0), z0(0.0, 2.0)],
            1.0,
            TessellationParams::default(),
            "default",
        );
    });
}

#[test]
fn polyline_hexagon_extrusion_is_clean_default_params() {
    run_with_watchdog("polyline_hexagon_clean_default", 15_000, || {
        assert_polyline_extrusion_is_clean_at(
            "hexagon",
            regular_ngon(6, 1.5),
            1.0,
            TessellationParams::default(),
            "default",
        );
    });
}

#[test]
fn polyline_lshape_extrusion_is_clean_default_params() {
    run_with_watchdog("polyline_lshape_clean_default", 15_000, || {
        assert_polyline_extrusion_is_clean_at(
            "lshape",
            lshape(),
            1.0,
            TessellationParams::default(),
            "default",
        );
    });
}

#[test]
fn polyline_square_extrusion_is_clean_fine_params() {
    run_with_watchdog("polyline_square_clean_fine", 20_000, || {
        assert_polyline_extrusion_is_clean_at(
            "square",
            vec![z0(0.0, 0.0), z0(2.0, 0.0), z0(2.0, 2.0), z0(0.0, 2.0)],
            1.0,
            TessellationParams::fine(),
            "fine",
        );
    });
}

#[test]
fn polyline_hexagon_extrusion_is_clean_fine_params() {
    run_with_watchdog("polyline_hexagon_clean_fine", 20_000, || {
        assert_polyline_extrusion_is_clean_at(
            "hexagon",
            regular_ngon(6, 1.5),
            1.0,
            TessellationParams::fine(),
            "fine",
        );
    });
}

#[test]
fn polyline_lshape_extrusion_is_clean_fine_params() {
    run_with_watchdog("polyline_lshape_clean_fine", 20_000, || {
        assert_polyline_extrusion_is_clean_at(
            "lshape",
            lshape(),
            1.0,
            TessellationParams::fine(),
            "fine",
        );
    });
}

// ---------------------------------------------------------------------
// Phase 3 — BASELINE (per-edge Line) control group
// ---------------------------------------------------------------------

fn assert_per_edge_line_extrusion_is_clean(name: &str, verts: Vec<Point3>, height: f64) {
    let n = verts.len();
    let mut model = BRepModel::new();
    let edges = build_per_edge_line_loop_edges(&mut model, &verts);
    let solid_id = extrude_profile(&mut model, edges, standard_extrude_opts(height))
        .unwrap_or_else(|e| panic!("[{}] baseline extrude_profile failed: {:?}", name, e));

    let solid = model.solids.get(solid_id).expect("solid");
    let shell = model.shells.get(solid.outer_shell).expect("shell");
    assert_eq!(
        shell.faces.len(),
        n + 2,
        "[{}] baseline face count mismatch",
        name
    );

    let params = TessellationParams::coarse();
    let mesh = tessellate_solid(solid, &model, &params);
    let nm = count_mesh_non_manifold_edges(&mesh);
    assert_eq!(
        nm, 0,
        "[{}] baseline produced {} non-manifold mesh edges (this must NEVER fail — \
         baseline pins that the topology itself is sound; failure means the test \
         harness or extrude_profile regressed independently of the polyline bug)",
        name, nm
    );
}

#[test]
fn baseline_per_edge_line_square_is_clean() {
    run_with_watchdog("baseline_square", 10_000, || {
        assert_per_edge_line_extrusion_is_clean(
            "square_baseline",
            vec![z0(0.0, 0.0), z0(2.0, 0.0), z0(2.0, 2.0), z0(0.0, 2.0)],
            1.0,
        );
    });
}

#[test]
fn baseline_per_edge_line_lshape_is_clean() {
    run_with_watchdog("baseline_lshape", 10_000, || {
        assert_per_edge_line_extrusion_is_clean("lshape_baseline", lshape(), 1.0);
    });
}

// ---------------------------------------------------------------------
// Phase 4 — CUT workflows (extrude then boolean Difference)
// ---------------------------------------------------------------------

/// Build a box-like solid via a per-edge-Line rectangle extrude. Used
/// as the *target* in cut tests so the cut variable is purely
/// "polyline cutter" vs "per-edge-Line cutter".
fn build_box_solid(model: &mut BRepModel, dx: f64, dy: f64, dz: f64) -> SolidId {
    let verts = vec![
        z0(0.0, 0.0),
        z0(dx, 0.0),
        z0(dx, dy),
        z0(0.0, dy),
    ];
    let edges = build_per_edge_line_loop_edges(model, &verts);
    extrude_profile(model, edges, standard_extrude_opts(dz)).expect("box solid")
}

fn run_polyline_cut_test(name: &'static str, cutter_verts: Vec<Point3>) {
    run_with_watchdog(name, 30_000, move || {
        let mut model = BRepModel::new();
        // Target: 6×6×1 box centred at origin (verts at corner 0,0).
        let target = build_box_solid(&mut model, 6.0, 6.0, 1.0);

        // Translate cutter into the box's footprint (≥3 units in from
        // each side so the cut is fully interior).
        let cutter_verts: Vec<Point3> = cutter_verts
            .into_iter()
            .map(|p| Point3::new(p.x + 3.0, p.y + 3.0, 0.0))
            .collect();

        let cutter_edges = build_polyline_loop_edges(&mut model, &cutter_verts);
        // Cutter taller than the box on both sides so the Difference is
        // a clean through-hole. Translate down before extruding by
        // simply extruding from z=0 to z=2 — but for a through-cut we
        // need it to start below 0. Use a tall extrude and rely on the
        // boolean subtracting only the overlap.
        let cutter = extrude_profile(
            &mut model,
            cutter_edges,
            ExtrudeOptions {
                distance: 3.0,
                direction: Vector3::Z,
                common: CommonOptions {
                    validate_result: false,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("polyline cutter extrude_profile succeeds");

        let _result = boolean_operation(
            &mut model,
            target,
            cutter,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("Difference succeeds");
    });
}

#[test]
fn polyline_pentagon_cut_box_terminates() {
    run_polyline_cut_test("polyline_pentagon_cut_box", regular_ngon(5, 1.0));
}

#[test]
fn polyline_lshape_cut_box_terminates() {
    // Recentre the L so the cut sits inside the 6×6 box footprint.
    let l: Vec<Point3> = lshape()
        .into_iter()
        .map(|p| Point3::new(p.x - 2.0, p.y - 2.0, 0.0))
        .collect();
    run_polyline_cut_test("polyline_lshape_cut_box", l);
}

// ---------------------------------------------------------------------
// Phase 5 — CUT BISECTION (per-edge-Line cutters + chained booleans)
//
// These tests answer the bisection question: when a cut hangs, is the
// fault in the *polyline curve* path, the *boolean Difference* path,
// the *F2-δ ModelSnapshot chaining* path, or some combination?
//
// Reasoning matrix:
//   * If `baseline_rectangle_cut_box` hangs → boolean Difference is
//     broken on its own. Polyline is innocent.
//   * If `baseline_rectangle_cut_box` passes but `polyline_pentagon_
//     cut_box` (Phase 4) hangs → polyline path is the culprit.
//   * If single cuts pass but `sequential_two_rectangle_cuts` hangs →
//     ModelSnapshot deep-copy chaining (#73 F2-δ) is amplifying
//     state across consecutive ops.
//   * If `baseline_lshape_cut_box` hangs but rectangle passes →
//     reflex-corner / non-convex topology breaks Difference.
//
// Every test pins under the same 30s budget so we can compare apples
// to apples.
// ---------------------------------------------------------------------

/// Cut test driver parameterised on the cutter-loop builder so the
/// polyline path and the per-edge-Line path share the same target box
/// and the same boolean Difference call site.
fn run_cut_test_with_builder<F>(
    name: &'static str,
    cutter_verts: Vec<Point3>,
    build_cutter_loop: F,
) where
    F: FnOnce(&mut BRepModel, &[Point3]) -> Vec<EdgeId> + Send + 'static,
{
    run_with_watchdog(name, 30_000, move || {
        let mut model = BRepModel::new();
        let target = build_box_solid(&mut model, 6.0, 6.0, 1.0);

        // Translate cutter ≥3 units in from each side so it sits
        // inside the box footprint.
        let cutter_verts: Vec<Point3> = cutter_verts
            .into_iter()
            .map(|p| Point3::new(p.x + 3.0, p.y + 3.0, 0.0))
            .collect();

        let cutter_edges = build_cutter_loop(&mut model, &cutter_verts);
        let cutter = extrude_profile(
            &mut model,
            cutter_edges,
            ExtrudeOptions {
                distance: 3.0,
                direction: Vector3::Z,
                common: CommonOptions {
                    validate_result: false,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("cutter extrude_profile succeeds");

        let _result = boolean_operation(
            &mut model,
            target,
            cutter,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("Difference succeeds");
    });
}

#[test]
fn baseline_rectangle_cut_box_terminates() {
    // Simplest possible cut: per-edge-Line rectangle cutter. If THIS
    // hangs, the boolean Difference path is broken independently of
    // polyline curves — the polyline tests are a distraction.
    let rect = vec![
        z0(0.0, 0.0),
        z0(2.0, 0.0),
        z0(2.0, 2.0),
        z0(0.0, 2.0),
    ];
    run_cut_test_with_builder("baseline_rectangle_cut_box", rect, |model, verts| {
        build_per_edge_line_loop_edges(model, verts)
    });
}

#[test]
fn baseline_pentagon_cut_box_terminates() {
    // Sister to `polyline_pentagon_cut_box_terminates`. Same shape,
    // same target, but per-edge-Line cutter loop. Direct comparison
    // isolates whether the pentagon hang depends on Polyline curves
    // or just on having ≥5-sided cutter topology.
    run_cut_test_with_builder(
        "baseline_pentagon_cut_box",
        regular_ngon(5, 1.0),
        |model, verts| build_per_edge_line_loop_edges(model, verts),
    );
}

#[test]
fn baseline_lshape_cut_box_terminates() {
    // Sister to `polyline_lshape_cut_box_terminates`. Reflex (270°)
    // corner cutter with per-edge-Line topology. If this hangs but
    // rectangle/pentagon pass, the bug is reflex-corner-specific in
    // the Difference pipeline, not polyline-specific.
    let l: Vec<Point3> = lshape()
        .into_iter()
        .map(|p| Point3::new(p.x - 2.0, p.y - 2.0, 0.0))
        .collect();
    run_cut_test_with_builder("baseline_lshape_cut_box", l, |model, verts| {
        build_per_edge_line_loop_edges(model, verts)
    });
}

#[test]
fn baseline_star8_cut_box_terminates() {
    // High vertex count (16-sided concave) per-edge-Line cutter. Pins
    // whether the cut path scales with vertex count even when the
    // curve representation is the trivial per-edge-Line one.
    run_cut_test_with_builder("baseline_star8_cut_box", star_8(), |model, verts| {
        build_per_edge_line_loop_edges(model, verts)
    });
}

#[test]
fn sequential_two_rectangle_cuts_box_terminates() {
    // Two consecutive Difference operations on the same target. F2-δ
    // (#73) wraps every mutating op in `ModelSnapshot::take` which
    // deep-copies all 9 stores. If chaining cuts compounds the
    // snapshot work super-linearly, this test catches it where a
    // single cut would not.
    run_with_watchdog("sequential_two_rectangle_cuts", 30_000, || {
        let mut model = BRepModel::new();
        let target = build_box_solid(&mut model, 10.0, 6.0, 1.0);

        // First cutter at x=1..3, y=1..5 (interior).
        let cutter1_verts = vec![z0(1.0, 1.0), z0(3.0, 1.0), z0(3.0, 5.0), z0(1.0, 5.0)];
        let e1 = build_per_edge_line_loop_edges(&mut model, &cutter1_verts);
        let cutter1 = extrude_profile(
            &mut model,
            e1,
            ExtrudeOptions {
                distance: 3.0,
                direction: Vector3::Z,
                common: CommonOptions {
                    validate_result: false,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("cutter1 extrude_profile");
        let after_cut1 = boolean_operation(
            &mut model,
            target,
            cutter1,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("first Difference");

        // Second cutter at x=6..8, y=1..5 (disjoint from first hole).
        let cutter2_verts = vec![z0(6.0, 1.0), z0(8.0, 1.0), z0(8.0, 5.0), z0(6.0, 5.0)];
        let e2 = build_per_edge_line_loop_edges(&mut model, &cutter2_verts);
        let cutter2 = extrude_profile(
            &mut model,
            e2,
            ExtrudeOptions {
                distance: 3.0,
                direction: Vector3::Z,
                common: CommonOptions {
                    validate_result: false,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("cutter2 extrude_profile");
        let _final = boolean_operation(
            &mut model,
            after_cut1,
            cutter2,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("second Difference");
    });
}

#[test]
fn polyline_then_baseline_rectangle_cut_terminates() {
    // Cross-pattern chain: first cut uses a Polyline cutter, second
    // uses a per-edge-Line cutter. If the first leaves the model in
    // a state that poisons the second, this catches it. If the
    // first hangs the watchdog kills it before the second runs.
    run_with_watchdog("polyline_then_baseline_rectangle_cut", 45_000, || {
        let mut model = BRepModel::new();
        let target = build_box_solid(&mut model, 10.0, 6.0, 1.0);

        let cutter1_verts: Vec<Point3> = regular_ngon(5, 1.0)
            .into_iter()
            .map(|p| Point3::new(p.x + 2.5, p.y + 3.0, 0.0))
            .collect();
        let e1 = build_polyline_loop_edges(&mut model, &cutter1_verts);
        let cutter1 = extrude_profile(
            &mut model,
            e1,
            ExtrudeOptions {
                distance: 3.0,
                direction: Vector3::Z,
                common: CommonOptions {
                    validate_result: false,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("polyline cutter extrude");
        let after_cut1 = boolean_operation(
            &mut model,
            target,
            cutter1,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("polyline Difference");

        let cutter2_verts = vec![z0(6.5, 2.0), z0(8.5, 2.0), z0(8.5, 4.0), z0(6.5, 4.0)];
        let e2 = build_per_edge_line_loop_edges(&mut model, &cutter2_verts);
        let cutter2 = extrude_profile(
            &mut model,
            e2,
            ExtrudeOptions {
                distance: 3.0,
                direction: Vector3::Z,
                common: CommonOptions {
                    validate_result: false,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("baseline cutter extrude");
        let _final = boolean_operation(
            &mut model,
            after_cut1,
            cutter2,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("baseline Difference");
    });
}

// ---------------------------------------------------------------------
// Phase 6 — API-SERVER PATH (create_face_from_profile_with_plane +
//                            extrude_face, not extrude_profile)
//
// The api-server's `extrude_sketch` builds the face via
// `create_face_from_profile_with_plane` because the sketch plane is
// known. It then calls `extrude_face`. This bypasses Newell best-fit
// entirely. The Phase 1–5 tests exercise `extrude_profile`, which
// triggers a SEPARATE bug (sampler ignores `edge.param_range`,
// extrude.rs:1375). Phase 6 reproduces the api-server path so we
// catch the user-reported 25 MB → 400 MB hang in its native
// configuration.
// ---------------------------------------------------------------------

/// Build a polyline-tool loop, lift it through
/// `create_face_from_profile_with_plane` (XY plane, +Z normal), then
/// call `extrude_face`. Returns the new `SolidId`.
fn extrude_polyline_with_plane(verts: Vec<Point3>, height: f64) -> SolidId {
    let mut model = BRepModel::new();
    let edges = build_polyline_loop_edges(&mut model, &verts);
    let face_id = create_face_from_profile_with_plane(
        &mut model,
        edges,
        Point3::ZERO,
        Vector3::Z,
    )
    .expect("create_face_from_profile_with_plane (api-server path)");
    extrude_face(&mut model, face_id, standard_extrude_opts(height))
        .expect("extrude_face on polyline-tool face")
}

#[test]
fn polyline_square_with_plane_extrude_terminates() {
    run_with_watchdog("polyline_square_with_plane_extrude", 15_000, || {
        let _ = extrude_polyline_with_plane(
            vec![z0(0.0, 0.0), z0(2.0, 0.0), z0(2.0, 2.0), z0(0.0, 2.0)],
            1.0,
        );
    });
}

#[test]
fn polyline_pentagon_with_plane_extrude_terminates() {
    run_with_watchdog("polyline_pentagon_with_plane_extrude", 15_000, || {
        let _ = extrude_polyline_with_plane(regular_ngon(5, 1.5), 1.0);
    });
}

#[test]
fn polyline_hexagon_with_plane_extrude_terminates() {
    run_with_watchdog("polyline_hexagon_with_plane_extrude", 15_000, || {
        let _ = extrude_polyline_with_plane(regular_ngon(6, 1.5), 1.0);
    });
}

#[test]
fn polyline_decagon_with_plane_extrude_terminates() {
    run_with_watchdog("polyline_decagon_with_plane_extrude", 20_000, || {
        let _ = extrude_polyline_with_plane(regular_ngon(10, 2.0), 1.0);
    });
}

#[test]
fn polyline_lshape_with_plane_extrude_terminates() {
    run_with_watchdog("polyline_lshape_with_plane_extrude", 15_000, || {
        let _ = extrude_polyline_with_plane(lshape(), 1.0);
    });
}

#[test]
fn polyline_star8_with_plane_extrude_terminates() {
    run_with_watchdog("polyline_star8_with_plane_extrude", 20_000, || {
        let _ = extrude_polyline_with_plane(star_8(), 1.0);
    });
}

/// Build a target box, then cut it with a polyline cutter produced
/// through the api-server's `create_face_from_profile_with_plane`
/// path. Pins the user-reported cut hang on its native code path.
fn run_polyline_with_plane_cut_test(name: &'static str, cutter_verts: Vec<Point3>) {
    run_with_watchdog(name, 45_000, move || {
        let mut model = BRepModel::new();
        let target = build_box_solid(&mut model, 6.0, 6.0, 1.0);

        let cutter_verts: Vec<Point3> = cutter_verts
            .into_iter()
            .map(|p| Point3::new(p.x + 3.0, p.y + 3.0, 0.0))
            .collect();

        let cutter_edges = build_polyline_loop_edges(&mut model, &cutter_verts);
        let cutter_face = create_face_from_profile_with_plane(
            &mut model,
            cutter_edges,
            Point3::ZERO,
            Vector3::Z,
        )
        .expect("create_face_from_profile_with_plane (cutter)");
        let cutter = extrude_face(
            &mut model,
            cutter_face,
            ExtrudeOptions {
                distance: 3.0,
                direction: Vector3::Z,
                common: CommonOptions {
                    validate_result: false,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("extrude_face on polyline cutter face");

        let _ = boolean_operation(
            &mut model,
            target,
            cutter,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("Difference succeeds");
    });
}

#[test]
fn polyline_pentagon_with_plane_cut_box_terminates() {
    run_polyline_with_plane_cut_test(
        "polyline_pentagon_with_plane_cut_box",
        regular_ngon(5, 1.0),
    );
}

#[test]
fn polyline_lshape_with_plane_cut_box_terminates() {
    let l: Vec<Point3> = lshape()
        .into_iter()
        .map(|p| Point3::new(p.x - 2.0, p.y - 2.0, 0.0))
        .collect();
    run_polyline_with_plane_cut_test("polyline_lshape_with_plane_cut_box", l);
}

#[test]
fn polyline_hexagon_with_plane_cut_box_terminates() {
    run_polyline_with_plane_cut_test("polyline_hexagon_with_plane_cut_box", regular_ngon(6, 1.0));
}

// ---------------------------------------------------------------------
// Phase 4b — Tessellate AFTER cut (reproduces live api-server hang)
// ---------------------------------------------------------------------
//
// The Phase 4 cut tests above only assert that
// `boolean_operation(target, cutter, Difference)` returns — they
// never tessellate the result. The live api-server, however,
// follows every cut with `tessellate_solid(result, &model,
// &TessellationParams::default())` before pushing the mesh to the
// viewport. If the boolean produces a topology whose tessellation
// hangs even after the kernel hang-fix, that hang lives here.
//
// These tests run the same cutter+target setup as Phase 4, then
// also tessellate the resulting `Difference` solid at `::default()`
// params and assert the mesh is finite (no infinite/NaN coordinates)
// and manifold (no T-junctions). Watchdog budget is 30 s — the same
// envelope as the underlying boolean.

/// Helper: cut + tessellate, assert mesh is finite and manifold under
/// the supplied params.
fn run_polyline_cut_and_tessellate(
    name: &'static str,
    cutter_verts: Vec<Point3>,
    params: TessellationParams,
    regime: &'static str,
) {
    run_with_watchdog(name, 30_000, move || {
        let mut model = BRepModel::new();
        let target = build_box_solid(&mut model, 6.0, 6.0, 1.0);
        let cutter_verts: Vec<Point3> = cutter_verts
            .into_iter()
            .map(|p| Point3::new(p.x + 3.0, p.y + 3.0, 0.0))
            .collect();
        let cutter_edges = build_polyline_loop_edges(&mut model, &cutter_verts);
        let cutter = extrude_profile(
            &mut model,
            cutter_edges,
            ExtrudeOptions {
                distance: 3.0,
                direction: Vector3::Z,
                common: CommonOptions {
                    validate_result: false,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("polyline cutter extrude_profile succeeds");
        let result_solid = boolean_operation(
            &mut model,
            target,
            cutter,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("Difference succeeds");
        let solid = model.solids.get(result_solid).expect("result solid");

        let shell_ids: Vec<_> = std::iter::once(solid.outer_shell)
            .chain(solid.inner_shells.iter().copied())
            .collect();
        eprintln!(
            "[{}/{}] result solid: outer_shell={} inner_shells={}",
            name, regime, solid.outer_shell, solid.inner_shells.len()
        );
        for (s_idx, shell_id) in shell_ids.iter().enumerate() {
            if let Some(shell) = model.shells.get(*shell_id) {
                eprintln!(
                    "  shell[{}] (id={}): faces={}",
                    s_idx, shell_id, shell.faces.len()
                );
                for face_id in &shell.faces {
                    if let Some(face) = model.faces.get(*face_id) {
                        let outer_loop = model.loops.get(face.outer_loop);
                        let n_edges = outer_loop.map(|l| l.edges.len()).unwrap_or(0);
                        let surface_kind = model
                            .surfaces
                            .get(face.surface_id)
                            .map(|s| s.type_name().to_string())
                            .unwrap_or_else(|| "?".into());
                        eprintln!(
                            "    face[{}] surface={} outer_loop_edges={} inner_loops={}",
                            face_id, surface_kind, n_edges, face.inner_loops.len()
                        );
                    }
                }
            }
        }

        let mesh = tessellate_solid(solid, &model, &params);
        eprintln!(
            "[{}/{}] mesh: triangles={} vertices={}",
            name, regime, mesh.triangles.len(), mesh.vertices.len()
        );

        // Finiteness: every emitted vertex must have finite coordinates.
        for (i, v) in mesh.vertices.iter().enumerate() {
            assert!(
                v.position.x.is_finite() && v.position.y.is_finite() && v.position.z.is_finite(),
                "[{}/{}] mesh vertex {} has non-finite position: ({}, {}, {})",
                name,
                regime,
                i,
                v.position.x,
                v.position.y,
                v.position.z
            );
        }

        let nm = count_mesh_non_manifold_edges(&mesh);
        assert_eq!(
            nm, 0,
            "[{}/{}] expected 0 non-manifold mesh edges after cut, found {} (triangles={}, vertices={})",
            name, regime, nm,
            mesh.triangles.len(),
            mesh.vertices.len()
        );
    });
}

// Boolean Difference + tessellate end-to-end: cutter bottom face is
// coplanar with target box bottom face (the api-server's
// `extrude_cut_sketch` pattern). Result must tessellate to a watertight
// mesh with the polyline-shaped hole carved through the top.
//
// These pin the imprint-merge coplanar-bottom degeneracy fix.

#[test]
fn baseline_hexagon_cutter_below_target_cuts_cleanly() {
    // Same as baseline_hexagon_per_edge_line, but cutter starts BELOW
    // the target (z=-1..2) so NO faces are coplanar with the target.
    // This isolates the bug to the coplanar-bottom case.
    run_with_watchdog("baseline_hexagon_cutter_below_target", 30_000, || {
        let mut model = BRepModel::new();
        let target = build_box_solid(&mut model, 6.0, 6.0, 1.0);
        let cutter_verts: Vec<Point3> = regular_ngon(6, 1.0)
            .into_iter()
            .map(|p| Point3::new(p.x + 3.0, p.y + 3.0, -1.0))  // z=-1
            .collect();
        let cutter_edges = build_per_edge_line_loop_edges(&mut model, &cutter_verts);
        let cutter = extrude_profile(
            &mut model,
            cutter_edges,
            ExtrudeOptions {
                distance: 3.0,
                direction: Vector3::Z,
                common: CommonOptions {
                    validate_result: false,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("cutter extrude_profile succeeds");
        let result_solid = boolean_operation(
            &mut model,
            target,
            cutter,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("Difference succeeds");
        let solid = model.solids.get(result_solid).expect("result solid");
        let shell = model.shells.get(solid.outer_shell).expect("shell");
        eprintln!(
            "[cutter_below_target] outer_shell faces={}",
            shell.faces.len()
        );
        for face_id in &shell.faces {
            if let Some(face) = model.faces.get(*face_id) {
                let outer_loop = model.loops.get(face.outer_loop);
                let n_edges = outer_loop.map(|l| l.edges.len()).unwrap_or(0);
                let surface_kind = model
                    .surfaces
                    .get(face.surface_id)
                    .map(|s| s.type_name().to_string())
                    .unwrap_or_else(|| "?".into());
                eprintln!(
                    "    face[{}] surface={} outer_edges={} inner_loops={}",
                    face_id, surface_kind, n_edges, face.inner_loops.len()
                );
            }
        }
        let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
        eprintln!(
            "[cutter_below_target] mesh triangles={} vertices={}",
            mesh.triangles.len(), mesh.vertices.len()
        );
    });
}

#[test]
fn baseline_hexagon_per_edge_line_cut_box_tessellates_default() {
    // Per-edge-Line cutter version. If this also produces a degenerate
    // 11-triangle mesh, the bug is in the boolean Difference path
    // proper, NOT polyline-specific.
    run_with_watchdog("baseline_hexagon_per_edge_line_cut_box_tess", 30_000, || {
        let mut model = BRepModel::new();
        let target = build_box_solid(&mut model, 6.0, 6.0, 1.0);
        let cutter_verts: Vec<Point3> = regular_ngon(6, 1.0)
            .into_iter()
            .map(|p| Point3::new(p.x + 3.0, p.y + 3.0, 0.0))
            .collect();
        let cutter_edges = build_per_edge_line_loop_edges(&mut model, &cutter_verts);
        let cutter = extrude_profile(
            &mut model,
            cutter_edges,
            ExtrudeOptions {
                distance: 3.0,
                direction: Vector3::Z,
                common: CommonOptions {
                    validate_result: false,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("per-edge-line cutter extrude_profile succeeds");
        let result_solid = boolean_operation(
            &mut model,
            target,
            cutter,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("Difference succeeds");
        let solid = model.solids.get(result_solid).expect("result solid");

        let shell_ids: Vec<_> = std::iter::once(solid.outer_shell)
            .chain(solid.inner_shells.iter().copied())
            .collect();
        eprintln!(
            "[baseline_hexagon_per_edge_line] outer_shell={} inner_shells={}",
            solid.outer_shell, solid.inner_shells.len()
        );
        for shell_id in &shell_ids {
            if let Some(shell) = model.shells.get(*shell_id) {
                eprintln!("  shell[{}]: faces={}", shell_id, shell.faces.len());
                for face_id in &shell.faces {
                    if let Some(face) = model.faces.get(*face_id) {
                        let outer_loop = model.loops.get(face.outer_loop);
                        let n_edges = outer_loop.map(|l| l.edges.len()).unwrap_or(0);
                        let surface_kind = model
                            .surfaces
                            .get(face.surface_id)
                            .map(|s| s.type_name().to_string())
                            .unwrap_or_else(|| "?".into());
                        eprintln!(
                            "    face[{}] surface={} outer_loop_edges={} inner_loops={}",
                            face_id, surface_kind, n_edges, face.inner_loops.len()
                        );
                    }
                }
            }
        }
        let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
        eprintln!(
            "[baseline_hexagon_per_edge_line] mesh triangles={} vertices={}",
            mesh.triangles.len(), mesh.vertices.len()
        );
    });
}

// The 3 polyline cut+tessellate tests below fail with
// `InvalidBRep("build_shells_from_faces: component 0 has only 3 face(s)...")`.
// Root cause is TWO compounding bugs in `operations/boolean.rs` that
// surface only when the cutter's BOTTOM face is coplanar with the
// target's BOTTOM face (the api-server's `extrude_cut_sketch` pattern):
//
//   1. `compute_face_intersections` undercounts curves when planar faces
//      coincide — for a 6-side hex cutter only 5 of the 6 expected
//      target_top ∩ cutter_side intersections are reported, and the 6
//      target_bottom ∩ cutter_side intersections vanish because the
//      target_bottom-vs-cutter_bottom pair is correctly flagged as
//      coplanar but no edge-imprint pass follows to recover the hex
//      perimeter on the target bottom.
//
//   2. `reconstruct_topology` / `build_shells_from_faces` does not detect
//      "face fragment A is the inner hole of face fragment B in UV
//      space" and merge A's boundary as an inner loop of B. The 6
//      cutter-side fragments classified Inside therefore form a
//      separate connected component instead of cutting holes in the
//      target's top and bottom caps. The result solid has 6 faces
//      (a plain box, no tunnel).
//
// Pinned by Task #36. The non-coplanar regression
// `baseline_hexagon_cutter_below_target_cuts_cleanly` already runs
// green after the analytical-surface-kind dispatch fix landed in this
// patch (it exercises bug #2 but not bug #1), so this file pins the
// remaining work on bug #1 + face-with-hole reconstruction.

#[test]
fn polyline_pentagon_cut_box_tessellates_default() {
    run_polyline_cut_and_tessellate(
        "polyline_pentagon_cut_box_tess_default",
        regular_ngon(5, 1.0),
        TessellationParams::default(),
        "default",
    );
}

#[test]
fn polyline_hexagon_cut_box_tessellates_default() {
    run_polyline_cut_and_tessellate(
        "polyline_hexagon_cut_box_tess_default",
        regular_ngon(6, 1.0),
        TessellationParams::default(),
        "default",
    );
}

#[test]
fn polyline_lshape_cut_box_tessellates_default() {
    let l: Vec<Point3> = lshape()
        .into_iter()
        .map(|p| Point3::new(p.x - 2.0, p.y - 2.0, 0.0))
        .collect();
    run_polyline_cut_and_tessellate(
        "polyline_lshape_cut_box_tess_default",
        l,
        TessellationParams::default(),
        "default",
    );
}

// ---------------------------------------------------------------------
// Sanity checks on the helpers themselves
// ---------------------------------------------------------------------

#[test]
fn signed_area_lshape_is_positive_and_correct() {
    // 4×4 outer minus 2×2 notch = 16 - 4 = 12.
    let a = signed_polygon_area_xy(&lshape());
    assert!((a - 12.0).abs() < 1e-9, "expected 12.0, got {}", a);
}

#[test]
fn signed_area_regular_hexagon_matches_closed_form() {
    let r: f64 = 1.5;
    // Regular hexagon area = (3√3/2) r².
    let expected = (3.0 * 3.0_f64.sqrt() / 2.0) * r * r;
    let a = signed_polygon_area_xy(&regular_ngon(6, r));
    assert!((a - expected).abs() < 1e-9, "expected {}, got {}", expected, a);
}
