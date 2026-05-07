//! Offset and Shell Operations for B-Rep Models
//!
//! Creates offset surfaces and shells (hollowed solids) by moving faces
//! normal to their surface by specified distances.
//!
//! # References
//! - Maekawa, T. (1999). An overview of offset curves and surfaces. CAD.
//! - Pham, B. (1992). Offset curves and surfaces: a brief survey. CAD.
//!
//! Indexed access into offset sample arrays is the canonical idiom —
//! bounded by sample / face count. Matches the pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::{
    edge::{Edge, EdgeId},
    face::{Face, FaceId},
    r#loop::Loop,
    shell::{Shell, ShellType},
    solid::{Solid, SolidId},
    surface::Surface,
    topology_builder::BRepModel,
};

/// Options for offset operations
#[derive(Debug)]
pub struct OffsetOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Type of offset
    pub offset_type: OffsetType,

    /// How to handle self-intersections
    pub intersection_handling: IntersectionHandling,

    /// Maximum deviation for approximations
    pub max_deviation: f64,
}

impl Default for OffsetOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            offset_type: OffsetType::Distance(1.0),
            intersection_handling: IntersectionHandling::Trim,
            max_deviation: 0.001,
        }
    }
}

/// Type of offset
pub enum OffsetType {
    /// Constant distance offset
    Distance(f64),
    /// Different distances per face
    PerFace(std::collections::HashMap<FaceId, f64>),
}

impl std::fmt::Debug for OffsetType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OffsetType::Distance(d) => write!(f, "Distance({})", d),
            OffsetType::PerFace(map) => write!(f, "PerFace({:?})", map),
        }
    }
}

/// How to handle self-intersections in offset
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IntersectionHandling {
    /// Trim self-intersecting regions
    Trim,
    /// Keep all geometry (may be invalid)
    Keep,
    /// Fail if self-intersection occurs
    Fail,
}

/// Offset a single face
pub fn offset_face(
    model: &mut BRepModel,
    face_id: FaceId,
    options: OffsetOptions,
) -> OperationResult<FaceId> {
    // Validate inputs
    validate_offset_face_inputs(model, face_id, &options)?;

    // Get face data
    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?
        .clone();

    // Get offset distance
    let distance = match &options.offset_type {
        OffsetType::Distance(d) => *d,
        OffsetType::PerFace(map) => *map.get(&face_id).unwrap_or(&0.0),
    };

    // Create offset surface
    let offset_surface = create_offset_surface(model, &face, distance)?;
    let surface_id = model.surfaces.add(offset_surface);

    // Create offset edges
    let offset_loop = create_offset_loop(model, &face, distance, &options)?;
    let loop_id = model.loops.add(offset_loop);

    // Create new face
    let offset_face = Face::new(
        0, // ID will be assigned by store
        surface_id,
        loop_id,
        face.orientation,
    );
    let face_id = model.faces.add(offset_face);

    Ok(face_id)
}

