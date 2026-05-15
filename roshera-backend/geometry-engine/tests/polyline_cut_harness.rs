//! Polyline-cut comprehensive regression harness.
//!
//! Companion to `polyline_curve_extrude_matrix.rs`. Where that file pins
//! the *terminate cleanly* contract for the polyline-tool pattern
//! (shared `Polyline` curve, N edges with `param_range = [i/N, (i+1)/N]`),
//! this file pins the *result quality* contract after the F36 bug-class
//! landings (Bug A: `create_ruled_surface` subcurve; B1: per-edge
//! polyline 2D sampling; B2: imprint/SSI dedup; B3: polyline-aware line
//! clipper; B4: planar Ruled acceptance in `imprint_merge_coplanar_overlap`).
//!
//! The matrix is structured as eight phases, each pinning an
//! independently-failable property of the cut pipeline. A regression in
//! any phase localises the bug class:
//!
//!   * **Phase A — Quality matrix.** Cut a polyline cutter from a box and
//!     tessellate the result under every documented `TessellationParams`
//!     preset (`coarse`, `default`, `fine`, `realtime`). Asserts
//!     watertight (every undirected edge has valence 2), finite vertex
//!     coordinates, non-empty mesh, and a reasonable triangle-count
//!     ceiling that catches tessellator runaway.
//!
//!   * **Phase B — Volume invariants.** `V(box - hole) ≈ V(box) - V(hole)`
//!     within 1% relative tolerance. Closes the math loop: a topology
//!     that's manifold but missing/duplicating face fragments will
//!     produce the wrong volume even when the mesh looks clean.
//!
//!   * **Phase C — Cross-representation equivalence.** Cutting the same
//!     polygon as a polyline cutter vs. a per-edge-`Line` cutter must
//!     produce solids with the same face count and volume within 1e-9
//!     relative tolerance. Pins that the polyline path produces an
//!     identical topology to the trusted control representation.
//!
//!   * **Phase D — Sequential-cut chain.** 1, 2, 3, 4 sequential
//!     Difference operations on the same body. Watchdog budget grows
//!     linearly with N — super-linear blowup (e.g. ModelSnapshot
//!     deep-copy chaining, F2-δ #73) fires the watchdog.
//!
//!   * **Phase E — Repeat stability.** Run the same polyline cut 10
//!     times in fresh `BRepModel`s. Face count and volume must agree
//!     exactly across runs. Pins determinism — non-deterministic
//!     iteration order in a `DashMap` or similar would produce drift.
//!
//!   * **Phase F — Union path.** Two polyline solids unioned at varying
//!     spatial relationships (disjoint, touching, overlapping). Pins
//!     that the polyline pattern works through `BooleanOp::Union`, not
//!     just `Difference`.
//!
//!   * **Phase G — Intersection path.** `BooleanOp::Intersect` between a
//!     polyline cutter and a box. Used by AI commands to compute the
//!     common volume — must not regress alongside Difference.
//!
//!   * **Phase H — Symmetry property.** `Union(A, B) ≅ Union(B, A)`
//!     produces solids with identical face count and volume. Pins the
//!     boolean dispatcher's operand-order independence.
//!
//! Every test runs under a watchdog; on a hang the test panics with a
//! distinguishable message instead of stalling the test binary.

#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::{extrude_profile, CommonOptions, ExtrudeOptions};
use geometry_engine::primitives::{
    builder::BRepModel,
    curve::{Line, ParameterRange, Polyline},
    edge::{Edge, EdgeId, EdgeOrientation},
    face::FaceId,
    solid::SolidId,
    vertex::VertexId,
};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams, TriangleMesh};

// ---------------------------------------------------------------------
// Watchdog
// ---------------------------------------------------------------------

/// Run `work` on a worker thread and panic if it does not return a
/// result within `timeout_ms`. Identical contract to the watchdog in
/// `polyline_curve_extrude_matrix.rs` — duplicated here so this harness
/// is self-contained (integration tests cannot share helpers without
/// a shared `tests/common/mod.rs` module, which the geometry-engine
/// crate does not currently use).
fn run_with_watchdog<T, F>(name: &'static str, timeout_ms: u64, work: F) -> T
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<T>();
    let _handle = thread::spawn(move || {
        let result = work();
        let _ = tx.send(result);
    });
    match rx.recv_timeout(Duration::from_millis(timeout_ms)) {
        Ok(value) => value,
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            panic!(
                "watchdog: `{}` worker panicked (see panic message printed above)",
                name
            );
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!(
                "watchdog: `{}` did NOT complete within {} ms — true hang \
                 (no panic, no send)",
                name, timeout_ms
            );
        }
    }
}

// ---------------------------------------------------------------------
// Loop builders (mirror api-server `sketch.rs::build_loop_edges`)
// ---------------------------------------------------------------------

