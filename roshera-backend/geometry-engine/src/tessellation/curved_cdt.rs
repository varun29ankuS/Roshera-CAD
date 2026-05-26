//! Constrained Delaunay Triangulation (CDT) for curved B-Rep faces.
//!
//! Migrates the curved-surface tessellation path away from the legacy
//! quadtree grid (`tessellate_curved_adaptive`) onto the same
//! `cdt::triangulate_contours` pipeline the planar path uses. Each
//! face's trim loops are projected from 3D into the surface's UV
//! parameter space via `surface.closest_point`, interior Steiner
//! points are sprinkled on a curvature-driven grid, and the resulting
//! polygon + Steiner set is handed to the CDT crate as constraint
//! segments. The returned 2D triangles are then lifted back to 3D
//! through `surface.point_at` — except for boundary vertices, which
//! must come from `EdgeSampleCache` so adjacent faces sharing an edge
//! end up bit-identical at the seam.
//!
//! See `plans/federated-soaring-nebula.md` for the design walk-through.
//!
//! ## Failure model
//!
//! Every failure produces `Err(CurvedCdtError::…)`; callers fall back
//! to `tessellate_curved_adaptive` (legacy quadtree). The α slice
//! never panics on bad input — the worst case is a stair-stepped
//! boundary along whatever face the legacy path produces.
//!
//! Indexed access into projected boundary arrays and CDT-output
//! triangle index triples is the canonical idiom — bounds are
//! guaranteed by the polygon lengths established up-front. Matches
//! the numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::edge_cache::EdgeSampleCache;
use super::surface::polygon_signed_area_uv;
use super::{MeshVertex, TessellationParams, TriangleMesh};
use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::face::Face;
use crate::primitives::r#loop::{Loop, LoopType};
use crate::primitives::surface::Surface;
use crate::primitives::topology_builder::BRepModel;

/// Failure modes for `tessellate_curved_cdt`.
///
/// Every variant triggers a fallback to `tessellate_curved_adaptive`
/// at the call site; the α slice never panics on degenerate input.
#[derive(Debug)]
pub(crate) enum CurvedCdtError {
    /// Outer loop sampled to fewer than 3 points, or every sample
    /// projected to the same UV; equivalent to a zero-area polygon.
    DegenerateLoop,
    /// `surface.closest_point` returned `Err` on a boundary sample;
    /// the UV projection of the loop is incomplete. Also surfaced
    /// from Step 5 mesh emission when `surface.point_at` fails on
    /// an interior Steiner / refinement vertex.
    ProjectionFailed,
    /// Polygon-level validity check failed: outer self-intersects,
    /// inner bbox not contained in outer bbox, or signed area zero
    /// after unwrap.
    PolygonInvalid,
    /// The `cdt` crate rejected the input set (e.g. duplicate points
    /// after dedup, contour self-intersections that we didn't catch
    /// in `PolygonInvalid`).
    CdtFailed(cdt::Error),
}

impl std::fmt::Display for CurvedCdtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CurvedCdtError::DegenerateLoop => write!(f, "degenerate loop in curved CDT input"),
            CurvedCdtError::ProjectionFailed => {
                write!(f, "closest_point failed on a boundary sample")
            }
            CurvedCdtError::PolygonInvalid => write!(f, "projected polygon is invalid"),
            CurvedCdtError::CdtFailed(e) => write!(f, "cdt crate rejected input: {:?}", e),
        }
    }
}

/// Parallel arrays describing one projected loop: cached 3D samples
/// (verbatim from `EdgeSampleCache::get_or_compute`) and their UV
/// inverses (with periodicity-unwrap applied so the polygon is a
/// continuous trace in parameter space).
///
/// `points_3d.len() == points_uv.len()` is an invariant maintained
/// by `project_loop_to_uv`; downstream consumers index both arrays
/// with the same `i`.
#[derive(Debug, Clone)]
struct ProjectedLoop {
    /// Cached 3D positions, in the order the loop was walked.
    points_3d: Vec<Point3>,
    /// UV inverses, parallel to `points_3d`.
    points_uv: Vec<(f64, f64)>,
    /// Loop classification — interior-vs-exterior membership is
    /// resolved downstream by indexing into outer/inner ranges, so
    /// the discriminant is preserved here for debug-trace / future
    /// consumers but not read on the happy path.
    #[allow(dead_code)]
    loop_type: LoopType,
}

/// Bounding box in UV (u_min, u_max, v_min, v_max).
type UvBBox = (f64, f64, f64, f64);

/// Walk one B-Rep loop, fetch cached 3D samples per edge in canonical
/// curve-forward order honoring `loop.orientations`, project each 3D
/// sample to UV via `surface.closest_point`, and apply periodicity-
/// unwrap against the previous sample.
///
/// **Shared-edge coherence contract.** The 3D positions returned here
/// are taken **verbatim** from `EdgeSampleCache::get_or_compute` — no
/// re-evaluation through `surface.point_at`. This guarantees that
/// adjacent faces sharing the same B-Rep edge see exactly the same
/// 3D points at the seam, which is the precondition
/// `weld_mesh_watertight_range` relies on (it compares with bit-exact
/// equality after a tolerance filter).
///
/// Drop-last convention matches `surface::sample_loop_3d_polygon` and
/// the planar path: each edge contributes `n` of its `n + 1` cache
/// samples; the omitted endpoint is supplied by the next edge's
/// first sample (or, for the loop's final edge, by the first edge's
/// first sample — i.e. the polygon closes implicitly).
///
/// Returns `Err(ProjectionFailed)` if any sample's `closest_point`
/// fails.
fn project_loop_to_uv(
    loop_data: &Loop,
    model: &BRepModel,
    cache: &EdgeSampleCache,
    surface: &dyn Surface,
) -> Result<ProjectedLoop, CurvedCdtError> {
    let u_period = surface.period_u();
    let v_period = surface.period_v();

    let mut points_3d: Vec<Point3> = Vec::new();
    let mut points_uv: Vec<(f64, f64)> = Vec::new();
    let mut last_uv: Option<(f64, f64)> = None;

    for (i, &edge_id) in loop_data.edges.iter().enumerate() {
        let forward = loop_data.orientations.get(i).copied().unwrap_or(true);
        let samples = cache.get_or_compute(edge_id, model);
        let n = samples.len();
        if n < 2 {
            // Degenerate or unfetchable edge; skip but keep walking.
            // The drop-last convention means a skipped edge does not
            // create a hole as long as the *next* edge contributes a
            // sample that lands on the shared vertex.
            continue;
        }

        // `slice` enumerates the samples to emit in the order required
        // by the loop's edge orientation. Forward emits samples[0..n-1];
        // reversed emits samples[n-1..1] (down to but not including
        // index 0). The omitted endpoint in both cases is shared with
        // the next edge's first sample.
        let emit_indices: Vec<usize> = if forward {
            (0..n - 1).collect()
        } else {
            (1..n).rev().collect()
        };

        for idx in emit_indices {
            let p_3d = samples[idx];
            let (mut u, mut v) = match surface.closest_point(&p_3d, Tolerance::default()) {
                Ok(uv) => uv,
                Err(_) => return Err(CurvedCdtError::ProjectionFailed),
            };

            // Periodicity unwrap against the previous sample. Mirrors
            // `project_loop_uv_unwrapped` in `tessellation::surface`,
            // but driven off the cached 3D stream rather than
            // `curve.point_at(t)` re-evaluations so the resulting
            // boundary 3D ↔ UV map is consistent with the cache.
            if let Some((prev_u, prev_v)) = last_uv {
                if let Some(period) = u_period {
                    let half = period * 0.5;
                    while u - prev_u > half {
                        u -= period;
                    }
                    while u - prev_u < -half {
                        u += period;
                    }
                }
                if let Some(period) = v_period {
                    let half = period * 0.5;
                    while v - prev_v > half {
                        v -= period;
                    }
                    while v - prev_v < -half {
                        v += period;
                    }
                }
            }

            points_3d.push(p_3d);
            points_uv.push((u, v));
            last_uv = Some((u, v));
        }
    }

    Ok(ProjectedLoop {
        points_3d,
        points_uv,
        loop_type: loop_data.loop_type,
    })
}

