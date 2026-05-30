//! Canonical edge sample cache.
//!
//! In a B-Rep every edge is shared by ≥1 face (≥2 on a closed shell).
//! If each face independently parameterises its boundary samples, the
//! resulting tessellations disagree by sub-millimetre at interior sample
//! positions along shared edges — adjacent faces' meshes meet only at
//! the edge endpoints and produce **T-junctions** at every interior
//! sample, which makes the unioned mesh non-manifold (≥3 triangles
//! at the same undirected edge) and breaks STL export, BVH builders,
//! and CSG downstream.
//!
//! This module implements the Parasolid-style fix: every B-Rep edge has
//! a **canonical** sample sequence that every face bounding the edge
//! consumes verbatim. The cache is keyed by [`EdgeId`] and stores
//! `Arc<Vec<Point3>>` so callers can hold a stable reference across
//! the face tessellation pass without copying.
//!
//! ## Sample layout
//!
//! Each cached entry holds `n + 1` samples in **canonical forward
//! curve-parameter direction** (i.e. `samples[0] = curve.point_at(
//! param_range.start)` and `samples[n] = curve.point_at(
//! param_range.end)`), where `n` is the integer step count returned
//! by [`compute_curve_sample_count`]. Both endpoints are included so
//! that face tessellators can slice as needed:
//!
//! * Forward traversal emits `samples[..n]` — the n-th sample is
//!   dropped because the next edge in the loop will provide it as
//!   its own `samples[0]` (they share a vertex).
//! * Reverse traversal emits `samples[n], samples[n-1], …, samples[1]`
//!   — symmetrically, `samples[0]` is the shared vertex with the next
//!   edge in the reverse walk.
//!
//! This matches the "emit n samples per edge, last shared with next
//! edge's first" convention that the previous in-line per-edge sampler
//! used, so existing watertight regression tests over planar primitive
//! faces remain green by construction.
//!
//! ## Sample-count policy
//!
//! Mirrors the previous per-edge branching in
//! `surface::sample_loop_3d_polygon`:
//!
//! * **Closed edges** (`start_vertex == end_vertex`) — always treated as
//!   curved; sample count comes from [`compute_curve_sample_count`]
//!   directly. The 3-point collinearity short-circuit is unsafe here
//!   because three points on a closed curve can be coplanar yet the
//!   curve is not a degenerate line.
//! * **Open edges** — a 3-point collinearity probe (`t_start`, mid,
//!   `t_end`). If the cross product magnitude is below
//!   [`COLLINEAR_TOL`] the edge is treated as a straight line and the
//!   cache emits **2** samples (just the endpoints). Otherwise the
//!   full triple-guard count applies.
//!
//! ## Concurrency
//!
//! The cache uses [`dashmap::DashMap`] so it is safe to share by
//! reference across rayon worker threads. The first request for a
//! given [`EdgeId`] populates the entry under one shard's write lock;
//! subsequent reads are lock-free. This is required by the parallel
//! face tessellator (`tessellate_solid_parallel`).

use super::TessellationParams;
use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::curve::Curve;
use crate::primitives::edge::EdgeId;
use crate::primitives::face::FaceId;
use crate::primitives::surface::Surface;
use crate::primitives::topology_builder::BRepModel;
use dashmap::DashMap;
use std::sync::Arc;

/// Magnitude threshold for the 3-point collinearity probe used to
/// short-circuit straight-line open edges to two samples. Matches the
/// previous `surface.rs::sample_loop_3d_polygon` literal so behaviour
/// over planar primitive faces (boxes, prisms) is bit-identical.
const COLLINEAR_TOL: f64 = 1e-9;

/// Canonical curve-sample cache shared by every face bounding an edge.
///
/// Construct once per tessellation pass via [`Self::new`] and pass by
/// reference to every face tessellator. The cache is interior-mutable
/// (`DashMap`) so it can be threaded through `&self` call chains and
/// shared across rayon workers via `Arc<Self>`.
pub struct EdgeSampleCache {
    params: TessellationParams,
    samples: DashMap<EdgeId, Arc<Vec<Point3>>>,
}

