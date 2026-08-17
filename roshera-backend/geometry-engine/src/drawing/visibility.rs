//! Hidden-line removal (#22) — visibility classification via the raytrace eye.
//!
//! A mechanical drawing draws OCCLUDED edges as a distinct dashed line, not as
//! a solid edge and not omitted. Visibility here is decided by the SAME analytic
//! ray-cast the perception layer uses (`queries::raycast_solid`): a point on an
//! edge is HIDDEN when, looking from the camera along the view direction, the
//! solid's own surface is hit nearer than that point — i.e. another face is in
//! front of it. No tessellation, no z-buffer raster: every classification is an
//! exact ray↔analytic-surface test, so the drawing cannot claim a hidden edge
//! is visible (a sound-eye violation) or vice-versa.
//!
//! Edges are classified PER SEGMENT (at each sampled sub-span's midpoint), so a
//! partially-occluded edge splits at the crossover into a visible run and a
//! hidden run — the drafting convention.

use std::collections::HashSet;

use crate::math::{Matrix4, Point3, Vector3};
use crate::primitives::edge::EdgeId;
use crate::primitives::face::{Face, FaceId};
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::queries::raycast::ray_hit_face_t;
use crate::queries::raycast_solid;
use crate::tessellation::surface::LoopUvCache;

use super::projection::{view_matrix_for_projection, ProjectionError};
use super::types::{Polyline2d, ProjectionType};

/// Test-only instrument: total ray↔trimmed-face tests [`OcclusionGrid::occluded`]
/// actually executed on this thread (i.e. candidates surviving both the spatial
/// and depth culls), since the last [`reset_ray_face_test_counter`]. Compiled
/// only under `#[cfg(test)]` — zero cost and zero surface in production builds.
///
/// Exists so a regression test can assert on the WORK DONE, not just wall-clock
/// time: a wall-clock budget alone caught the `drawing_perf` 11x regression only
/// because it happened to blow a generous 30s budget on THIS fixture — a smaller
/// regression, or a faster/loaded machine, would pass a time-only gate while a
/// cull silently degraded. Thread-local (not a shared `static`) so parallel test
/// threads never see each other's counts.
#[cfg(test)]
thread_local! {
    static RAY_FACE_TEST_COUNTER: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_ray_face_test_counter() {
    RAY_FACE_TEST_COUNTER.with(|c| c.set(0));
}

#[cfg(test)]
pub(crate) fn ray_face_test_counter() -> u64 {
    RAY_FACE_TEST_COUNTER.with(|c| c.get())
}

/// Broad-phase occlusion accelerator for HLR (#22 perf).
///
/// Every occlusion ray in a view is PARALLEL — cast along the fixed into-scene
/// direction `w`. So the brute-force `raycast_solid` (O(F) per sampled segment,
/// giving the pathological O(views · E · samples · F) that wedged the backend on
/// a 293-face gear) is replaced by a 2D bucket grid over each face's projected
/// AABB: a query point can only be occluded by a face whose projection covers
/// it, so only that grid cell's faces are ray-tested.
///
/// **Soundness / determinism.** Each face's stored 2D AABB is a *conservative
/// superset* of the face's true projection (built from dense boundary-curve
/// samples plus a surface parameter grid, then padded by one cell). Any face the
/// ray can actually hit projects onto the query point and therefore lands in its
/// cell, so the accelerated occlusion boolean is *identical* to the brute-force
/// nearest-hit result — no hidden/visible edge is reclassified. Over-inclusion
/// (a candidate whose projection does not really cover the point) is harmless:
/// the exact per-face ray test simply reports no hit. The drawing is byte-for-
/// byte the same as the brute path; only the work is smaller.
struct OcclusionGrid {
    /// Into-scene unit view direction (occlusion ray direction).
    w: Vector3,
    /// Ray back-off distance and occlusion epsilon (mirrors [`occluded`]).
    back: f64,
    eps: f64,
    /// Grid origin (min projected u, v) and inverse cell size.
    min_u: f64,
    min_v: f64,
    cell_u: f64,
    cell_v: f64,
    nx: usize,
    ny: usize,
    /// Face ids bucketed per cell (row-major `y * nx + x`), plus the flat face
    /// list used when a query falls outside the grid extent.
    cells: Vec<Vec<FaceId>>,
    all_faces: Vec<FaceId>,
    /// Per-face conservative NEAR depth along `w` (minimum of `p·w` over the
    /// face's padded 3D box). A face can occlude a query point `m` only if some
    /// of it is nearer the camera than `m` (smaller `·w`); if even this lower
    /// bound is deeper than `m`, the whole face is behind `m` and is skipped.
    /// Indexed by `FaceId as usize`; `f64::NEG_INFINITY` = "never cull".
    near_depth: Vec<f64>,
    /// Per-view cache of each probed face's trim-loop UV polygon (see
    /// [`LoopUvCache`]) — a face is probed by [`OcclusionGrid::occluded`] once
    /// per candidate sample point, and the polygon it rebuilds from scratch
    /// per probe is the dominant per-test cost (task: `drawing_perf` 11x
    /// regression root-cause). Scoped to this grid's lifetime, i.e. one view
    /// of one `&BRepModel` snapshot.
    poly_cache: LoopUvCache,
}

impl OcclusionGrid {
    fn build(
        model: &BRepModel,
        solid_id: SolidId,
        vm: &Matrix4,
        w: Vector3,
        back: f64,
        eps: f64,
    ) -> Self {
        // Gather every face (outer + inner shells) — the same set brute-force
        // `raycast_solid` intersects.
        let mut all_faces: Vec<FaceId> = Vec::new();
        if let Some(solid) = model.solids.get(solid_id) {
            let mut shell_ids = vec![solid.outer_shell];
            shell_ids.extend_from_slice(&solid.inner_shells);
            shell_ids.extend_from_slice(&solid.peer_shells);
            for sh in shell_ids {
                if let Some(shell) = model.shells.get(sh) {
                    all_faces.extend_from_slice(&shell.faces);
                }
            }
        }

        // Conservative projected 2D AABB (spatial cull) + near-depth (depth cull)
        // per face.
        let max_fid = all_faces.iter().copied().max().unwrap_or(0) as usize;
        let mut near_depth = vec![f64::NEG_INFINITY; max_fid + 1];
        let mut boxes: Vec<(FaceId, [f64; 4])> = Vec::with_capacity(all_faces.len());
        let (mut gmin_u, mut gmin_v, mut gmax_u, mut gmax_v) = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        for &fid in &all_faces {
            if let Some(fb) = face_bounds(model, fid, vm) {
                let bb = fb.aabb2;
                gmin_u = gmin_u.min(bb[0]);
                gmin_v = gmin_v.min(bb[1]);
                gmax_u = gmax_u.max(bb[2]);
                gmax_v = gmax_v.max(bb[3]);
                // Support of the padded 3D box along `-w`: the minimum `p·w` over
                // the box = Σ (w_k ≥ 0 ? min_k : max_k)·w_k. A rigorous lower
                // bound of the face's nearest depth.
                let wv = [w.x, w.y, w.z];
                let mut near = 0.0;
                for k in 0..3 {
                    near += if wv[k] >= 0.0 {
                        fb.min3[k] * wv[k]
                    } else {
                        fb.max3[k] * wv[k]
                    };
                }
                near_depth[fid as usize] = near;
                boxes.push((fid, bb));
            }
        }

        if boxes.is_empty() || !gmin_u.is_finite() {
            return OcclusionGrid {
                w,
                back,
                eps,
                min_u: 0.0,
                min_v: 0.0,
                cell_u: 1.0,
                cell_v: 1.0,
                nx: 1,
                ny: 1,
                cells: vec![all_faces.clone()],
                all_faces,
                near_depth,
                poly_cache: LoopUvCache::new(),
            };
        }

        // Grid resolution ~ sqrt(F) per axis (so average ~1 face/cell), capped so
        // a huge part can't allocate an enormous grid.
        let f = boxes.len().max(1);
        let n = (f as f64).sqrt().ceil().max(1.0) as usize;
        let nx = n.clamp(1, 96);
        let ny = n.clamp(1, 96);
        let span_u = (gmax_u - gmin_u).max(1e-6);
        let span_v = (gmax_v - gmin_v).max(1e-6);
        let cell_u = span_u / nx as f64;
        let cell_v = span_v / ny as f64;

        let mut cells: Vec<Vec<FaceId>> = vec![Vec::new(); nx * ny];
        let idx = |x: usize, y: usize| y * nx + x;
        let clampx = |x: i64| x.clamp(0, nx as i64 - 1) as usize;
        let clampy = |y: i64| y.clamp(0, ny as i64 - 1) as usize;
        for (fid, bb) in &boxes {
            // Pad by one cell so a face grazing a cell boundary is never dropped
            // from the neighbouring cell it truly touches.
            let x0 = clampx(((bb[0] - cell_u - gmin_u) / cell_u).floor() as i64);
            let y0 = clampy(((bb[1] - cell_v - gmin_v) / cell_v).floor() as i64);
            let x1 = clampx(((bb[2] + cell_u - gmin_u) / cell_u).floor() as i64);
            let y1 = clampy(((bb[3] + cell_v - gmin_v) / cell_v).floor() as i64);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    cells[idx(x, y)].push(*fid);
                }
            }
        }

        OcclusionGrid {
            w,
            back,
            eps,
            min_u: gmin_u,
            min_v: gmin_v,
            cell_u,
            cell_v,
            nx,
            ny,
            cells,
            all_faces,
            near_depth,
            poly_cache: LoopUvCache::new(),
        }
    }

    /// Candidate occluder faces for a 2D query point (the cell it lands in).
    fn candidates(&self, u: f64, v: f64) -> &[FaceId] {
        if self.cells.len() == 1 {
            return &self.cells[0];
        }
        let x = ((u - self.min_u) / self.cell_u).floor();
        let y = ((v - self.min_v) / self.cell_v).floor();
        if !x.is_finite() || !y.is_finite() {
            return &self.all_faces;
        }
        let xi = (x as i64).clamp(0, self.nx as i64 - 1) as usize;
        let yi = (y as i64).clamp(0, self.ny as i64 - 1) as usize;
        &self.cells[yi * self.nx + xi]
    }

    /// Is world point `m` occluded, viewed along `w`? Early-outs on the first
    /// candidate face whose trimmed hit is nearer than `m` — existentially
    /// equal to brute-force `raycast_solid`'s nearest-hit `< back − eps` test.
    ///
    /// Two conservative culls precede the (expensive) trimmed ray test:
    /// the *spatial* cull (only the query cell's faces, [`candidates`]) and the
    /// *depth* cull (a face whose whole padded box is deeper than `m` can't be
    /// nearer than `m`, so it can't occlude). Both only ever DROP faces that
    /// genuinely can't occlude, so the boolean is identical to brute force —
    /// critically, this keeps visible silhouette points (no early-out) cheap on
    /// coaxial parts, where every face projects onto the same region.
    fn occluded(&self, model: &BRepModel, m: Point3, u: f64, v: f64) -> bool {
        let origin = m - self.w * self.back;
        let m_depth = m.x * self.w.x + m.y * self.w.y + m.z * self.w.z;
        for &fid in self.candidates(u, v) {
            // Depth cull: face entirely behind `m` (its nearest point is still
            // deeper) cannot occlude `m`.
            if self.near_depth[fid as usize] > m_depth + self.eps {
                continue;
            }
            #[cfg(test)]
            RAY_FACE_TEST_COUNTER.with(|c| c.set(c.get() + 1));
            if let Some(t) = ray_hit_face_t(model, fid, origin, self.w, &self.poly_cache) {
                if t < self.back - self.eps {
                    return true;
                }
            }
        }
        false
    }
}