/// Create a shell (hollow) from a solid
pub fn offset_solid(
    model: &mut BRepModel,
    solid_id: SolidId,
    thickness: f64,
    faces_to_remove: Vec<FaceId>,
    options: OffsetOptions,
) -> OperationResult<SolidId> {
    // Validate inputs
    validate_shell_inputs(model, solid_id, thickness, &faces_to_remove)?;

    // Get solid data
    let solid = model
        .solids
        .get(solid_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Solid not found".to_string()))?
        .clone();

    // Create offset faces for interior
    let interior_faces =
        create_interior_offset_faces(model, &solid, -thickness.abs(), &faces_to_remove, &options)?;

    // Create side walls for removed faces
    let wall_faces = create_shell_walls(model, &solid, thickness, &faces_to_remove, &options)?;

    // Combine original exterior (minus removed faces) with new interior
    let shell_faces =
        combine_shell_faces(model, &solid, &faces_to_remove, interior_faces, wall_faces)?;

    // Create new shell and solid
    let mut shell = Shell::new(0, ShellType::Closed); // ID will be assigned by store
    for face_id in shell_faces {
        shell.add_face(face_id);
    }
    let shell_id = model.shells.add(shell);

    let hollow_solid = Solid::new(0, shell_id); // ID will be assigned by store
    let hollow_id = model.solids.add(hollow_solid);

    // Validate result if requested
    if options.common.validate_result {
        validate_shell_solid(model, hollow_id)?;
    }

    // Record for attached recorders so the timeline can replay shell
    // operations alongside extrudes / booleans / fillets.
    model.record_operation(
        crate::operations::recorder::RecordedOperation::new("offset_solid")
            .with_parameters(serde_json::json!({
                "solid_id": solid_id,
                "thickness": thickness,
                "faces_to_remove": faces_to_remove,
                "max_deviation": options.max_deviation,
            }))
            .with_inputs(
                std::iter::once(solid_id as u64)
                    .chain(faces_to_remove.iter().map(|&f| f as u64))
                    .collect(),
            )
            .with_outputs(vec![hollow_id as u64]),
    );

    Ok(hollow_id)
}

/// Create offset surface from original surface
fn create_offset_surface(
    model: &mut BRepModel,
    face: &Face,
    distance: f64,
) -> OperationResult<Box<dyn Surface>> {
    let surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    // Create offset based on surface type. Every surviving SurfaceType
    // variant is handled — analytical types use closed-form constructions,
    // SurfaceOfRevolution/Ruled/Offset delegate to the trait `offset()`
    // (which produces an exact result for revolution and a NURBS-grid
    // approximation for ruled), and BSpline/NURBS use the local-rebuild
    // path.
    use crate::primitives::surface::SurfaceType;
    match surface.surface_type() {
        SurfaceType::Plane => create_offset_plane(surface, distance),
        SurfaceType::Cylinder => create_offset_cylinder(surface, distance),
        SurfaceType::Sphere => create_offset_sphere(surface, distance),
        SurfaceType::Cone => create_offset_cone(surface, distance),
        SurfaceType::Torus => create_offset_torus(surface, distance),
        SurfaceType::BSpline => create_offset_bspline(surface, distance),
        SurfaceType::NURBS => create_offset_nurbs(surface, distance),
        SurfaceType::SurfaceOfRevolution | SurfaceType::Ruled | SurfaceType::Offset => {
            Ok(surface.offset(distance))
        }
    }
}

/// Create offset plane
fn create_offset_plane(surface: &dyn Surface, distance: f64) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::surface::Plane;

    // Get plane normal and offset along it
    let normal = surface.normal_at(0.5, 0.5)?;
    let point = surface.point_at(0.5, 0.5)?;
    let offset_point = point + normal * distance;

    // Create new plane at offset position
    let offset_plane = Plane::from_point_normal(offset_point, normal)?;
    Ok(Box::new(offset_plane))
}

/// Create offset cylinder by adjusting radius
fn create_offset_cylinder(
    surface: &dyn Surface,
    distance: f64,
) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::surface::Cylinder;

    let cyl = surface
        .as_any()
        .downcast_ref::<Cylinder>()
        .ok_or_else(|| OperationError::InvalidGeometry("Expected Cylinder surface".into()))?;

    let new_radius = cyl.radius + distance;
    if new_radius <= 0.0 {
        return Err(OperationError::InvalidGeometry(
            "Offset produces zero or negative cylinder radius".into(),
        ));
    }

    let mut offset = Cylinder::new(cyl.origin, cyl.axis, new_radius)?;
    offset.height_limits = cyl.height_limits;
    offset.angle_limits = cyl.angle_limits;
    Ok(Box::new(offset))
}

/// Create offset sphere by adjusting radius
fn create_offset_sphere(surface: &dyn Surface, distance: f64) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::surface::Sphere;

    let sph = surface
        .as_any()
        .downcast_ref::<Sphere>()
        .ok_or_else(|| OperationError::InvalidGeometry("Expected Sphere surface".into()))?;

    let new_radius = sph.radius + distance;
    if new_radius <= 0.0 {
        return Err(OperationError::InvalidGeometry(
            "Offset produces zero or negative sphere radius".into(),
        ));
    }

    let mut offset = Sphere::new(sph.center, new_radius)?;
    offset.param_limits = sph.param_limits;
    Ok(Box::new(offset))
}

