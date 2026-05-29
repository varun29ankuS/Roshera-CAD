//! Tessellation benchmark harness.
//!
//! Tracks per-primitive tessellation cost across the three quality
//! presets exposed on the REST surface (coarse / default / fine).
//! Each benchmark builds its primitive once outside the timed loop,
//! then re-runs `tessellate_solid` against the prebuilt BRep so we
//! measure tessellation work only — vertex/edge/face/surface store
//! construction is excluded.
//!
//! The shape set is the minimum that exercises every code path
//! touched by T-1..T-4:
//!
//! * **box** — six planar faces. Pure ear-clipping baseline; should
//!   not move when curvature-adaptive sampling changes.
//! * **sphere** — UV grid with two poles. Density grows with
//!   `chord_tolerance` (T-1) and `max_angle_deviation` (T-4 angle
//!   guard on the equator).
//! * **cylinder** — lateral face + two circular caps. Stresses the
//!   watertight invariant between `compute_curve_sample_count` and
//!   `arc_steps_for_quality` (T-4).
//! * **cone** — apex singularity handling.
//! * **torus** — two-axis curvature (major + minor), both subject
//!   to the triple-guard.
//!
//! # CDT-γ.4 — curved NURBS face benchmarks
//!
//! The `bench_curved_nurbs_tessellation` group below targets the
//! CDT-β refinement path: a high-curvature bicubic NURBS bump patch
//! tessellated end-to-end via `tessellate_face`. This is the only
//! group that exercises Ruppert refinement (skinny-triangle splits,
//! encroachment drops, chord/normal centroid splits). The analytic
//! primitive groups above route through the fast paths in
//! `tessellate_{spherical,cylindrical,conical,toroidal}_face` and
//! never enter `curved_cdt`.
//!
//! At startup the harness also emits a one-shot summary line to
//! stderr per quality preset reporting:
//!   * triangle count
//!   * worst chord error (max ⊥distance from a triangle's plane
//!     to the surface-evaluated centroid lift, in model units)
//!   * mean chord error
//! so the bench output captures the *quality* signal (mesh size +
//! chord error) alongside the *cost* signal (wall-clock). This is
//! the regression budget the CDT-β.1 Ruppert convergence cap is
//! sized against.
//!
//! Run with: `cargo bench --bench tessellation_bench`.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use geometry_engine::math::nurbs::NurbsSurface as MathNurbs;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::curve::{Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
use geometry_engine::primitives::face::{Face, FaceOrientation};
use geometry_engine::primitives::r#loop::{Loop, LoopType};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::GeneralNurbsSurface;
use geometry_engine::primitives::topology_builder::{BRepModel, TopologyBuilder};
use geometry_engine::tessellation::{
    edge_cache::EdgeSampleCache, tessellate_face, tessellate_solid, TessellationParams,
    TriangleMesh,
};

/// Build a primitive into a fresh `BRepModel` and return the
/// `SolidId` of the last solid added. Returns `None` if construction
/// fails (e.g. degenerate parameters) so the benchmark is skipped
/// instead of panicking.
fn build<F>(build_fn: F) -> Option<(BRepModel, SolidId)>
where
    F: FnOnce(&mut TopologyBuilder),
{
    let mut model = BRepModel::new();
    {
        let mut builder = TopologyBuilder::new(&mut model);
        build_fn(&mut builder);
    }
    let last_solid = model.solids.iter().last().map(|(id, _)| id)?;
    Some((model, last_solid))
}

/// Time `tessellate_solid` on a prebuilt model at each quality preset.
fn bench_shape(c: &mut Criterion, label: &str, model: &BRepModel, sid: SolidId) {
    let solid = match model.solids.get(sid) {
        Some(s) => s,
        None => return,
    };

    let presets: [(&str, TessellationParams); 3] = [
        ("coarse", TessellationParams::coarse()),
        ("default", TessellationParams::default()),
        ("fine", TessellationParams::fine()),
    ];

    let mut group = c.benchmark_group(format!("tessellate_{label}"));
    for (quality, params) in presets.iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(quality),
            params,
            |b, params| {
                b.iter(|| {
                    let mesh = tessellate_solid(
                        black_box(solid),
                        black_box(model),
                        black_box(params),
                    );
                    black_box(mesh)
                });
            },
        );
    }
    group.finish();
}

fn bench_tessellation(c: &mut Criterion) {
    if let Some((model, sid)) = build(|b| {
        let _ = b.create_box_3d(10.0, 10.0, 10.0);
    }) {
        bench_shape(c, "box", &model, sid);
    }

    if let Some((model, sid)) = build(|b| {
        let _ = b.create_sphere_3d(Point3::new(0.0, 0.0, 0.0), 5.0);
    }) {
        bench_shape(c, "sphere", &model, sid);
    }

    if let Some((model, sid)) = build(|b| {
        let _ = b.create_cylinder_3d(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            2.0,
            5.0,
        );
    }) {
        bench_shape(c, "cylinder", &model, sid);
    }

    if let Some((model, sid)) = build(|b| {
        let _ = b.create_cone_3d(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            2.0,
            0.0,
            5.0,
        );
    }) {
        bench_shape(c, "cone", &model, sid);
    }

    if let Some((model, sid)) = build(|b| {
        let _ = b.create_torus_3d(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            5.0,
            1.0,
        );
    }) {
        bench_shape(c, "torus", &model, sid);
    }
}

