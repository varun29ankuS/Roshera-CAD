//! Projection Operations for B-Rep Models
//!
//! Projects points, curves, and other entities onto surfaces and faces.
//!
//! Indexed access into projection sample arrays is the canonical idiom —
//! bounded by sample count. Matches the pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Point3, Vector3};
use crate::primitives::{
    curve::Curve,
    edge::EdgeId,
    face::{Face, FaceId},
    surface::Surface,
    topology_builder::BRepModel,
    vertex::VertexId,
};

/// Options for projection operations
#[derive(Debug, Clone)]
pub struct ProjectionOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Projection direction (None for closest point)
    pub direction: Option<Vector3>,

    /// Maximum projection distance
    pub max_distance: Option<f64>,

    /// Whether to project both directions
    pub bidirectional: bool,

    /// Number of samples for curve projection
    pub curve_samples: u32,
}

impl Default for ProjectionOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            direction: None,
            max_distance: None,
            bidirectional: false,
            curve_samples: 20,
        }
    }
}

/// Result of point projection
#[derive(Debug, Clone)]
pub struct ProjectedPoint {
    /// 3D position on surface
    pub position: Point3,
    /// UV parameters on surface
    pub uv: (f64, f64),
    /// Distance from original point
    pub distance: f64,
    /// Surface normal at projection
    pub normal: Vector3,
}

/// Result of curve projection
#[derive(Debug)]
pub struct ProjectedCurve {
    /// 3D curve on surface
    pub curve_3d: Box<dyn Curve>,
    /// UV curve on surface
    pub curve_uv: Vec<(f64, f64)>,
    /// Original curve parameter to UV mapping
    pub parameter_map: Vec<(f64, f64, f64)>, // (t_original, u, v)
}

/// Project a point onto a face
pub fn project_point_on_face(
    model: &BRepModel,
    point: Point3,
    face_id: FaceId,
    options: &ProjectionOptions,
) -> OperationResult<ProjectedPoint> {
    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

    let surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    let projected = if let Some(direction) = options.direction {
        // Directional projection
        project_point_directional(point, surface, direction, options)?
    } else {
        // Closest point projection
        project_point_closest(point, surface)?
    };

    // Check if projection is within face boundaries
    if !point_in_face_bounds(model, &projected, face)? {
        return Err(OperationError::InvalidGeometry(
            "Projected point is outside face boundaries".to_string(),
        ));
    }

    Ok(projected)
}