impl EdgeSampleCache {
    /// Build an empty cache parameterised by `params`. The params are
    /// cloned so callers can drop their handle immediately; the clone
    /// is cheap (five primitive fields).
    pub fn new(params: &TessellationParams) -> Self {
        Self {
            params: params.clone(),
            samples: DashMap::new(),
        }
    }

    /// Fetch the canonical sample sequence for `edge_id`, computing it
    /// on first request. Returns an `Arc` so the caller can hold the
    /// slice across nested function calls without re-locking.
    ///
    /// Returns an `Arc<Vec<Point3>>` containing `0`, `2`, or `n + 1`
    /// samples:
    ///
    /// * `0` — edge or curve lookup failed, or the curve could not be
    ///   evaluated at either endpoint. Callers should skip the edge.
    /// * `2` — open edge that passed the collinearity probe (straight
    ///   line); the cache emits the two endpoints only.
    /// * `n + 1` — curved or closed edge sampled at `n + 1` parametric
    ///   stations spanning `[param_range.start, param_range.end]`.
    pub fn get_or_compute(&self, edge_id: EdgeId, model: &BRepModel) -> Arc<Vec<Point3>> {
        // DashMap's `entry` API gives us "one writer per shard, lock-free
        // reads after insertion" semantics: a parallel face tessellator
        // hitting the same edge from two faces simultaneously will see
        // exactly one compute call followed by two cheap clones.
        if let Some(existing) = self.samples.get(&edge_id) {
            return Arc::clone(existing.value());
        }
        let computed = Arc::new(self.compute_samples(edge_id, model));
        self.samples
            .entry(edge_id)
            .or_insert_with(|| Arc::clone(&computed));
        computed
    }

    /// Heavy-lifting compute path: looks up edge + curve, decides
    /// curvature/collinearity, and emits the canonical sample sequence
    /// in forward parameter direction.
    fn compute_samples(&self, edge_id: EdgeId, model: &BRepModel) -> Vec<Point3> {
        let edge = match model.edges.get(edge_id) {
            Some(e) => e,
            None => return Vec::new(),
        };
        // `CurveStore::get` returns `Option<&dyn Curve>` directly; no
        // `.as_ref()` needed because the reference IS the trait object.
        let curve = match model.curves.get(edge.curve_id) {
            Some(c) => c,
            None => return Vec::new(),
        };
        let t_start = edge.param_range.start;
        let t_end = edge.param_range.end;

        // Closed edges (start_vertex == end_vertex) bypass the
        // collinearity probe — three points on a closed loop can be
        // coplanar while the curve as a whole is genuinely curved
        // (e.g. a circular edge sampled at three approximately collinear
        // positions just shy of a diameter). Always use the full
        // triple-guard count.
        let is_closed_edge = edge.start_vertex == edge.end_vertex;
        let n_curve = if is_closed_edge {
            compute_curve_sample_count(curve, t_start, t_end, &self.params)
        } else {
            let mid = (t_start + t_end) * 0.5;
            match (
                curve.point_at(t_start),
                curve.point_at(mid),
                curve.point_at(t_end),
            ) {
                (Ok(p_start), Ok(p_mid), Ok(p_end)) => {
                    let v1 = p_mid - p_start;
                    let v2 = p_end - p_start;
                    if v1.cross(&v2).magnitude() < COLLINEAR_TOL {
                        1
                    } else {
                        compute_curve_sample_count(curve, t_start, t_end, &self.params)
                    }
                }
                _ => 1,
            }
        };

        // CDT-γ.1: face-aware densification. A boundary edge must be
        // sampled finely enough to match the interior triangle density
        // each adjacent face's surface curvature demands, or Ruppert
        // refinement keeps hitting boundary encroachment near curved faces
        // and drops the encroaching interior Steiner points (option (c)).
        // Take the max over the edge's adjacent faces of a surface-
        // curvature-driven count. This is a pure function of (edge curve,
        // adjacent surfaces, params), so every face bounding the edge
        // derives the same `n` and shares one bit-exact sample set —
        // shared-edge coherence is preserved by construction. Flat /
        // ruled-along-the-edge faces contribute zero, so straight edges
        // keep their two-sample collapse.
        let mut n = n_curve;
        for face_id in adjacent_faces_of(model, edge_id) {
            if let Some(face) = model.faces.get(face_id) {
                if let Some(surface) = model.surfaces.get(face.surface_id) {
                    n = n.max(compute_face_boundary_sample_count(
                        curve,
                        t_start,
                        t_end,
                        surface,
                        &self.params,
                    ));
                }
            }
        }

        // Emit n+1 samples spanning [t_start, t_end] inclusive of both
        // endpoints. Any individual point_at failure leaves a gap; the
        // resulting vector is dropped down to the runs of valid samples
        // by callers via the `samples.len() < 2` skip guard.
        let mut out = Vec::with_capacity(n + 1);
        for j in 0..=n {
            let t = t_start + (j as f64) * (t_end - t_start) / (n as f64);
            if let Ok(p) = curve.point_at(t) {
                out.push(p);
            } else {
                // Sampling failure — bail with what we have. If we have
                // fewer than two points the consumer skips the edge.
                return out;
            }
        }
        out
    }
}