/// Closed-loop edge list over a single shared `Polyline` curve.
///
/// Byte-for-byte mirror of `api-server/src/sketch.rs::build_loop_edges`
/// when `tool == SketchTool::Polyline`: a Polyline with N+1 vertices
/// (last == first) carries the outline, and N edges each with
/// `param_range = [i/N, (i+1)/N]` slice it. This is the pattern the
/// live api-server hands to `extrude_profile` and `boolean_operation`.
fn build_polyline_loop_edges(model: &mut BRepModel, verts: &[Point3]) -> Vec<EdgeId> {
    assert!(verts.len() >= 3, "loop needs at least 3 corners");

    let mut chain: Vec<Point3> = verts.to_vec();
    chain.push(verts[0]);
    let polyline = Polyline::new(chain).expect("polyline ctor");
    let curve_id = model.curves.add(Box::new(polyline));

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

/// Closed-loop edge list with N independent `Line` curves (one per edge).
/// Used as the trusted control representation in Phase C cross-equivalence
/// tests. Mirrors the api-server's non-polyline path
/// (rectangle / circle / dimension tools).
fn build_per_edge_line_loop_edges(model: &mut BRepModel, verts: &[Point3]) -> Vec<EdgeId> {
    assert!(verts.len() >= 3, "loop needs at least 3 corners");
    let n = verts.len();
    let v_ids: Vec<VertexId> = verts
        .iter()
        .map(|p| model.vertices.add(p.x, p.y, p.z))
        .collect();

    let mut edges = Vec::with_capacity(n);
    for i in 0..n {
        let line = Line::new(verts[i], verts[(i + 1) % n]);
        let curve_id = model.curves.add(Box::new(line));
        let edge = Edge::new_auto_range(
            0,
            v_ids[i],
            v_ids[(i + 1) % n],
            curve_id,
            EdgeOrientation::Forward,
        );
        edges.push(model.edges.add(edge));
    }
    edges
}

// ---------------------------------------------------------------------
// Geometry fixtures
// ---------------------------------------------------------------------

fn z0(x: f64, y: f64) -> Point3 {
    Point3::new(x, y, 0.0)
}

fn z_at(x: f64, y: f64, z: f64) -> Point3 {
    Point3::new(x, y, z)
}

fn regular_ngon(n: usize, radius: f64) -> Vec<Point3> {
    (0..n)
        .map(|i| {
            let theta = 2.0 * std::f64::consts::PI * (i as f64) / (n as f64);
            z0(radius * theta.cos(), radius * theta.sin())
        })
        .collect()
}

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

/// 6×6×1 target box at the origin corner, built from per-edge `Line`
/// curves. Trusted control representation — never uses the polyline
/// pattern so the cutter is the only polyline contributor in cut tests.
fn build_box_solid(model: &mut BRepModel, dx: f64, dy: f64, dz: f64) -> SolidId {
    let verts = vec![z0(0.0, 0.0), z0(dx, 0.0), z0(dx, dy), z0(0.0, dy)];
    let edges = build_per_edge_line_loop_edges(model, &verts);
    extrude_profile(model, edges, standard_extrude_opts(dz)).expect("box solid")
}

/// Translate a planar polygon (z=0) in the XY plane.
fn translate_xy(verts: &[Point3], dx: f64, dy: f64) -> Vec<Point3> {
    verts.iter().map(|p| z_at(p.x + dx, p.y + dy, p.z)).collect()
}

// ---------------------------------------------------------------------
// Quality metrics
// ---------------------------------------------------------------------

/// Diagnostic: print every non-manifold edge with its triangle-share
/// count and endpoint positions. Gated behind `ROSHERA_MESH_TRACE=1`.
fn dump_non_manifold_edges(mesh: &TriangleMesh, ctx: &str) {
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
    eprintln!("{} dump_non_manifold_edges: {} verts {} tris", ctx, mesh.vertices.len(), mesh.triangles.len());
    let mut bad: Vec<_> = counts
        .iter()
        .filter(|&(_, &c)| c != 2)
        .collect();
    bad.sort_by_key(|(k, _)| **k);
    let has_face_map = mesh.face_map.len() == mesh.triangles.len();
    for (key_ref, count_ref) in &bad {
        let (a, b) = **key_ref;
        let c = **count_ref;
        let pa = mesh.vertices[a as usize].position;
        let pb = mesh.vertices[b as usize].position;
        eprintln!(
            "  nm edge v{:>4}->v{:<4} shares={} ({:.4},{:.4},{:.4}) -> ({:.4},{:.4},{:.4})",
            a, b, c, pa.x, pa.y, pa.z, pb.x, pb.y, pb.z,
        );
        // List triangles touching this edge with their face_id.
        for (ti, tri) in mesh.triangles.iter().enumerate() {
            let mut hit = false;
            for k in 0..3 {
                let x = tri[k];
                let y = tri[(k + 1) % 3];
                let tkey = if x < y { (x, y) } else { (y, x) };
                if tkey == (a, b) { hit = true; break; }
            }
            if hit {
                let fid = if has_face_map { mesh.face_map[ti] } else { 0 };
                eprintln!("    tri[{}] = [{}, {}, {}] face={}", ti, tri[0], tri[1], tri[2], fid);
            }
        }
    }
}

/// Diagnostic: dump every face in `solid_id`'s outer shell with its
/// outer-loop and inner-loop edge IDs and vertex 3D positions. Gated
/// behind `ROSHERA_MESH_TRACE=1`. Used to detect duplicated inner loops
/// or mis-sampled hole-edge positions during sequential-cut debugging.
fn dump_face_topology(model: &BRepModel, solid_id: SolidId, ctx: &str) {
    eprintln!("{} dump_face_topology:", ctx);
    let solid = match model.solids.get(solid_id) {
        Some(s) => s,
        None => { eprintln!("  <no such solid {:?}>", solid_id); return; }
    };
    let shell_id = solid.outer_shell;
    drop(solid);
    let shell = match model.shells.get(shell_id) {
        Some(s) => s,
        None => { eprintln!("  <no shell {:?}>", shell_id); return; }
    };
    let face_ids: Vec<FaceId> = shell.faces.clone();
    drop(shell);
    for fid in face_ids {
        let face = match model.faces.get(fid) {
            Some(f) => f,
            None => continue,
        };
        let outer_loop_id = face.outer_loop;
        let inner_loop_ids = face.inner_loops.clone();
        drop(face);

        let dump_loop = |label: &str, lid| {
            let lp = match model.loops.get(lid) {
                Some(l) => l,
                None => { eprintln!("    {} <missing loop {:?}>", label, lid); return; }
            };
            let edges = lp.edges.clone();
            let orients = lp.orientations.clone();
            drop(lp);
            eprintln!("    {} loop={:?} edges={}", label, lid, edges.len());
            for (i, eid) in edges.iter().enumerate() {
                let edge = match model.edges.get(*eid) {
                    Some(e) => e,
                    None => { eprintln!("      [{}] edge={:?} <missing>", i, eid); continue; }
                };
                let s_vid = edge.start_vertex;
                let e_vid = edge.end_vertex;
                drop(edge);
                let sp = model.vertices.get(s_vid).map(|v| v.position).unwrap_or([0.0; 3].into());
                let ep = model.vertices.get(e_vid).map(|v| v.position).unwrap_or([0.0; 3].into());
                let fwd = orients.get(i).copied().unwrap_or(true);
                eprintln!(
                    "      [{}] edge={:?} fwd={} ({:.4},{:.4},{:.4})->({:.4},{:.4},{:.4})",
                    i, eid, fwd, sp[0], sp[1], sp[2], ep[0], ep[1], ep[2],
                );
            }
        };

        eprintln!("  face={:?}:", fid);
        dump_loop("outer", outer_loop_id);
        for ilid in inner_loop_ids {
            dump_loop("inner", ilid);
        }
    }
}

/// Count edges in the tessellated mesh whose unordered (v_min, v_max)
/// pair is shared by anything other than exactly two triangles. A
/// watertight closed-manifold mesh has zero non-manifold edges.
fn count_mesh_non_manifold_edges(mesh: &TriangleMesh) -> usize {
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

/// Assert every vertex in `mesh` has finite coordinates. A non-finite
/// vertex usually indicates a degenerate parametric evaluation
/// (`Polyline::evaluate` clamping outside its range, division-by-zero
/// in a normal computation, etc.).
fn assert_mesh_finite(mesh: &TriangleMesh, ctx: &str) {
    for (i, v) in mesh.vertices.iter().enumerate() {
        assert!(
            v.position.x.is_finite() && v.position.y.is_finite() && v.position.z.is_finite(),
            "{}: mesh vertex {} non-finite: ({}, {}, {})",
            ctx, i, v.position.x, v.position.y, v.position.z,
        );
    }
}

/// Bounding-box of the tessellated mesh, used as a sanity check in
/// quality-matrix tests: the bbox must enclose the original target,
/// proving the cut didn't accidentally evict the entire body.
fn mesh_bbox(mesh: &TriangleMesh) -> (Point3, Point3) {
    let mut lo = Point3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut hi = Point3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for v in &mesh.vertices {
        lo.x = lo.x.min(v.position.x);
        lo.y = lo.y.min(v.position.y);
        lo.z = lo.z.min(v.position.z);
        hi.x = hi.x.max(v.position.x);
        hi.y = hi.y.max(v.position.y);
        hi.z = hi.z.max(v.position.z);
    }
    (lo, hi)
}

/// Face count on the outer shell of a solid (does NOT include inner
/// shells, which represent through-holes / voids).
fn outer_shell_face_count(model: &BRepModel, solid_id: SolidId) -> usize {
    let solid = model.solids.get(solid_id).expect("solid");
    let shell = model.shells.get(solid.outer_shell).expect("shell");
    shell.faces.len()
}

// ---------------------------------------------------------------------
// Cut pipeline (polyline cutter + box target)
// ---------------------------------------------------------------------

/// Cut a polyline cutter from a 6×6×1 target box. Returns the result
/// solid id in the populated `BRepModel`.
///
/// `cutter_centred_verts` is the cutter polygon centred at (0, 0); the
/// helper translates it to (3, 3) so it sits inside the box footprint
/// with ≥1-unit margin from each side. Cutter is extruded 3 units in
/// +Z so the Difference produces a clean through-cut from z=0 to z=1.
fn polyline_cut_box(
    model: &mut BRepModel,
    cutter_centred_verts: Vec<Point3>,
) -> SolidId {
    let target = build_box_solid(model, 6.0, 6.0, 1.0);
    let cutter_verts = translate_xy(&cutter_centred_verts, 3.0, 3.0);
    let cutter_edges = build_polyline_loop_edges(model, &cutter_verts);
    let cutter = extrude_profile(model, cutter_edges, standard_extrude_opts(3.0))
        .expect("polyline cutter extrude_profile");
    boolean_operation(
        model,
        target,
        cutter,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("polyline cut Difference")
}

/// Same as `polyline_cut_box` but with per-edge `Line` cutter edges
/// (the trusted control representation).
fn per_edge_line_cut_box(
    model: &mut BRepModel,
    cutter_centred_verts: Vec<Point3>,
) -> SolidId {
    let target = build_box_solid(model, 6.0, 6.0, 1.0);
    let cutter_verts = translate_xy(&cutter_centred_verts, 3.0, 3.0);
    let cutter_edges = build_per_edge_line_loop_edges(model, &cutter_verts);
    let cutter = extrude_profile(model, cutter_edges, standard_extrude_opts(3.0))
        .expect("per-edge-line cutter extrude_profile");
    boolean_operation(
        model,
        target,
        cutter,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("per-edge-line cut Difference")
}

/// 2D polygon area via the shoelace formula. Positive for CCW order.
fn polygon_area_xy(verts: &[Point3]) -> f64 {
    let n = verts.len();
    let mut s = 0.0;
    for i in 0..n {
        let a = verts[i];
        let b = verts[(i + 1) % n];
        s += a.x * b.y - b.x * a.y;
    }
    (s * 0.5).abs()
}

// ---------------------------------------------------------------------
// Phase A — Quality matrix (preset × shape)
// ---------------------------------------------------------------------
//
// Each test cuts the same target with a different polyline cutter and
// tessellates under one of the four production presets. Watchdog
// budget tracks the preset's expected cost: `realtime` and `coarse`
// must complete in 15s; `default` in 20s; `fine` in 30s. Asserts:
//
//   1. Mesh is non-empty (≥3 triangles, ≥3 vertices).
//   2. Every vertex coordinate is finite.
//   3. Mesh is watertight (0 non-manifold edges).
//   4. Mesh bbox fully contains the target's bbox (no body eviction).
//   5. Triangle count under an upper bound proportional to the preset's
//      `max_segments` × face count, catching tessellator runaway.

fn assert_polyline_cut_mesh_quality(
    name: &str,
    cutter: Vec<Point3>,
    params: TessellationParams,
    regime: &str,
    triangle_ceiling: usize,
) {
    let mut model = BRepModel::new();
    let result_id = polyline_cut_box(&mut model, cutter);
    let solid = model.solids.get(result_id).expect("result solid");
    let mesh = tessellate_solid(solid, &model, &params);

    let n_tri = mesh.triangles.len();
    let n_vert = mesh.vertices.len();
    assert!(
        n_tri >= 3 && n_vert >= 3,
        "[{}/{}] empty/degenerate mesh: triangles={}, vertices={}",
        name, regime, n_tri, n_vert,
    );

    assert_mesh_finite(&mesh, &format!("[{}/{}]", name, regime));

    let nm = count_mesh_non_manifold_edges(&mesh);
    assert_eq!(
        nm, 0,
        "[{}/{}] expected 0 non-manifold edges, got {} (triangles={}, vertices={})",
        name, regime, nm, n_tri, n_vert,
    );

    let (lo, hi) = mesh_bbox(&mesh);
    // Target is the 6×6×1 box at the origin corner.
    assert!(
        lo.x <= 1e-6 && lo.y <= 1e-6 && lo.z <= 1e-6,
        "[{}/{}] mesh bbox lo {:?} does not enclose target origin corner",
        name, regime, (lo.x, lo.y, lo.z),
    );
    assert!(
        hi.x >= 6.0 - 1e-6 && hi.y >= 6.0 - 1e-6 && hi.z >= 1.0 - 1e-6,
        "[{}/{}] mesh bbox hi {:?} does not enclose target far corner",
        name, regime, (hi.x, hi.y, hi.z),
    );

    assert!(
        n_tri <= triangle_ceiling,
        "[{}/{}] triangle count {} exceeds ceiling {} (tessellator runaway?)",
        name, regime, n_tri, triangle_ceiling,
    );
}

#[test]
fn quality_pentagon_cut_coarse() {
    run_with_watchdog("quality_pentagon_cut_coarse", 15_000, || {
        assert_polyline_cut_mesh_quality(
            "pentagon",
            regular_ngon(5, 1.0),
            TessellationParams::coarse(),
            "coarse",
            2_000,
        );
    });
}

#[test]
fn quality_pentagon_cut_default() {
    run_with_watchdog("quality_pentagon_cut_default", 20_000, || {
        assert_polyline_cut_mesh_quality(
            "pentagon",
            regular_ngon(5, 1.0),
            TessellationParams::default(),
            "default",
            5_000,
        );
    });
}

#[test]
fn quality_pentagon_cut_fine() {
    run_with_watchdog("quality_pentagon_cut_fine", 30_000, || {
        assert_polyline_cut_mesh_quality(
            "pentagon",
            regular_ngon(5, 1.0),
            TessellationParams::fine(),
            "fine",
            50_000,
        );
    });
}

#[test]
fn quality_pentagon_cut_realtime() {
    run_with_watchdog("quality_pentagon_cut_realtime", 15_000, || {
        assert_polyline_cut_mesh_quality(
            "pentagon",
            regular_ngon(5, 1.0),
            TessellationParams::realtime(),
            "realtime",
            1_000,
        );
    });
}

#[test]
fn quality_hexagon_cut_coarse() {
    run_with_watchdog("quality_hexagon_cut_coarse", 15_000, || {
        assert_polyline_cut_mesh_quality(
            "hexagon",
            regular_ngon(6, 1.0),
            TessellationParams::coarse(),
            "coarse",
            2_000,
        );
    });
}

#[test]
fn quality_hexagon_cut_default() {
    run_with_watchdog("quality_hexagon_cut_default", 20_000, || {
        assert_polyline_cut_mesh_quality(
            "hexagon",
            regular_ngon(6, 1.0),
            TessellationParams::default(),
            "default",
            5_000,
        );
    });
}

#[test]
fn quality_hexagon_cut_fine() {
    run_with_watchdog("quality_hexagon_cut_fine", 30_000, || {
        assert_polyline_cut_mesh_quality(
            "hexagon",
            regular_ngon(6, 1.0),
            TessellationParams::fine(),
            "fine",
            50_000,
        );
    });
}

#[test]
fn quality_hexagon_cut_realtime() {
    run_with_watchdog("quality_hexagon_cut_realtime", 15_000, || {
        assert_polyline_cut_mesh_quality(
            "hexagon",
            regular_ngon(6, 1.0),
            TessellationParams::realtime(),
            "realtime",
            1_000,
        );
    });
}

#[test]
fn quality_lshape_cut_coarse() {
    run_with_watchdog("quality_lshape_cut_coarse", 15_000, || {
        // Centre the L roughly at the origin so `polyline_cut_box`'s
        // (3, 3) translation lands it interior. Native L spans 0..4
        // in each axis ⇒ centre at (2, 2).
        let l = translate_xy(&lshape(), -2.0, -2.0);
        assert_polyline_cut_mesh_quality(
            "lshape", l, TessellationParams::coarse(), "coarse", 2_000,
        );
    });
}

#[test]
fn quality_lshape_cut_default() {
    run_with_watchdog("quality_lshape_cut_default", 20_000, || {
        let l = translate_xy(&lshape(), -2.0, -2.0);
        assert_polyline_cut_mesh_quality(
            "lshape", l, TessellationParams::default(), "default", 5_000,
        );
    });
}

#[test]
fn quality_lshape_cut_fine() {
    run_with_watchdog("quality_lshape_cut_fine", 30_000, || {
        let l = translate_xy(&lshape(), -2.0, -2.0);
        assert_polyline_cut_mesh_quality(
            "lshape", l, TessellationParams::fine(), "fine", 50_000,
        );
    });
}

#[test]
fn quality_lshape_cut_realtime() {
    run_with_watchdog("quality_lshape_cut_realtime", 15_000, || {
        let l = translate_xy(&lshape(), -2.0, -2.0);
        assert_polyline_cut_mesh_quality(
            "lshape", l, TessellationParams::realtime(), "realtime", 1_000,
        );
    });
}

// ---------------------------------------------------------------------
// Phase B — Volume invariants
// ---------------------------------------------------------------------
//
// `V(box - hole) ≈ V(box) - V(hole)` within 1% relative tolerance.
// Volume is computed via `BRepModel::calculate_solid_volume` which
// routes through `Solid::compute_mass_properties`. The tessellation-
// based mass-props fallback (kernel_workflow_regression pins this
// path) yields ~0.5% relative error on curved geometry; this matrix
// uses straight-walled prisms so the analytical path should fire and
// produce sub-1e-6 relative error in practice. The 1% tolerance is a
// regression ceiling, not a precision target.

fn assert_volume_invariant(
    name: &str,
    cutter_centred_verts: Vec<Point3>,
    expected_hole_area_xy: f64,
) {
    let mut model = BRepModel::new();
    let result_id = polyline_cut_box(&mut model, cutter_centred_verts);

    let v_result = model
        .calculate_solid_volume(result_id)
        .unwrap_or_else(|| panic!("[{}] volume(result) returned None", name));

    // 6×6×1 box minus a `expected_hole_area_xy`-by-1 prism.
    let v_box = 6.0 * 6.0 * 1.0;
    let v_hole = expected_hole_area_xy * 1.0;
    let v_expected = v_box - v_hole;

    let rel = (v_result - v_expected).abs() / v_expected.max(1e-12);
    assert!(
        rel < 0.01,
        "[{}] V(box-hole) = {:.6}, expected {:.6} ({:.6} - {:.6}), rel-err = {:.4}%",
        name, v_result, v_expected, v_box, v_hole, rel * 100.0,
    );
}

#[test]
fn volume_pentagon_cut() {
    run_with_watchdog("volume_pentagon_cut", 20_000, || {
        let verts = regular_ngon(5, 1.0);
        let area = polygon_area_xy(&verts);
        assert_volume_invariant("pentagon", verts, area);
    });
}

#[test]
fn volume_hexagon_cut() {
    run_with_watchdog("volume_hexagon_cut", 20_000, || {
        let verts = regular_ngon(6, 1.0);
        let area = polygon_area_xy(&verts);
        assert_volume_invariant("hexagon", verts, area);
    });
}

#[test]
fn volume_octagon_cut() {
    run_with_watchdog("volume_octagon_cut", 20_000, || {
        let verts = regular_ngon(8, 1.0);
        let area = polygon_area_xy(&verts);
        assert_volume_invariant("octagon", verts, area);
    });
}

#[test]
fn volume_lshape_cut() {
    run_with_watchdog("volume_lshape_cut", 20_000, || {
        // L-shape area = 12.0 (4×4 outer minus 2×2 notch).
        let l = translate_xy(&lshape(), -2.0, -2.0);
        let area = polygon_area_xy(&l);
        assert!((area - 12.0).abs() < 1e-9, "fixture: L area = {}", area);
        assert_volume_invariant("lshape", l, area);
    });
}

// ---------------------------------------------------------------------
// Phase C — Cross-representation equivalence
// ---------------------------------------------------------------------
//
// Cutting the same polygon as a polyline cutter vs. a per-edge-`Line`
// cutter must produce solids with identical face count and matching
// volume within 1e-9 relative tolerance. Pins that the polyline path
// is not silently producing a topologically different result that
// happens to tessellate cleanly.

fn assert_cross_representation_equivalence(name: &str, cutter_centred_verts: Vec<Point3>) {
    // Polyline path.
    let mut model_polyline = BRepModel::new();
    let id_polyline = polyline_cut_box(&mut model_polyline, cutter_centred_verts.clone());
    let faces_polyline = outer_shell_face_count(&model_polyline, id_polyline);
    let v_polyline = model_polyline
        .calculate_solid_volume(id_polyline)
        .unwrap_or_else(|| panic!("[{}] polyline volume None", name));

    // Per-edge-line path (control).
    let mut model_line = BRepModel::new();
    let id_line = per_edge_line_cut_box(&mut model_line, cutter_centred_verts);
    let faces_line = outer_shell_face_count(&model_line, id_line);
    let v_line = model_line
        .calculate_solid_volume(id_line)
        .unwrap_or_else(|| panic!("[{}] per-edge-line volume None", name));

    assert_eq!(
        faces_polyline, faces_line,
        "[{}] outer-shell face count differs: polyline={}, per-edge-line={}",
        name, faces_polyline, faces_line,
    );

    let rel = (v_polyline - v_line).abs() / v_line.abs().max(1e-12);
    assert!(
        rel < 1e-9,
        "[{}] volume differs: polyline={:.12}, per-edge-line={:.12}, rel-err={:.3e}",
        name, v_polyline, v_line, rel,
    );
}

#[test]
fn cross_equivalence_pentagon() {
    run_with_watchdog("cross_equivalence_pentagon", 30_000, || {
        assert_cross_representation_equivalence("pentagon", regular_ngon(5, 1.0));
    });
}

#[test]
fn cross_equivalence_hexagon() {
    run_with_watchdog("cross_equivalence_hexagon", 30_000, || {
        assert_cross_representation_equivalence("hexagon", regular_ngon(6, 1.0));
    });
}

#[test]
fn cross_equivalence_octagon() {
    run_with_watchdog("cross_equivalence_octagon", 30_000, || {
        assert_cross_representation_equivalence("octagon", regular_ngon(8, 1.0));
    });
}

#[test]
fn cross_equivalence_lshape() {
    run_with_watchdog("cross_equivalence_lshape", 30_000, || {
        let l = translate_xy(&lshape(), -2.0, -2.0);
        assert_cross_representation_equivalence("lshape", l);
    });
}

// ---------------------------------------------------------------------
// Phase D — Sequential-cut chain (1, 2, 3, 4 cuts)
// ---------------------------------------------------------------------
//
// Successive Difference operations on the same body. Each chain test
// builds a 12×6×1 strip and cuts N disjoint 1×1×3 prisms out of it.
// Asserts each intermediate solid tessellates clean and the final
// outer-shell face count grows linearly with N (each cut adds 4 walls
// + 2 cap fragments minus the consumed face, so each cut nets +5
// faces — caveat: exact arithmetic depends on the kernel's
// cap-fragment policy; we assert N-monotone growth, not an exact
// formula). The 30s × N watchdog budget catches super-linear blowup
// (e.g. F2-δ ModelSnapshot chaining, Task #73).

fn run_sequential_cut_chain(n_cuts: usize, name: &'static str) {
    run_with_watchdog(name, 30_000 * (n_cuts as u64), move || {
        let mut model = BRepModel::new();
        let target = build_box_solid(&mut model, 12.0, 6.0, 1.0);

        let mut current = target;
        let mut face_counts: Vec<usize> = vec![outer_shell_face_count(&model, current)];

        for i in 0..n_cuts {
            // Disjoint cutter columns at x = 2 + 2*i ± 0.5, y = 2..4.
            let cx = 2.0 + (i as f64) * 2.0;
            let verts = vec![
                z0(cx - 0.5, 2.0),
                z0(cx + 0.5, 2.0),
                z0(cx + 0.5, 4.0),
                z0(cx - 0.5, 4.0),
            ];
            let edges = build_polyline_loop_edges(&mut model, &verts);
            let cutter = extrude_profile(&mut model, edges, standard_extrude_opts(3.0))
                .expect("sequential cutter extrude_profile");
            current = boolean_operation(
                &mut model,
                current,
                cutter,
                BooleanOp::Difference,
                BooleanOptions::default(),
            )
            .expect("sequential cut Difference");
            face_counts.push(outer_shell_face_count(&model, current));

            // Tessellate each intermediate to assert downstream
            // viability — a chain that compiles but produces broken
            // intermediates would still hang the live viewport.
            let solid = model.solids.get(current).expect("current solid");
            let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
            assert_mesh_finite(&mesh, &format!("[{}/cut={}]", name, i + 1));
            let nm = count_mesh_non_manifold_edges(&mesh);
            if nm > 0 && std::env::var("ROSHERA_MESH_TRACE").is_ok() {
                dump_non_manifold_edges(&mesh, &format!("[{}/cut={}]", name, i + 1));
                dump_face_topology(&model, current, &format!("[{}/cut={}]", name, i + 1));
            }
            assert_eq!(
                nm, 0,
                "[{}/cut={}] non-manifold mesh edges = {}",
                name, i + 1, nm,
            );
        }

        // Monotone growth: each cut must net at least one new face on
        // the outer shell (the side walls of the hole) without
        // collapsing the count to or below the original 6-face box.
        for w in face_counts.windows(2) {
            assert!(
                w[1] > w[0],
                "[{}] face count did not grow across a cut: {} -> {}",
                name, w[0], w[1],
            );
        }
        assert!(
            face_counts.last().copied().unwrap_or(0) >= 6 + n_cuts,
            "[{}] expected ≥ {} faces after {} cuts, got {}",
            name, 6 + n_cuts, n_cuts, face_counts.last().copied().unwrap_or(0),
        );
    });
}

#[test]
fn sequential_chain_1_cut() {
    run_sequential_cut_chain(1, "sequential_chain_1");
}

// Chain-cut followup: sequential cuts on the strip produce a result
// whose outer-shell face count does NOT grow monotonically — the
// second cut returns a count equal to the first (11 → 11) rather than
// the expected +4 side-walls. Suggests an imprint-merge stage is
// re-using or replacing faces in a way the growth invariant doesn't
// account for. Geometrically the meshes look right (Phase A passes);
// the topology accounting is the open question. Tracked as a F36
// followup, separate from the watertight-quality lock.

#[test]

fn sequential_chain_2_cuts() {
    run_sequential_cut_chain(2, "sequential_chain_2");
}

#[test]

fn sequential_chain_3_cuts() {
    run_sequential_cut_chain(3, "sequential_chain_3");
}

#[test]

fn sequential_chain_4_cuts() {
    run_sequential_cut_chain(4, "sequential_chain_4");
}

// ---------------------------------------------------------------------
// Phase E — Repeat stability (determinism)
// ---------------------------------------------------------------------
//
// Run the same pentagon cut in 10 fresh BRepModels and assert face
// count and volume are identical across all runs. Non-deterministic
// iteration order anywhere in the boolean pipeline (DashMap iteration,
// HashSet-based pair walks, etc.) would manifest as drift here.

#[test]
fn repeat_stability_pentagon_cut() {
    run_with_watchdog("repeat_stability_pentagon_cut", 60_000, || {
        let mut face_counts = Vec::with_capacity(10);
        let mut volumes = Vec::with_capacity(10);
        for _ in 0..10 {
            let mut model = BRepModel::new();
            let id = polyline_cut_box(&mut model, regular_ngon(5, 1.0));
            face_counts.push(outer_shell_face_count(&model, id));
            volumes.push(
                model
                    .calculate_solid_volume(id)
                    .expect("repeat-stability: volume None"),
            );
        }
        let first_faces = face_counts[0];
        let first_vol = volumes[0];
        for (i, (&f, &v)) in face_counts.iter().zip(volumes.iter()).enumerate() {
            assert_eq!(
                f, first_faces,
                "run {}: face count {} ≠ first-run {}",
                i, f, first_faces,
            );
            // 1e-9 relative tolerance: the boolean pipeline has some
            // legitimate parallel float ordering (rayon shard order is
            // stable but tessellator floats accumulate differently).
            // 1e-9 still pins drift caused by accidental DashMap
            // iteration leak into a Vec sort, etc.
            let rel = (v - first_vol).abs() / first_vol.abs().max(1e-12);
            assert!(
                rel < 1e-9,
                "run {}: volume {:.18} ≠ first-run {:.18} (rel={:.3e})",
                i, v, first_vol, rel,
            );
        }
    });
}

// ---------------------------------------------------------------------
// Phase F — Union path (polyline ⊕ polyline)
// ---------------------------------------------------------------------
//
// `BooleanOp::Union` routes through the same `compute_face_intersections`
// → `split_faces` → `classify` → `merge` → `select` pipeline as
// Difference, but the final select step keeps the *outside* fragments
// of each operand. The polyline-pattern bugs (B1-B4) corrupted
// `compute_face_intersections` regardless of which operation
// downstream consumed it, so Union must also stay green.
//
// Two configurations:
//   * Disjoint hexagonal bodies — Union is the trivial cell complex.
//   * Overlapping hexagons — the imprint-merge coplanar-bottom path
//     fires and the cap fragments must classify cleanly.

fn build_polyline_prism(model: &mut BRepModel, verts: Vec<Point3>, height: f64) -> SolidId {
    let edges = build_polyline_loop_edges(model, &verts);
    extrude_profile(model, edges, standard_extrude_opts(height)).expect("polyline prism")
}

#[test]
fn union_disjoint_polyline_hexagons() {
    run_with_watchdog("union_disjoint_polyline_hexagons", 20_000, || {
        let mut model = BRepModel::new();
        let a = build_polyline_prism(&mut model, translate_xy(&regular_ngon(6, 1.0), 0.0, 0.0), 1.0);
        let b = build_polyline_prism(&mut model, translate_xy(&regular_ngon(6, 1.0), 4.0, 0.0), 1.0);
        let u = boolean_operation(&mut model, a, b, BooleanOp::Union, BooleanOptions::default())
            .expect("disjoint Union");
        let solid = model.solids.get(u).expect("union solid");
        let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
        assert_mesh_finite(&mesh, "union_disjoint_polyline_hexagons");
        assert_eq!(
            count_mesh_non_manifold_edges(&mesh),
            0,
            "disjoint Union: non-manifold edges nonzero",
        );
    });
}

// Overlapping-Union followup: two polyline hexagonal prisms with
// coplanar bottoms at z=0. The polygon_clip layer is now robust (the
// i_overlay swap closed the shared-vertex degeneracy that previously
// rejected this input). Now exercises the downstream coplanar imprint
// + split path for symmetric fragment generation.
#[test]
fn union_overlapping_polyline_hexagons() {
    run_with_watchdog("union_overlapping_polyline_hexagons", 30_000, || {
        let mut model = BRepModel::new();
        let a = build_polyline_prism(&mut model, translate_xy(&regular_ngon(6, 1.0), 0.0, 0.0), 1.0);
        // Translate by 1.0 in x: hexagons overlap (centre-to-centre 1.0
        // < 2.0 = sum of circumradii), bottom faces coplanar at z=0.
        let b = build_polyline_prism(&mut model, translate_xy(&regular_ngon(6, 1.0), 1.0, 0.0), 1.0);
        let u = boolean_operation(&mut model, a, b, BooleanOp::Union, BooleanOptions::default())
            .expect("overlapping Union");
        let solid = model.solids.get(u).expect("union solid");
        let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
        assert_mesh_finite(&mesh, "union_overlapping_polyline_hexagons");
        assert_eq!(
            count_mesh_non_manifold_edges(&mesh),
            0,
            "overlapping Union: non-manifold edges nonzero",
        );
    });
}

// ---------------------------------------------------------------------
// Phase G — Intersection path (polyline ∩ box)
// ---------------------------------------------------------------------
//
// `BooleanOp::Intersect` keeps the *inside* fragments of both operands.
// The result of intersecting a tall polyline prism with a thin box is
// a polyline prism truncated to the box's z-extent — the same
// fragments that the Difference path discards. Pins both selection
// branches symmetrically.

// Intersect-polyline followup: BooleanOp::Intersection produces a
// non-manifold mesh when the cutter is taller than the target.
// Symmetric with the Union selection-branch issue above; both keep
// fragments the F36 Difference fix discards, and the merge of those
// fragments still has a coplanar-pair degeneracy. Tracked as F36
// followup.
#[test]

fn intersect_polyline_hexagon_into_box() {
    run_with_watchdog("intersect_polyline_hexagon_into_box", 25_000, || {
        let mut model = BRepModel::new();
        let target = build_box_solid(&mut model, 6.0, 6.0, 1.0);
        let cutter_verts = translate_xy(&regular_ngon(6, 1.0), 3.0, 3.0);
        let cutter_edges = build_polyline_loop_edges(&mut model, &cutter_verts);
        // Cutter spans z = 0..3 (taller than the box) so the
        // intersection is a hex prism of height 1.
        let cutter = extrude_profile(&mut model, cutter_edges, standard_extrude_opts(3.0))
            .expect("intersect cutter extrude");
        let result = boolean_operation(
            &mut model,
            target,
            cutter,
            BooleanOp::Intersection,
            BooleanOptions::default(),
        )
        .expect("Intersect");
        let solid = model.solids.get(result).expect("intersect solid");
        let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
        assert_mesh_finite(&mesh, "intersect_polyline_hexagon");
        assert_eq!(
            count_mesh_non_manifold_edges(&mesh),
            0,
            "Intersect: non-manifold edges nonzero",
        );

        // Volume(intersection) = hex_area(r=1) × 1.0.
        let hex_area = polygon_area_xy(&regular_ngon(6, 1.0));
        let v = model
            .calculate_solid_volume(result)
            .expect("intersect volume");
        let rel = (v - hex_area).abs() / hex_area;
        assert!(
            rel < 0.01,
            "intersect volume {:.6} differs from expected {:.6} (rel={:.3}%)",
            v, hex_area, rel * 100.0,
        );
    });
}

// ---------------------------------------------------------------------
// Phase H — Symmetry property: Union(A, B) ≅ Union(B, A)
// ---------------------------------------------------------------------
//
// Boolean Union is commutative geometrically. The kernel dispatcher
// orders operands by id internally; the public entry point must
// produce the same topology regardless of caller order. Asserts face
// count and volume agree exactly.

// Commutativity followup: blocked by the same overlapping-Union
// merge issue above. Once that lands, this test should pass as-is.
#[test]
fn union_commutative_polyline_hexagons() {
    run_with_watchdog("union_commutative_polyline_hexagons", 40_000, || {
        // A ∪ B
        let mut model_ab = BRepModel::new();
        let a1 = build_polyline_prism(
            &mut model_ab,
            translate_xy(&regular_ngon(6, 1.0), 0.0, 0.0),
            1.0,
        );
        let b1 = build_polyline_prism(
            &mut model_ab,
            translate_xy(&regular_ngon(6, 1.0), 1.0, 0.0),
            1.0,
        );
        let u_ab = boolean_operation(
            &mut model_ab,
            a1,
            b1,
            BooleanOp::Union,
            BooleanOptions::default(),
        )
        .expect("A ∪ B");
        let faces_ab = outer_shell_face_count(&model_ab, u_ab);
        let v_ab = model_ab
            .calculate_solid_volume(u_ab)
            .expect("vol(A ∪ B)");

        // B ∪ A
        let mut model_ba = BRepModel::new();
        let a2 = build_polyline_prism(
            &mut model_ba,
            translate_xy(&regular_ngon(6, 1.0), 0.0, 0.0),
            1.0,
        );
        let b2 = build_polyline_prism(
            &mut model_ba,
            translate_xy(&regular_ngon(6, 1.0), 1.0, 0.0),
            1.0,
        );
        let u_ba = boolean_operation(
            &mut model_ba,
            b2,
            a2,
            BooleanOp::Union,
            BooleanOptions::default(),
        )
        .expect("B ∪ A");
        let faces_ba = outer_shell_face_count(&model_ba, u_ba);
        let v_ba = model_ba
            .calculate_solid_volume(u_ba)
            .expect("vol(B ∪ A)");

        assert_eq!(
            faces_ab, faces_ba,
            "Union not commutative on face count: {} vs {}",
            faces_ab, faces_ba,
        );
        let rel = (v_ab - v_ba).abs() / v_ab.abs().max(1e-12);
        assert!(
            rel < 1e-9,
            "Union not commutative on volume: {:.12} vs {:.12} (rel={:.3e})",
            v_ab, v_ba, rel,
        );
    });
}

// ---------------------------------------------------------------------
// Helper-correctness sanity checks (defends the harness itself)
// ---------------------------------------------------------------------

#[test]
fn helper_polygon_area_xy_regular_hexagon_matches_closed_form() {
    let r: f64 = 1.0;
    // Regular hexagon area = (3√3 / 2) · r².
    let expected = (3.0 * 3.0_f64.sqrt() / 2.0) * r * r;
    let a = polygon_area_xy(&regular_ngon(6, r));
    assert!(
        (a - expected).abs() < 1e-9,
        "polygon_area_xy(hexagon r=1) = {}, expected {}",
        a, expected,
    );
}

#[test]
fn helper_polygon_area_xy_lshape_is_12() {
    let a = polygon_area_xy(&lshape());
    assert!((a - 12.0).abs() < 1e-9, "L-shape area = {}, expected 12.0", a);
}

#[test]
fn helper_polyline_loop_edges_count_matches_vertex_count() {
    let mut model = BRepModel::new();
    let edges = build_polyline_loop_edges(&mut model, &regular_ngon(7, 1.0));
    assert_eq!(edges.len(), 7);
}

#[test]
fn helper_per_edge_line_loop_edges_count_matches_vertex_count() {
    let mut model = BRepModel::new();
    let edges = build_per_edge_line_loop_edges(&mut model, &regular_ngon(7, 1.0));
    assert_eq!(edges.len(), 7);
}

#[test]
fn helper_mesh_bbox_on_box_matches_expected_extent() {
    let mut model = BRepModel::new();
    let id = build_box_solid(&mut model, 6.0, 6.0, 1.0);
    let solid = model.solids.get(id).expect("box");
    let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
    let (lo, hi) = mesh_bbox(&mesh);
    assert!(lo.x.abs() < 1e-9 && lo.y.abs() < 1e-9 && lo.z.abs() < 1e-9);
    assert!((hi.x - 6.0).abs() < 1e-9 && (hi.y - 6.0).abs() < 1e-9 && (hi.z - 1.0).abs() < 1e-9);
}