/// Create offset cone — offset along normal moves the surface, keeping half angle constant
fn create_offset_cone(surface: &dyn Surface, distance: f64) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::surface::Cone;

    let cone = surface
        .as_any()
        .downcast_ref::<Cone>()
        .ok_or_else(|| OperationError::InvalidGeometry("Expected Cone surface".into()))?;

    // Offsetting a cone along its normal shifts the apex along the axis
    // by distance / sin(half_angle), keeping the half angle constant.
    let shift = distance / cone.half_angle.sin();
    let new_apex = cone.apex + cone.axis * shift;

    let mut offset = Cone::new(new_apex, cone.axis, cone.half_angle)?;
    offset.height_limits = cone.height_limits.map(|[lo, hi]| [lo - shift, hi - shift]);
    offset.angle_limits = cone.angle_limits;
    Ok(Box::new(offset))
}

/// Create offset torus by adjusting minor radius
fn create_offset_torus(surface: &dyn Surface, distance: f64) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::surface::Torus;

    let tor = surface
        .as_any()
        .downcast_ref::<Torus>()
        .ok_or_else(|| OperationError::InvalidGeometry("Expected Torus surface".into()))?;

    let new_minor = tor.minor_radius + distance;
    if new_minor <= 0.0 {
        return Err(OperationError::InvalidGeometry(
            "Offset produces zero or negative torus minor radius".into(),
        ));
    }

    let mut offset = Torus::new(tor.center, tor.axis, tor.major_radius, new_minor)?;
    offset.param_limits = tor.param_limits;
    Ok(Box::new(offset))
}

/// Create offset B-spline / NURBS surface using the Surface::offset trait method
fn create_offset_bspline(
    surface: &dyn Surface,
    distance: f64,
) -> OperationResult<Box<dyn Surface>> {
    Ok(surface.offset(distance))
}

/// Create offset NURBS surface using the Surface::offset trait method
fn create_offset_nurbs(surface: &dyn Surface, distance: f64) -> OperationResult<Box<dyn Surface>> {
    Ok(surface.offset(distance))
}