// =====================================================================
// CDT-γ.4 — curved NURBS face benchmark
// =====================================================================

/// Build a degree-2 NURBS bicubic bump patch on the unit square. The
/// centre control point is raised in +Z so the surface carries
/// significant curvature; default tessellation density triggers
/// chord-tolerance and angle-deviation violations that Ruppert
/// refinement resolves. Mirrors the fixture in
/// `tests/tess_curved_cdt.rs::build_curved_nurbs_unit_square_bump`
/// but lives in the bench so the test crate is not pulled in.
fn build_curved_nurbs_bump_face(model: &mut BRepModel) -> Option<u32> {
    let cp = vec![
        vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(0.5, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
        ],
        vec![
            Point3::new(0.0, 0.5, 0.0),
            Point3::new(0.5, 0.5, 1.0), // bump apex
            Point3::new(1.0, 0.5, 0.0),
        ],
        vec![
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(0.5, 1.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
        ],
    ];
    let w = vec![vec![1.0; 3]; 3];
    let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    let math_nurbs = MathNurbs::new(cp, w, knots.clone(), knots, 2, 2).ok()?;
    let surface_id = model
        .surfaces
        .add(Box::new(GeneralNurbsSurface { nurbs: math_nurbs }));

    let tol = 1e-6;
    let v00 = model.vertices.add_or_find(0.0, 0.0, 0.0, tol);
    let v10 = model.vertices.add_or_find(1.0, 0.0, 0.0, tol);
    let v11 = model.vertices.add_or_find(1.0, 1.0, 0.0, tol);
    let v01 = model.vertices.add_or_find(0.0, 1.0, 0.0, tol);

    let c0 = model.curves.add(Box::new(Line::new(
        Point3::new(0.0, 0.0, 0.0),
        Point3::new(1.0, 0.0, 0.0),
    )));
    let c1 = model.curves.add(Box::new(Line::new(
        Point3::new(1.0, 0.0, 0.0),
        Point3::new(1.0, 1.0, 0.0),
    )));
    let c2 = model.curves.add(Box::new(Line::new(
        Point3::new(1.0, 1.0, 0.0),
        Point3::new(0.0, 1.0, 0.0),
    )));
    let c3 = model.curves.add(Box::new(Line::new(
        Point3::new(0.0, 1.0, 0.0),
        Point3::new(0.0, 0.0, 0.0),
    )));

    let e0 = model.edges.add(Edge::new(
        0, v00, v10, c0, EdgeOrientation::Forward, ParameterRange::unit(),
    ));
    let e1 = model.edges.add(Edge::new(
        0, v10, v11, c1, EdgeOrientation::Forward, ParameterRange::unit(),
    ));
    let e2 = model.edges.add(Edge::new(
        0, v11, v01, c2, EdgeOrientation::Forward, ParameterRange::unit(),
    ));
    let e3 = model.edges.add(Edge::new(
        0, v01, v00, c3, EdgeOrientation::Forward, ParameterRange::unit(),
    ));

    let mut outer = Loop::new(0, LoopType::Outer);
    outer.add_edge(e0, true);
    outer.add_edge(e1, true);
    outer.add_edge(e2, true);
    outer.add_edge(e3, true);
    let outer_id = model.loops.add(outer);

    let face = Face::new(0, surface_id, outer_id, FaceOrientation::Forward);
    Some(model.faces.add(face))
}

/// Tessellate the prebuilt curved face at `params` and return the
/// resulting mesh. Returns `None` if `face_id` is missing from the
/// model (bench skipped) so the harness fails soft.
fn tessellate_curved_face(
    model: &BRepModel,
    face_id: u32,
    params: &TessellationParams,
) -> Option<TriangleMesh> {
    let face = model.faces.get(face_id)?;
    let cache = EdgeSampleCache::new(params);
    let mut mesh = TriangleMesh::new();
    tessellate_face(face, model, params, &cache, &mut mesh);
    Some(mesh)
}

/// Compute (worst, mean) chord error in model units for `mesh` over
/// `face`. For each triangle, lift the UV-centroid of the triangle's
/// (u, v) bbox to 3D via the surface evaluator and measure the
/// signed ⊥distance from the triangle's plane to that lifted point.
/// We return absolute distance because the sign depends on the local
/// chart handedness, which is not the quantity we want to track.
///
/// Triangles with degenerate area (collinear vertices, ‖n‖ < 1e-12)
/// are skipped — they contribute neither a meaningful plane nor a
/// meaningful chord error.
///
/// This is a best-effort fidelity signal, not a watertightness
/// proof. It catches a Ruppert-refinement regression that would let
/// triangle planes drift away from the underlying surface; it does
/// not (and cannot) replace the existing CDT-β.1 integration test
/// `chord_tolerance_actually_enforced_after_refinement`.
fn chord_error_stats(mesh: &TriangleMesh, model: &BRepModel, face_id: u32) -> Option<(f64, f64)> {
    let face = model.faces.get(face_id)?;
    let surface = model.surfaces.get(face.surface_id)?;

    let mut worst = 0.0_f64;
    let mut sum = 0.0_f64;
    let mut counted = 0usize;

    for tri in &mesh.triangles {
        let a = mesh.vertices[tri[0] as usize].position;
        let b = mesh.vertices[tri[1] as usize].position;
        let c = mesh.vertices[tri[2] as usize].position;
        let ab = b - a;
        let ac = c - a;
        let n = ab.cross(&ac);
        let n_len = n.magnitude();
        if n_len < 1e-12 {
            continue;
        }
        let n_hat = n * (1.0 / n_len);

        // UV centroid recovered from the triangle vertices' positions.
        // The bump patch is unit-square in (u, v) and the bench
        // fixture's outer loop maps directly to (x, y), so the (x, y)
        // centroid is the (u, v) centroid. For a more general surface
        // we'd run a closest-point solve; the bench corpus is the
        // bump patch only, so we use the cheap direct mapping.
        let cu = (a.x + b.x + c.x) / 3.0;
        let cv = (a.y + b.y + c.y) / 3.0;
        let p = match surface.point_at(cu, cv) {
            Ok(pt) => pt,
            Err(_) => continue,
        };

        let d = (p - a).dot(&n_hat).abs();
        worst = worst.max(d);
        sum += d;
        counted += 1;
    }

    if counted == 0 {
        return None;
    }
    Some((worst, sum / counted as f64))
}

/// Emit a one-shot quality summary to stderr for the curved face
/// across the three quality presets. Runs once at bench startup so
/// the criterion stdout (wall-clock means + confidence intervals)
/// reads cleanly without interleaving stats lines.
fn report_curved_face_quality(model: &BRepModel, face_id: u32) {
    eprintln!(
        "[tessellation_bench] CDT-γ.4 curved NURBS bump-patch quality report:"
    );
    for (quality, params) in [
        ("coarse", TessellationParams::coarse()),
        ("default", TessellationParams::default()),
        ("fine", TessellationParams::fine()),
    ] {
        let Some(mesh) = tessellate_curved_face(model, face_id, &params) else {
            eprintln!("  {quality:>7}: tessellation skipped (face missing)");
            continue;
        };
        let tris = mesh.triangles.len();
        match chord_error_stats(&mesh, model, face_id) {
            Some((worst, mean)) => {
                eprintln!(
                    "  {quality:>7}: tris={tris:>5}  chord_err worst={worst:.6e}  mean={mean:.6e}"
                );
            }
            None => {
                eprintln!(
                    "  {quality:>7}: tris={tris:>5}  chord_err: no non-degenerate triangles"
                );
            }
        }
    }
}

/// Time `tessellate_face` on the prebuilt curved-NURBS face at each
/// quality preset. The `EdgeSampleCache` is fresh per iteration so
/// the bench reflects a cold-cache call — matching the per-face
/// dispatch the real pipeline performs.
fn bench_curved_face(c: &mut Criterion, label: &str, model: &BRepModel, face_id: u32) {
    let face = match model.faces.get(face_id) {
        Some(f) => f,
        None => return,
    };

    let presets: [(&str, TessellationParams); 3] = [
        ("coarse", TessellationParams::coarse()),
        ("default", TessellationParams::default()),
        ("fine", TessellationParams::fine()),
    ];

    let mut group = c.benchmark_group(format!("tessellate_{label}"));
    for (quality, params) in presets.iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(quality),
            params,
            |b, params| {
                b.iter(|| {
                    let cache = EdgeSampleCache::new(params);
                    let mut mesh = TriangleMesh::new();
                    tessellate_face(
                        black_box(face),
                        black_box(model),
                        black_box(params),
                        black_box(&cache),
                        black_box(&mut mesh),
                    );
                    black_box(mesh)
                });
            },
        );
    }
    group.finish();
}

fn bench_curved_nurbs_tessellation(c: &mut Criterion) {
    let mut model = BRepModel::new();
    let Some(face_id) = build_curved_nurbs_bump_face(&mut model) else {
        eprintln!(
            "[tessellation_bench] curved NURBS bump-patch fixture failed to construct; \
             curved-face benches skipped"
        );
        return;
    };

    report_curved_face_quality(&model, face_id);
    bench_curved_face(c, "curved_nurbs_bump", &model, face_id);
}

criterion_group!(benches, bench_tessellation, bench_curved_nurbs_tessellation);
criterion_main!(benches);