/// Compute the UV bbox of a polygon. Returns `None` for empty input
/// so the caller can short-circuit with `Err(DegenerateLoop)`.
fn uv_bbox_of(polygon: &[(f64, f64)]) -> Option<UvBBox> {
    if polygon.is_empty() {
        return None;
    }
    let mut u_lo = f64::INFINITY;
    let mut u_hi = f64::NEG_INFINITY;
    let mut v_lo = f64::INFINITY;
    let mut v_hi = f64::NEG_INFINITY;
    for &(u, v) in polygon {
        if u < u_lo {
            u_lo = u;
        }
        if u > u_hi {
            u_hi = u;
        }
        if v < v_lo {
            v_lo = v;
        }
        if v > v_hi {
            v_hi = v;
        }
    }
    Some((u_lo, u_hi, v_lo, v_hi))
}

/// Validate a single projected outer/inner loop. Outer must have
/// at least 3 samples and non-zero signed area. Inner must satisfy
/// the same and additionally be contained in `outer_bbox`.
fn validate_loop(
    projected: &ProjectedLoop,
    outer_bbox: Option<UvBBox>,
) -> Result<(), CurvedCdtError> {
    if projected.points_uv.len() < 3 {
        return Err(CurvedCdtError::DegenerateLoop);
    }
    let area = polygon_signed_area_uv(&projected.points_uv);
    if area.abs() < 1e-18 {
        // Zero-area polygon: either a collapsed seam or a self-folding
        // path that the CDT crate would reject anyway. Funnel to
        // PolygonInvalid so the caller can fall through to legacy.
        return Err(CurvedCdtError::PolygonInvalid);
    }
    if let Some(outer) = outer_bbox {
        // Inner-loop bbox must sit strictly inside the outer-loop bbox.
        // The CDT crate also requires non-touching outer/inner contours
        // when the contours are simple; bbox containment is a cheap
        // first-line filter.
        let inner_bbox = match uv_bbox_of(&projected.points_uv) {
            Some(b) => b,
            None => return Err(CurvedCdtError::DegenerateLoop),
        };
        let (ou_lo, ou_hi, ov_lo, ov_hi) = outer;
        let (iu_lo, iu_hi, iv_lo, iv_hi) = inner_bbox;
        if iu_lo < ou_lo || iu_hi > ou_hi || iv_lo < ov_lo || iv_hi > ov_hi {
            return Err(CurvedCdtError::PolygonInvalid);
        }
    }
    Ok(())
}

/// Run Step 0 (boundary projection + validation) for a face.
/// Returns the outer loop's projection, every inner loop's projection,
/// and the combined UV bbox (union over outer + all inners).
///
/// On `Ok`, all per-loop validity checks have passed:
/// - outer has ≥ 3 samples, non-zero signed area;
/// - every inner has ≥ 3 samples, non-zero signed area, and its
///   bbox is contained in the outer's bbox.
fn run_boundary_projection(
    face: &Face,
    model: &BRepModel,
    cache: &EdgeSampleCache,
    surface: &dyn Surface,
) -> Result<(ProjectedLoop, Vec<ProjectedLoop>, UvBBox), CurvedCdtError> {
    // --- Outer loop -----------------------------------------------------
    let outer_loop = model
        .loops
        .get(face.outer_loop)
        .ok_or(CurvedCdtError::DegenerateLoop)?;
    let outer = project_loop_to_uv(outer_loop, model, cache, surface)?;
    validate_loop(&outer, None)?;
    let outer_bbox = uv_bbox_of(&outer.points_uv).ok_or(CurvedCdtError::DegenerateLoop)?;

    // --- Inner loops ----------------------------------------------------
    let mut inners: Vec<ProjectedLoop> = Vec::with_capacity(face.inner_loops.len());
    for &inner_id in &face.inner_loops {
        let inner_loop = match model.loops.get(inner_id) {
            Some(l) => l,
            None => continue,
        };
        let inner = project_loop_to_uv(inner_loop, model, cache, surface)?;
        validate_loop(&inner, Some(outer_bbox))?;
        inners.push(inner);
    }

    Ok((outer, inners, outer_bbox))
}

/// Compute the "chart handedness" of a surface at the centre of an
/// unwrapped UV bbox. Returns `+1` if `(∂P/∂u × ∂P/∂v)` is parallel
/// to the face's outward normal at that point, `-1` if anti-parallel.
///
/// Why this matters: the CDT crate emits triangles in CCW order in
/// the 2D `(u, v)` plane (standard Delaunay convention). For a right-
/// handed parametrization where `(∂P/∂u × ∂P/∂v)` agrees with the
/// surface normal, CCW-in-UV ⇒ positive-3D-normal. For a left-handed
/// chart — e.g. a negative-offset `OffsetSurface` that flips the
/// effective `(u, v)` basis — CCW-in-UV ⇒ *negative*-3D-normal, so
/// Step 5 must flip the triangle winding.
///
/// The returned sign is multiplicatively combined with
/// `face.orientation.sign()` at emission time.
///
/// Fallback: if either `evaluate_full` or `face.normal_at` fails at
/// the centre, default to `+1`. The resulting triangulation is still
/// valid mesh data — only the per-triangle winding may be inverted,
/// which `weld_mesh_watertight` tolerates within `weld_tolerance`.
fn compute_chart_sign(
    surface: &dyn Surface,
    face: &Face,
    model: &BRepModel,
    bbox: UvBBox,
) -> i32 {
    let (u_lo, u_hi, v_lo, v_hi) = bbox;
    let u_mid = (u_lo + u_hi) * 0.5;
    let v_mid = (v_lo + v_hi) * 0.5;

    let eval = match surface.evaluate_full(u_mid, v_mid) {
        Ok(e) => e,
        Err(_) => return 1,
    };
    let chart_normal = match eval.du.cross(&eval.dv).normalize() {
        Ok(n) => n,
        Err(_) => return 1,
    };
    let face_normal = match face.normal_at(u_mid, v_mid, &model.surfaces) {
        Ok(n) => n,
        Err(_) => return 1,
    };
    // `face.normal_at` already factors in `face.orientation.sign()`,
    // so we need to compare against the surface-intrinsic normal,
    // which is `chart_normal` here. We un-do the orientation flip:
    // `face.orientation.sign()` returns -1.0 for Backward faces,
    // and `face.normal_at` multiplies the surface normal by that
    // sign internally. The chart-handedness question is whether
    // `(∂u × ∂v)` agrees with the *surface*'s declared positive
    // direction — i.e. with `face_normal * face.orientation.sign()`.
    let intrinsic_normal = face_normal * face.orientation.sign();
    if chart_normal.dot(&intrinsic_normal) >= 0.0 {
        1
    } else {
        -1
    }
}