/// Create offset loop (boundary curves)
fn create_offset_loop(
    model: &mut BRepModel,
    face: &Face,
    distance: f64,
    options: &OffsetOptions,
) -> OperationResult<Loop> {
    let original_loop = model
        .loops
        .get(face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?
        .clone();

    let mut offset_edges = Vec::new();

    // Offset each edge in the loop. Adjacent edges share a source vertex on
    // the source face; both adjacent offset edges therefore compute the same
    // surface normal at that shared point, producing coincident offset
    // vertices and a watertight loop without explicit corner handling.
    for (i, &edge_id) in original_loop.edges.iter().enumerate() {
        let forward = original_loop.orientations[i];
        let offset_edge_id =
            create_offset_edge(model, edge_id, face.surface_id, distance, forward, options)?;
        offset_edges.push((offset_edge_id, forward));
    }

    // Create new loop
    let mut offset_loop = Loop::new(
        0, // ID will be assigned by store
        original_loop.loop_type,
    );
    for (edge_id, forward) in offset_edges {
        offset_loop.add_edge(edge_id, forward);
    }

    Ok(offset_loop)
}

/// Create offset edge
fn create_offset_edge(
    model: &mut BRepModel,
    edge_id: EdgeId,
    surface_id: u32,
    distance: f64,
    forward: bool,
    options: &OffsetOptions,
) -> OperationResult<EdgeId> {
    // Validate that the requested offset distance is geometrically meaningful
    // relative to the user-supplied tolerance. A near-zero offset would
    // produce vertices coincident with the source edge, generating a
    // numerical artifact rather than a real offset.
    let tol = options.common.tolerance.distance();
    if !distance.is_finite() {
        return Err(OperationError::InvalidGeometry(format!(
            "create_offset_edge: distance {} is not finite",
            distance
        )));
    }
    if distance.abs() <= tol {
        return Err(OperationError::InvalidGeometry(format!(
            "create_offset_edge: |distance|={:.3e} is not greater than tolerance {:.3e}",
            distance.abs(),
            tol
        )));
    }
    // Reject offsets that exceed the configured deviation budget — those
    // approximations would silently degrade surface quality without warning.
    if options.max_deviation > 0.0 && distance.abs() > options.max_deviation * 1e6 {
        return Err(OperationError::InvalidGeometry(format!(
            "create_offset_edge: distance {:.3e} far exceeds max_deviation {:.3e}",
            distance, options.max_deviation,
        )));
    }
    // Honor the caller's direction preference: forward=false flips the offset
    // sign so callers can request either side of the source curve.
    let signed_distance = if forward { distance } else { -distance };

    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "create_offset_edge: edge {} not found",
                edge_id
            ))
        })?
        .clone();

    // Get edge curve
    let curve = model
        .curves
        .get(edge.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;

    // Create offset vertices
    let start_pos = edge.evaluate(0.0, &model.curves)?;
    let end_pos = edge.evaluate(1.0, &model.curves)?;
    let mid_pos = edge.evaluate(0.5, &model.curves)?;

    let surface = model
        .surfaces
        .get(surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    // Offset vertices along surface normal (signed by forward flag)
    let start_normal = compute_surface_normal_at_point(surface, start_pos)?;
    let end_normal = compute_surface_normal_at_point(surface, end_pos)?;
    let mid_normal = compute_surface_normal_at_point(surface, mid_pos)?;

    // Self-intersection guard. An offset curve folds onto itself wherever
    // the curve is concave toward the offset direction and |distance|
    // exceeds 1/κ at that point. We sample the parameter range and
    // compute κ_eff(t) = (curvature_vector(t) · offset_direction); a
    // positive κ_eff means the offset is on the concave side, and the
    // offset is regular only while signed_distance · κ_eff < 1.
    if let Some(t_fold) = detect_offset_self_intersection(curve, &mid_normal, signed_distance) {
        match options.intersection_handling {
            IntersectionHandling::Fail => {
                return Err(OperationError::InvalidGeometry(format!(
                    "create_offset_edge: edge {} curve folds at parameter {:.4} \
                     — |distance|={:.3e} exceeds local radius of curvature",
                    edge_id,
                    t_fold,
                    signed_distance.abs()
                )));
            }
            // `Trim` and `Keep` surface a warning and proceed; full
            // trimming requires re-parameterising the offset over the
            // regular sub-range, which the curve-offset trait does not
            // expose today. Logging keeps the failure visible without
            // silently producing degenerate geometry.
            IntersectionHandling::Trim | IntersectionHandling::Keep => {
                tracing::warn!(
                    "offset edge {}: self-intersection detected at t={:.4} \
                     (handling={:?}); offset may contain folded geometry",
                    edge_id,
                    t_fold,
                    options.intersection_handling
                );
            }
        }
    }

    // Offset the curve along the surface normal at its midpoint. For planar
    // surfaces this is exact (constant normal); for low-curvature surfaces
    // it is a first-order accurate approximation. Higher-order accuracy
    // requires per-sample normal variation and is delegated to the
    // surface-level offset path used for non-planar faces.
    let offset_curve = curve.offset(signed_distance, &mid_normal).map_err(|e| {
        OperationError::InvalidGeometry(format!(
            "create_offset_edge: curve {} offset failed: {}",
            edge.curve_id, e
        ))
    })?;
    let curve_id = model.curves.add(offset_curve);

    // Deduplicate offset endpoints against existing vertices via the
    // caller-supplied tolerance. Adjacent boundary edges share a source
    // vertex, and both adjacent offset edges produce coincident endpoints
    // (same surface normal at the shared point) — `add_or_find` collapses
    // those to a single VertexId, yielding a watertight offset loop.
    let offset_start = model.vertices.add_or_find(
        start_pos.x + start_normal.x * signed_distance,
        start_pos.y + start_normal.y * signed_distance,
        start_pos.z + start_normal.z * signed_distance,
        tol,
    );
    let offset_end = model.vertices.add_or_find(
        end_pos.x + end_normal.x * signed_distance,
        end_pos.y + end_normal.y * signed_distance,
        end_pos.z + end_normal.z * signed_distance,
        tol,
    );

    // Create new edge
    let offset_edge = Edge::new(
        0, // ID will be assigned by store
        offset_start,
        offset_end,
        curve_id,
        edge.orientation,
        edge.param_range,
    );
    let offset_edge_id = model.edges.add(offset_edge);

    Ok(offset_edge_id)
}

/// Compute surface normal at a point by finding the closest parametric location
fn compute_surface_normal_at_point(
    surface: &dyn Surface,
    point: Point3,
) -> OperationResult<Vector3> {
    let tol = Tolerance::new(1e-6, 1e-6);
    let (u, v) = surface.closest_point(&point, tol)?;
    Ok(surface.normal_at(u, v)?)
}

/// Detect curve-offset self-intersection.
///
/// An offset curve C_off(t) = C(t) + d · n folds onto itself where C is
/// concave toward n and |d| ≥ 1 / κ_concave(t). Concavity is measured
/// by the dot product `cv(t) · n` where `cv` is the curvature vector
/// (points to the centre of the osculating circle, magnitude = κ).
/// When that dot product is positive the offset is on the concave side,
/// and the offset is regular only while `signed_distance · κ_eff < 1`.
///
/// Returns the first sampled parameter at which the fold condition is
/// reached, or `None` if the offset is regular over the entire range.
///
/// Sampling density (32 points) matches the granularity used elsewhere
/// in the kernel for curve-property scans (see Patrikalakis-Maekawa
/// §4.5 for the same approach in curve-surface intersection seeding).
fn detect_offset_self_intersection(
    curve: &dyn crate::primitives::curve::Curve,
    offset_dir: &Vector3,
    signed_distance: f64,
) -> Option<f64> {
    const SAMPLES: usize = 32;
    if signed_distance.abs() <= f64::EPSILON {
        return None;
    }
    let n = offset_dir.normalize().ok()?;
    for i in 0..=SAMPLES {
        let t = i as f64 / SAMPLES as f64;
        let point = match curve.evaluate(t) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let cv = match point.curvature_vector() {
            Some(v) => v,
            None => continue,
        };
        // κ_eff = curvature_vector · n. Positive ⇒ offset is on concave
        // side. Negative or zero ⇒ offset is on convex side or curve
        // is locally straight — never folds in the offset direction.
        let kappa_eff = cv.dot(&n);
        if kappa_eff <= 0.0 {
            continue;
        }
        // Fold iff signed_distance · κ_eff ≥ 1. Use ≥ so the boundary
        // case (offset lands exactly on the centre of curvature) also
        // counts as degenerate.
        if signed_distance * kappa_eff >= 1.0 {
            return Some(t);
        }
    }
    None
}

/// Create interior offset faces for shell
fn create_interior_offset_faces(
    model: &mut BRepModel,
    solid: &Solid,
    thickness: f64,
    faces_to_remove: &[FaceId],
    options: &OffsetOptions,
) -> OperationResult<Vec<FaceId>> {
    let shell = model
        .shells
        .get(solid.outer_shell)
        .ok_or_else(|| OperationError::InvalidGeometry("Shell not found".to_string()))?
        .clone();

    let mut interior_faces = Vec::new();

    for &face_id in &shell.faces {
        // Skip faces that will be removed (openings)
        if faces_to_remove.contains(&face_id) {
            continue;
        }

        // Create inward offset of face
        let offset_options = OffsetOptions {
            common: options.common.clone(),
            offset_type: OffsetType::Distance(thickness),
            intersection_handling: options.intersection_handling,
            max_deviation: options.max_deviation,
        };

        let interior_face = offset_face(model, face_id, offset_options)?;
        interior_faces.push(interior_face);
    }

    Ok(interior_faces)
}

/// Create wall faces for shell openings.
///
/// Walls are erected at every face the caller wants to remove (the
/// "openings"). We first verify each `faces_to_remove` actually belongs
/// to the solid's outer shell — otherwise the wall topology would
/// connect the new shell to faces that don't share boundary edges, and
/// the resulting solid would be non-manifold.
fn create_shell_walls(
    model: &mut BRepModel,
    solid: &Solid,
    thickness: f64,
    faces_to_remove: &[FaceId],
    options: &OffsetOptions,
) -> OperationResult<Vec<FaceId>> {
    // Confirm every face being removed lives on this solid's outer shell.
    let outer_shell_faces: std::collections::HashSet<FaceId> = model
        .shells
        .get(solid.outer_shell)
        .ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "create_shell_walls: outer shell {} of solid {} not found",
                solid.outer_shell, solid.id
            ))
        })?
        .faces
        .iter()
        .copied()
        .collect();

    for &face_id in faces_to_remove {
        if !outer_shell_faces.contains(&face_id) {
            return Err(OperationError::InvalidGeometry(format!(
                "create_shell_walls: face {} is not on outer shell {} of solid {}",
                face_id, solid.outer_shell, solid.id
            )));
        }
    }

    let mut wall_faces = Vec::new();

    for &face_id in faces_to_remove {
        // Get boundary edges of removed face
        let face = model
            .faces
            .get(face_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?
            .clone();

        let loop_data = model
            .loops
            .get(face.outer_loop)
            .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?
            .clone();

        // Compute the removed face's outward normal — the wall offset
        // direction is `-thickness * outward_normal` so walls hang inward
        // (matching the inward direction used by the interior offset
        // faces). Using a global-axis cross product as in the previous
        // implementation produced walls perpendicular to the wrong plane
        // for any face that wasn't axis-aligned.
        let removed_surface = model
            .surfaces
            .get(face.surface_id)
            .ok_or_else(|| {
                OperationError::InvalidGeometry(format!(
                    "create_shell_walls: surface {} of removed face {} not found",
                    face.surface_id, face_id
                ))
            })?;
        let mut removed_normal = removed_surface.normal_at(0.5, 0.5)?;
        if matches!(
            face.orientation,
            crate::primitives::face::FaceOrientation::Backward
        ) {
            removed_normal = -removed_normal;
        }

        // Create wall face for each edge. Pass the caller's tolerance
        // so adjacent walls can dedup their shared corner vertices.
        let tol = options.common.tolerance.distance();
        for (i, &edge_id) in loop_data.edges.iter().enumerate() {
            let forward = loop_data.orientations[i];
            let wall_face =
                create_wall_face(model, edge_id, thickness, forward, removed_normal, tol)?;
            wall_faces.push(wall_face);
        }
    }

    Ok(wall_faces)
}