/// Compute the curvature-adaptive sample count for a curve segment.
///
/// Probes the curve with 16 uniform parametric samples and estimates
/// two scalars from the resulting polyline:
///
/// * `total_length`  — sum of chord magnitudes (lower-bound on arc length).
/// * `total_angle`   — sum of consecutive segment turn angles
///   (lower-bound on total tangent rotation).
///
/// The sample count is then the strictest of three constraints, the
/// same triple-guard used by `arc_steps_for_quality` for primitive
/// surface grids:
///
/// 1. **max_edge_length** — `n_len = ceil(total_length / max_edge_length)`.
/// 2. **max_angle_deviation** — `n_angle = ceil(total_angle / max_angle_deviation)`.
/// 3. **chord_tolerance (sagitta)** — from the effective mean radius
///    `r ≈ total_length / total_angle`, the per-segment subtended angle
///    that keeps sagitta below the tolerance is
///    `theta_seg = 2·acos(1 − chord_tolerance / r)`, giving
///    `n_sag = ceil(total_angle / theta_seg)`.
///
/// This keeps face-boundary samples in lockstep with the cylindrical /
/// spherical / conical / toroidal tessellators (which derive their
/// step counts from the same triple-guard over the parametric span),
/// so cap and lateral faces agree on every closed-circle boundary
/// point. Watertightness then survives `weld_mesh_watertight_range`
/// without relying on the welder's spatial tolerance as a safety net.
pub(crate) fn compute_curve_sample_count(
    curve: &dyn Curve,
    t_start: f64,
    t_end: f64,
    params: &TessellationParams,
) -> usize {
    const PROBE: usize = 16;

    // 16-point parametric probe → polyline.
    let mut pts: Vec<Option<Point3>> = Vec::with_capacity(PROBE + 1);
    pts.push(curve.point_at(t_start).ok());
    for i in 1..=PROBE {
        let t = t_start + (i as f64) * (t_end - t_start) / (PROBE as f64);
        pts.push(curve.point_at(t).ok());
    }

    // Total chord length (lower-bound on arc length).
    let mut total_length = 0.0_f64;
    for i in 1..pts.len() {
        if let (Some(a), Some(b)) = (pts[i - 1].as_ref(), pts[i].as_ref()) {
            total_length += (*b - *a).magnitude();
        }
    }

    // Total turning angle (lower-bound on tangent rotation). The probe
    // misses up to one segment of curvature per endpoint, but for any
    // reasonably refined curve this underestimate is small and we
    // always clamp by min_segments afterwards.
    let mut total_angle = 0.0_f64;
    for i in 1..pts.len() - 1 {
        if let (Some(a), Some(b), Some(c)) =
            (pts[i - 1].as_ref(), pts[i].as_ref(), pts[i + 1].as_ref())
        {
            let v1 = *b - *a;
            let v2 = *c - *b;
            let m1 = v1.magnitude();
            let m2 = v2.magnitude();
            if m1 > 1e-12 && m2 > 1e-12 {
                let cos_t = (v1.dot(&v2) / (m1 * m2)).clamp(-1.0, 1.0);
                total_angle += cos_t.acos();
            }
        }
    }

    sample_count_from_length_angle(total_length, total_angle, params)
}