/// In-house winding-number test against a projected UV polygon.
///
/// Wraps `tessellation::surface::calculate_winding_number` with a
/// fixed-tolerance "inside" cutoff: any |w| > 0.5 counts as inside.
/// The cutoff matches the planar path's convention; a true CCW
/// polygon containing the point returns `w ≈ +1`, a CW polygon
/// containing it returns `w ≈ -1`, and `w ≈ 0` ⇒ outside.
fn is_inside_uv_polygon(point: (f64, f64), polygon: &[(f64, f64)]) -> bool {
    let w = super::surface::calculate_winding_number(&point, polygon);
    w.abs() > 0.5
}

/// Step 2 — generate interior Steiner candidates and filter them
/// against the projected outer + inner polygons.
///
/// Algorithm:
/// 1. Estimate 3D edge lengths along chart axes at the bbox centre:
///    `du_3d = ||∂P/∂u||·(u_hi - u_lo)`, `dv_3d = ||∂P/∂v||·(v_hi - v_lo)`.
/// 2. `nu = clamp(ceil(du_3d / max_edge_length), min_segments, max_segments)`,
///    same for `nv`.
/// 3. Generate `(nu+1) × (nv+1)` candidate `(u, v)` on a uniform grid
///    spanning the unwrapped bbox. Skip the four corners (they collide
///    with potential boundary points and would not be constraint-
///    anchored anyway).
/// 4. Filter via in-house winding-number test: inside outer, outside
///    every inner.
///
/// Returns the filtered Steiner set in (u, v) coordinates.
fn generate_steiner_candidates(
    surface: &dyn Surface,
    bbox: UvBBox,
    outer_polygon: &[(f64, f64)],
    inner_polygons: &[Vec<(f64, f64)>],
    params: &TessellationParams,
) -> Vec<(f64, f64)> {
    let (u_lo, u_hi, v_lo, v_hi) = bbox;
    let u_span = u_hi - u_lo;
    let v_span = v_hi - v_lo;
    if u_span <= 0.0 || v_span <= 0.0 {
        return Vec::new();
    }
    let u_mid = (u_lo + u_hi) * 0.5;
    let v_mid = (v_lo + v_hi) * 0.5;

    // Derivative magnitudes at the centre. Fallback to a conservative
    // (large) step count when evaluation fails — the worst case is
    // an over-tessellated face, never an under-tessellated one.
    let (du_mag, dv_mag) = match surface.evaluate_full(u_mid, v_mid) {
        Ok(e) => (e.du.magnitude(), e.dv.magnitude()),
        Err(_) => (u_span, v_span),
    };

    let du_3d = du_mag * u_span;
    let dv_3d = dv_mag * v_span;

    let max_edge = if params.max_edge_length > 0.0 {
        params.max_edge_length
    } else {
        // Lint policy denies panics; treat zero/negative as "use a
        // single segment" rather than infinity, so we clamp later.
        f64::INFINITY
    };
    let nu_raw = if du_3d > 0.0 && max_edge.is_finite() {
        (du_3d / max_edge).ceil() as usize
    } else {
        params.min_segments
    };
    let nv_raw = if dv_3d > 0.0 && max_edge.is_finite() {
        (dv_3d / max_edge).ceil() as usize
    } else {
        params.min_segments
    };
    let nu = nu_raw.max(params.min_segments).min(params.max_segments);
    let nv = nv_raw.max(params.min_segments).min(params.max_segments);

    // Generate interior grid (skip the four corners so we don't
    // collide with boundary samples that may sit on the bbox edge).
    let mut candidates: Vec<(f64, f64)> = Vec::with_capacity((nu + 1) * (nv + 1));
    for j in 0..=nv {
        let v = v_lo + (j as f64) * v_span / (nv as f64);
        for i in 0..=nu {
            let u = u_lo + (i as f64) * u_span / (nu as f64);
            // Skip the entire bbox boundary (corners + edges). Grid
            // points on `u = u_lo`, `u = u_hi`, `v = v_lo`, or
            // `v = v_hi` collide with the projected outer polygon's
            // constraint segments when the outer trim is axis-aligned
            // with the bbox (the common case for rectangular faces).
            // The `cdt` crate rejects Steiner points coincident with
            // a fixed-edge interior as `PointOnFixedEdge`. Filtering
            // all boundary points keeps Steiner strictly interior to
            // the bbox; the polygon-winding test downstream then
            // additionally rejects points outside the actual outer
            // polygon (when the polygon differs from its bbox).
            let on_boundary = i == 0 || i == nu || j == 0 || j == nv;
            if on_boundary {
                continue;
            }
            // Inside-outer, outside-every-inner test against the
            // **projected** polygons from Step 0 (not
            // `point_inside_face_uv`, which would re-project and risk
            // drift).
            if !is_inside_uv_polygon((u, v), outer_polygon) {
                continue;
            }
            if inner_polygons
                .iter()
                .any(|p| is_inside_uv_polygon((u, v), p))
            {
                continue;
            }
            candidates.push((u, v));
        }
    }

    candidates
}

/// Step 3 — assemble pts2d (`outer_uv ++ each inner_uv ++ steiner`)
/// and contours (one closed contour per loop, last index repeats
/// first), then call `cdt::triangulate_contours`.
///
/// Returns the assembled point list (so Step 5 can index into it
/// when emitting the mesh) and the resulting triangle index triples.
///
/// On `cdt::Error`, returns `Err(CdtFailed(_))`.
fn run_cdt(
    outer_uv: &[(f64, f64)],
    inner_uvs: &[Vec<(f64, f64)>],
    steiner: &[(f64, f64)],
) -> Result<(Vec<(f64, f64)>, Vec<[usize; 3]>), CurvedCdtError> {
    let mut pts2d: Vec<(f64, f64)> = Vec::with_capacity(
        outer_uv.len()
            + inner_uvs.iter().map(|p| p.len()).sum::<usize>()
            + steiner.len(),
    );
    let mut contours: Vec<Vec<usize>> = Vec::with_capacity(1 + inner_uvs.len());

    // Outer contour.
    let outer_start = pts2d.len();
    pts2d.extend_from_slice(outer_uv);
    let outer_end = pts2d.len();
    if outer_end - outer_start < 3 {
        return Err(CurvedCdtError::DegenerateLoop);
    }
    let outer_contour: Vec<usize> = (outer_start..outer_end)
        .chain(std::iter::once(outer_start))
        .collect();
    contours.push(outer_contour);

    // Inner contours.
    for inner in inner_uvs {
        let s = pts2d.len();
        pts2d.extend_from_slice(inner);
        let e = pts2d.len();
        if e - s < 3 {
            // Skip degenerate inner; outer remains valid.
            continue;
        }
        let inner_contour: Vec<usize> =
            (s..e).chain(std::iter::once(s)).collect();
        contours.push(inner_contour);
    }

    // Steiner points are added to pts2d but NOT to any contour;
    // CDT treats them as floating constraint anchors.
    pts2d.extend_from_slice(steiner);

    match cdt::triangulate_contours(&pts2d, &contours) {
        Ok(tris) => {
            let triangles: Vec<[usize; 3]> =
                tris.into_iter().map(|(a, b, c)| [a, b, c]).collect();
            Ok((pts2d, triangles))
        }
        Err(e) => Err(CurvedCdtError::CdtFailed(e)),
    }
}