/// Create a wall face between outer and inner edges.
///
/// `removed_face_outward_normal` is the outward normal of the face being
/// removed (i.e., the opening). Walls extend along `-thickness *
/// outward_normal` so they meet the inward-offset interior faces in a
/// coplanar fashion at the boundary loop.
fn create_wall_face(
    model: &mut BRepModel,
    outer_edge_id: EdgeId,
    thickness: f64,
    forward: bool,
    removed_face_outward_normal: Vector3,
    tol: f64,
) -> OperationResult<FaceId> {
    use crate::primitives::curve::Line;
    use crate::primitives::edge::EdgeOrientation;
    use crate::primitives::face::FaceOrientation;
    use crate::primitives::r#loop::LoopType;
    use crate::primitives::surface::Plane;

    let outer_edge = model
        .edges
        .get(outer_edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Outer edge not found".into()))?
        .clone();

    // Get the outer edge endpoints
    let p1_arr = model
        .vertices
        .get_position(outer_edge.start_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("Start vertex not found".into()))?;
    let p2_arr = model
        .vertices
        .get_position(outer_edge.end_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("End vertex not found".into()))?;
    let p1 = Vector3::new(p1_arr[0], p1_arr[1], p1_arr[2]);
    let p2 = Vector3::new(p2_arr[0], p2_arr[1], p2_arr[2]);

    // Validate the edge's curve is consistent with its endpoints: the
    // curve mid-point must lie on the segment between the stored vertex
    // positions. A drift here means the curve and vertices disagree;
    // building a wall on inconsistent geometry would silently corrupt
    // the result.
    let edge_curve = model
        .curves
        .get(outer_edge.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge curve not found".into()))?;
    let mid = edge_curve.evaluate(0.5)?.position;
    let segment_mid = (p1 + p2) * 0.5;
    let drift = (mid - segment_mid).magnitude();
    // Use 0.5% of the edge length as the per-edge consistency budget;
    // anything beyond that signals a curve/vertex mismatch worth aborting on.
    let edge_len = (p2 - p1).magnitude();
    let drift_budget = (edge_len * 5e-3).max(1e-9);
    if drift > drift_budget {
        return Err(OperationError::InvalidGeometry(format!(
            "create_wall_face: edge {} curve mid-point drift {:.3e} \
             exceeds budget {:.3e} (edge length {:.3e})",
            outer_edge_id, drift, drift_budget, edge_len
        )));
    }
    let edge_dir = (p2 - p1).normalize()?;

    // Wall offset direction is the inward direction of the removed face:
    // the face whose plane the wall hangs from points outward, so the
    // wall extends opposite that normal by `thickness`. This produces a
    // wall perpendicular to the removed-face plane and parallel to the
    // edge — independent of the global coordinate frame.
    let offset_dir = (-removed_face_outward_normal).normalize()?;

    let offset = offset_dir * thickness.abs();
    let p3 = p2 + offset;
    let p4 = p1 + offset;

    // Create the planar surface through the four corners. The wall
    // normal must be perpendicular to both the edge direction and the
    // wall's depth direction.
    let wall_normal = edge_dir.cross(&offset_dir).normalize()?;
    let wall_surface = Plane::from_point_normal(p1, wall_normal)?;
    let surface_id = model.surfaces.add(Box::new(wall_surface));

    // Dedup wall corner vertices via tolerance — adjacent walls along
    // the same boundary loop share their meeting corner, so the second
    // wall finds the corner the first wall planted instead of creating
    // a coincident duplicate. This is what makes the resulting shell
    // topologically watertight at the opening boundary.
    let v3 = model.vertices.add_or_find(p3.x, p3.y, p3.z, tol);
    let v4 = model.vertices.add_or_find(p4.x, p4.y, p4.z, tol);

    // Create four edges for the rectangular face
    let e_top = outer_edge_id; // reuse outer edge
    let line_right = Line::new(p2, p3);
    let c_right = model.curves.add(Box::new(line_right));
    let e_right = model.edges.add(Edge::new_auto_range(
        0,
        outer_edge.end_vertex,
        v3,
        c_right,
        EdgeOrientation::Forward,
    ));

    let line_bottom = Line::new(p3, p4);
    let c_bottom = model.curves.add(Box::new(line_bottom));
    let e_bottom = model.edges.add(Edge::new_auto_range(
        0,
        v3,
        v4,
        c_bottom,
        EdgeOrientation::Forward,
    ));

    let line_left = Line::new(p4, p1);
    let c_left = model.curves.add(Box::new(line_left));
    let e_left = model.edges.add(Edge::new_auto_range(
        0,
        v4,
        outer_edge.start_vertex,
        c_left,
        EdgeOrientation::Forward,
    ));

    // Create loop
    let mut wall_loop = Loop::new(0, LoopType::Outer);
    wall_loop.add_edge(e_top, forward);
    wall_loop.add_edge(e_right, true);
    wall_loop.add_edge(e_bottom, true);
    wall_loop.add_edge(e_left, true);
    let loop_id = model.loops.add(wall_loop);

    // Create face
    let face = Face::new(0, surface_id, loop_id, FaceOrientation::Forward);
    let face_id = model.faces.add(face);

    Ok(face_id)
}

/// Combine faces for shell solid
fn combine_shell_faces(
    model: &mut BRepModel,
    solid: &Solid,
    faces_to_remove: &[FaceId],
    interior_faces: Vec<FaceId>,
    wall_faces: Vec<FaceId>,
) -> OperationResult<Vec<FaceId>> {
    let shell = model
        .shells
        .get(solid.outer_shell)
        .ok_or_else(|| OperationError::InvalidGeometry("Shell not found".to_string()))?;

    let mut all_faces = Vec::new();

    // Add original exterior faces (except removed ones)
    for &face_id in &shell.faces {
        if !faces_to_remove.contains(&face_id) {
            all_faces.push(face_id);
        }
    }

    // Add interior offset faces
    all_faces.extend(interior_faces);

    // Add wall faces
    all_faces.extend(wall_faces);

    Ok(all_faces)
}

/// Validate offset face inputs
fn validate_offset_face_inputs(
    model: &BRepModel,
    face_id: FaceId,
    options: &OffsetOptions,
) -> OperationResult<()> {
    // Check face exists
    if model.faces.get(face_id).is_none() {
        return Err(OperationError::InvalidGeometry(
            "Face not found".to_string(),
        ));
    }

    // Check offset distance
    match &options.offset_type {
        OffsetType::Distance(d) => {
            if d.abs() < options.common.tolerance.distance() {
                return Err(OperationError::InvalidGeometry(
                    "Offset distance too small".to_string(),
                ));
            }
        }
        _ => {} // Other types validated during execution
    }

    Ok(())
}

/// Validate shell inputs
fn validate_shell_inputs(
    model: &BRepModel,
    solid_id: SolidId,
    thickness: f64,
    faces_to_remove: &[FaceId],
) -> OperationResult<()> {
    // Check solid exists
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidGeometry(
            "Solid not found".to_string(),
        ));
    }

    // Check thickness
    if thickness.abs() < 1e-10 {
        return Err(OperationError::InvalidGeometry(
            "Shell thickness too small".to_string(),
        ));
    }

    // Check faces to remove exist
    for &face_id in faces_to_remove {
        if model.faces.get(face_id).is_none() {
            return Err(OperationError::InvalidGeometry(
                "Face to remove not found".to_string(),
            ));
        }
    }

    Ok(())
}