/// Conservative bounds of a face for the occlusion broad phase: the projected 2D
/// AABB `[min_u, min_v, max_u, max_v]` (spatial cull) and the 3D world AABB
/// `([min_xyz], [max_xyz])` (depth cull). Built from dense boundary edge-curve
/// samples plus a surface parameter grid, with sphere faces folded in
/// analytically, so a curved face's interior bulge and rim arcs are bounded and
/// never under-bounded (see [`OcclusionGrid`] soundness note). The 3D box is
/// expanded 1% + 1e-3 so sphere/NURBS inter-sample dip can't escape it — over-
/// inclusion only weakens culling, never correctness.
struct FaceBounds {
    aabb2: [f64; 4],
    min3: [f64; 3],
    max3: [f64; 3],
}

fn face_bounds(model: &BRepModel, face_id: FaceId, vm: &Matrix4) -> Option<FaceBounds> {
    let face = model.faces.get(face_id)?;
    let surface = model.surfaces.get(face.surface_id)?;
    let tol = model.tolerance();
    let (mut min_u, mut min_v, mut max_u, mut max_v) = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    let (mut min3, mut max3) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    let mut include = |p: Point3| {
        let q = vm.transform_point(&p);
        min_u = min_u.min(q.x);
        min_v = min_v.min(q.y);
        max_u = max_u.max(q.x);
        max_v = max_v.max(q.y);
        for (k, c) in [p.x, p.y, p.z].into_iter().enumerate() {
            min3[k] = min3[k].min(c);
            max3[k] = max3[k].max(c);
        }
    };

    // Boundary edge-curve samples (captures rim arcs / silhouette rings exactly)
    // and the (u, v) parameter box those boundary vertices span.
    let mut uvs: Vec<(f64, f64)> = Vec::new();
    let mut loop_ids = vec![face.outer_loop];
    loop_ids.extend(face.inner_loops.iter().copied());
    for lid in loop_ids {
        let Some(lp) = model.loops.get(lid) else {
            continue;
        };
        for &eid in &lp.edges {
            let Some(edge) = model.edges.get(eid) else {
                continue;
            };
            let Some(curve) = model.curves.get(edge.curve_id) else {
                continue;
            };
            let (t0, t1) = (edge.param_range.start, edge.param_range.end);
            let is_linear = curve.is_linear(crate::math::Tolerance::default());
            let n = if is_linear { 2 } else { 24 };
            for i in 0..n {
                let frac = i as f64 / (n - 1).max(1) as f64;
                let t = t0 + (t1 - t0) * frac;
                if let Ok(p) = curve.point_at(t) {
                    include(p);
                    if let Ok(uv) = surface.closest_point(&p, tol) {
                        uvs.push(uv);
                    }
                }
            }
        }
    }

    // Surface parameter grid across the boundary's projected param box, so a
    // curved face's interior (a sphere cap's crown, a cone's flank) is bounded.
    if uvs.len() >= 2 {
        let (mut u0, mut u1, mut v0, mut v1) = (
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
        );
        for &(u, v) in &uvs {
            u0 = u0.min(u);
            u1 = u1.max(u);
            v0 = v0.min(v);
            v1 = v1.max(v);
        }
        const N: usize = 8;
        for i in 0..N {
            let fu = i as f64 / (N - 1) as f64;
            let u = u0 + (u1 - u0) * fu;
            for j in 0..N {
                let fv = j as f64 / (N - 1) as f64;
                let v = v0 + (v1 - v0) * fv;
                if let Ok(p) = surface.point_at(u, v) {
                    include(p);
                }
            }
        }
    }

    // Analytic sphere envelope: a sphere face's interior crown can dip past any
    // finite sample grid, so fold the exact ±radius box in (rigorous depth bound).
    if let Some(sph) = surface
        .as_any()
        .downcast_ref::<crate::primitives::surface::Sphere>()
    {
        let (c, r) = (sph.center, sph.radius);
        include(Point3::new(c.x - r, c.y - r, c.z - r));
        include(Point3::new(c.x + r, c.y + r, c.z + r));
    }

    if !(min_u.is_finite() && max_u >= min_u) {
        return None;
    }

    // Expand the 3D box (1% of diagonal + 1e-3) so any residual inter-sample dip
    // on a curved surface stays inside it; a looser box only culls less.
    let diag =
        ((max3[0] - min3[0]).powi(2) + (max3[1] - min3[1]).powi(2) + (max3[2] - min3[2]).powi(2))
            .sqrt();
    let pad = 0.01 * diag + 1e-3;
    for k in 0..3 {
        min3[k] -= pad;
        max3[k] += pad;
    }

    Some(FaceBounds {
        aabb2: [min_u, min_v, max_u, max_v],
        min3,
        max3,
    })
}

/// The edges of a view split by visibility. `visible` draws solid; `hidden`
/// draws dashed. Closed circular edges that project to a TRUE circle are split
/// out as analytic `circles` / `hidden_circles` (rendered as exact SVG
/// circles, not faceted polylines); circular edges viewed OBLIQUELY are split
/// out as `ellipses` / `hidden_ellipses` (Fix 2,
/// `.superpowers/sdd/2026-08-16-exact-curves/brief.md`) — the general case of
/// a circle projected under an orthographic view.
#[derive(Debug, Clone)]
pub struct ViewEdges {
    pub visible: Vec<Polyline2d>,
    pub hidden: Vec<Polyline2d>,
    pub circles: Vec<super::types::ProjectedCircle>,
    pub hidden_circles: Vec<super::types::ProjectedCircle>,
    pub ellipses: Vec<super::types::ProjectedEllipse>,
    pub hidden_ellipses: Vec<super::types::ProjectedEllipse>,
    /// Per-polyline B-Rep lineage, parallel to [`Self::visible`] (campaign #55
    /// residual — view-polyline edge provenance). `visible_sources[i]` is the
    /// edge/face identity of `visible[i]`.
    pub visible_sources: Vec<super::types::PolylineSource>,
    /// Per-polyline lineage parallel to [`Self::hidden`].
    pub hidden_sources: Vec<super::types::PolylineSource>,
}

/// `(center3d, unit normal, radius)` if `curve` lies on a circle (a full
/// Circle or an Arc — a boolean often fragments a rim into several co-circular
/// arc-edges, which we regroup downstream into one drawn circle).
fn circular_geom(curve: &dyn crate::primitives::curve::Curve) -> Option<(Point3, Vector3, f64)> {
    if let Some(c) = curve
        .as_any()
        .downcast_ref::<crate::primitives::curve::Circle>()
    {
        return Some((c.center(), c.normal(), c.radius()));
    }
    if let Some(a) = curve
        .as_any()
        .downcast_ref::<crate::primitives::curve::Arc>()
    {
        return Some((a.center, a.normal, a.radius));
    }
    None
}

/// Accumulates co-circular arc-edges of one rim so the whole circle is drawn
/// once. `all_visible`/`all_hidden` stay true only while every arc is uniformly
/// that; a genuinely mixed rim falls back to its per-arc `fallback` polylines.
struct CircleGroup {
    cx: f64,
    cy: f64,
    r: f64,
    all_visible: bool,
    all_hidden: bool,
    fallback: Vec<(bool, Polyline2d)>,
    /// All B-Rep face ids adjacent to any arc-edge of this rim (cap face +
    /// lateral face). Threaded onto [`ProjectedCircle::face_ids`] so downstream
    /// consumers (hole-table tag assignment) can resolve a circle back to the
    /// feature that produced it by entity identity.
    face_ids: Vec<u32>,
}

/// As [`CircleGroup`], but for a rim whose circle plane does NOT face the
/// camera and therefore projects to a true ellipse (Fix 2).
struct EllipseGroup {
    cx: f64,
    cy: f64,
    rx: f64,
    ry: f64,
    rotation: f64,
    all_visible: bool,
    all_hidden: bool,
    fallback: Vec<(bool, Polyline2d)>,
    face_ids: Vec<u32>,
}

/// A circle projected through an orthographic view matrix, exactly: view-space
/// centre, semi-major/semi-minor axis lengths, and the major axis's rotation
/// from view-space +X (radians). `None` only on a degenerate input (a
/// non-unit-normalizable normal, radius/scale collapsing to zero) — the
/// caller falls back to sampling in that case.
///
/// Below [`MIN_ELLIPSE_MINOR_FRACTION`] of the major axis (or an absolute
/// floor), the minor axis is treated as having collapsed to a line: an SVG/DXF
/// consumer cannot draw a meaningfully thin ellipse (an `rx`/`ry` of exactly
/// zero does not render at all in SVG), and geometrically this is the
/// EDGE-ON case — a rim whose plane contains the view direction, which
/// projects to a straight chord, not a curve. That is a real and distinct
/// shape from "a very flat true ellipse"; sampling remains the honest
/// representation for it rather than an ellipse a renderer would drop.
///
/// # Method
///
/// The circle is `C(θ) = center + r·cosθ·e1 + r·sinθ·e2` for an orthonormal
/// basis `(e1, e2)` of the circle's plane. Projecting through the view's
/// linear (orthographic) map `M = [u; v]` (its rotation, dropping view-space
/// Z) gives `p(θ) = M·center + r·(M·e1)·cosθ + r·(M·e2)·sinθ` — the image of
/// a unit circle under the 2×2 matrix whose columns are `r·(M·e1)` and
/// `r·(M·e2)`. That image is an ellipse whose semi-axis lengths are the
/// singular values of that matrix and whose axis directions are its left
/// singular vectors; both are recovered in closed form from the eigenvalues
/// of the matrix's own `M·Mᵀ` (a 2×2 symmetric matrix), which is
/// numerically identical to an SVD for a real 2×2 map and avoids pulling in
/// a general SVD routine for a two-line closed form. Verified by hand for
/// the camera-facing case (recovers `rx = ry = r`) and a circle tilted by
/// angle `α` from facing the camera about an in-plane axis (recovers the
/// standard foreshortening `rx = r`, `ry = r·cos α`, major axis normal to
/// the tilt) — see this module's own tests.
fn circle_to_view_ellipse(
    view_matrix: &Matrix4,
    center: Point3,
    normal: Vector3,
    radius: f64,
) -> Option<ViewEllipse> {
    let n = normal.normalize().ok()?;
    // Any world axis not (near-)parallel to n serves as a helper to build an
    // orthonormal in-plane basis.
    let helper = if n.x.abs() < 0.9 {
        Vector3::X
    } else {
        Vector3::Y
    };
    let e1 = n.cross(&helper).normalize().ok()?;
    let e2 = n.cross(&e1).normalize().ok()?;

    let c2 = view_matrix.transform_point(&center);
    let a2 = view_matrix.transform_vector(&e1);
    let b2 = view_matrix.transform_vector(&e2);

    // M's columns, scaled by radius.
    let m00 = radius * a2.x;
    let m01 = radius * b2.x;
    let m10 = radius * a2.y;
    let m11 = radius * b2.y;

    // S = M·Mᵀ (symmetric 2×2): eigen-decomposition gives the ellipse's
    // semi-axis lengths (sqrt of eigenvalues) and major-axis rotation.
    let sxx = m00 * m00 + m01 * m01;
    let syy = m10 * m10 + m11 * m11;
    let sxy = m00 * m10 + m01 * m11;

    let mid = (sxx + syy) * 0.5;
    let diff = (sxx - syy) * 0.5;
    let disc = (diff * diff + sxy * sxy).max(0.0).sqrt();
    let lambda1 = (mid + disc).max(0.0);
    let lambda2 = (mid - disc).max(0.0);
    let rx = lambda1.sqrt();
    let ry = lambda2.sqrt();
    if !(rx.is_finite() && ry.is_finite()) || rx <= 0.0 {
        return None;
    }
    let rotation = if disc > 1e-15 {
        0.5 * sxy.atan2(diff)
    } else {
        0.0
    };

    Some(ViewEllipse {
        cx: c2.x,
        cy: c2.y,
        rx,
        ry,
        rotation,
    })
}