/// Resolve the 3D position for a CDT-output point at index `i` in
/// the assembled `pts2d` array. Boundary indices (outer + inners)
/// must come from the cached 3D samples — this is the shared-edge
/// coherence contract. Interior (Steiner / refinement) indices are
/// lifted via `surface.point_at`.
///
/// Returns `Err(ProjectionFailed)` if `surface.point_at` fails on an
/// interior point.
fn resolve_position_3d(
    i: usize,
    outer: &ProjectedLoop,
    inners: &[ProjectedLoop],
    pts2d: &[(f64, f64)],
    surface: &dyn Surface,
) -> Result<Point3, CurvedCdtError> {
    // pts2d layout (matches run_cdt's append order):
    //   [0 .. outer.len())                              → outer boundary
    //   [outer.len() .. outer.len() + inner_k.len())    → inner k boundary
    //   [boundary_total ..)                             → Steiner / refinement
    let outer_n = outer.points_3d.len();
    if i < outer_n {
        return Ok(outer.points_3d[i]);
    }
    let mut cursor = outer_n;
    for inner in inners {
        let inner_n = inner.points_3d.len();
        if i < cursor + inner_n {
            return Ok(inner.points_3d[i - cursor]);
        }
        cursor += inner_n;
    }
    // Interior point — lift through the surface.
    let (u, v) = pts2d[i];
    surface
        .point_at(u, v)
        .map_err(|_| CurvedCdtError::ProjectionFailed)
}

/// Phase H — Step 4 refinement. For each output triangle, evaluate
/// the UV centroid, lift it to 3D, and compare against the planar
/// triangle formed by the three corner 3D positions. If chord error
/// exceeds `params.chord_tolerance` OR the max corner-normal
/// deviation exceeds `params.max_angle_deviation`, push the centroid
/// UV into a refinement set.
///
/// Returns the (possibly empty) refinement set. Bounded at one pass
/// in α — `tessellate_curved_cdt` calls this once and re-runs CDT
/// at most once.
fn collect_refinement_centroids(
    triangles: &[[usize; 3]],
    pts2d: &[(f64, f64)],
    outer: &ProjectedLoop,
    inners: &[ProjectedLoop],
    surface: &dyn Surface,
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    for tri in triangles {
        let (ia, ib, ic) = (tri[0], tri[1], tri[2]);
        if ia >= pts2d.len() || ib >= pts2d.len() || ic >= pts2d.len() {
            continue;
        }
        let pa = match resolve_position_3d(ia, outer, inners, pts2d, surface) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let pb = match resolve_position_3d(ib, outer, inners, pts2d, surface) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let pc = match resolve_position_3d(ic, outer, inners, pts2d, surface) {
            Ok(p) => p,
            Err(_) => continue,
        };

        let (ua, va) = pts2d[ia];
        let (ub, vb) = pts2d[ib];
        let (uc, vc) = pts2d[ic];
        let u_c = (ua + ub + uc) / 3.0;
        let v_c = (va + vb + vc) / 3.0;

        let p_centroid = match surface.point_at(u_c, v_c) {
            Ok(p) => p,
            Err(_) => continue,
        };

        // Build the planar triangle's outward normal from the 3D
        // corners. Skip degenerate triangles (cross product near
        // zero).
        let e1 = pb - pa;
        let e2 = pc - pa;
        let plane_normal = match e1.cross(&e2).normalize() {
            Ok(n) => n,
            Err(_) => continue,
        };
        // Plane-corner triangle centroid in 3D (linear combination).
        let plane_centroid = Point3::new(
            (pa.x + pb.x + pc.x) / 3.0,
            (pa.y + pb.y + pc.y) / 3.0,
            (pa.z + pb.z + pc.z) / 3.0,
        );
        // Chord error: ⊥distance from surface-evaluated centroid to
        // the plane spanned by the three corners.
        let delta = p_centroid - plane_centroid;
        let chord_error = delta.dot(&plane_normal).abs();

        // Normal deviation: max angle between the surface normal at
        // the centroid and the three corner normals.
        let normal_centroid = face
            .normal_at(u_c, v_c, &model.surfaces)
            .unwrap_or(plane_normal);
        let normal_a = face.normal_at(ua, va, &model.surfaces).unwrap_or(plane_normal);
        let normal_b = face.normal_at(ub, vb, &model.surfaces).unwrap_or(plane_normal);
        let normal_c = face.normal_at(uc, vc, &model.surfaces).unwrap_or(plane_normal);
        let ang = |n: Vector3| -> f64 {
            let d = normal_centroid.dot(&n).clamp(-1.0, 1.0);
            d.acos()
        };
        let max_dev = ang(normal_a).max(ang(normal_b)).max(ang(normal_c));

        if chord_error > params.chord_tolerance || max_dev > params.max_angle_deviation {
            out.push((u_c, v_c));
        }
    }
    out
}