/// The triple-guard sample count from a polyline's total chord length and
/// total turning angle: the strictest of the max-edge-length,
/// max-angle-deviation, and chord-tolerance (sagitta) constraints, floored
/// at `min_segments.max(3)` and capped at `max_segments`. Shared by the
/// curve-only probe ([`compute_curve_sample_count`]) and — without the
/// floor — by the face-curvature probe ([`compute_face_boundary_sample_count`]).
fn sample_count_from_length_angle(
    total_length: f64,
    total_angle: f64,
    params: &TessellationParams,
) -> usize {
    // 1. Arc-length constraint.
    let n_len = if params.max_edge_length > 0.0 && total_length > 0.0 {
        (total_length / params.max_edge_length).ceil() as usize
    } else {
        params.min_segments
    };

    // 2. Angle-deviation constraint.
    let n_angle = if params.max_angle_deviation > 0.0 && total_angle > 0.0 {
        (total_angle / params.max_angle_deviation).ceil() as usize
    } else {
        params.min_segments
    };

    // 3. Chord-height (sagitta) constraint. Effective radius is
    //    r = total_length / total_angle (matches a circular arc
    //    exactly, conservative for non-uniform curvature). For
    //    sagitta < r, the per-segment angle is well-defined.
    let n_sag = if params.chord_tolerance > 0.0 && total_angle > 1e-9 && total_length > 0.0 {
        let radius = total_length / total_angle;
        if params.chord_tolerance < radius {
            let cos_half = 1.0 - params.chord_tolerance / radius;
            let theta_seg = 2.0 * cos_half.acos();
            if theta_seg > 0.0 {
                (total_angle / theta_seg).ceil() as usize
            } else {
                params.min_segments
            }
        } else {
            params.min_segments
        }
    } else {
        params.min_segments
    };

    n_len
        .max(n_angle)
        .max(n_sag)
        .max(params.min_segments.max(3))
        .min(params.max_segments)
}

/// Surface-curvature-driven boundary sample count contributed by ONE
/// adjacent face (CDT-γ.1). Probes the edge curve, projects each probe
/// point onto `surface`, and accumulates the turning of the surface unit
/// normal along the boundary plus the chord length. Returns a count from
/// the angle + sagitta constraints only — NO min-segment floor and NO
/// length-only term — so a boundary lying on a flat (or ruled-along-the-
/// edge) face contributes zero and the curve-only count governs, while a
/// face that bends along the boundary contributes a count proportional to
/// that bending. The caller takes the max over adjacent faces.
fn compute_face_boundary_sample_count(
    curve: &dyn Curve,
    t_start: f64,
    t_end: f64,
    surface: &dyn Surface,
    params: &TessellationParams,
) -> usize {
    const PROBE: usize = 16;
    let tol = Tolerance::default();

    let mut pts: Vec<Option<Point3>> = Vec::with_capacity(PROBE + 1);
    let mut normals: Vec<Option<Vector3>> = Vec::with_capacity(PROBE + 1);
    for i in 0..=PROBE {
        let t = t_start + (i as f64) * (t_end - t_start) / (PROBE as f64);
        match curve.point_at(t) {
            Ok(p) => {
                let n = surface
                    .closest_point(&p, tol)
                    .ok()
                    .and_then(|(u, v)| surface.normal_at(u, v).ok());
                pts.push(Some(p));
                normals.push(n);
            }
            Err(_) => {
                pts.push(None);
                normals.push(None);
            }
        }
    }

    let mut total_length = 0.0_f64;
    for i in 1..pts.len() {
        if let (Some(a), Some(b)) = (pts[i - 1], pts[i]) {
            total_length += (b - a).magnitude();
        }
    }
    let mut total_angle = 0.0_f64;
    for i in 1..normals.len() {
        if let (Some(a), Some(b)) = (normals[i - 1], normals[i]) {
            total_angle += a.dot(&b).clamp(-1.0, 1.0).acos();
        }
    }
    // Flat / ruled-along-the-edge boundary: no surface-curvature demand.
    if total_angle <= 1e-9 {
        return 0;
    }

    let n_angle = if params.max_angle_deviation > 0.0 {
        (total_angle / params.max_angle_deviation).ceil() as usize
    } else {
        0
    };
    let n_sag = if params.chord_tolerance > 0.0 && total_length > 0.0 {
        let radius = total_length / total_angle;
        if params.chord_tolerance < radius {
            let theta_seg = 2.0 * (1.0 - params.chord_tolerance / radius).acos();
            if theta_seg > 0.0 {
                (total_angle / theta_seg).ceil() as usize
            } else {
                0
            }
        } else {
            0
        }
    } else {
        0
    };
    n_angle.max(n_sag).min(params.max_segments)
}