/// Project a curve onto a face
pub fn project_curve_on_face(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face_id: FaceId,
    options: &ProjectionOptions,
) -> OperationResult<ProjectedCurve> {
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

    let curve = model
        .curves
        .get(edge.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;

    let surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    // Sample points along curve
    let mut projected_points_3d = Vec::new();
    let mut projected_points_uv = Vec::new();
    let mut parameter_map = Vec::new();

    for i in 0..=options.curve_samples {
        let t = i as f64 / options.curve_samples as f64;
        let curve_t = edge.edge_to_curve_parameter(t);
        let point = curve.point_at(curve_t)?;

        // Project point
        let projected = if let Some(direction) = options.direction {
            project_point_directional(point, surface, direction, options)?
        } else {
            project_point_closest(point, surface)?
        };

        projected_points_3d.push(projected.position);
        projected_points_uv.push(projected.uv);
        parameter_map.push((t, projected.uv.0, projected.uv.1));
    }

    // Create interpolated 3D curve
    let curve_3d = create_interpolated_curve_3d(&projected_points_3d)?;

    Ok(ProjectedCurve {
        curve_3d,
        curve_uv: projected_points_uv,
        parameter_map,
    })
}

/// Project multiple points onto a face
pub fn project_points_on_face(
    model: &BRepModel,
    points: &[Point3],
    face_id: FaceId,
    options: &ProjectionOptions,
) -> OperationResult<Vec<ProjectedPoint>> {
    let mut projected = Vec::new();

    for &point in points {
        match project_point_on_face(model, point, face_id, options) {
            Ok(proj) => projected.push(proj),
            Err(_) => {} // Skip points that can't be projected
        }
    }

    Ok(projected)
}

/// Project a point onto a surface by closest-point search.
///
/// First a 20×20 uniform grid samples the parameter domain to seed the
/// search; the best seed is then refined with Newton iteration on the
/// orthogonality conditions
/// `f₁ = (S(u,v) - P) · S_u = 0` and `f₂ = (S(u,v) - P) · S_v = 0`,
/// which are the necessary conditions for `|S(u,v) - P|²` to be a
/// stationary point. The Jacobian is approximated by the first
/// fundamental form `[E F; F G]` (Gauss-Newton-style; this is exact
/// for the orthogonality system at the converged point and stable
/// against the curvature term that pure Newton struggles with near
/// flat regions).
///
/// Returns the converged point and its (u,v); falls back to the grid
/// best when Newton stalls or the linearised system is singular.
fn project_point_closest(point: Point3, surface: &dyn Surface) -> OperationResult<ProjectedPoint> {
    let bounds = surface.parameter_bounds();
    let (u_min, u_max) = bounds.0;
    let (v_min, v_max) = bounds.1;
    let u_span = (u_max - u_min).max(1e-12);
    let v_span = (v_max - v_min).max(1e-12);

    // Stage 1: uniform grid seed.
    const SAMPLES: usize = 20;
    let mut best_distance = f64::MAX;
    let mut best_u = u_min;
    let mut best_v = v_min;
    for i in 0..=SAMPLES {
        for j in 0..=SAMPLES {
            let u = u_min + (i as f64 / SAMPLES as f64) * u_span;
            let v = v_min + (j as f64 / SAMPLES as f64) * v_span;
            let surface_point = surface.point_at(u, v)?;
            let d = point.distance(&surface_point);
            if d < best_distance {
                best_distance = d;
                best_u = u;
                best_v = v;
            }
        }
    }

    // Stage 2: Gauss-Newton refinement on the orthogonality conditions.
    let tolerance = crate::math::Tolerance::default();
    let pos_tol = tolerance.distance().max(1e-10);
    const MAX_ITERS: usize = 32;
    let mut u = best_u;
    let mut v = best_v;
    for _ in 0..MAX_ITERS {
        let s_pt = match surface.point_at(u, v) {
            Ok(p) => p,
            Err(_) => break,
        };
        let d_vec = s_pt - point;
        let (su, sv) = match surface.derivatives_at(u, v) {
            Ok(pair) => pair,
            Err(_) => break,
        };
        let f1 = d_vec.dot(&su);
        let f2 = d_vec.dot(&sv);
        if f1.abs() < pos_tol && f2.abs() < pos_tol {
            break;
        }
        // First fundamental form coefficients.
        let e = su.dot(&su);
        let f = su.dot(&sv);
        let g = sv.dot(&sv);
        let det = e * g - f * f;
        if det.abs() < 1e-18 {
            break;
        }
        let du = (-g * f1 + f * f2) / det;
        let dv = (f * f1 - e * f2) / det;
        let new_u = (u + du).clamp(u_min, u_max);
        let new_v = (v + dv).clamp(v_min, v_max);
        if (new_u - u).abs() < 1e-14 && (new_v - v).abs() < 1e-14 {
            u = new_u;
            v = new_v;
            break;
        }
        u = new_u;
        v = new_v;
    }

    let position = surface.point_at(u, v)?;
    let distance = point.distance(&position);
    let normal = surface.normal_at(u, v)?;

    Ok(ProjectedPoint {
        position,
        uv: (u, v),
        distance,
        normal,
    })
}

/// Project point using directional method
#[allow(clippy::expect_used)] // intersections non-empty: is_empty() early-return above
fn project_point_directional(
    point: Point3,
    surface: &dyn Surface,
    direction: Vector3,
    options: &ProjectionOptions,
) -> OperationResult<ProjectedPoint> {
    let direction = direction.normalize()?;

    // Ray-surface intersection
    let intersections = ray_surface_intersections(
        point,
        direction,
        surface,
        options.max_distance,
        options.bidirectional,
    )?;

    if intersections.is_empty() {
        return Err(OperationError::InvalidGeometry(
            "No intersection found in projection direction".to_string(),
        ));
    }

    // Use closest intersection. NaN-safe ordering; `intersections` is
    // guaranteed non-empty by the early-return check above, so `min_by`
    // returns `Some` here.
    let closest = intersections
        .into_iter()
        .min_by(|a, b| {
            a.distance
                .partial_cmp(&b.distance)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .expect("intersections verified non-empty above (is_empty early-return)");

    Ok(closest)
}

/// Find ray-surface intersections
fn ray_surface_intersections(
    origin: Point3,
    direction: Vector3,
    surface: &dyn Surface,
    max_distance: Option<f64>,
    bidirectional: bool,
) -> OperationResult<Vec<ProjectedPoint>> {
    let mut intersections = Vec::new();

    // Forward direction
    if let Some(intersection) = ray_surface_intersection(origin, direction, surface, max_distance)?
    {
        intersections.push(intersection);
    }

    // Backward direction if requested
    if bidirectional {
        if let Some(intersection) =
            ray_surface_intersection(origin, -direction, surface, max_distance)?
        {
            intersections.push(intersection);
        }
    }

    Ok(intersections)
}

/// Find single ray-surface intersection
fn ray_surface_intersection(
    origin: Point3,
    direction: Vector3,
    surface: &dyn Surface,
    max_distance: Option<f64>,
) -> OperationResult<Option<ProjectedPoint>> {
    use crate::primitives::surface::SurfaceType;

    match surface.surface_type() {
        SurfaceType::Plane => ray_plane_intersection(origin, direction, surface, max_distance),
        SurfaceType::Cylinder => {
            ray_cylinder_intersection(origin, direction, surface, max_distance)
        }
        SurfaceType::Sphere => ray_sphere_intersection(origin, direction, surface, max_distance),
        _ => {
            // General surface - use iterative method
            ray_general_surface_intersection(origin, direction, surface, max_distance)
        }
    }
}

/// Ray-plane intersection
fn ray_plane_intersection(
    origin: Point3,
    direction: Vector3,
    plane: &dyn Surface,
    max_distance: Option<f64>,
) -> OperationResult<Option<ProjectedPoint>> {
    // Get plane normal and point
    let normal = plane.normal_at(0.5, 0.5)?;
    let point_on_plane = plane.point_at(0.5, 0.5)?;

    // Ray-plane intersection formula
    let denom = direction.dot(&normal);
    if denom.abs() < 1e-10 {
        // Ray parallel to plane
        return Ok(None);
    }

    let t = (point_on_plane - origin).dot(&normal) / denom;

    if t < 0.0 {
        return Ok(None);
    }

    if let Some(max_dist) = max_distance {
        if t > max_dist {
            return Ok(None);
        }
    }

    let position = origin + direction * t;
    // Recover the plane's (u,v) coordinates of the hit point via the
    // surface's exact closest-point query. For an unbounded plane this
    // is the orthogonal projection and is exact in a single Newton step.
    let tolerance = crate::math::Tolerance::default();
    let (u, v) = plane
        .closest_point(&position, tolerance)
        .unwrap_or((0.0, 0.0));

    Ok(Some(ProjectedPoint {
        position,
        uv: (u, v),
        distance: t,
        normal,
    }))
}

/// Ray-cylinder intersection
fn ray_cylinder_intersection(
    origin: Point3,
    direction: Vector3,
    cylinder: &dyn Surface,
    max_distance: Option<f64>,
) -> OperationResult<Option<ProjectedPoint>> {
    use crate::primitives::surface::Cylinder;

    // Downcast to the analytic Cylinder; non-cylindrical Surface impls
    // that report SurfaceType::Cylinder are not expected, but if one
    // arrives we fall back to the general iterative path.
    let cyl = match cylinder.as_any().downcast_ref::<Cylinder>() {
        Some(c) => c,
        None => {
            return ray_general_surface_intersection(origin, direction, cylinder, max_distance);
        }
    };

    // Project ray and origin into the cylinder's axis-perpendicular plane.
    let q = origin - cyl.origin;
    let d_axis = direction.dot(&cyl.axis);
    let q_axis = q.dot(&cyl.axis);
    let d_perp = direction - cyl.axis * d_axis;
    let q_perp = q - cyl.axis * q_axis;

    // Solve |q_perp + t·d_perp|² = r²
    let a = d_perp.dot(&d_perp);
    if a < 1e-20 {
        // Ray is parallel to the cylinder axis — never crosses the surface.
        return Ok(None);
    }
    let b = 2.0 * q_perp.dot(&d_perp);
    let c = q_perp.dot(&q_perp) - cyl.radius * cyl.radius;
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return Ok(None);
    }
    let sqrt_disc = disc.sqrt();
    let t0 = (-b - sqrt_disc) / (2.0 * a);
    let t1 = (-b + sqrt_disc) / (2.0 * a);

    // Pick the closest non-negative root, honouring max_distance.
    let mut best_t = f64::INFINITY;
    for &t in &[t0, t1] {
        if t < 0.0 {
            continue;
        }
        if let Some(max_dist) = max_distance {
            if t > max_dist {
                continue;
            }
        }
        // Check axial limits if the cylinder is finite.
        if let Some([h_min, h_max]) = cyl.height_limits {
            let h = q_axis + t * d_axis;
            if h < h_min || h > h_max {
                continue;
            }
        }
        if t < best_t {
            best_t = t;
        }
    }
    if !best_t.is_finite() {
        return Ok(None);
    }

    let position = origin + direction * best_t;
    let normal = cylinder.normal_at(0.0, 0.0).ok().unwrap_or(cyl.axis);
    let tolerance = crate::math::Tolerance::default();
    let (u, v) = cylinder
        .closest_point(&position, tolerance)
        .unwrap_or((0.0, 0.0));
    let actual_normal = cylinder.normal_at(u, v).unwrap_or(normal);

    Ok(Some(ProjectedPoint {
        position,
        uv: (u, v),
        distance: best_t,
        normal: actual_normal,
    }))
}

/// Analytical ray-sphere intersection.
///
/// Solves `|(origin + t·direction) - center|² = radius²` for the
/// smallest non-negative `t` that also respects `max_distance`. Falls
/// back to the iterative general-surface path when the supplied
/// `Surface` impl reports `SurfaceType::Sphere` but isn't an analytical
/// `Sphere` (defensive — no current Surface impl forges that report).
fn ray_sphere_intersection(
    origin: Point3,
    direction: Vector3,
    sphere: &dyn Surface,
    max_distance: Option<f64>,
) -> OperationResult<Option<ProjectedPoint>> {
    use crate::primitives::surface::Sphere;

    let sph = match sphere.as_any().downcast_ref::<Sphere>() {
        Some(s) => s,
        None => return ray_general_surface_intersection(origin, direction, sphere, max_distance),
    };

    let q = origin - sph.center;
    let a = direction.dot(&direction);
    if a < 1e-20 {
        return Ok(None);
    }
    let b = 2.0 * q.dot(&direction);
    let c = q.dot(&q) - sph.radius * sph.radius;
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return Ok(None);
    }
    let sqrt_disc = disc.sqrt();
    let t0 = (-b - sqrt_disc) / (2.0 * a);
    let t1 = (-b + sqrt_disc) / (2.0 * a);

    let mut best_t = f64::INFINITY;
    for &t in &[t0, t1] {
        if t < 0.0 {
            continue;
        }
        if let Some(max_dist) = max_distance {
            if t > max_dist {
                continue;
            }
        }
        if t < best_t {
            best_t = t;
        }
    }
    if !best_t.is_finite() {
        return Ok(None);
    }

    let position = origin + direction * best_t;
    let tolerance = crate::math::Tolerance::default();
    let (u, v) = sphere
        .closest_point(&position, tolerance)
        .unwrap_or((0.0, 0.0));
    let normal = sphere
        .normal_at(u, v)
        .or_else(|_| (position - sph.center).normalize())
        .unwrap_or(Vector3::Z);

    Ok(Some(ProjectedPoint {
        position,
        uv: (u, v),
        distance: best_t,
        normal,
    }))
}

/// Ray-general surface intersection.
///
/// Uniform-grid seeding followed by Newton-Raphson on the 3-equation
/// system `S(u,v) - (O + t·D) = 0`, with Jacobian columns `[S_u, S_v,
/// -D]` solved via Gaussian elimination. Returns the closest valid hit
/// (smallest `t ≥ 0` within `max_distance`) across all converged seeds,
/// `Ok(None)` when nothing converges. Used as the fallback for
/// surfaces lacking an analytical specialization (NURBS patches, swept,
/// trimmed, etc.).
fn ray_general_surface_intersection(
    origin: Point3,
    direction: Vector3,
    surface: &dyn Surface,
    max_distance: Option<f64>,
) -> OperationResult<Option<ProjectedPoint>> {
    use crate::math::linear_solver::gaussian_elimination;

    let bounds = surface.parameter_bounds();
    let (u_min, u_max) = bounds.0;
    let (v_min, v_max) = bounds.1;
    let u_span = (u_max - u_min).max(1e-12);
    let v_span = (v_max - v_min).max(1e-12);

    const SEED_GRID: usize = 8;
    const MAX_NEWTON_ITERS: usize = 24;
    let tolerance = crate::math::Tolerance::default();
    let pos_tol = tolerance.distance().max(1e-9);

    let mut best: Option<(f64, f64, f64, Point3)> = None; // (t, u, v, position)

    for i in 0..=SEED_GRID {
        for j in 0..=SEED_GRID {
            let mut u = u_min + (i as f64 / SEED_GRID as f64) * u_span;
            let mut v = v_min + (j as f64 / SEED_GRID as f64) * v_span;
            // Initial t seeded from the projection onto the ray of the
            // sample point — a single dot product gives a reasonable
            // start for Newton.
            let mut s_pt = match surface.point_at(u, v) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let mut t = (s_pt - origin).dot(&direction) / direction.dot(&direction).max(1e-20);

            let mut converged = false;
            for _ in 0..MAX_NEWTON_ITERS {
                let ray_pt = origin + direction * t;
                let f = s_pt - ray_pt; // residual: surface - ray
                if f.magnitude() < pos_tol {
                    converged = true;
                    break;
                }
                let (su, sv) = match surface.derivatives_at(u, v) {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                // Solve [S_u  S_v  -D] · [du dv dt]^T = -f
                let a_mat = vec![
                    vec![su.x, sv.x, -direction.x],
                    vec![su.y, sv.y, -direction.y],
                    vec![su.z, sv.z, -direction.z],
                ];
                let b_vec = vec![-f.x, -f.y, -f.z];
                let solved = match gaussian_elimination(a_mat, b_vec, tolerance) {
                    Ok(x) => x,
                    Err(_) => break,
                };
                u = (u + solved[0]).clamp(u_min, u_max);
                v = (v + solved[1]).clamp(v_min, v_max);
                t += solved[2];
                s_pt = match surface.point_at(u, v) {
                    Ok(p) => p,
                    Err(_) => break,
                };
            }

            if !converged {
                continue;
            }
            if t < 0.0 {
                continue;
            }
            if let Some(max_dist) = max_distance {
                if t > max_dist {
                    continue;
                }
            }
            let position = origin + direction * t;
            match &best {
                None => best = Some((t, u, v, position)),
                Some((bt, _, _, _)) if t < *bt => best = Some((t, u, v, position)),
                _ => {}
            }
        }
    }

    let Some((t, u, v, position)) = best else {
        return Ok(None);
    };
    let normal = surface.normal_at(u, v).unwrap_or(Vector3::Z);
    Ok(Some(ProjectedPoint {
        position,
        uv: (u, v),
        distance: t,
        normal,
    }))
}

/// Check if projected point is within face boundaries
fn point_in_face_bounds(
    model: &BRepModel,
    projected: &ProjectedPoint,
    face: &Face,
) -> OperationResult<bool> {
    // Check if UV point is within face trimming loops
    face.contains_point(
        projected.uv.0,
        projected.uv.1,
        &model.loops,
        &model.vertices,
        &model.edges,
        &model.surfaces,
    )
    .map_err(|e| {
        OperationError::InvalidGeometry(format!("Failed to check point containment: {}", e))
    })
}

/// Create an interpolated 3D curve through projected sample points.
///
/// Two points → exact `Line`. Three or more points → degree-min(3, n-1)
/// clamped NURBS curve fit through the points so the projected curve
/// preserves the curvature of the source curve on the target surface,
/// not just the endpoints.
fn create_interpolated_curve_3d(points: &[Point3]) -> OperationResult<Box<dyn Curve>> {
    use crate::primitives::curve::{Line, NurbsCurve};

    if points.len() < 2 {
        return Err(OperationError::InvalidGeometry(
            "Not enough points for curve interpolation".to_string(),
        ));
    }
    if points.len() == 2 {
        return Ok(Box::new(Line::new(points[0], points[points.len() - 1])));
    }
    let tolerance = crate::math::Tolerance::default();
    let nurbs = NurbsCurve::fit_to_points(points, 3, tolerance.distance()).map_err(|e| {
        OperationError::NumericalError(format!("projected curve fit failed: {:?}", e))
    })?;
    Ok(Box::new(nurbs))
}

/// Project a vertex onto a surface
pub fn project_vertex_on_surface(
    model: &BRepModel,
    vertex_id: VertexId,
    surface_id: u32,
    options: &ProjectionOptions,
) -> OperationResult<ProjectedPoint> {
    let vertex = model
        .vertices
        .get(vertex_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Vertex not found".to_string()))?;

    let surface = model
        .surfaces
        .get(surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    let point = Point3::from(vertex.position);

    if let Some(direction) = options.direction {
        project_point_directional(point, surface, direction, options)
    } else {
        project_point_closest(point, surface)
    }
}

/// Find silhouette curves of a surface from a view direction
pub fn find_silhouette_curves(
    model: &BRepModel,
    face_id: FaceId,
    view_direction: Vector3,
    _options: &ProjectionOptions,
) -> OperationResult<Vec<EdgeId>> {
    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

    let mut silhouette_edges = Vec::new();

    // Get face boundary edges
    let loop_data = model
        .loops
        .get(face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?;

    for &edge_id in &loop_data.edges {
        if is_silhouette_edge(model, edge_id, face_id, view_direction)? {
            silhouette_edges.push(edge_id);
        }
    }

    Ok(silhouette_edges)
}

/// Check if edge is a silhouette edge
fn is_silhouette_edge(
    model: &BRepModel,
    edge_id: EdgeId,
    face_id: FaceId,
    view_direction: Vector3,
) -> OperationResult<bool> {
    // Edge is silhouette if surface normal is perpendicular to view direction
    // along the edge

    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

    let surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    // Sample along the edge and test the surface normal at each sample.
    // The normal is taken at the (u,v) parameters that project the 3D
    // edge point onto the surface, not at a fixed mid-bound point — using
    // the centre would make every edge of every face look like a
    // silhouette, since the normal would be evaluated in the same place
    // for every sample.
    let tolerance = crate::math::Tolerance::default();
    for i in 0..=10 {
        let t = i as f64 / 10.0;
        let point = edge.evaluate(t, &model.curves)?;

        let (u, v) = surface
            .closest_point(&point, tolerance)
            .unwrap_or_else(|_| {
                let bounds = surface.parameter_bounds();
                (
                    (bounds.0 .0 + bounds.0 .1) * 0.5,
                    (bounds.1 .0 + bounds.1 .1) * 0.5,
                )
            });
        let normal = surface.normal_at(u, v)?;

        let dot = normal.dot(&view_direction).abs();
        if dot < 0.1 {
            return Ok(true);
        }
    }

    Ok(false)
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_projection_options() {
//         let options = ProjectionOptions::default();
//         assert!(options.direction.is_none());
//         assert!(!options.bidirectional);
//     }
// }