/// A circle's exact projection through an orthographic view matrix. See
/// [`circle_to_view_ellipse`].
#[derive(Debug, Clone, Copy, PartialEq)]
struct ViewEllipse {
    cx: f64,
    cy: f64,
    rx: f64,
    ry: f64,
    rotation: f64,
}

/// Below this fraction of the major axis, an ellipse is treated as
/// degenerate (edge-on) rather than drawn. See [`circle_to_view_ellipse`]'s
/// doc comment for why this is a distinct shape, not just "a thin ellipse".
const MIN_ELLIPSE_MINOR_FRACTION: f64 = 0.02;

/// Into-scene view direction (unit) for a projection: the third row of the
/// world→view matrix, recovered as `(Tx.z, Ty.z, Tz.z)` where `T` transforms a
/// world vector to view space (`row_w · e_i = w_i`).
fn view_direction(projection: ProjectionType) -> Vector3 {
    let vm = view_matrix_for_projection(projection);
    let w = Vector3::new(
        vm.transform_vector(&Vector3::X).z,
        vm.transform_vector(&Vector3::Y).z,
        vm.transform_vector(&Vector3::Z).z,
    );
    w.normalize().unwrap_or(Vector3::Z)
}

/// World AABB diagonal of a solid, from its face-loop vertices. Used to place
/// ray origins safely outside the part and to scale the occlusion epsilon.
fn solid_diagonal(model: &BRepModel, solid_id: SolidId) -> f64 {
    let solid = match model.solids.get(solid_id) {
        Some(s) => s,
        None => return 1.0,
    };
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    let mut any = false;
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    shells.extend_from_slice(&solid.peer_shells);
    for sh in shells {
        let shell = match model.shells.get(sh) {
            Some(s) => s,
            None => continue,
        };
        for &fid in &shell.faces {
            let face = match model.faces.get(fid) {
                Some(f) => f,
                None => continue,
            };
            let mut loops = vec![face.outer_loop];
            loops.extend_from_slice(&face.inner_loops);
            for lid in loops {
                let lp = match model.loops.get(lid) {
                    Some(l) => l,
                    None => continue,
                };
                for &eid in &lp.edges {
                    if let Some(e) = model.edges.get(eid) {
                        for vid in [e.start_vertex, e.end_vertex] {
                            if let Some(v) = model.vertices.get(vid) {
                                for i in 0..3 {
                                    if v.position[i] < min[i] {
                                        min[i] = v.position[i];
                                    }
                                    if v.position[i] > max[i] {
                                        max[i] = v.position[i];
                                    }
                                }
                                any = true;
                            }
                        }
                    }
                }
            }
        }
    }
    if !any {
        return 1.0;
    }
    let dx = max[0] - min[0];
    let dy = max[1] - min[1];
    let dz = max[2] - min[2];
    (dx * dx + dy * dy + dz * dz).sqrt().max(1.0)
}

/// Core occlusion test: is `m` hidden, viewed along `w`? Cast from `back` units
/// behind `m` (toward the camera) along `w`; `m` sits at ray parameter `back`,
/// so a nearer hit (`< back − eps`) means another face occludes it.
fn occluded(
    model: &BRepModel,
    solid_id: SolidId,
    m: Point3,
    w: Vector3,
    back: f64,
    eps: f64,
) -> bool {
    let origin = m - w * back;
    match raycast_solid(model, solid_id, origin, w) {
        Some(hit) => hit.distance < back - eps,
        None => false,
    }
}

/// Is world point `p` hidden behind the solid in this view? Public so callers /
/// tests can probe visibility of an arbitrary point directly (the crisp sound
/// property: a point on the far face is hidden, one on the near face is not).
pub fn is_point_hidden(
    model: &BRepModel,
    solid_id: SolidId,
    projection: ProjectionType,
    p: Point3,
) -> bool {
    let w = view_direction(projection);
    let diag = solid_diagonal(model, solid_id);
    let back = 2.0 * diag + 10.0;
    let eps = diag * 1e-5 + 1e-3;
    occluded(model, solid_id, p, w, back, eps)
}

// ─────────────────────────────────────────────────────────────────────────
// Synthesized curved-surface silhouettes (2026-08-17 brief)
//
// A B-Rep cylinder/cone carries exactly one topological SEAM edge — the
// parameterisation's `u = 0` wrap line, referenced TWICE by the lateral
// face's own loop (`TopologyBuilder::create_cylinder_topology`'s doc
// comment: "the seam MUST coincide with the circles' parametric origin").
// The seam gets drawn today because the HLR walk draws every real
// topological edge, so it reads as a silhouette only when it happens to
// face the camera. The true outline — the locus where the surface normal
// turns perpendicular to the view, `n(u)·w = 0` — is generally a DIFFERENT
// line, and nothing has ever synthesized it.
//
// Cylinder and Cone are RULED surfaces whose normal is constant along each
// generator (independent of `v` — see both surfaces' own `evaluate_full`
// doc comments), so `n(u)·w = 0` reduces to a trig equation in `u` alone
// and picks out up to two whole straight GENERATOR lines, not merely two
// points. A Sphere's silhouette is the great circle in the plane through
// its centre perpendicular to `w` — always camera-facing by construction,
// so unlike the ruled-surface case it is a CIRCLE, and is synthesized by
// reusing the exact analytic-circle accumulation a real rim edge already
// takes (`circle_groups`), so it flushes through the SAME
// uniform-visibility / mixed-rim-fallback logic downstream rather than a
// second copy of it — and renders as an exact SVG circle, never a sampled
// polyline, matching the crate's exact-conic principle.
// ─────────────────────────────────────────────────────────────────────────

/// True when `edge_id` is a pure parameterisation SEAM rather than a design
/// feature: a cylinder/cone lateral face's own loop references it TWICE (u=0
/// forward and backward), so it borders exactly ONE distinct face — where a
/// genuine shared edge borders two. Combined with the surface-type check
/// this is a tight, false-positive-free predicate: no legitimate edge in a
/// closed manifold B-Rep is bordered by only one face-use.
///
/// **Ruling (brief's open question):** a seam is suppressed HERE
/// unconditionally, whether or not it happens to coincide with the true
/// silhouette in this view. It is a parameterisation artifact, not a
/// feature — drawing it when it does NOT lie on the silhouette would ink a
/// false line down the middle of the surface, and drawing it when it DOES
/// would risk a double-draw with the synthesized silhouette below. Both
/// true silhouette generators are synthesized unconditionally by
/// [`emit_line_silhouette`], so a seam that does face the camera still gets
/// exactly the right ink — just sourced from that synthesis, not the
/// topological edge, and carrying [`super::types::PolylineRole::Silhouette`]
/// instead of `Edge` provenance.
fn is_parameterization_seam(
    model: &BRepModel,
    edge_id: EdgeId,
    edge_faces: &std::collections::HashMap<EdgeId, Vec<u32>>,
) -> bool {
    let Some(faces) = edge_faces.get(&edge_id) else {
        return false;
    };
    if faces.len() != 1 {
        return false;
    }
    let Some(face) = model.faces.get(faces[0]) else {
        return false;
    };
    let Some(surface) = model.surfaces.get(face.surface_id) else {
        return false;
    };
    surface
        .as_any()
        .downcast_ref::<crate::primitives::surface::Cylinder>()
        .is_some()
        || surface
            .as_any()
            .downcast_ref::<crate::primitives::surface::Cone>()
            .is_some()
}

/// Bundled occlusion-classification context, shared by the real-edge walk
/// and synthesized silhouette curves so both go through IDENTICAL logic —
/// the accelerator strategy and the `occlude = false` iso shortcut apply
/// uniformly to every segment classified in a view, real or synthesized.
struct ViewOcclusionCtx<'a> {
    model: &'a BRepModel,
    solid_id: SolidId,
    w: Vector3,
    back: f64,
    eps: f64,
    occlude: bool,
    accel: &'a Option<OcclusionGrid>,
}

impl ViewOcclusionCtx<'_> {
    fn visible(&self, mid: Point3, mu: f64, mv: f64) -> bool {
        if !self.occlude {
            true
        } else if let Some(accel) = self.accel {
            !accel.occluded(self.model, mid, mu, mv)
        } else {
            !occluded(self.model, self.solid_id, mid, self.w, self.back, self.eps)
        }
    }
}

