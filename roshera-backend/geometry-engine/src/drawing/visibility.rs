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
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::queries::raycast::ray_hit_face_t;
use crate::queries::raycast_solid;

use super::projection::{view_matrix_for_projection, ProjectionError};
use super::types::{Polyline2d, ProjectionType};

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
            if let Some(t) = ray_hit_face_t(model, fid, origin, self.w) {
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
/// circles, not faceted polylines).
#[derive(Debug, Clone)]
pub struct ViewEdges {
    pub visible: Vec<Polyline2d>,
    pub hidden: Vec<Polyline2d>,
    pub circles: Vec<super::types::ProjectedCircle>,
    pub hidden_circles: Vec<super::types::ProjectedCircle>,
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

    let mut visited: HashSet<EdgeId> = HashSet::new();
    let mut out = ViewEdges {
        visible: Vec::new(),
        hidden: Vec::new(),
        circles: Vec::new(),
        hidden_circles: Vec::new(),
    };
    // Co-circular arc-edges of each camera-facing rim, keyed by quantised
    // (centre, radius), regrouped after the edge walk into one drawn circle.
    let mut circle_groups: std::collections::HashMap<(i64, i64, i64, i64), CircleGroup> =
        std::collections::HashMap::new();

    // All shells (outer + inner), so a bore's own walls are classified too.
    let mut shell_ids = vec![solid.outer_shell];
    shell_ids.extend_from_slice(&solid.inner_shells);
    let _ = shell; // outer shell fetched above only to validate existence.

    // Edge → adjacent-faces reverse map. The main walk below visits each edge
    // ONCE (from whichever face's loop reaches it first), but a rim edge is
    // shared by TWO faces (planar cap + lateral cylinder) and the circle's
    // entity identity must carry BOTH — the hole-table tag assigner matches
    // on the LATERAL face id, which may not be the walk-encounter face.
    let mut edge_faces: std::collections::HashMap<EdgeId, Vec<u32>> =
        std::collections::HashMap::new();
    for sh in &shell_ids {
        let Some(shell) = model.shells.get(*sh) else {
            continue;
        };
        for face_id in &shell.faces {
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
                    // runs into polylines.
                    let mut runs: Vec<(bool, Vec<[f64; 2]>)> = Vec::new();
                    for i in 0..p2.len() - 1 {
                        let mid = Point3::new(
                            0.5 * (p3[i].x + p3[i + 1].x),
                            0.5 * (p3[i].y + p3[i + 1].y),
                            0.5 * (p3[i].z + p3[i + 1].z),
                        );
                        // View projection is linear (orthographic), so the 2D
                        // midpoint is the projection of the 3D midpoint — used to
                        // index the occlusion grid cell.
                        let mu = 0.5 * (p2[i][0] + p2[i + 1][0]);
                        let mv = 0.5 * (p2[i][1] + p2[i + 1][1]);
                        let visible = if !occlude {
                            true
                        } else if let Some(accel) = &accel {
                            !accel.occluded(model, mid, mu, mv)
                        } else {
                            !occluded(model, solid_id, mid, w, back, eps)
                        };
                        match runs.last_mut() {
                            Some((v, pts)) if *v == visible => pts.push(p2[i + 1]),
                            _ => runs.push((visible, vec![p2[i], p2[i + 1]])),
                        }
                    }

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
                    }

                    for (visible, pts) in runs {
                        let pl = Polyline2d::from_points(pts);
                        if pl.points.len() < 2 {
                            continue;
                        }
                        if visible {
                            out.visible.push(pl);
                        } else {
                            out.hidden.push(pl);
                        }
                    }
                }
            }
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
            for (vis, pl) in g.fallback {
                if vis {
                    out.visible.push(pl);
                } else {
                    out.hidden.push(pl);
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
}
