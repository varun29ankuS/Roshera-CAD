//! Surface-Plane Intersection Algorithm
//!
//! Computes intersection curves between an arbitrary parametric surface and a
//! plane by extracting the zero-set of the signed plane-distance field
//! `d(u,v) = (S(u,v) - origin) · normal` over the surface parameter domain.
//!
//! # Algorithm Overview
//!
//! 1. **Grid sampling** — evaluate `d(u,v)` on a uniform `(n+1)×(n+1)` grid.
//! 2. **Marching squares** — for every grid cell, the corner signs of `d`
//!    determine which of the four cell edges the contour crosses; the crossing
//!    on each edge is found by linear interpolation and snapped onto `d = 0`
//!    with one Newton step. Edge crossings are stored ONCE per grid edge so the
//!    two cells sharing an edge reference the same point — this is what makes
//!    the linking exact.
//! 3. **Linking** — within each cell the crossings are paired into segments
//!    (the 16 marching-squares cases; saddles disambiguated by the cell-centre
//!    sign). Segments are then walked end-to-end into polylines: chains that
//!    start at a domain-boundary crossing are OPEN curves; the rest are CLOSED
//!    loops.
//!
//! Unlike predictor-corrector curve tracing, this is **bounded** — O(cells)
//! work, no marching step that can stall or oscillate on a closed/periodic
//! surface (the failure mode that hung the boolean on a lofted barrel). It also
//! recovers multiple disjoint branches and interior loops for free.
//!
//! # References
//!
//! - Patrikalakis, N.M. & Maekawa, T. (2002). *Shape Interrogation for Computer
//!   Aided Design and Manufacturing*. Springer. (§ surface intersection)
//! - Lorensen & Cline (1987), "Marching cubes" — the 2-D specialisation here.
//!
//! Indexed access into the `(Nu × Nv)` signed-distance grid is the canonical
//! idiom — all `grid[i][j]` sites use indices bounded by the sampling grid
//! dimensions established at solver entry.
#![allow(clippy::indexing_slicing)]

use crate::math::{MathError, MathResult, Point3, Tolerance, Vector3};
use crate::primitives::surface::Surface;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Configuration for surface-plane intersection computation.
#[derive(Debug, Clone)]
pub struct SurfacePlaneIntersectionConfig {
    /// Geometric distance tolerance for convergence checks.
    pub tolerance: Tolerance,
    /// Subdivisions along each parameter axis for the signed-distance grid.
    /// Higher resolves finer features at O(n²) evaluations. The contour is
    /// exact at grid resolution; features thinner than one cell can be missed,
    /// so callers with a tight known feature size should raise this.
    pub grid_resolution: usize,
    /// Retained for API compatibility; the marching-squares core does not march
    /// in fixed steps, so this is unused by the contour extractor.
    pub marching_step: f64,
    /// Hard cap on the number of intersection curves returned.
    pub max_curves: usize,
    /// Optional override for the `(u, v)` rectangle searched, as
    /// `((u_min, u_max), (v_min, v_max))`. When `None`, uses
    /// `surface.parameter_bounds()` clamped to `±1e6`.
    pub param_bounds_override: Option<((f64, f64), (f64, f64))>,
}

impl Default for SurfacePlaneIntersectionConfig {
    fn default() -> Self {
        Self {
            tolerance: Tolerance::default(),
            grid_resolution: 48,
            marching_step: 0.01,
            max_curves: 64,
            param_bounds_override: None,
        }
    }
}

/// A single point on the intersection curve, carrying 3-D position and the
/// surface parameter coordinates where the intersection was found.
#[derive(Debug, Clone, Copy)]
pub struct ParametricIntersectionPoint {
    pub position: Point3,
    pub u: f64,
    pub v: f64,
}