/// Classify every consecutive segment of a sampled (3D, 2D) point chain as
/// visible/hidden and group into maximal same-visibility runs — the exact
/// grouping the real-edge walk performs (per-segment midpoint classification
/// into visible/hidden runs), factored out so a synthesized silhouette curve
/// goes through the identical rule instead of a re-implementation that could
/// drift from it.
///
/// The classification point for each segment is the raw CHORD midpoint —
/// correct for anything whose chord lies exactly ON the true curve (a
/// straight edge, or a cylinder/cone generator, which IS a straight line)
/// and safe for a planar rim curve (a circle/arc edge's chord midpoint stays
/// coplanar with the rim, which borders a void, not solid material, so it
/// cannot be spuriously self-occluded). It is NOT safe for a curve embedded
/// in the MIDDLE of a solid, like a sphere's synthesized great-circle
/// silhouette — see [`split_by_visibility_on_sphere`], which every sphere
/// silhouette call site uses instead.
fn split_by_visibility(
    p3: &[Point3],
    p2: &[[f64; 2]],
    ctx: &ViewOcclusionCtx,
) -> Vec<(bool, Vec<[f64; 2]>)> {
    split_by_visibility_with(p3, p2, ctx, |a, b| {
        Point3::new(0.5 * (a.x + b.x), 0.5 * (a.y + b.y), 0.5 * (a.z + b.z))
    })
}

/// As [`split_by_visibility`], but the occlusion PROBE point for each
/// segment is the chord midpoint projected radially back onto the sphere
/// (`center + (mid − center)·normalize() * radius`) rather than the raw
/// chord midpoint. A great-circle chord's raw midpoint sits at radius
/// `r·cos(Δθ/2)` from the centre — strictly INSIDE the ball, at the exact
/// SAME depth-along-`w` as the equator itself — where the solid's own near
/// hemisphere legitimately (but spuriously, for this synthesized silhouette)
/// occludes it: a lone, fully unoccluded sphere would classify its own
/// silhouette as entirely hidden. Radially re-projecting the probe point
/// back onto the true surface removes the artificial sagitta while leaving
/// the drawn geometry (`p2`, the actual sampled circle points) untouched.
fn split_by_visibility_on_sphere(
    p3: &[Point3],
    p2: &[[f64; 2]],
    center: Point3,
    radius: f64,
    ctx: &ViewOcclusionCtx,
) -> Vec<(bool, Vec<[f64; 2]>)> {
    split_by_visibility_with(p3, p2, ctx, |a, b| {
        let raw_mid = Point3::new(0.5 * (a.x + b.x), 0.5 * (a.y + b.y), 0.5 * (a.z + b.z));
        match (raw_mid - center).normalize() {
            Ok(dir) => center + dir * radius,
            Err(_) => raw_mid,
        }
    })
}

/// Shared core of [`split_by_visibility`] / [`split_by_visibility_on_sphere`]:
/// classify every consecutive segment as visible/hidden — using `midpoint_of`
/// to compute the 3D point occlusion is evaluated AT, while the 2D geometry
/// (`p2`) that actually gets drawn is always the true sampled points — and
/// group into maximal same-visibility runs.
fn split_by_visibility_with(
    p3: &[Point3],
    p2: &[[f64; 2]],
    ctx: &ViewOcclusionCtx,
    midpoint_of: impl Fn(Point3, Point3) -> Point3,
) -> Vec<(bool, Vec<[f64; 2]>)> {
    let mut runs: Vec<(bool, Vec<[f64; 2]>)> = Vec::new();
    for i in 0..p2.len().saturating_sub(1) {
        let mid = midpoint_of(p3[i], p3[i + 1]);
        let mu = 0.5 * (p2[i][0] + p2[i + 1][0]);
        let mv = 0.5 * (p2[i][1] + p2[i + 1][1]);
        let visible = ctx.visible(mid, mu, mv);
        match runs.last_mut() {
            Some((v, pts)) if *v == visible => pts.push(p2[i + 1]),
            _ => runs.push((visible, vec![p2[i], p2[i + 1]])),
        }
    }
    runs
}

/// Exact axis-projected v-span of a face's own trim boundary: the min/max of
/// `axis · (p − origin)` over densely sampled points on every boundary edge
/// (outer + inner loops) — the same per-edge sampling convention
/// [`face_bounds`] uses for its own boundary pass (2 points for a linear
/// edge, 24 for a curved one). For a cylinder/cone lateral face, `v` IS the
/// axial coordinate and the surface is monotonic in it (no interior bulge
/// along `axis`), so the boundary alone determines the true v-extent
/// exactly — deliberately NO safety padding, unlike `face_bounds` (which
/// pads for an unrelated purpose: a conservative screen-space AABB for
/// occlusion culling). Padding here would only cost precision: reusing
/// `face_bounds`'s padded box pushed the untrimmed common case in
/// [`emit_line_silhouette`] out of its fast (exact-endpoint) path on every
/// ordinary cylinder, insetting the emitted line from the real `0..height`
/// span by half a sample step for no reason.
fn face_axis_span(
    model: &BRepModel,
    face: &Face,
    origin: Point3,
    axis: Vector3,
) -> Option<(f64, f64)> {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let mut any = false;
    let mut loop_ids = vec![face.outer_loop];
    loop_ids.extend(face.inner_loops.iter().copied());
    for lid in loop_ids {
        let Some(lp) = model.loops.get(lid) else {
            continue;
        };
        for &eid in &lp.edges {
            let Some(edge) = model.edges.get(eid) else {
                continue;
            };
            let Some(curve) = model.curves.get(edge.curve_id) else {
                continue;
            };
            let (t0, t1) = (edge.param_range.start, edge.param_range.end);
            let is_linear = curve.is_linear(crate::math::Tolerance::default());
            let n = if is_linear { 2 } else { 24 };
            for i in 0..n {
                let frac = i as f64 / (n - 1).max(1) as f64;
                let t = t0 + (t1 - t0) * frac;
                if let Ok(p) = curve.point_at(t) {
                    let v = (p - origin).dot(&axis);
                    lo = lo.min(v);
                    hi = hi.max(v);
                    any = true;
                }
            }
        }
    }
    if any {
        Some((lo, hi))
    } else {
        None
    }
}

/// Synthesize the generator LINE silhouette of a ruled (cylinder/cone)
/// lateral face at fixed parameter `u`, spanning `[v_lo, v_hi]`, and push its
/// classified visible/hidden runs into `out`.
///
/// Two-tier sampling: the COMMON untrimmed case (both span endpoints land on
/// the real trimmed face) costs exactly what a real straight edge costs —
/// one segment, one occlusion probe — matching the existing linear-edge
/// convention in the main walk (`is_linear ⇒ n = 2`) rather than paying a
/// curve-fidelity sample budget for a shape that is, by construction,
/// perfectly straight. Only a face whose span endpoints are NOT both on the
/// trimmed surface (a partial angle sweep, a feature cut into the lateral
/// face) pays for the denser fallback scan that actually locates the
/// trimmed sub-run(s).
#[allow(clippy::too_many_arguments)]
fn emit_line_silhouette(
    model: &BRepModel,
    face_id: FaceId,
    face: &Face,
    surface: &dyn crate::primitives::surface::Surface,
    vm: &Matrix4,
    u: f64,
    v_lo: f64,
    v_hi: f64,
    samples_per_curve: usize,
    ctx: &ViewOcclusionCtx,
    out: &mut ViewEdges,
) {
    // Trim membership is probed slightly INSET from `v`, toward the span's
    // interior, rather than at `v` itself. The common case is a span
    // endpoint that sits EXACTLY on the face's own trim boundary (v_lo/v_hi
    // come from `face_axis_span`, sampled off that same boundary) — a point
    // exactly on a winding-number boundary is a genuine numerical
    // coin-flip, and losing it silently downgraded every ordinary untrimmed
    // cylinder to the denser fallback scan, which then re-lands on the same
    // ambiguous boundary sample and reports an inset that reads as almost
    // right rather than exactly right. The nudge only affects the trim
    // PROBE; the emitted geometry still uses the untouched `v_lo`/`v_hi`.
    let probe_eps = (v_hi - v_lo).abs().max(1e-9) * 1e-6;
    let mid = 0.5 * (v_lo + v_hi);
    let sample = |v: f64| -> Option<(Point3, [f64; 2], bool)> {
        let p = surface.point_at(u, v).ok()?;
        let probe_v = if v <= mid {
            v + probe_eps
        } else {
            v - probe_eps
        };
        let inside = crate::tessellation::surface::point_inside_face_uv(u, probe_v, face, model);
        let q = vm.transform_point(&p);
        Some((p, [q.x, q.y], inside))
    };

    let lo = sample(v_lo);
    let hi = sample(v_hi);
    let fast_path = matches!((&lo, &hi), (Some((_, _, true)), Some((_, _, true))));

    let mut point_runs: Vec<Vec<(Point3, [f64; 2])>> = Vec::new();
    if fast_path {
        if let (Some((p0, q0, _)), Some((p1, q1, _))) = (lo, hi) {
            point_runs.push(vec![(p0, q0), (p1, q1)]);
        }
    } else {
        let n = samples_per_curve.clamp(2, 24);
        let mut current: Vec<(Point3, [f64; 2])> = Vec::new();
        for i in 0..n {
            let frac = i as f64 / (n - 1) as f64;
            let v = v_lo + (v_hi - v_lo) * frac;
            match sample(v) {
                Some((p, q, true)) => current.push((p, q)),
                _ => {
                    if current.len() >= 2 {
                        point_runs.push(std::mem::take(&mut current));
                    } else {
                        current.clear();
                    }
                }
            }
        }
        if current.len() >= 2 {
            point_runs.push(current);
        }
    }

    for run in point_runs {
        let p3: Vec<Point3> = run.iter().map(|(p, _)| *p).collect();
        let p2: Vec<[f64; 2]> = run.iter().map(|(_, q)| *q).collect();
        for (visible, pts) in split_by_visibility(&p3, &p2, ctx) {
            let pl = Polyline2d::from_points(pts);
            if pl.points.len() < 2 {
                continue;
            }
            let source = super::types::PolylineSource {
                edge_id: None,
                face_ids: vec![face_id],
                role: super::types::PolylineRole::Silhouette,
            };
            if visible {
                out.visible.push(pl);
                out.visible_sources.push(source);
            } else {
                out.hidden.push(pl);
                out.hidden_sources.push(source);
            }
        }
    }
}