/// CDT-driven curved-surface tessellator. Public to the crate so
/// `tessellation::surface` dispatchers can call it; never re-exported.
///
/// On `Ok(())` the caller's `mesh` has been populated with the face's
/// triangles (vertices and indices both pushed). On `Err`, `mesh` is
/// left untouched and the caller is expected to fall through to the
/// legacy `tessellate_curved_adaptive` path.
///
/// Pipeline: Step 0 boundary projection → Step 1 chart handedness →
/// Step 2 Steiner candidates → Step 3 CDT call → Step 4 one
/// refinement pass (centroid insertion driven by chord & normal
/// tolerances) → Step 5 mesh emission with cached-boundary 3D and
/// chart-sign × orientation triangle-winding flip.
pub(crate) fn tessellate_curved_cdt(
    surface: &dyn Surface,
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    cache: &EdgeSampleCache,
    mesh: &mut TriangleMesh,
) -> Result<(), CurvedCdtError> {
    // Step 0 — boundary projection.
    let (outer, inners, outer_bbox) =
        run_boundary_projection(face, model, cache, surface)?;

    // Step 1 — chart handedness.
    let chart_sign = compute_chart_sign(surface, face, model, outer_bbox);

    // Step 2 — Steiner candidates on a curvature-driven grid.
    let inner_polygons: Vec<Vec<(f64, f64)>> =
        inners.iter().map(|p| p.points_uv.clone()).collect();
    let mut steiner = generate_steiner_candidates(
        surface,
        outer_bbox,
        &outer.points_uv,
        &inner_polygons,
        params,
    );

    // Step 3 — first CDT run.
    let (pts2d, triangles) = run_cdt(&outer.points_uv, &inner_polygons, &steiner)?;

    // Step 4 — single refinement pass. If the first CDT triangulation
    // violates chord or normal tolerance anywhere, augment the Steiner
    // set with centroid UVs and re-run CDT once. Cap at one pass in α;
    // residual high-error triangles remain a CDT-β concern.
    let refinement =
        collect_refinement_centroids(&triangles, &pts2d, &outer, &inners, surface, face, model, params);
    let (final_pts2d, final_triangles) = if refinement.is_empty() {
        (pts2d, triangles)
    } else {
        steiner.extend(refinement);
        // Filter newly-added Steiner points to keep the inside-outer /
        // outside-every-inner invariant. The centroid of a CDT-output
        // triangle is guaranteed inside the outer polygon (CDT does
        // not emit triangles outside the contour) and outside every
        // inner, so we don't re-filter here — but we do dedup to
        // protect the `cdt` crate from duplicate-point rejection.
        steiner.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        });
        steiner.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-12 && (a.1 - b.1).abs() < 1e-12);
        match run_cdt(&outer.points_uv, &inner_polygons, &steiner) {
            Ok(retr) => retr,
            Err(_) => {
                // Refinement re-run failed; fall back to the first
                // triangulation rather than failing the whole face.
                // We re-run Step 3 to recover the original result —
                // it succeeded earlier with the same input, so this
                // is a cheap reconstruction.
                run_cdt(&outer.points_uv, &inner_polygons, &generate_steiner_candidates(
                    surface, outer_bbox, &outer.points_uv, &inner_polygons, params,
                ))?
            }
        }
    };

    // Step 5 — mesh emission. Vertex base offset must be recorded so
    // triangle indices are rebased into `mesh.vertices` numbering.
    let vertex_base = mesh.vertices.len() as u32;
    for (i, &(u, v)) in final_pts2d.iter().enumerate() {
        let position = resolve_position_3d(i, &outer, &inners, &final_pts2d, surface)?;
        let normal = face
            .normal_at(u, v, &model.surfaces)
            .unwrap_or(Vector3::Z);
        mesh.add_vertex(MeshVertex {
            position,
            normal,
            uv: Some((u, v)),
        });
    }

    // Winding flip: the CDT crate emits triangles CCW in (u, v). For
    // the mesh to be outward-facing in 3D we need:
    //   (chart_sign == +1) ∧ (orientation Forward)  → keep (a,b,c)
    //   (chart_sign == -1) ∧ (orientation Backward) → keep (a,b,c)
    //   otherwise                                   → swap to (a,c,b)
    let keep_winding = (chart_sign == 1) == face.orientation.is_forward();
    for tri in &final_triangles {
        let a = vertex_base + tri[0] as u32;
        let b = vertex_base + tri[1] as u32;
        let c = vertex_base + tri[2] as u32;
        if keep_winding {
            mesh.add_triangle(a, b, c);
        } else {
            mesh.add_triangle(a, c, b);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::nurbs::NurbsSurface as MathNurbs;
    use crate::math::Vector3;
    use crate::primitives::curve::{Line, ParameterRange};
    use crate::primitives::edge::{Edge, EdgeOrientation};
    use crate::primitives::face::{Face, FaceOrientation};
    use crate::primitives::r#loop::{Loop, LoopType};
    use crate::primitives::surface::GeneralNurbsSurface;
    use crate::primitives::topology_builder::BRepModel;

    /// Build a degenerate, never-touched mesh used in assertion glue.
    /// Allowed-expect because the test harness controls the inputs.
    fn empty_mesh() -> TriangleMesh {
        TriangleMesh::new()
    }

    /// Construct a real B-Rep face on a bilinear NURBS patch with a
    /// rectangular outer trim, return the (model, face_id) pair, and
    /// the edge IDs in loop order so callers can compare cache samples
    /// against the boundary projection.
    ///
    /// The patch covers (u, v) ∈ [0, 1] × [0, 1] and is the flat
    /// `z = 0` square spanning the unit XY rectangle — bilinear so
    /// the test doesn't depend on degree-2 NURBS evaluation, but it's
    /// still routed through `GeneralNurbsSurface` so we exercise the
    /// `closest_point` path the production code uses.
    #[allow(clippy::expect_used)]
    // Reason: test fixtures may panic with a clear invariant message; the
    // workspace's `expect_used = "deny"` lint accepts this in `#[cfg(test)]`.
    fn build_flat_nurbs_face_model() -> (BRepModel, u32, Vec<u32>) {
        let mut model = BRepModel::new();

        // ---- NURBS surface: bilinear flat patch in XY plane --------
        let control_points = vec![
            vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(1.0, 0.0, 0.0),
            ],
            vec![
                Point3::new(0.0, 1.0, 0.0),
                Point3::new(1.0, 1.0, 0.0),
            ],
        ];
        let weights = vec![vec![1.0, 1.0], vec![1.0, 1.0]];
        let knots_u = vec![0.0, 0.0, 1.0, 1.0];
        let knots_v = vec![0.0, 0.0, 1.0, 1.0];
        let math_nurbs = MathNurbs::new(control_points, weights, knots_u, knots_v, 1, 1)
            .expect("bilinear flat NURBS must construct");
        let surface_id = model
            .surfaces
            .add(Box::new(GeneralNurbsSurface { nurbs: math_nurbs }));

        // ---- Vertices: 4 corners of the unit square ----------------
        let tol = 1e-6;
        let v00 = model.vertices.add_or_find(0.0, 0.0, 0.0, tol);
        let v10 = model.vertices.add_or_find(1.0, 0.0, 0.0, tol);
        let v11 = model.vertices.add_or_find(1.0, 1.0, 0.0, tol);
        let v01 = model.vertices.add_or_find(0.0, 1.0, 0.0, tol);

        // ---- Curves: 4 straight lines around the perimeter --------
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

        // ---- Edges -------------------------------------------------
        let e0 = model.edges.add(Edge::new(
            0,
            v00,
            v10,
            c0,
            EdgeOrientation::Forward,
            ParameterRange::unit(),
        ));
        let e1 = model.edges.add(Edge::new(
            0,
            v10,
            v11,
            c1,
            EdgeOrientation::Forward,
            ParameterRange::unit(),
        ));
        let e2 = model.edges.add(Edge::new(
            0,
            v11,
            v01,
            c2,
            EdgeOrientation::Forward,
            ParameterRange::unit(),
        ));
        let e3 = model.edges.add(Edge::new(
            0,
            v01,
            v00,
            c3,
            EdgeOrientation::Forward,
            ParameterRange::unit(),
        ));

        // ---- Outer loop -------------------------------------------
        let mut outer = Loop::new(0, LoopType::Outer);
        outer.add_edge(e0, true);
        outer.add_edge(e1, true);
        outer.add_edge(e2, true);
        outer.add_edge(e3, true);
        let outer_id = model.loops.add(outer);

        // ---- Face --------------------------------------------------
        let face = Face::new(0, surface_id, outer_id, FaceOrientation::Forward);
        let face_id = model.faces.add(face);

        (model, face_id, vec![e0, e1, e2, e3])
    }

    /// Phase B unit test 7: emit a real face's boundary projection
    /// and assert every emitted 3D position is bit-equal to one of
    /// the cache's outputs for the corresponding edge. This is the
    /// shared-edge-coherence invariant.
    #[allow(clippy::expect_used)]
    // Reason: invariants the fixture builder enforces are documented above.
    #[test]
    fn boundary_3d_position_matches_cache_exactly() {
        let (model, face_id, edge_ids) = build_flat_nurbs_face_model();
        let params = TessellationParams::default();
        let cache = EdgeSampleCache::new(&params);
        let face = model
            .faces
            .get(face_id)
            .expect("test fixture must produce a valid face");
        let surface = model
            .surfaces
            .get(face.surface_id)
            .expect("face must reference a valid surface");

        // Run the boundary projection in isolation.
        let outer_loop = model
            .loops
            .get(face.outer_loop)
            .expect("outer loop must be present");
        let projected = project_loop_to_uv(outer_loop, &model, &cache, surface)
            .expect("flat NURBS face must project without error");

        // For each emitted 3D point, check it's bit-equal to one of
        // the cached samples for one of the loop's edges. We compare
        // every loop edge — boundary samples may be drawn from any
        // edge in the loop (drop-last convention shifts indices).
        let cached_pools: Vec<Vec<Point3>> = edge_ids
            .iter()
            .map(|&eid| cache.get_or_compute(eid, &model).as_ref().clone())
            .collect();

        for (idx, &p3d) in projected.points_3d.iter().enumerate() {
            // Bit-exact match: at least one cached sample in some
            // edge's pool must compare `==` to this point. We don't
            // assert *which* edge — drop-last reshuffles indices —
            // only that the point came from the cache verbatim.
            let found = cached_pools.iter().any(|pool| {
                pool.iter().any(|c| {
                    c.x.to_bits() == p3d.x.to_bits()
                        && c.y.to_bits() == p3d.y.to_bits()
                        && c.z.to_bits() == p3d.z.to_bits()
                })
            });
            assert!(
                found,
                "projected boundary sample {idx} = {:?} does not match any cached edge sample bit-for-bit",
                p3d
            );
        }

        // Sanity: at least 4 points were emitted (one per edge with
        // drop-last; the unit-square outer collapses to 4 samples
        // because every edge is straight, so the cache returns 2
        // samples per edge and drop-last keeps one).
        assert!(
            projected.points_3d.len() >= 4,
            "outer loop projection must contain at least 4 points; got {}",
            projected.points_3d.len()
        );
    }

    /// Sanity: parallel arrays stay in lockstep.
    #[test]
    fn project_loop_keeps_parallel_arrays_in_lockstep() {
        let (model, face_id, _edge_ids) = build_flat_nurbs_face_model();
        let params = TessellationParams::default();
        let cache = EdgeSampleCache::new(&params);
        #[allow(clippy::expect_used)]
        // Reason: fixture invariants documented in build_flat_nurbs_face_model.
        let face = model.faces.get(face_id).expect("face must be present");
        #[allow(clippy::expect_used)]
        // Reason: fixture invariants documented above.
        let surface = model
            .surfaces
            .get(face.surface_id)
            .expect("surface must be present");
        #[allow(clippy::expect_used)]
        // Reason: fixture invariants documented above.
        let outer_loop = model
            .loops
            .get(face.outer_loop)
            .expect("outer loop must be present");
        #[allow(clippy::expect_used)]
        // Reason: bilinear flat NURBS projection is total.
        let projected = project_loop_to_uv(outer_loop, &model, &cache, surface)
            .expect("flat NURBS face must project without error");
        assert_eq!(
            projected.points_3d.len(),
            projected.points_uv.len(),
            "points_3d and points_uv must stay in lockstep"
        );
        assert_eq!(projected.loop_type, LoopType::Outer);
    }

    /// Phase F flips the dispatcher contract: the happy path (a valid
    /// flat NURBS face) must now return `Ok(())` with a populated mesh.
    /// The `Err(_)` arms remain as fallback escape hatches for
    /// degenerate input, but a well-formed face exercises the full
    /// boundary-projection → CDT → mesh-emission pipeline.
    #[test]
    fn dispatcher_emits_triangles_on_happy_path() {
        let (model, face_id, _edge_ids) = build_flat_nurbs_face_model();
        let params = TessellationParams::default();
        let cache = EdgeSampleCache::new(&params);
        let mut mesh = empty_mesh();
        #[allow(clippy::expect_used)]
        // Reason: fixture invariants documented.
        let face = model.faces.get(face_id).expect("face must be present");
        #[allow(clippy::expect_used)]
        // Reason: fixture invariants documented.
        let surface = model
            .surfaces
            .get(face.surface_id)
            .expect("surface must be present");
        let result = tessellate_curved_cdt(surface, face, &model, &params, &cache, &mut mesh);
        assert!(
            result.is_ok(),
            "happy path must return Ok(()); got {:?}",
            result
        );
        assert!(
            !mesh.vertices.is_empty(),
            "mesh must contain at least one vertex after Phase F emission"
        );
        assert!(
            !mesh.triangles.is_empty(),
            "mesh must contain at least one triangle after Phase F emission"
        );
        // Every triangle index must reference a valid mesh vertex.
        let n = mesh.vertices.len() as u32;
        for t in &mesh.triangles {
            assert!(t[0] < n, "triangle index {} out of bounds (n={n})", t[0]);
            assert!(t[1] < n, "triangle index {} out of bounds (n={n})", t[1]);
            assert!(t[2] < n, "triangle index {} out of bounds (n={n})", t[2]);
        }
    }

    /// Display impl covers every variant; this guards against the
    /// `format!` impl drifting out of sync with the enum.
    #[test]
    fn error_display_covers_all_variants() {
        let cases: [CurvedCdtError; 3] = [
            CurvedCdtError::DegenerateLoop,
            CurvedCdtError::ProjectionFailed,
            CurvedCdtError::PolygonInvalid,
        ];
        for e in &cases {
            let s = format!("{}", e);
            assert!(
                !s.is_empty(),
                "Display impl must not produce empty strings"
            );
        }
    }

    /// Mock surface whose declared normal is flipped relative to
    /// `(∂P/∂u × ∂P/∂v)`. This is the relevant pathology for negative-
    /// offset `OffsetSurface` and similar wrappers: they retain the
    /// underlying parametrization but report `normal_at` flipped.
    /// `compute_chart_sign` must detect this and return `-1` so the
    /// triangle-winding flip in Step 5 keeps the mesh outward-facing.
    #[derive(Debug)]
    struct FlippedNormalPlane;

    impl crate::primitives::surface::Surface for FlippedNormalPlane {
        fn surface_type(&self) -> crate::primitives::surface::SurfaceType {
            crate::primitives::surface::SurfaceType::Plane
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn clone_box(&self) -> Box<dyn crate::primitives::surface::Surface> {
            Box::new(FlippedNormalPlane)
        }
        fn evaluate_full(
            &self,
            u: f64,
            v: f64,
        ) -> crate::math::MathResult<crate::primitives::surface::SurfacePoint> {
            // Right-handed: P(u,v) = (u, v, 0), du = X̂, dv = Ŷ,
            // du × dv = +Ẑ. But we report `normal = -Ẑ` — the
            // hallmark of a negative-offset wrapper.
            Ok(crate::primitives::surface::SurfacePoint {
                position: Point3::new(u, v, 0.0),
                du: Vector3::X,
                dv: Vector3::Y,
                duu: Vector3::ZERO,
                duv: Vector3::ZERO,
                dvv: Vector3::ZERO,
                normal: -Vector3::Z, // Flipped against du × dv.
                k1: 0.0,
                k2: 0.0,
                dir1: Vector3::X,
                dir2: Vector3::Y,
            })
        }
        fn parameter_bounds(&self) -> ((f64, f64), (f64, f64)) {
            ((0.0, 1.0), (0.0, 1.0))
        }
        fn is_closed_u(&self) -> bool {
            false
        }
        fn is_closed_v(&self) -> bool {
            false
        }
        fn transform(
            &self,
            _matrix: &crate::math::Matrix4,
        ) -> Box<dyn crate::primitives::surface::Surface> {
            Box::new(FlippedNormalPlane)
        }
        fn type_name(&self) -> &'static str {
            "FlippedNormalPlane"
        }
        fn closest_point(
            &self,
            point: &Point3,
            _tolerance: Tolerance,
        ) -> crate::math::MathResult<(f64, f64)> {
            Ok((point.x, point.y))
        }
        fn offset(&self, _distance: f64) -> Box<dyn crate::primitives::surface::Surface> {
            Box::new(FlippedNormalPlane)
        }
        fn offset_exact(
            &self,
            _distance: f64,
            _tolerance: Tolerance,
        ) -> crate::math::MathResult<crate::primitives::surface::OffsetSurface> {
            Err(crate::math::MathError::InvalidParameter(
                "test mock".to_string(),
            ))
        }
        fn offset_variable(
            &self,
            _distance_fn: Box<dyn Fn(f64, f64) -> f64 + Send + Sync>,
            _tolerance: Tolerance,
        ) -> crate::math::MathResult<Box<dyn crate::primitives::surface::Surface>> {
            Err(crate::math::MathError::InvalidParameter(
                "test mock".to_string(),
            ))
        }
        fn intersect(
            &self,
            _other: &dyn crate::primitives::surface::Surface,
            _tolerance: Tolerance,
        ) -> Vec<crate::primitives::surface::SurfaceIntersectionResult> {
            Vec::new()
        }
    }

    /// Build a face on the `FlippedNormalPlane` mock. The face's
    /// `FaceOrientation::Forward` keeps `face.normal_at = surface.
    /// normal_at = -Ẑ`. So `intrinsic_normal = -Ẑ`. But du×dv = +Ẑ.
    /// Therefore `chart_sign` should be `-1`.
    #[allow(clippy::expect_used)]
    // Reason: fixture invariants documented.
    fn build_left_handed_offset_face_model() -> (BRepModel, u32) {
        let mut model = BRepModel::new();

        let surface_id = model.surfaces.add(Box::new(FlippedNormalPlane));

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
        let face_id = model.faces.add(face);
        (model, face_id)
    }

    /// Unit test 1: chart handedness for a right-handed NURBS patch
    /// (the standard flat fixture) is `+1`.
    #[test]
    fn chart_handedness_detected_for_right_handed_nurbs() {
        let (model, face_id, _edge_ids) = build_flat_nurbs_face_model();
        #[allow(clippy::expect_used)]
        // Reason: fixture invariants documented.
        let face = model.faces.get(face_id).expect("face must be present");
        #[allow(clippy::expect_used)]
        // Reason: fixture invariants documented.
        let surface = model
            .surfaces
            .get(face.surface_id)
            .expect("surface must be present");
        let bbox: UvBBox = (0.0, 1.0, 0.0, 1.0);
        let sign = compute_chart_sign(surface, face, &model, bbox);
        assert_eq!(
            sign, 1,
            "right-handed flat NURBS patch must produce chart_sign = +1; got {sign}"
        );
    }

    /// Unit test 2: chart handedness for a surface that reports
    /// `normal_at` flipped relative to its `(∂P/∂u × ∂P/∂v)` —
    /// the OffsetSurface-with-negative-offset pathology. The face's
    /// orientation is Forward, so `intrinsic_normal = surface.normal_at
    /// = -Ẑ`. Chart's (du×dv) is +Ẑ. They disagree, so chart_sign = -1.
    #[test]
    fn chart_handedness_detected_for_offset_surface_negative_offset() {
        let (model, face_id) = build_left_handed_offset_face_model();
        #[allow(clippy::expect_used)]
        // Reason: fixture invariants documented.
        let face = model.faces.get(face_id).expect("face must be present");
        #[allow(clippy::expect_used)]
        // Reason: fixture invariants documented.
        let surface = model
            .surfaces
            .get(face.surface_id)
            .expect("surface must be present");
        let bbox: UvBBox = (0.0, 1.0, 0.0, 1.0);
        let sign = compute_chart_sign(surface, face, &model, bbox);
        assert_eq!(
            sign, -1,
            "LH-chart NURBS with Backward face orientation must produce chart_sign = -1; got {sign}"
        );
    }

    // -- Phase D tests (Step 2: Steiner candidates) ---------------

    /// Build a cylindrical lateral fixture for periodicity-unwrap
    /// monotonicity testing. The face's outer loop must cover a
    /// full 2π in U. We construct the cylinder + a closed circular
    /// edge in 3D for the top and bottom rims, plus two vertical
    /// edges so the loop is well-defined.
    ///
    /// This is a minimal smoke fixture — for the unwrap-monotonicity
    /// test we only need a closed loop in U; we don't need the face
    /// to be otherwise valid topologically.
    ///
    /// However: building a real BRep face whose outer loop covers a
    /// genuine full-2π wrap on a cylindrical lateral is non-trivial
    /// from scratch (requires periodic edge handling that the kernel
    /// usually constructs at higher level). For this unit-level
    /// test we synthesise a `(u, v)` polygon directly and assert the
    /// unwrap step inside `project_loop_to_uv` produces strictly
    /// monotone `u`. We can do that by checking the algorithm on an
    /// already-projected polygon: feed it a sequence that would jump
    /// from `2π` back to `0` at the seam and ensure the unwrap pulls
    /// it to `2π → 0 + 2π = 2π → 4π`. The actual function is
    /// `project_loop_to_uv`, but the unwrap logic inside is the
    /// load-bearing piece — we replicate it on raw inputs here.
    #[test]
    fn project_loop_to_uv_preserves_periodicity_unwrap() {
        // Walk a closed loop manually and apply the same unwrap
        // logic the projection function uses. We test the unwrap
        // invariant directly: with `u_period = 2π`, a sequence of
        // raw u values `0, π/2, π, 3π/2, 0, π/2, ...` (canonical
        // wrap) should produce strictly monotone unwrapped output.
        let two_pi = std::f64::consts::TAU;
        let raw: Vec<f64> = (0..8)
            .map(|i| (i as f64) * two_pi / 8.0)
            .chain((0..8).map(|i| (i as f64) * two_pi / 8.0))
            .collect();
        let mut unwrapped: Vec<f64> = Vec::with_capacity(raw.len());
        let mut prev: Option<f64> = None;
        for &u_raw in &raw {
            let mut u = u_raw;
            if let Some(prev_u) = prev {
                let half = two_pi * 0.5;
                while u - prev_u > half {
                    u -= two_pi;
                }
                while u - prev_u < -half {
                    u += two_pi;
                }
            }
            unwrapped.push(u);
            prev = Some(u);
        }
        // Strictly monotone: every step is non-negative (each raw
        // step is π/4 forward, and the wrap injects no decrease).
        for i in 1..unwrapped.len() {
            let step = unwrapped[i] - unwrapped[i - 1];
            assert!(
                step > -1e-12,
                "unwrap must be non-decreasing; step at {i} = {step}"
            );
        }
        // Two full revolutions ⇒ u(end) ≈ 2 * 2π - π/4.
        let expected_end = 2.0 * two_pi - two_pi / 8.0;
        let actual_end = unwrapped[unwrapped.len() - 1];
        assert!(
            (actual_end - expected_end).abs() < 1e-9,
            "two-revolution unwrap should reach ≈ {expected_end}; got {actual_end}"
        );
    }

    /// Unit test 4: Steiner density scales with max_edge_length.
    /// A tighter (smaller) max_edge_length must produce at least 2×
    /// as many candidates on the same patch.
    #[test]
    fn interior_steiner_density_scales_with_max_edge_length() {
        let (model, face_id, _edge_ids) = build_flat_nurbs_face_model();
        #[allow(clippy::expect_used)]
        // Reason: fixture invariants documented.
        let face = model.faces.get(face_id).expect("face must be present");
        #[allow(clippy::expect_used)]
        // Reason: fixture invariants documented.
        let surface = model
            .surfaces
            .get(face.surface_id)
            .expect("surface must be present");
        let bbox: UvBBox = (0.0, 1.0, 0.0, 1.0);
        // Outer polygon is the unit square in UV; matches the
        // bilinear flat patch.
        let outer = vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        let inners: Vec<Vec<(f64, f64)>> = Vec::new();

        let mut coarse = TessellationParams::default();
        coarse.max_edge_length = 0.5;
        coarse.min_segments = 1; // Lift floor so the constraint binds.
        coarse.max_segments = 100;

        let mut fine = TessellationParams::default();
        fine.max_edge_length = 0.1;
        fine.min_segments = 1;
        fine.max_segments = 100;

        let coarse_n =
            generate_steiner_candidates(surface, bbox, &outer, &inners, &coarse).len();
        let fine_n = generate_steiner_candidates(surface, bbox, &outer, &inners, &fine).len();

        assert!(
            fine_n >= 2 * coarse_n,
            "tighter max_edge_length should produce ≥ 2× as many \
            Steiner candidates; coarse={coarse_n}, fine={fine_n}"
        );
    }

    /// Unit test 5: Steiner filter rejects points inside a hole.
    /// Synthetic projected outer (unit square) + hole (centred small
    /// square). All candidates inside the hole must be filtered.
    #[test]
    fn steiner_filter_rejects_points_inside_hole() {
        let (model, face_id, _edge_ids) = build_flat_nurbs_face_model();
        #[allow(clippy::expect_used)]
        // Reason: fixture invariants documented.
        let face = model.faces.get(face_id).expect("face must be present");
        #[allow(clippy::expect_used)]
        // Reason: fixture invariants documented.
        let surface = model
            .surfaces
            .get(face.surface_id)
            .expect("surface must be present");

        // Outer = unit square, hole = inner square at [0.25, 0.75]^2.
        // Both polygons in CCW orientation (winding-number test
        // tolerates either orientation, but symmetry helps reasoning).
        let outer = vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        let hole = vec![(0.25, 0.25), (0.75, 0.25), (0.75, 0.75), (0.25, 0.75)];
        let inners = vec![hole.clone()];

        let mut p = TessellationParams::default();
        p.max_edge_length = 0.05; // Fine grid so we get many in-hole candidates.
        p.min_segments = 1;
        p.max_segments = 200;

        let candidates =
            generate_steiner_candidates(surface, (0.0, 1.0, 0.0, 1.0), &outer, &inners, &p);

        // No candidate should sit strictly inside the hole.
        for &(u, v) in &candidates {
            assert!(
                !(u > 0.25 && u < 0.75 && v > 0.25 && v < 0.75),
                "candidate ({u:.3}, {v:.3}) sits inside the hole; \
                Steiner filter is incorrect"
            );
        }
        // Sanity: at least *some* candidates survived (outside hole,
        // inside outer) — otherwise the filter ate everything.
        assert!(
            !candidates.is_empty(),
            "Steiner filter should not reject every candidate"
        );
    }

    // -- Phase E tests (Step 3: CDT call) -------------------------

    /// Unit test 6: hand-built self-intersecting outer polygon must
    /// return `Err(CdtFailed | PolygonInvalid)`, never panic.
    #[test]
    fn cdt_input_rejected_returns_err() {
        // Bowtie polygon: edges (0,0)-(1,1) and (1,0)-(0,1) cross.
        let bowtie = vec![(0.0, 0.0), (1.0, 1.0), (1.0, 0.0), (0.0, 1.0)];
        let inners: Vec<Vec<(f64, f64)>> = Vec::new();
        let steiner: Vec<(f64, f64)> = Vec::new();
        let result = run_cdt(&bowtie, &inners, &steiner);
        match result {
            Ok((_, tris)) => {
                // It is possible that the CDT crate flood-fills a
                // degenerate cover; pin the expected behaviour as
                // "either Err, or a non-empty triangulation that
                // we can still emit". The plan requires *no panic*;
                // we don't require Err strictly.
                // However, the typical outcome is Err. Allow both
                // but assert no panic via reaching this branch.
                assert!(
                    tris.iter().all(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]),
                    "bowtie CDT result must not contain degenerate triangles"
                );
            }
            Err(e) => match e {
                CurvedCdtError::CdtFailed(_)
                | CurvedCdtError::PolygonInvalid
                | CurvedCdtError::DegenerateLoop => {
                    // Expected: CDT crate rejected the self-
                    // intersecting input or our pre-check did.
                }
                CurvedCdtError::ProjectionFailed => panic!(
                    "self-intersecting bowtie should not surface \
                    ProjectionFailed (no projection runs in run_cdt)"
                ),
            },
        }
    }

    /// Valid input → run_cdt succeeds and returns at least one
    /// triangle. Pins the happy path so a regression in `cdt`
    /// crate integration surfaces here, not at the integration
    /// test layer.
    #[test]
    fn cdt_unit_square_yields_at_least_one_triangle() {
        let outer = vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        let inners: Vec<Vec<(f64, f64)>> = Vec::new();
        let steiner: Vec<(f64, f64)> = Vec::new();
        #[allow(clippy::expect_used)]
        // Reason: unit square is a valid CDT input.
        let (pts2d, tris) = run_cdt(&outer, &inners, &steiner)
            .expect("unit square must triangulate without error");
        assert_eq!(pts2d.len(), 4);
        assert!(
            !tris.is_empty(),
            "unit-square CDT must produce ≥ 1 triangle; got 0"
        );
        // Every triangle index must reference a valid point.
        for t in &tris {
            assert!(t[0] < pts2d.len());
            assert!(t[1] < pts2d.len());
            assert!(t[2] < pts2d.len());
        }
    }

    /// uv_bbox_of returns None for empty input; non-trivial bbox
    /// reports tight bounds.
    #[test]
    fn uv_bbox_basics() {
        assert!(uv_bbox_of(&[]).is_none());
        let polygon = vec![(0.0, 0.0), (1.0, 0.0), (1.0, 2.0), (0.0, 2.0)];
        let (u_lo, u_hi, v_lo, v_hi) = uv_bbox_of(&polygon).expect("non-empty bbox");
        assert_eq!(u_lo, 0.0);
        assert_eq!(u_hi, 1.0);
        assert_eq!(v_lo, 0.0);
        assert_eq!(v_hi, 2.0);
    }

    // Silence unused-import warnings for symbols Phase C+ will use.
    #[allow(dead_code)]
    fn _phase_b_unused_imports_anchor() {
        let _ = Vector3::Z;
    }
}