/// An ordered, possibly closed, intersection curve.
#[derive(Debug, Clone)]
pub struct ParametricIntersectionCurve {
    pub points: Vec<ParametricIntersectionPoint>,
    /// `true` when the last point connects back to the first within tolerance.
    pub is_closed: bool,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Compute intersection curves between an arbitrary parametric surface and a
/// plane defined by `(plane_origin, plane_normal)`.
///
/// Returns one [`ParametricIntersectionCurve`] per connected branch of the
/// intersection; an empty vector when the surface does not meet the plane.
///
/// # Errors
/// * `MathError::InvalidParameter` — `plane_normal` is zero-length.
pub fn intersect_surface_plane(
    surface: &dyn Surface,
    plane_origin: Point3,
    plane_normal: Vector3,
    config: &SurfacePlaneIntersectionConfig,
) -> MathResult<Vec<ParametricIntersectionCurve>> {
    let normal = plane_normal
        .normalize()
        .map_err(|_| MathError::InvalidParameter("plane_normal must be non-zero".into()))?;

    let ((raw_u_min, raw_u_max), (raw_v_min, raw_v_max)) = config
        .param_bounds_override
        .unwrap_or_else(|| surface.parameter_bounds());

    let clamp_bound = 1e6;
    let u_min = raw_u_min.max(-clamp_bound);
    let u_max = raw_u_max.min(clamp_bound);
    let v_min = raw_v_min.max(-clamp_bound);
    let v_max = raw_v_max.min(clamp_bound);

    if u_min >= u_max || v_min >= v_max {
        return Ok(Vec::new());
    }

    let n = config.grid_resolution.max(2);
    let du = (u_max - u_min) / n as f64;
    let dv = (v_max - v_min) / n as f64;

    // Step 1 — signed-distance grid.
    let mut grid = vec![vec![0.0_f64; n + 1]; n + 1];
    for i in 0..=n {
        let u = u_min + i as f64 * du;
        for j in 0..=n {
            let v = v_min + j as f64 * dv;
            let pos = surface.point_at(u, v)?;
            grid[i][j] = (pos - plane_origin).dot(&normal);
        }
    }

    // Steps 2+3 — marching-squares contour extraction.
    let mut curves = contour_zero_set(
        surface,
        &normal,
        plane_origin,
        &grid,
        u_min,
        du,
        v_min,
        dv,
        n,
        config,
    );
    if curves.len() > config.max_curves {
        curves.truncate(config.max_curves);
    }
    Ok(curves)
}

// ---------------------------------------------------------------------------
// Marching-squares contour extraction
// ---------------------------------------------------------------------------

/// Orientation tag for a grid edge: `H` is the horizontal edge from `(i,j)` to
/// `(i+1,j)`, `V` the vertical edge from `(i,j)` to `(i,j+1)`.
type EdgeKey = (u8, usize, usize);
const H: u8 = 0;
const V: u8 = 1;

#[allow(clippy::too_many_arguments)]
fn contour_zero_set(
    surface: &dyn Surface,
    normal: &Vector3,
    plane_origin: Point3,
    grid: &[Vec<f64>],
    u_min: f64,
    du: f64,
    v_min: f64,
    dv: f64,
    n: usize,
    config: &SurfacePlaneIntersectionConfig,
) -> Vec<ParametricIntersectionCurve> {
    // --- Pass 1: one crossing point per grid edge that changes sign. ---
    let mut nodes: Vec<ParametricIntersectionPoint> = Vec::new();
    let mut node_of_edge: HashMap<EdgeKey, usize> = HashMap::new();

    let uv = |i: usize, j: usize| (u_min + i as f64 * du, v_min + j as f64 * dv);

    // Horizontal edges: (i,j)->(i+1,j).
    for i in 0..n {
        for j in 0..=n {
            let da = grid[i][j];
            let db = grid[i + 1][j];
            if !crosses(da, db) {
                continue;
            }
            let (ua, va) = uv(i, j);
            let (ub, _vb) = uv(i + 1, j);
            let t = lerp_t(da, db);
            let (us, vs) = (ua + t * (ub - ua), va);
            if let Some(node) = make_node(surface, normal, plane_origin, us, vs, config) {
                let id = nodes.len();
                nodes.push(node);
                node_of_edge.insert((H, i, j), id);
            }
        }
    }
    // Vertical edges: (i,j)->(i,j+1).
    for i in 0..=n {
        for j in 0..n {
            let da = grid[i][j];
            let db = grid[i][j + 1];
            if !crosses(da, db) {
                continue;
            }
            let (ua, va) = uv(i, j);
            let (_ub, vb) = uv(i, j + 1);
            let t = lerp_t(da, db);
            let (us, vs) = (ua, va + t * (vb - va));
            if let Some(node) = make_node(surface, normal, plane_origin, us, vs, config) {
                let id = nodes.len();
                nodes.push(node);
                node_of_edge.insert((V, i, j), id);
            }
        }
    }

    if nodes.is_empty() {
        return Vec::new();
    }

    // --- Pass 2: pair crossings within each cell into adjacency. ---
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    let mut connect = |a: usize, b: usize, adj: &mut Vec<Vec<usize>>| {
        if a != b {
            adj[a].push(b);
            adj[b].push(a);
        }
    };
    for i in 0..n {
        for j in 0..n {
            // Cell edges.
            let b = node_of_edge.get(&(H, i, j)).copied();
            let t = node_of_edge.get(&(H, i, j + 1)).copied();
            let l = node_of_edge.get(&(V, i, j)).copied();
            let r = node_of_edge.get(&(V, i + 1, j)).copied();

            let present: Vec<usize> = [b, r, t, l].into_iter().flatten().collect();
            match present.len() {
                0 => {}
                2 => connect(present[0], present[1], &mut adj),
                4 => {
                    // Saddle — disambiguate by the cell-centre sign.
                    let centre =
                        0.25 * (grid[i][j] + grid[i + 1][j] + grid[i][j + 1] + grid[i + 1][j + 1]);
                    // Unwraps guarded: all four are Some here.
                    let (bb, rr, tt, ll) = (
                        b.unwrap_or(present[0]),
                        r.unwrap_or(present[0]),
                        t.unwrap_or(present[0]),
                        l.unwrap_or(present[0]),
                    );
                    if centre < 0.0 {
                        connect(bb, ll, &mut adj);
                        connect(tt, rr, &mut adj);
                    } else {
                        connect(bb, rr, &mut adj);
                        connect(tt, ll, &mut adj);
                    }
                }
                // 1 or 3 crossings: a corner sits exactly on the plane (d == 0)
                // so an edge pair is degenerate. Connect the available pair if
                // two exist; a lone crossing is a dangling node (left as a
                // degree-≤1 endpoint, harmless to the walk).
                _ => {
                    if present.len() >= 2 {
                        connect(present[0], present[1], &mut adj);
                    }
                }
            }
        }
    }

    // --- Pass 3: walk adjacency into polylines (open chains first). ---
    let mut curves: Vec<ParametricIntersectionCurve> = Vec::new();

    // Open chains start at a node that currently has a single connection
    // (a contour terminating on the domain boundary).
    loop {
        let start = (0..adj.len()).find(|&k| adj[k].len() == 1);
        let start = match start {
            Some(s) => s,
            None => break,
        };
        let chain = walk_chain(start, &mut adj);
        push_curve(&mut curves, &nodes, &chain, config.tolerance);
    }
    // Remaining connected nodes form closed loops.
    loop {
        let start = (0..adj.len()).find(|&k| !adj[k].is_empty());
        let start = match start {
            Some(s) => s,
            None => break,
        };
        let chain = walk_chain(start, &mut adj);
        push_curve(&mut curves, &nodes, &chain, config.tolerance);
    }

    curves
}

/// A sign change (or a touch where exactly one endpoint is on the plane).
fn crosses(da: f64, db: f64) -> bool {
    (da < 0.0) != (db < 0.0)
}

/// Interpolation parameter of the zero-crossing along an edge `[da, db]`.
fn lerp_t(da: f64, db: f64) -> f64 {
    let denom = da - db;
    if denom.abs() < 1e-30 {
        0.5
    } else {
        (da / denom).clamp(0.0, 1.0)
    }
}

/// Build a contour node at `(u, v)`, snapping onto `d = 0` with Newton when
/// possible and falling back to the raw surface point otherwise.
fn make_node(
    surface: &dyn Surface,
    normal: &Vector3,
    plane_origin: Point3,
    u: f64,
    v: f64,
    config: &SurfacePlaneIntersectionConfig,
) -> Option<ParametricIntersectionPoint> {
    if let Some(refined) = newton_correct(surface, normal, plane_origin, u, v, config) {
        return Some(refined);
    }
    surface
        .point_at(u, v)
        .ok()
        .map(|position| ParametricIntersectionPoint { position, u, v })
}

/// Walk a chain from `start`, consuming connections, until a dead end or a
/// return to `start` (closed loop). Returns the ordered node ids.
fn walk_chain(start: usize, adj: &mut [Vec<usize>]) -> Vec<usize> {
    let mut chain = vec![start];
    let mut cur = start;
    loop {
        let next = match adj[cur].first().copied() {
            Some(nx) => nx,
            None => break,
        };
        remove_first(&mut adj[cur], next);
        remove_first(&mut adj[next], cur);
        chain.push(next);
        cur = next;
        if cur == start {
            break; // closed loop
        }
    }
    chain
}

fn remove_first(v: &mut Vec<usize>, x: usize) {
    if let Some(p) = v.iter().position(|&y| y == x) {
        v.swap_remove(p);
    }
}

/// Append a curve built from a node-id chain (if it has ≥2 distinct points).
fn push_curve(
    curves: &mut Vec<ParametricIntersectionCurve>,
    nodes: &[ParametricIntersectionPoint],
    chain: &[usize],
    tolerance: Tolerance,
) {
    if chain.len() < 2 {
        return;
    }
    let closed = chain.len() >= 4 && chain.first() == chain.last();
    let slice = if closed {
        &chain[..chain.len() - 1]
    } else {
        chain
    };
    if slice.len() < 2 {
        return;
    }
    let pts: Vec<ParametricIntersectionPoint> = slice.iter().map(|&id| nodes[id]).collect();
    // Reject a hair-thin degenerate chain (all points coincident).
    let span = (pts[pts.len() - 1].position - pts[0].position).magnitude();
    if !closed && span < tolerance.distance() {
        return;
    }
    curves.push(ParametricIntersectionCurve {
        points: pts,
        is_closed: closed,
    });
}

// ---------------------------------------------------------------------------
// Newton corrector
// ---------------------------------------------------------------------------

/// Newton-Raphson corrector: from an approximate `(u, v)`, iterate until
/// `d(u,v) = 0` within tolerance. Returns `None` if derivatives are degenerate.
fn newton_correct(
    surface: &dyn Surface,
    normal: &Vector3,
    plane_origin: Point3,
    mut u: f64,
    mut v: f64,
    config: &SurfacePlaneIntersectionConfig,
) -> Option<ParametricIntersectionPoint> {
    let tol = config.tolerance.distance();
    let max_iter = 16;
    let ((raw_u_min, raw_u_max), (raw_v_min, raw_v_max)) = config
        .param_bounds_override
        .unwrap_or_else(|| surface.parameter_bounds());
    let clamp_bound = 1e6;
    let u_min = raw_u_min.max(-clamp_bound);
    let u_max = raw_u_max.min(clamp_bound);
    let v_min = raw_v_min.max(-clamp_bound);
    let v_max = raw_v_max.min(clamp_bound);

    for _ in 0..max_iter {
        let eval = surface.evaluate_full(u, v).ok()?;
        let d = (eval.position - plane_origin).dot(normal);
        if d.abs() < tol {
            return Some(ParametricIntersectionPoint {
                position: eval.position,
                u,
                v,
            });
        }
        let grad_u = eval.du.dot(normal);
        let grad_v = eval.dv.dot(normal);
        let grad_mag_sq = grad_u * grad_u + grad_v * grad_v;
        if grad_mag_sq < 1e-30 {
            return None; // tangent plane parallel to cutting plane
        }
        let scale = -d / grad_mag_sq;
        u = (u + scale * grad_u).clamp(u_min, u_max);
        v = (v + scale * grad_v).clamp(v_min, v_max);
    }

    let pos = surface.point_at(u, v).ok()?;
    let d = (pos - plane_origin).dot(normal);
    if d.abs() < tol * 100.0 {
        Some(ParametricIntersectionPoint {
            position: pos,
            u,
            v,
        })
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Point3, Tolerance, Vector3};
    use crate::primitives::surface::Plane as SurfacePlane;

    /// Intersect a tilted plane surface with a horizontal cutting plane.
    #[test]
    fn test_plane_plane_intersection() {
        let s2 = std::f64::consts::FRAC_1_SQRT_2;
        let surface = SurfacePlane::new_bounded(
            Point3::ZERO,
            Vector3::new(s2, 0.0, s2),
            Vector3::new(s2, 0.0, -s2),
            (-5.0, 5.0),
            (-5.0, 5.0),
        )
        .expect("plane construction should succeed");

        let config = SurfacePlaneIntersectionConfig {
            tolerance: Tolerance::from_distance(1e-8),
            grid_resolution: 20,
            marching_step: 0.02,
            max_curves: 10,
            ..Default::default()
        };

        let curves = intersect_surface_plane(&surface, Point3::ZERO, Vector3::Z, &config)
            .expect("intersection should succeed");

        assert!(
            !curves.is_empty(),
            "expected at least one intersection curve"
        );
        for curve in &curves {
            for pt in &curve.points {
                assert!(
                    pt.position.z.abs() < 1e-6,
                    "point z = {} should be ~0",
                    pt.position.z
                );
            }
        }
    }

    /// A surface entirely above the cutting plane yields no intersection.
    #[test]
    fn test_no_intersection() {
        let surface = SurfacePlane::new_bounded(
            Point3::new(0.0, 0.0, 10.0),
            Vector3::Z,
            Vector3::X,
            (-5.0, 5.0),
            (-5.0, 5.0),
        )
        .expect("plane construction should succeed");

        let config = SurfacePlaneIntersectionConfig::default();
        let curves = intersect_surface_plane(&surface, Point3::ZERO, Vector3::Z, &config)
            .expect("should succeed with empty result");
        assert!(curves.is_empty(), "no intersection expected");
    }

    /// Zero-length plane normal must return an error.
    #[test]
    fn test_zero_normal_error() {
        let surface = SurfacePlane::new_bounded(
            Point3::ZERO,
            Vector3::Z,
            Vector3::X,
            (-1.0, 1.0),
            (-1.0, 1.0),
        )
        .expect("plane construction should succeed");
        let config = SurfacePlaneIntersectionConfig::default();
        let result = intersect_surface_plane(&surface, Point3::ZERO, Vector3::ZERO, &config);
        assert!(result.is_err(), "zero normal should produce an error");
    }

    /// Boundedness guard: a tilted plane fully crossing the cutting plane must
    /// return quickly with a bounded number of points — the regression test for
    /// the predictor-corrector stall that hung the boolean. Marching-squares is
    /// O(cells), so the point count cannot exceed a few per cell.
    #[test]
    fn test_contour_is_bounded() {
        let s2 = std::f64::consts::FRAC_1_SQRT_2;
        let surface = SurfacePlane::new_bounded(
            Point3::ZERO,
            Vector3::new(s2, 0.0, s2),
            Vector3::new(s2, 0.0, -s2),
            (-5.0, 5.0),
            (-5.0, 5.0),
        )
        .expect("plane");
        let config = SurfacePlaneIntersectionConfig {
            tolerance: Tolerance::from_distance(1e-7),
            grid_resolution: 32,
            marching_step: 0.01,
            max_curves: 16,
            param_bounds_override: None,
        };
        let curves = intersect_surface_plane(&surface, Point3::ZERO, Vector3::Z, &config)
            .expect("section should succeed");
        assert!(!curves.is_empty(), "expected an intersection");
        let total: usize = curves.iter().map(|c| c.points.len()).sum();
        // Far below any unbounded-marching blowup: O(grid_resolution) per branch.
        assert!(
            total < 16 * 64,
            "contour must be bounded, got {total} points"
        );
    }
}