/// Synthesize a sphere's silhouette: the great circle in the plane through
/// its centre perpendicular to the view direction `w`. Always camera-facing
/// by construction, so — unlike a cylinder/cone's straight generators — this
/// is a CIRCLE, sampled around its circumference for trim + occlusion
/// classification and accumulated into `circle_groups` exactly as a real rim
/// edge would be, so it flushes through the SAME uniform-visibility /
/// mixed-rim-fallback logic downstream (an unoccluded standalone sphere
/// flushes as one exact analytic circle, never a polyline).
#[allow(clippy::too_many_arguments)]
fn synth_sphere_silhouette(
    model: &BRepModel,
    face_id: FaceId,
    face: &Face,
    sphere: &crate::primitives::surface::Sphere,
    vm: &Matrix4,
    w: Vector3,
    samples_per_curve: usize,
    ctx: &ViewOcclusionCtx,
    circle_groups: &mut std::collections::HashMap<(i64, i64, i64, i64), CircleGroup>,
) {
    let Ok(n) = w.normalize() else {
        return;
    };
    let helper = if n.x.abs() < 0.9 {
        Vector3::X
    } else {
        Vector3::Y
    };
    let Ok(e1) = n.cross(&helper).normalize() else {
        return;
    };
    let Ok(e2) = n.cross(&e1).normalize() else {
        return;
    };

    let center = sphere.center;
    let radius = sphere.radius;
    let tol = model.tolerance();
    // Endpoint-inclusive (i=0 -> theta=0, i=n-1 -> theta=TAU): explicitly
    // closes the loop with a sample coincident with the first, matching how
    // a real closed circle edge samples its own full [t0, t0+2π) range.
    let n_samples = samples_per_curve.clamp(3, 96);

    let mut run: Vec<(Point3, [f64; 2])> = Vec::new();
    let mut runs: Vec<Vec<(Point3, [f64; 2])>> = Vec::new();
    for i in 0..n_samples {
        let frac = i as f64 / (n_samples - 1) as f64;
        let theta = frac * std::f64::consts::TAU;
        let p = center + e1 * (radius * theta.cos()) + e2 * (radius * theta.sin());
        let inside = match crate::primitives::surface::Surface::closest_point(sphere, &p, tol) {
            Ok((u, v)) => crate::tessellation::surface::point_inside_face_uv(u, v, face, model),
            Err(_) => false,
        };
        if inside {
            let q = vm.transform_point(&p);
            run.push((p, [q.x, q.y]));
        } else if run.len() >= 2 {
            runs.push(std::mem::take(&mut run));
        } else {
            run.clear();
        }
    }
    if run.len() >= 2 {
        runs.push(run);
    }
    if runs.is_empty() {
        return;
    }

    let c2 = vm.transform_point(&center);
    let key = (
        (center.x * 1e3).round() as i64,
        (center.y * 1e3).round() as i64,
        (center.z * 1e3).round() as i64,
        (radius * 1e3).round() as i64,
    );
    let g = circle_groups.entry(key).or_insert(CircleGroup {
        cx: c2.x,
        cy: c2.y,
        r: radius,
        all_visible: true,
        all_hidden: true,
        fallback: Vec::new(),
        face_ids: Vec::new(),
    });
    if !g.face_ids.contains(&face_id) {
        g.face_ids.push(face_id);
    }
    for pts in runs {
        let p3: Vec<Point3> = pts.iter().map(|(p, _)| *p).collect();
        let p2: Vec<[f64; 2]> = pts.iter().map(|(_, q)| *q).collect();
        let classified = split_by_visibility_on_sphere(&p3, &p2, center, radius, ctx);
        let arc_vis = classified.iter().all(|(v, _)| *v);
        let arc_hid = classified.iter().all(|(v, _)| !*v);
        g.all_visible &= arc_vis;
        g.all_hidden &= arc_hid;
        for (vis, seg) in classified {
            let pl = Polyline2d::from_points(seg);
            if pl.points.len() >= 2 {
                g.fallback.push((vis, pl));
            }
        }
    }
}

/// Project a solid's edges, classifying every sub-segment visible / hidden.
///
/// Occlusion uses the projected-AABB [`OcclusionGrid`] broad phase unless the
/// `ROSHERA_DRAW_NOACCEL` environment variable forces the brute-force whole-solid
/// ray-cast — the escape hatch used to A/B profile the accelerator. Both paths
/// produce byte-identical output (see `OcclusionGrid` soundness note and the
/// `accel_matches_brute_force` test).
pub fn project_solid_edges_visibility(
    model: &BRepModel,
    solid_id: SolidId,
    projection: ProjectionType,
    samples_per_curve: usize,
) -> Result<ViewEdges, ProjectionError> {
    project_solid_edges_visibility_occ(model, solid_id, projection, samples_per_curve, true)
}

/// [`project_solid_edges_visibility`] with the occlusion pass made OPTIONAL
/// (`occlude`). The isometric pictorial merges its visible + hidden edges into a
/// single all-solid set (the drawing convention omits hidden lines in the iso
/// cell), so classifying its segments is discarded work — the dominant cost on a
/// coaxial part, where the oblique view defeats both the spatial and depth culls.
/// With `occlude = false` every segment is reported visible (no ray tests at
/// all); the caller that merges anyway gets byte-identical inked geometry. The
/// accelerator strategy still keys off `ROSHERA_DRAW_NOACCEL`.
pub(crate) fn project_solid_edges_visibility_occ(
    model: &BRepModel,
    solid_id: SolidId,
    projection: ProjectionType,
    samples_per_curve: usize,
    occlude: bool,
) -> Result<ViewEdges, ProjectionError> {
    let use_accel = !std::env::var_os("ROSHERA_DRAW_NOACCEL").is_some_and(|v| !v.is_empty());
    project_solid_edges_visibility_mode(
        model,
        solid_id,
        projection,
        samples_per_curve,
        use_accel,
        occlude,
    )
}