/// Faces whose outer or inner loop references `edge_id`, found by walking
/// the model's shells. Kept local so the tessellation layer needs no
/// dependency on `operations`; mirrors the model-wide adjacency walk used
/// by edge classification.
fn adjacent_faces_of(model: &BRepModel, edge_id: EdgeId) -> Vec<FaceId> {
    let mut faces: Vec<FaceId> = Vec::with_capacity(2);
    for shell_entry in model.shells.iter() {
        let (_shell_id, shell) = shell_entry;
        for &face_id in &shell.faces {
            if faces.contains(&face_id) {
                continue;
            }
            let face = match model.faces.get(face_id) {
                Some(f) => f,
                None => continue,
            };
            let touches = model
                .loops
                .get(face.outer_loop)
                .map_or(false, |l| l.edges.iter().any(|&e| e == edge_id))
                || face.inner_loops.iter().any(|&il| {
                    model
                        .loops
                        .get(il)
                        .map_or(false, |l| l.edges.iter().any(|&e| e == edge_id))
                });
            if touches {
                faces.push(face_id);
            }
        }
    }
    faces
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vector3;
    use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

    /// Build a unit box centred at the origin and return the model
    /// together with the IDs of (a) one straight box edge and (b) the
    /// edges store for ID enumeration.
    fn unit_box_model() -> BRepModel {
        let mut model = BRepModel::new();
        let mut builder = TopologyBuilder::new(&mut model);
        let geom = builder
            .create_box_3d(1.0, 1.0, 1.0)
            .expect("create_box_3d must succeed for positive dimensions");
        // Drop the builder before returning so the &mut borrow of model ends.
        let _ = match geom {
            GeometryId::Solid(id) => id,
            other => panic!("create_box_3d must return a Solid, got {other:?}"),
        };
        drop(builder);
        model
    }

    /// Pick any edge ID from the model. Box edges are all straight
    /// lines, so this is sufficient for the collinearity test.
    fn any_edge_id(model: &BRepModel) -> EdgeId {
        model
            .edges
            .iter()
            .next()
            .map(|(id, _edge)| id)
            .expect("box must have at least one edge")
    }

    #[test]
    fn cache_returns_same_arc_for_same_edge() {
        // Identity, not just equality: the second call must return a
        // clone of the SAME `Arc` so the DashMap entry is reused.
        let model = unit_box_model();
        let edge_id = any_edge_id(&model);
        let cache = EdgeSampleCache::new(&TessellationParams::default());
        let a = cache.get_or_compute(edge_id, &model);
        let b = cache.get_or_compute(edge_id, &model);
        assert!(
            Arc::ptr_eq(&a, &b),
            "cache must return the same Arc instance on repeated calls; got distinct pointers"
        );
    }

    #[test]
    fn cache_endpoints_match_vertex_positions() {
        // The first and last sample of each edge must coincide with
        // the edge's start/end vertex positions within numerical noise.
        let model = unit_box_model();
        let cache = EdgeSampleCache::new(&TessellationParams::default());
        for (edge_id, edge) in model.edges.iter() {
            let samples = cache.get_or_compute(edge_id, &model);
            assert!(
                samples.len() >= 2,
                "edge {edge_id} produced fewer than 2 samples: {}",
                samples.len()
            );
            let v_start = model
                .vertices
                .get(edge.start_vertex)
                .expect("start vertex must exist for valid edge");
            let v_end = model
                .vertices
                .get(edge.end_vertex)
                .expect("end vertex must exist for valid edge");
            let p_start = Point3::new(
                v_start.position[0],
                v_start.position[1],
                v_start.position[2],
            );
            let p_end = Point3::new(v_end.position[0], v_end.position[1], v_end.position[2]);
            let d_start = (samples[0] - p_start).magnitude();
            let d_end = (samples[samples.len() - 1] - p_end).magnitude();
            assert!(
                d_start < 1e-9,
                "samples[0] for edge {edge_id} disagrees with start vertex by {d_start:e}"
            );
            assert!(
                d_end < 1e-9,
                "samples[last] for edge {edge_id} disagrees with end vertex by {d_end:e}"
            );
        }
    }

    #[test]
    fn cache_collapses_collinear_edges_to_two_samples() {
        // Box edges are all straight lines between two vertices; the
        // collinearity probe should fire and the cache should emit
        // exactly 2 samples (endpoints only).
        let model = unit_box_model();
        let edge_id = any_edge_id(&model);
        let cache = EdgeSampleCache::new(&TessellationParams::default());
        let samples = cache.get_or_compute(edge_id, &model);
        assert_eq!(
            samples.len(),
            2,
            "straight box edge should collapse to 2 samples; got {} = {samples:?}",
            samples.len()
        );
    }

    #[test]
    fn cache_handles_closed_edge_without_collinearity_shortcircuit() {
        // Build a circle as a closed edge (start_vertex == end_vertex)
        // and confirm the cache emits the full triple-guard sample
        // count rather than collapsing to 2. We don't have a public
        // helper for "build a single closed edge" so we exercise the
        // compute_curve_sample_count branch directly through a curve
        // proxy and assert the closed-edge branch yields ≥ min_segments
        // points.
        use crate::primitives::curve::Circle;
        let curve = Circle::new(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 1.0)
            .expect("unit circle in xy-plane must be constructible");
        let params = TessellationParams::default();
        // Probe count over the full circle should respect min_segments
        // and exceed the trivial collinear case.
        let n = compute_curve_sample_count(&curve, 0.0, std::f64::consts::TAU, &params);
        assert!(
            n >= params.min_segments.max(3),
            "closed circle should sample at least min_segments={} times; got {n}",
            params.min_segments.max(3)
        );
    }

    #[test]
    fn face_boundary_count_zero_on_flat_plane() {
        // A straight edge lying on a flat plane: the surface normal is
        // constant along it, so the face-curvature term contributes no
        // extra boundary samples (the curve-only count governs, keeping
        // straight box edges collapsed to two samples).
        use crate::primitives::curve::Line;
        use crate::primitives::surface::Plane;
        let plane = Plane::xy(0.0);
        let line = Line::new(Point3::new(-1.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0));
        let params = TessellationParams::default();
        let n = compute_face_boundary_sample_count(&line, 0.0, 1.0, &plane, &params);
        assert_eq!(
            n, 0,
            "a straight edge on a flat plane needs no surface-curvature densification"
        );
    }

    #[test]
    fn face_boundary_count_positive_on_curved_surface() {
        // The equator circle lies on the sphere; the surface normal
        // (radial) turns through 2π as the curve is traversed, so the face
        // demands a denser boundary than a flat face would.
        use crate::primitives::curve::Circle;
        use crate::primitives::surface::Sphere;
        let r = 2.0;
        let sphere = Sphere::new(Point3::new(0.0, 0.0, 0.0), r).expect("sphere must construct");
        let circle =
            Circle::new(Point3::new(0.0, 0.0, 0.0), Vector3::Z, r).expect("circle must construct");
        let params = TessellationParams::default();
        let n = compute_face_boundary_sample_count(
            &circle,
            0.0,
            std::f64::consts::TAU,
            &sphere,
            &params,
        );
        assert!(
            n > 0,
            "a curved edge on a sphere should request surface-curvature densification, got {n}"
        );
    }
}