/// Validate shell solid by running the full B-Rep validation suite.
fn validate_shell_solid(model: &BRepModel, solid_id: SolidId) -> OperationResult<()> {
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidBRep(
            "Shell solid not found".to_string(),
        ));
    }
    let result = crate::primitives::validation::validate_model_enhanced(
        model,
        Tolerance::default(),
        crate::primitives::validation::ValidationLevel::Standard,
    );
    if !result.is_valid {
        let summary = result
            .errors
            .iter()
            .take(3)
            .map(|e| format!("{:?}", e))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(OperationError::InvalidBRep(format!(
            "Shell solid failed validation ({} errors): {}",
            result.errors.len(),
            summary
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Point3;
    use crate::primitives::curve::{Arc as ArcCurve, Line};

    /// A straight line has zero curvature; the offset never folds
    /// regardless of distance.
    #[test]
    fn line_offset_never_self_intersects() {
        let line = Line::new(Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 0.0, 0.0));
        let n = Vector3::new(0.0, 1.0, 0.0);
        assert!(detect_offset_self_intersection(&line, &n, 0.5).is_none());
        assert!(detect_offset_self_intersection(&line, &n, 100.0).is_none());
        assert!(detect_offset_self_intersection(&line, &n, -100.0).is_none());
    }

    /// An arc of radius 1 in the XY plane: offset on the convex side
    /// (away from centre) is always regular; offset on the concave side
    /// folds when |distance| ≥ radius.
    #[test]
    fn arc_offset_folds_when_distance_exceeds_radius_on_concave_side() {
        // Quarter arc of radius 1 centered at origin, in XY plane.
        // start_angle=0 sweeps from +X toward +Y for normal=+Z.
        let arc = ArcCurve::new(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            1.0,
            0.0,
            std::f64::consts::FRAC_PI_2,
        )
        .expect("quarter arc construction");

        // At t=0 the arc point is +X; the curvature vector at any t
        // points from C(t) toward the origin (the centre). At t=0
        // that direction is -X.
        let toward_centre = Vector3::new(-1.0, 0.0, 0.0);
        // Convex side: offset_dir · cv < 0 at every sample → never folds
        let convex = -toward_centre;
        assert!(detect_offset_self_intersection(&arc, &convex, 5.0).is_none());

        // Concave side: at t=0 the alignment is perfect (cv · n = κ = 1).
        // signed_distance × κ_eff = 1.5 × 1.0 ≥ 1 → fold detected.
        let fold = detect_offset_self_intersection(&arc, &toward_centre, 1.5);
        assert!(
            fold.is_some(),
            "expected fold detection on concave side at d=1.5 (radius=1)"
        );

        // Distance smaller than radius: still regular even on concave side.
        assert!(detect_offset_self_intersection(&arc, &toward_centre, 0.25).is_none());
    }
}