/// Core of [`project_solid_edges_visibility`] with the occlusion strategy chosen
/// explicitly (`use_accel`): the grid broad phase when `true`, the brute-force
/// whole-solid ray-cast when `false`. `occlude = false` skips occlusion entirely
/// (every segment visible). Exposed within the crate so a test can prove the two
/// strategies are equivalent without racy env manipulation.
pub(crate) fn project_solid_edges_visibility_mode(
    model: &BRepModel,
    solid_id: SolidId,
    projection: ProjectionType,
    samples_per_curve: usize,
    use_accel: bool,
    occlude: bool,
) -> Result<ViewEdges, ProjectionError> {
    let solid = model
        .solids
        .get(solid_id)
        .ok_or(ProjectionError::SolidNotFound(solid_id))?;
    let shell = model
        .shells
        .get(solid.outer_shell)
        .ok_or(ProjectionError::MissingShell(solid_id))?;

    let vm = view_matrix_for_projection(projection);
    let w = view_direction(projection);
    let diag = solid_diagonal(model, solid_id);
    let back = 2.0 * diag + 10.0;
    let eps = diag * 1e-5 + 1e-3;

    // Broad-phase occlusion accelerator (all occlusion rays share direction `w`).
    // Replaces the per-segment whole-solid `raycast_solid` with a projected-AABB
    // grid + depth cull; identical classification (see `OcclusionGrid`), far less
    // work. Only built when the caller actually wants occlusion.
    let accel = if occlude && use_accel {
        Some(OcclusionGrid::build(model, solid_id, &vm, w, back, eps))
    } else {
        None
    };
    let occ_ctx = ViewOcclusionCtx {
        model,
        solid_id,
        w,
        back,
        eps,
        occlude,
        accel: &accel,
    };

    let mut visited: HashSet<EdgeId> = HashSet::new();
    let mut out = ViewEdges {
        visible: Vec::new(),
        hidden: Vec::new(),
        circles: Vec::new(),
        hidden_circles: Vec::new(),
        ellipses: Vec::new(),
        hidden_ellipses: Vec::new(),
        visible_sources: Vec::new(),
        hidden_sources: Vec::new(),
    };
    // Co-circular arc-edges of each camera-facing rim, keyed by quantised
    // (centre, radius), regrouped after the edge walk into one drawn circle.
    let mut circle_groups: std::collections::HashMap<(i64, i64, i64, i64), CircleGroup> =
        std::collections::HashMap::new();
    // As `circle_groups`, for rims viewed obliquely (Fix 2) — regrouped into
    // one drawn ellipse per rim.
    let mut ellipse_groups: std::collections::HashMap<(i64, i64, i64, i64), EllipseGroup> =
        std::collections::HashMap::new();

    // All shells (outer + inner), so a bore's own walls are classified too.
    let mut shell_ids = vec![solid.outer_shell];
    shell_ids.extend_from_slice(&solid.inner_shells);
    shell_ids.extend_from_slice(&solid.peer_shells);
    let _ = shell; // outer shell fetched above only to validate existence.

    // Edge → adjacent-faces reverse map. The main walk below visits each edge
    // ONCE (from whichever face's loop reaches it first), but a rim edge is
    // shared by TWO faces (planar cap + lateral cylinder) and the circle's
    // entity identity must carry BOTH — the hole-table tag assigner matches
    // on the LATERAL face id, which may not be the walk-encounter face.
    let mut edge_faces: std::collections::HashMap<EdgeId, Vec<u32>> =
        std::collections::HashMap::new();
    // Every face of the solid, collected alongside `edge_faces` (same
    // traversal, no extra shell/face walk) — consumed below by the
    // silhouette-synthesis pass once `shell_ids` itself has been moved into
    // the main edge walk.
    let mut all_face_ids: Vec<FaceId> = Vec::new();
    for sh in &shell_ids {
        let Some(shell) = model.shells.get(*sh) else {
            continue;
        };
        for face_id in &shell.faces {
            all_face_ids.push(*face_id);
            let Some(face) = model.faces.get(*face_id) else {
                continue;
            };
            let loop_ids = std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied());
            for loop_id in loop_ids {
                let Some(topo_loop) = model.loops.get(loop_id) else {
                    continue;
                };
                for edge_id in &topo_loop.edges {
                    let faces = edge_faces.entry(*edge_id).or_default();
                    if !faces.contains(face_id) {
                        faces.push(*face_id);
                    }
                }
            }
        }
    }

    for sh in shell_ids {
        let shell = match model.shells.get(sh) {
            Some(s) => s,
            None => continue,
        };
        for face_id in &shell.faces {
            let face = match model.faces.get(*face_id) {
                Some(f) => f,
                None => continue,
            };
            let loop_ids = std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied());
            for loop_id in loop_ids {
                let topo_loop = match model.loops.get(loop_id) {
                    Some(l) => l,
                    None => continue,
                };
                for edge_id in &topo_loop.edges {
                    if !visited.insert(*edge_id) {
                        continue;
                    }
                    let edge = match model.edges.get(*edge_id) {
                        Some(e) => e,
                        None => continue,
                    };
                    let curve = match model.curves.get(edge.curve_id) {
                        Some(c) => c,
                        None => continue,
                    };
                    let is_linear = curve.is_linear(crate::math::Tolerance::default());
                    // A cylinder/cone seam is a parameterisation artifact, not a
                    // feature — suppressed unconditionally (see
                    // `is_parameterization_seam`'s doc comment for the ruling);
                    // the true silhouette is synthesized separately below,
                    // regardless of whether it coincides with this seam.
                    if is_linear && is_parameterization_seam(model, *edge_id, &edge_faces) {
                        continue;
                    }
                    let n = if is_linear {
                        2
                    } else {
                        samples_per_curve.max(2)
                    };
                    let t0 = edge.param_range.start;
                    let t1 = edge.param_range.end;

                    // Sample 3D + 2D in lockstep.
                    let mut p3: Vec<Point3> = Vec::with_capacity(n);
                    let mut p2: Vec<[f64; 2]> = Vec::with_capacity(n);
                    let mut ok = true;
                    for i in 0..n {
                        let frac = i as f64 / (n - 1) as f64;
                        let t = t0 + (t1 - t0) * frac;
                        match curve.point_at(t) {
                            Ok(p) => {
                                let v = vm.transform_point(&p);
                                p3.push(p);
                                p2.push([v.x, v.y]);
                            }
                            Err(_) => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if !ok || p2.len() < 2 {
                        continue;
                    }

                    // Classify each segment, grouping consecutive same-visibility
                    // runs into polylines — identical rule the synthesized
                    // silhouette curves below use, via the same helper.
                    let runs = split_by_visibility(&p3, &p2, &occ_ctx);

                    // Analytic circle: a circular arc-edge whose circle plane
                    // faces the camera (normal ∥ view dir) projects, under the
                    // orthonormal view matrix, to a TRUE circle of the same
                    // radius at the projected centre. A boolean fragments a rim
                    // into several co-circular arcs, so accumulate them by
                    // (centre, radius) and draw ONE circle once the rim is whole
                    // and uniformly visible/hidden. The per-arc polylines are
                    // kept as the fallback for a genuinely mixed rim.
                    if let Some((c3, nrm, r)) = circular_geom(curve) {
                        let faces_camera =
                            nrm.normalize().map(|u| u.dot(&w).abs()).unwrap_or(0.0) > 0.99;
                        if faces_camera && !runs.is_empty() {
                            let c2 = vm.transform_point(&c3);
                            // Key on the 3D centre so the two coincident rims of
                            // a through-hole (same projected circle, different
                            // depth + visibility) stay SEPARATE groups — the
                            // near rim draws solid, the far rim dashed.
                            let key = (
                                (c3.x * 1e3).round() as i64,
                                (c3.y * 1e3).round() as i64,
                                (c3.z * 1e3).round() as i64,
                                (r * 1e3).round() as i64,
                            );
                            let g = circle_groups.entry(key).or_insert(CircleGroup {
                                cx: c2.x,
                                cy: c2.y,
                                r,
                                all_visible: true,
                                all_hidden: true,
                                fallback: Vec::new(),
                                face_ids: Vec::new(),
                            });
                            // Entity identity: ALL faces adjacent to this arc
                            // edge (not just the walk-encounter face) join the
                            // rim's face-id set.
                            if let Some(adj) = edge_faces.get(edge_id) {
                                for f in adj {
                                    if !g.face_ids.contains(f) {
                                        g.face_ids.push(*f);
                                    }
                                }
                            }
                            let arc_vis = runs.iter().all(|(v, _)| *v);
                            let arc_hid = runs.iter().all(|(v, _)| !*v);
                            g.all_visible &= arc_vis;
                            g.all_hidden &= arc_hid;
                            for (vis, pts) in &runs {
                                let pl = Polyline2d::from_points(pts.clone());
                                if pl.points.len() >= 2 {
                                    g.fallback.push((*vis, pl));
                                }
                            }
                            continue;
                        }

                        // Fix 2 — the general case: the rim's plane does not
                        // face the camera (a hole viewed in the isometric,
                        // e.g.), so it projects to a true ELLIPSE rather than
                        // a circle. Computed exactly from (centre, normal,
                        // radius) — never sampled — unless the projection is
                        // itself near-degenerate (edge-on), where an ellipse
                        // would collapse below what an SVG/DXF consumer can
                        // draw; that case keeps the polyline fallback below.
                        if !runs.is_empty() {
                            if let Some(ell) = circle_to_view_ellipse(&vm, c3, nrm, r) {
                                if ell.ry >= ell.rx * MIN_ELLIPSE_MINOR_FRACTION {
                                    let key = (
                                        (c3.x * 1e3).round() as i64,
                                        (c3.y * 1e3).round() as i64,
                                        (c3.z * 1e3).round() as i64,
                                        (r * 1e3).round() as i64,
                                    );
                                    let g = ellipse_groups.entry(key).or_insert(EllipseGroup {
                                        cx: ell.cx,
                                        cy: ell.cy,
                                        rx: ell.rx,
                                        ry: ell.ry,
                                        rotation: ell.rotation,
                                        all_visible: true,
                                        all_hidden: true,
                                        fallback: Vec::new(),
                                        face_ids: Vec::new(),
                                    });
                                    if let Some(adj) = edge_faces.get(edge_id) {
                                        for f in adj {
                                            if !g.face_ids.contains(f) {
                                                g.face_ids.push(*f);
                                            }
                                        }
                                    }
                                    let arc_vis = runs.iter().all(|(v, _)| *v);
                                    let arc_hid = runs.iter().all(|(v, _)| !*v);
                                    g.all_visible &= arc_vis;
                                    g.all_hidden &= arc_hid;
                                    for (vis, pts) in &runs {
                                        let pl = Polyline2d::from_points(pts.clone());
                                        if pl.points.len() >= 2 {
                                            g.fallback.push((*vis, pl));
                                        }
                                    }
                                    continue;
                                }
                            }
                        }
                    }

                    // The edge's B-Rep lineage — this whole run set came from the
                    // single edge `*edge_id`; occlusion clipping splits it into
                    // runs but every run still belongs to that one edge, so the
                    // 1:1 edge link survives. The adjacent faces are the reverse
                    // map already built for circle identity.
                    let src_faces = edge_faces.get(edge_id).cloned().unwrap_or_default();
                    for (visible, pts) in runs {
                        let pl = Polyline2d::from_points(pts);
                        if pl.points.len() < 2 {
                            continue;
                        }
                        let source = super::types::PolylineSource {
                            edge_id: Some(*edge_id),
                            face_ids: src_faces.clone(),
                            role: super::types::PolylineRole::Edge,
                        };
                        if visible {
                            out.visible.push(pl);
                            out.visible_sources.push(source);
                        } else {
                            out.hidden.push(pl);
                            out.hidden_sources.push(source);
                        }
                    }
                }
            }
        }
    }

    // Synthesized curved-surface silhouettes — see the module-level comment
    // above `is_parameterization_seam` for the design. Runs over every face
    // of the solid (collected into `all_face_ids` above, before `shell_ids`
    // was moved into the edge walk); a face whose surface is not Cylinder,
    // Cone or Sphere is skipped immediately.
    for face_id in &all_face_ids {
        let Some(face) = model.faces.get(*face_id) else {
            continue;
        };
        let Some(surface) = model.surfaces.get(face.surface_id) else {
            continue;
        };

        if let Some(sph) = surface
            .as_any()
            .downcast_ref::<crate::primitives::surface::Sphere>()
        {
            synth_sphere_silhouette(
                model,
                *face_id,
                face,
                sph,
                &vm,
                w,
                samples_per_curve,
                &occ_ctx,
                &mut circle_groups,
            );
            continue;
        }

        // Cylinder / Cone share the same ruled-surface derivation: the
        // silhouette condition `n(u)·w = 0` reduces to `A cos u + B sin u =
        // C` with `A = w·x_dir`, `B = w·y_dir`, and `C = tan(half_angle) *
        // (w·axis)` (zero for a cylinder, which is the `half_angle = 0`
        // special case) — solved in closed form below.
        let (origin, axis, ref_dir, half_angle) = if let Some(cyl) =
            surface
                .as_any()
                .downcast_ref::<crate::primitives::surface::Cylinder>()
        {
            (cyl.origin, cyl.axis, cyl.ref_dir, None)
        } else if let Some(cone) = surface
            .as_any()
            .downcast_ref::<crate::primitives::surface::Cone>()
        {
            (cone.apex, cone.axis, cone.ref_dir, Some(cone.half_angle))
        } else {
            continue;
        };

        let x_dir = ref_dir;
        let y_dir = axis.cross(&x_dir);
        let wx = w.dot(&x_dir);
        let wy = w.dot(&y_dir);
        let wz = w.dot(&axis);
        let r = (wx * wx + wy * wy).sqrt();
        const DEGENERATE: f64 = 1e-9;
        if r < DEGENERATE {
            // View direction is (near-)parallel to the axis: looking straight
            // down a cylinder/cone shows only its end rim(s), no generator
            // line turns edge-on.
            continue;
        }
        let c = half_angle.map(|ha| ha.tan() * wz).unwrap_or(0.0);
        if c.abs() > r + DEGENERATE {
            // No real solution: this cone's flare never turns edge-on from
            // this view direction.
            continue;
        }
        let phi = wy.atan2(wx);
        let delta = (c / r).clamp(-1.0, 1.0).acos();
        let mut candidates = vec![phi + delta, phi - delta];
        if (candidates[0] - candidates[1]).rem_euclid(std::f64::consts::TAU) < 1e-9 {
            candidates.truncate(1);
        }

        let Some((v_lo, v_hi)) = face_axis_span(model, face, origin, axis) else {
            continue;
        };
        if !(v_hi > v_lo) {
            continue;
        }

        for raw_u in candidates {
            let u = raw_u.rem_euclid(std::f64::consts::TAU);
            emit_line_silhouette(
                model,
                *face_id,
                face,
                surface,
                &vm,
                u,
                v_lo,
                v_hi,
                samples_per_curve,
                &occ_ctx,
                &mut out,
            );
        }
    }

    // Flush the accumulated rims: a whole, uniformly-visible (or -hidden) rim
    // draws as ONE true circle; a genuinely mixed rim falls back to its arcs.
    for g in circle_groups.into_values() {
        let circ = super::types::ProjectedCircle {
            cx: g.cx,
            cy: g.cy,
            r: g.r,
            face_ids: g.face_ids.clone(),
        };
        if g.all_visible {
            out.circles.push(circ);
        } else if g.all_hidden {
            out.hidden_circles.push(circ);
        } else {
            // A mixed rim falls back to per-arc polylines: the single-edge link
            // is dissolved (the rim regrouped several arc-edges), so `edge_id` is
            // None — but the rim's adjacent faces are known, so readback can still
            // name the feature. Pushed in lockstep with `visible`/`hidden`.
            for (vis, pl) in g.fallback {
                let source = super::types::PolylineSource {
                    edge_id: None,
                    face_ids: g.face_ids.clone(),
                    role: super::types::PolylineRole::Edge,
                };
                if vis {
                    out.visible.push(pl);
                    out.visible_sources.push(source);
                } else {
                    out.hidden.push(pl);
                    out.hidden_sources.push(source);
                }
            }
        }
    }

    // As above, for oblique rims accumulated as ellipses (Fix 2).
    for g in ellipse_groups.into_values() {
        let ell = super::types::ProjectedEllipse {
            cx: g.cx,
            cy: g.cy,
            rx: g.rx,
            ry: g.ry,
            rotation: g.rotation,
            face_ids: g.face_ids.clone(),
        };
        if g.all_visible {
            out.ellipses.push(ell);
        } else if g.all_hidden {
            out.hidden_ellipses.push(ell);
        } else {
            for (vis, pl) in g.fallback {
                let source = super::types::PolylineSource {
                    edge_id: None,
                    face_ids: g.face_ids.clone(),
                    role: super::types::PolylineRole::Edge,
                };
                if vis {
                    out.visible.push(pl);
                    out.visible_sources.push(source);
                } else {
                    out.hidden.push(pl);
                    out.hidden_sources.push(source);
                }
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawing::projection::DEFAULT_CURVE_SAMPLES;
    use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    // ===== Fix 2 (exact-curves brief): circle_to_view_ellipse =====

    #[test]
    fn circle_to_view_ellipse_facing_camera_is_a_circle() {
        // Front view looks down −Y; a circle in the XZ plane (normal = Y)
        // faces the camera exactly, so its projection must be rx == ry == r,
        // matching the existing true-circle path's own answer.
        let vm = view_matrix_for_projection(ProjectionType::Front);
        let ell = circle_to_view_ellipse(&vm, Point3::new(1.0, 0.0, 2.0), Vector3::Y, 3.0)
            .expect("camera-facing circle must project");
        assert!((ell.rx - 3.0).abs() < 1e-12, "rx={}", ell.rx);
        assert!((ell.ry - 3.0).abs() < 1e-12, "ry={}", ell.ry);
    }

    #[test]
    fn circle_to_view_ellipse_tilted_matches_foreshortening() {
        // A circle in the XY plane (normal = Z), viewed by a camera tilted by
        // angle α off dead-on, foreshortens to semi-minor r·cos(α) while the
        // semi-major axis (perpendicular to the tilt) stays exactly r — the
        // standard optics result, independent of this module's own math.
        // Built directly (not via ProjectionType) so the test is an
        // independent check of the eigen-decomposition, not a re-assertion
        // of `view_matrix_for_projection`.
        let alpha = std::f64::consts::FRAC_PI_3; // 60°
        let w = Vector3::new(alpha.sin(), 0.0, alpha.cos()); // view direction
        let u = Vector3::new(alpha.cos(), 0.0, -alpha.sin()); // page X, ⟂ w
        let v = Vector3::new(0.0, 1.0, 0.0); // page Y, ⟂ w
        let vm = Matrix4::new(
            u.x, u.y, u.z, 0.0, v.x, v.y, v.z, 0.0, w.x, w.y, w.z, 0.0, 0.0, 0.0, 0.0, 1.0,
        );
        let r = 2.5;
        let ell = circle_to_view_ellipse(&vm, Point3::ORIGIN, Vector3::Z, r)
            .expect("tilted circle must project");
        assert!(
            (ell.rx - r).abs() < 1e-9,
            "major axis unchanged: rx={}",
            ell.rx
        );
        let expected_minor = r * alpha.cos();
        assert!(
            (ell.ry - expected_minor).abs() < 1e-9,
            "ry={} expected {}",
            ell.ry,
            expected_minor
        );
        // Major axis (the unforeshortened one) lies along page-Y (the +90°
        // frame relative to +X, since the tilt is about the page-Y axis).
        let rot_norm = ell.rotation.rem_euclid(std::f64::consts::PI);
        assert!(
            (rot_norm - std::f64::consts::FRAC_PI_2).abs() < 1e-9,
            "rotation={} expected pi/2",
            rot_norm
        );
    }

    #[test]
    fn diag_ellipse_matches_direct_sample_of_real_cylinder_rim_isometric() {
        // A real B-Rep rim (not a hand-built Point3/Vector3/f64 triple):
        // cylinder along Z, radius 10, base at z=0, height 5. Its TOP rim
        // circle has centre (0,0,5), normal Z, radius 10 — obliquely viewed
        // in the isometric (normal is not parallel to the -1,-1,-1 view
        // direction), so it is exactly the Fix 2 case.
        let mut m = BRepModel::new();
        let solid = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 10.0, 5.0)
            .expect("cylinder"));

        let edges = project_solid_edges_visibility(&m, solid, ProjectionType::Isometric, 96)
            .expect("project");
        assert!(
            !edges.ellipses.is_empty() || !edges.hidden_ellipses.is_empty(),
            "the cylinder's rim must be classified as an oblique ellipse in the isometric view"
        );
        let ell = edges
            .ellipses
            .iter()
            .chain(edges.hidden_ellipses.iter())
            .find(|e| (e.rx - 10.0).abs() < 1e-6)
            .expect("an ellipse with the cylinder's true radius (10) must exist");

        // Independently sample the TOP rim circle in 3D and project each
        // point through the SAME view matrix the rest of the view uses, then
        // check every sample lies ON the reported ellipse's boundary.
        let vm = view_matrix_for_projection(ProjectionType::Isometric);
        let mut max_err: f64 = 0.0;
        for i in 0..64 {
            let theta = (i as f64 / 64.0) * std::f64::consts::TAU;
            let p3 = Point3::new(10.0 * theta.cos(), 10.0 * theta.sin(), 5.0);
            let p2 = vm.transform_point(&p3);
            let dx = p2.x - ell.cx;
            let dy = p2.y - ell.cy;
            let (sin_r, cos_r) = ell.rotation.sin_cos();
            let lx = dx * cos_r + dy * sin_r;
            let ly = -dx * sin_r + dy * cos_r;
            let normalized = (lx / ell.rx).powi(2) + (ly / ell.ry).powi(2);
            max_err = max_err.max((normalized - 1.0).abs());
        }
        assert!(
            max_err < 1e-6,
            "every directly-projected rim sample must lie ON the reported ellipse boundary \
             (normalized radius within 1e-6 of 1.0); max deviation {max_err:e} — ellipse \
             cx={} cy={} rx={} ry={} rotation={}",
            ell.cx,
            ell.cy,
            ell.rx,
            ell.ry,
            ell.rotation
        );

        // AND: the ellipse's own centre must equal the rim centre projected
        // through that same matrix — catches a registration bug (right shape,
        // wrong place) that the boundary check alone could miss if the whole
        // ellipse were merely translated.
        let true_center_2d = vm.transform_point(&Point3::new(0.0, 0.0, 5.0));
        assert!(
            (ell.cx - true_center_2d.x).abs() < 1e-6 && (ell.cy - true_center_2d.y).abs() < 1e-6,
            "ellipse centre ({}, {}) must equal the rim centre projected through the view's \
             own matrix ({}, {})",
            ell.cx,
            ell.cy,
            true_center_2d.x,
            true_center_2d.y
        );
    }

    #[test]
    fn circle_to_view_ellipse_edge_on_collapses_minor_axis_to_near_zero() {
        // Circle plane CONTAINS the view direction (normal ⟂ view dir): the
        // rim is edge-on and projects to a line segment, i.e. an ellipse
        // whose minor axis has collapsed — this is what the caller's
        // MIN_ELLIPSE_MINOR_FRACTION gate exists to catch and fall back to
        // sampling on.
        let vm = view_matrix_for_projection(ProjectionType::Front); // looks down -Y
                                                                    // Normal = Z is perpendicular to the Front view direction (Y).
        let ell = circle_to_view_ellipse(&vm, Point3::ORIGIN, Vector3::Z, 1.0)
            .expect("edge-on circle still projects to SOME conic");
        assert!(
            ell.ry < ell.rx * 1e-9,
            "expected near-zero minor axis, got {}",
            ell.ry
        );
    }

    // ===== 2026-08-17 silhouette brief: synthesized curved-surface silhouettes =====

    /// A lone sphere's silhouette is its own great circle — a locus lying
    /// entirely ON the surface, so nothing on the SAME solid can occlude it.
    /// It must flush as a fully-VISIBLE analytic circle, not a hidden one.
    ///
    /// This is the mutation-catcher for a subtle sagitta bug: classifying
    /// occlusion at the CHORD MIDPOINT between two adjacent samples on the
    /// circle (rather than at a point on the true circle) places the probe
    /// strictly INSIDE the ball — a great-circle chord's midpoint sits at
    /// radius `r·cos(Δθ/2)` from the centre, at the SAME depth-along-`w` as
    /// the equator itself, which is squarely inside the solid, where the
    /// near hemisphere legitimately (but spuriously, for this purpose)
    /// occludes it. A real circular RIM edge does not have this problem —
    /// its chord midpoint stays coplanar with the rim, which borders a void
    /// (nothing behind it to occlude), not a solid ball — so the bug is
    /// specific to a curved silhouette synthesized in the middle of a solid.
    #[test]
    fn sphere_silhouette_of_a_lone_sphere_is_fully_visible() {
        let mut m = BRepModel::new();
        let sphere = sid(TopologyBuilder::new(&mut m)
            .create_sphere_3d(Point3::ORIGIN, 10.0)
            .expect("sphere"));
        let edges =
            project_solid_edges_visibility(&m, sphere, ProjectionType::Front, 96).expect("project");
        assert!(
            edges.hidden_circles.is_empty(),
            "a lone sphere's silhouette must not flush as hidden; hidden_circles={:?}",
            edges.hidden_circles
        );
        let circ = edges
            .circles
            .iter()
            .find(|c| (c.r - 10.0).abs() < 1e-6)
            .unwrap_or_else(|| {
                panic!(
                    "expected one fully-visible analytic circle with r=10; circles={:?} hidden={:?}",
                    edges.circles, edges.hidden_circles
                )
            });
        assert!((circ.r - 10.0).abs() < 1e-9, "r={}", circ.r);
    }

    #[test]
    fn box_far_face_hidden_near_face_visible() {
        // Box 20³ centred at origin. Front view (camera +Y). The +Y face centre
        // (0,10,0) is the near face → visible; the −Y face centre (0,−10,0) sits
        // behind it → hidden.
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("box"));
        assert!(
            !is_point_hidden(&m, b, ProjectionType::Front, Point3::new(0.0, 10.0, 0.0)),
            "near (+Y) face is visible"
        );
        assert!(
            is_point_hidden(&m, b, ProjectionType::Front, Point3::new(0.0, -10.0, 0.0)),
            "far (−Y) face is hidden"
        );
    }

    #[test]
    fn box_front_view_has_visible_and_hidden_runs() {
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("box"));
        let e = project_solid_edges_visibility(&m, b, ProjectionType::Front, DEFAULT_CURVE_SAMPLES)
            .expect("vis");
        // The front face's 4 edges are visible; the back face's 4 edges are
        // hidden (they project onto the same square but classify hidden).
        assert!(!e.visible.is_empty(), "some visible edges");
        assert!(!e.hidden.is_empty(), "some hidden edges (the back face)");
    }

    /// Campaign #55 residual: the HLR projector threads per-polyline B-Rep
    /// lineage parallel to `visible`/`hidden`. The invariant is a strict 1:1
    /// pairing (so `entity_at` can index it safely), and a box's clean outline
    /// edges each name their producing edge + at least one adjacent face.
    #[test]
    fn projected_polylines_carry_edge_and_face_lineage() {
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(30.0, 20.0, 10.0)
            .expect("box"));
        let e = project_solid_edges_visibility(&m, b, ProjectionType::Front, DEFAULT_CURVE_SAMPLES)
            .expect("vis");
        assert_eq!(
            e.visible.len(),
            e.visible_sources.len(),
            "sources are parallel (1:1) to visible polylines"
        );
        assert_eq!(
            e.hidden.len(),
            e.hidden_sources.len(),
            "sources are parallel (1:1) to hidden polylines"
        );
        assert!(!e.visible_sources.is_empty(), "some visible sources");
        for s in &e.visible_sources {
            assert!(
                s.edge_id.is_some(),
                "a clean box outline segment names its producing edge: {s:?}"
            );
            assert!(
                !s.face_ids.is_empty(),
                "a box outline segment names at least one adjacent face: {s:?}"
            );
        }
    }

    #[test]
    fn bored_plate_far_bore_wall_is_hidden_in_front() {
        // Plate 50×50×16 with a Ø20 through-bore on Z. In Front view the bore is
        // a vertical slot; its FAR wall (the +Y side of the cylinder, behind the
        // plate front) is hidden. Probe a point on the far bore wall.
        let mut m = BRepModel::new();
        let plate = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(50.0, 50.0, 16.0)
            .expect("plate"));
        let bore = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, -20.0), Vector3::Z, 10.0, 80.0)
            .expect("bore"));
        let part = boolean_operation(
            &mut m,
            plate,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("bore");
        // Far bore wall point: on the cylinder at +Y (y=+10), mid-thickness.
        assert!(
            is_point_hidden(&m, part, ProjectionType::Front, Point3::new(0.0, 10.0, 0.0)),
            "far bore wall is hidden behind the plate front"
        );
        // The near plate front face is visible.
        assert!(
            !is_point_hidden(
                &m,
                part,
                ProjectionType::Front,
                Point3::new(20.0, 25.0, 0.0)
            ),
            "plate front face is visible"
        );
    }

    #[test]
    fn visibility_split_is_deterministic() {
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(30.0, 20.0, 16.0)
            .expect("box"));
        let a = project_solid_edges_visibility(&m, b, ProjectionType::Isometric, 12).expect("a");
        let c = project_solid_edges_visibility(&m, b, ProjectionType::Isometric, 12).expect("c");
        assert_eq!(a.visible.len(), c.visible.len(), "visible count stable");
        assert_eq!(a.hidden.len(), c.hidden.len(), "hidden count stable");
    }

    /// Compare the accelerated (grid + depth cull) classification against the
    /// brute-force whole-solid ray-cast for one solid across all standard views:
    /// no visible/hidden edge or circle may flip.
    fn assert_accel_matches_brute(m: &BRepModel, part: SolidId, label: &str) {
        for proj in [
            ProjectionType::Front,
            ProjectionType::Top,
            ProjectionType::Right,
            ProjectionType::Isometric,
        ] {
            let accel = project_solid_edges_visibility_mode(
                m,
                part,
                proj,
                DEFAULT_CURVE_SAMPLES,
                true,
                true,
            )
            .expect("accel");
            let brute = project_solid_edges_visibility_mode(
                m,
                part,
                proj,
                DEFAULT_CURVE_SAMPLES,
                false,
                true,
            )
            .expect("brute");
            // Compare as an order-INSENSITIVE multiset of polylines: the circle-
            // group flush iterates a HashMap, so the emission ORDER of mixed-rim
            // fallback polylines is already non-deterministic in the original code
            // (independent of the accelerator). What must match is the SET of
            // classified polylines, not their order.
            let canon = |pls: &[Polyline2d]| -> Vec<Vec<[f64; 2]>> {
                let mut v: Vec<Vec<[f64; 2]>> = pls.iter().map(|p| p.points.clone()).collect();
                v.sort_by(|a, b| {
                    let ka = (a.first().map(|p| p[0]), a.first().map(|p| p[1]), a.len());
                    let kb = (b.first().map(|p| p[0]), b.first().map(|p| p[1]), b.len());
                    ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)
                });
                v
            };
            assert_eq!(
                canon(&accel.visible),
                canon(&brute.visible),
                "{label}: visible polylines identical ({proj:?})"
            );
            assert_eq!(
                canon(&accel.hidden),
                canon(&brute.hidden),
                "{label}: hidden polylines identical ({proj:?})"
            );
            assert_eq!(
                accel.circles.len(),
                brute.circles.len(),
                "{label}: visible circle count identical ({proj:?})"
            );
            assert_eq!(
                accel.hidden_circles.len(),
                brute.hidden_circles.len(),
                "{label}: hidden circle count identical ({proj:?})"
            );
        }
    }

    /// The projected-AABB occlusion grid + depth cull must classify EVERY
    /// sub-segment exactly as the brute-force whole-solid ray-cast — no
    /// visible/hidden edge (or circle) may flip, or the accelerated drawing would
    /// silently diverge from the sound reference. Checked on a bored plate
    /// (curved bore walls + rim circles + real occlusion).
    #[test]
    fn accel_matches_brute_force() {
        let mut m = BRepModel::new();
        let plate = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(50.0, 40.0, 16.0)
            .expect("plate"));
        let bore = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(8.0, 5.0, -20.0), Vector3::Z, 9.0, 80.0)
            .expect("bore"));
        let part = boolean_operation(
            &mut m,
            plate,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("bore");
        assert_accel_matches_brute(&m, part, "bored plate");
    }

    /// The DEPTH cull is what keeps coaxial parts (every face projecting onto the
    /// same region, defeating the 2D grid) fast. It must stay conservative on
    /// stacked CURVED bands viewed end-on and obliquely — the exact case the
    /// concentric-band depth ordering exercises. A stepped shaft (a small stack
    /// of frustums) must classify identically with and without the accelerator.
    #[test]
    fn accel_matches_brute_force_coaxial_curved() {
        use crate::operations::revolve::{revolve_meridian, RevolveOptions};
        let mut m = BRepModel::new();
        let profile = [
            (0.0, 0.0),
            (12.0, 0.0),
            (12.0, 8.0),
            (18.0, 8.0),
            (18.0, 16.0),
            (7.0, 16.0),
            (7.0, 24.0),
            (0.0, 24.0),
        ];
        let shaft = revolve_meridian(&mut m, &profile, RevolveOptions::default()).expect("shaft");
        assert_accel_matches_brute(&m, shaft, "stepped shaft");
    }

    /// Build the same 300-band "bumpy vase" `tests/drawing_perf.rs` uses to
    /// reproduce the gear-class HLR stress (hundreds of coaxial curved bands,
    /// hundreds of circular rim edges).
    fn bumpy_vase_300(m: &mut BRepModel) -> SolidId {
        use crate::operations::revolve::{revolve_meridian, RevolveOptions};
        let bands = 300usize;
        let height = 100.0_f64;
        let mut profile: Vec<(f64, f64)> = Vec::with_capacity(bands + 2);
        profile.push((0.0, 0.0));
        for k in 0..bands {
            let z = height * (k as f64 + 1.0) / bands as f64;
            let r = 20.0 + 6.0 * (k as f64 * 0.7).sin();
            profile.push((r, z));
        }
        profile.push((0.0, height));
        revolve_meridian(m, &profile, RevolveOptions::default()).expect("bumpy vase")
    }

    /// Mutation-proven guard for the exact defect the `exact_uv` trim-membership
    /// fix corrected: HEAD's `Surface::closest_point`-based occlusion test
    /// clamped a ray hit past a coaxial band's finite rim back onto the rim
    /// boundary, so floating-point noise at that boundary made almost every OTHER
    /// band spuriously accept the hit as its own occluder — the front view of
    /// this fixture classified as `visible=0, hidden=599` (every single edge
    /// hidden, a degenerate, useless drawing). This test fails on that code and
    /// passes on the fix: a front view of an axisymmetric solid MUST show both a
    /// non-empty visible set (the camera-facing hemisphere) and a non-empty
    /// hidden set (the far hemisphere occluded by the near one).
    #[test]
    fn front_view_of_coaxial_solid_is_not_degenerately_hidden() {
        let mut m = BRepModel::new();
        let part = bumpy_vase_300(&mut m);
        let res =
            project_solid_edges_visibility(&m, part, ProjectionType::Front, 8).expect("front view");
        assert!(
            !res.visible.is_empty(),
            "front view of a coaxial solid must have SOME visible edges — \
             an all-hidden classification is the false-positive-occluder defect \
             `Surface::exact_uv` fixed, not a legitimate result"
        );
        assert!(
            !res.hidden.is_empty(),
            "front view of a coaxial solid must have SOME hidden edges (the far \
             hemisphere occluded by the near one) — an all-visible classification \
             means occlusion isn't running at all"
        );
    }

    /// The broad-phase spatial + depth cull must keep narrowing candidates the
    /// way its soundness note promises: most of the model's faces must be culled
    /// before the expensive trimmed ray test runs. This is what a wall-clock
    /// budget alone cannot catch on a fast or quiet machine — if a future change
    /// silently defeats a cull (e.g. a broken depth bound, or a cell resolution
    /// that stops scaling with face count), the *work done* jumps by an order of
    /// magnitude while still finishing inside a generous time budget. The exact
    /// count is correctness-determined by the trim-membership fix (visible points
    /// on a coaxial part cannot early-out — see the test above) — this is a
    /// ceiling on candidates-tested-per-probe, not a bound on total probes.
    #[test]
    fn occlusion_ray_test_count_stays_bounded() {
        let mut m = BRepModel::new();
        let part = bumpy_vase_300(&mut m);
        let solid = m.solids.get(part).expect("solid");
        let shell = m.shells.get(solid.outer_shell).expect("shell");
        let face_count = shell.faces.len();

        reset_ray_face_test_counter();
        let res =
            project_solid_edges_visibility(&m, part, ProjectionType::Front, 8).expect("front view");
        let ray_tests = ray_face_test_counter();
        let probes = res.visible.len() + res.hidden.len();

        // A brute-force (unaccelerated) test would run one ray test per
        // candidate against EVERY live face, per probe — `probes * face_count`.
        // Measured on this fixture: ~81.5k tests / 601 probes / 301 faces ≈ 45%
        // of that bound (visible-point probes on a coaxial part are inherently
        // expensive — see the test above — so this is nowhere near the ~7%
        // "1 face/cell" design target). 70% leaves headroom for machine/fixture
        // jitter while still catching a cull that has silently degraded toward
        // the full O(probes · faces) brute-force scan (100%).
        let brute_force_upper_bound = (probes as u64) * (face_count as u64);
        assert!(
            (ray_tests as u128) * 100 < (brute_force_upper_bound as u128) * 70,
            "occlusion broad-phase did {ray_tests} ray-face tests over {probes} probes \
             / {face_count} faces — expected well under 70% of the brute-force upper \
             bound ({brute_force_upper_bound}); the spatial/depth cull may have \
             silently degraded toward brute force"
        );
    }
}
