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

use super::lifecycle::{self, OpSpec};
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
    vertex::VertexId,
};
use std::collections::HashMap;

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
    if options.common.validate_before {
        lifecycle::validate_can_apply(model, OpSpec::OffsetFace { face_id })?;
    }
    lifecycle::with_rollback(model, move |model| {
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

        // Create offset edges. The standalone offset_face surface does not
        // need the per-edge map (only the shell operation does) nor the
        // corner insets (those trim a shell cavity, not a lone face), so pass
        // an empty inset map and discard the edge map.
        let no_insets: HashMap<VertexId, Point3> = HashMap::new();
        // Standalone single-face offset: no cross-loop edge sharing to track.
        let mut local_shared: HashMap<EdgeId, EdgeId> = HashMap::new();
        let (offset_loop, _edge_map) = create_offset_loop(
            model,
            &face,
            distance,
            &no_insets,
            &options,
            &mut local_shared,
        )?;
        let loop_id = model.loops.add(offset_loop);

        // Create new face
        let offset_face = Face::new(
            0, // ID will be assigned by store
            surface_id,
            loop_id,
            face.orientation,
        );
        let new_face_id = model.faces.add(offset_face);

        Ok(new_face_id)
    })
}

/// Create a shell (hollow) from a solid
pub fn offset_solid(
    model: &mut BRepModel,
    solid_id: SolidId,
    thickness: f64,
    faces_to_remove: Vec<FaceId>,
    options: OffsetOptions,
) -> OperationResult<SolidId> {
    if options.common.validate_before {
        lifecycle::validate_can_apply(model, OpSpec::OffsetSolid { solid_id })?;
    }
    lifecycle::with_rollback(model, move |model| {
        offset_solid_body(model, solid_id, thickness, faces_to_remove, options)
    })
}

fn offset_solid_body(
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

    // Pre-compute the inset position of every original vertex by intersecting
    // the inward-offset planes of the faces meeting there (planar shells only;
    // see `compute_vertex_insets`). Using these shared corner positions in BOTH
    // the interior offset edges and the wall rims is what trims the inner faces
    // to the cavity footprint and makes their corners coincide (dedup-merge)
    // instead of self-intersecting. Vertices on any non-planar face are absent
    // from the map and fall back to the per-face normal offset.
    let vertex_insets = compute_vertex_insets(model, &solid, &faces_to_remove, thickness.abs())?;

    // Create offset faces for interior. Capture the per-edge map so
    // that walls erected over removed faces can reuse the offset edges
    // these interior faces just created — that's what gives the
    // resulting shell shared topology (manifold) instead of a stack of
    // disjoint surfaces that just happen to dedup at their corner
    // vertices. `source_to_offset_surface` records, per source edge, the
    // OFFSET SURFACE its adjacent interior face was built on, so a curved cap
    // wall can rebuild the offset rim as that surface's exact iso-curve (see
    // `create_curved_rim_wall`).
    let (interior_faces, original_to_offset_edge, source_to_offset_surface) =
        create_interior_offset_faces(
            model,
            &solid,
            -thickness.abs(),
            &faces_to_remove,
            &vertex_insets,
            &options,
        )?;

    // Create side walls for removed faces
    let wall_faces = create_shell_walls(
        model,
        &solid,
        thickness,
        &faces_to_remove,
        &original_to_offset_edge,
        &source_to_offset_surface,
        &vertex_insets,
        &options,
    )?;

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
            .with_input_solids([solid_id as u64])
            .with_input_faces(faces_to_remove.iter().map(|&f| f as u64))
            .with_output_solids([hollow_id as u64]),
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
    let sin_ha = cone.half_angle.sin();
    if sin_ha.abs() < 1e-9 {
        // A near-zero half-angle is a degenerate (near-cylindrical) cone; the
        // apex shift diverges to infinity. Refuse rather than emit an apex at ∞.
        return Err(OperationError::InvalidGeometry(
            "cannot offset a degenerate near-cylindrical cone (half-angle ~ 0)".into(),
        ));
    }
    let shift = distance / sin_ha;
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

/// Create offset loop (boundary curves).
///
/// Returns the new loop together with a per-edge mapping
/// `(source_edge_id, offset_edge_id)` in the same order the loop
/// traverses them. The shell operation uses that mapping to share
/// offset edges between interior offset faces and the walls erected
/// over removed faces — without it each side creates an independent
/// edge along the same boundary segment and the resulting solid is
/// non-manifold even when its vertices have been dedup'd.
fn create_offset_loop(
    model: &mut BRepModel,
    face: &Face,
    distance: f64,
    vertex_insets: &HashMap<VertexId, Point3>,
    options: &OffsetOptions,
    shared_offset_edges: &mut HashMap<EdgeId, EdgeId>,
) -> OperationResult<(Loop, Vec<(EdgeId, EdgeId)>)> {
    let original_loop = model
        .loops
        .get(face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?
        .clone();

    let mut offset_edges = Vec::new();
    let mut edge_map = Vec::with_capacity(original_loop.edges.len());

    // Offset each edge in the loop. Adjacent edges share a source vertex on
    // the source face; both adjacent offset edges therefore compute the same
    // surface normal at that shared point, producing coincident offset
    // vertices and a watertight loop without explicit corner handling.
    //
    // The loop's per-edge `forward` flag describes traversal direction
    // (which side of the manifold this loop is on), NOT which side of
    // the curve to offset to. The offset side is encoded entirely in the
    // sign of `distance`. We pass every edge to `create_offset_edge`
    // with the same offset distance — uniform shift along the surface
    // normal — and preserve the original `forward` flag in the new
    // loop so traversal stays consistent.
    // A source edge must offset to a SINGLE shared offset edge wherever it is
    // reused — both WITHIN this loop (a periodic seam edge appears twice,
    // forward + backward) and ACROSS loops (an edge shared by two kept faces,
    // e.g. a cap rim shared by a cap and the lateral, is offset once per face;
    // the offsets must coincide as one edge). Minting a fresh offset edge per
    // occurrence leaves each used once → a torn (non-manifold) seam. Dedup by
    // source edge id through the caller-shared map so every reuse — in this
    // loop or a sibling face's loop — resolves to the same offset edge.
    for (i, &edge_id) in original_loop.edges.iter().enumerate() {
        let forward = original_loop.orientations[i];
        let offset_edge_id = if let Some(&existing) = shared_offset_edges.get(&edge_id) {
            existing
        } else {
            let created = create_offset_edge(
                model,
                edge_id,
                face.surface_id,
                distance,
                vertex_insets,
                options,
            )?;
            shared_offset_edges.insert(edge_id, created);
            edge_map.push((edge_id, created));
            created
        };
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

    Ok((offset_loop, edge_map))
}

/// Create offset edge.
///
/// The offset is applied uniformly along the surface normal at every
/// sample point of the source curve. The sign of `distance` selects
/// which side of the surface to offset to — positive shifts along the
/// surface's natural normal, negative shifts opposite. This is
/// independent of any loop traversal flag the caller may have; an
/// outer loop is offset coherently when every edge in the loop is
/// offset by the same signed distance.
fn create_offset_edge(
    model: &mut BRepModel,
    edge_id: EdgeId,
    surface_id: u32,
    distance: f64,
    vertex_insets: &HashMap<VertexId, Point3>,
    options: &OffsetOptions,
) -> OperationResult<EdgeId> {
    let tol = options.common.tolerance.distance();

    // Planar-shell fast path: when both endpoints have a pre-computed corner
    // inset, the offset edge is the straight segment between the two inset
    // corners (the intersection lines of the inward-offset planes). Using these
    // shared corners — identical to the ones the wall rims use — is what trims
    // the inner face to the cavity footprint and lets adjacent inner edges and
    // walls dedup-merge at the corner. (Falls through to the per-face normal
    // offset below when either endpoint is on a non-planar face.)
    {
        let edge = model.edges.get(edge_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!("create_offset_edge: edge {edge_id} not found"))
        })?;
        let (sv, ev, orient, prange) = (
            edge.start_vertex,
            edge.end_vertex,
            edge.orientation,
            edge.param_range,
        );
        if let (Some(&sp), Some(&ep)) = (vertex_insets.get(&sv), vertex_insets.get(&ev)) {
            use crate::primitives::curve::Line;
            let vs = model.vertices.add_or_find(sp.x, sp.y, sp.z, tol);
            let ve = model.vertices.add_or_find(ep.x, ep.y, ep.z, tol);
            let curve_id = model.curves.add(Box::new(Line::new(sp, ep)));
            let offset_edge = Edge::new(0, vs, ve, curve_id, orient, prange);
            return Ok(model.edges.add(offset_edge));
        }
    }

    // Validate that the requested offset distance is geometrically meaningful
    // relative to the user-supplied tolerance. A near-zero offset would
    // produce vertices coincident with the source edge, generating a
    // numerical artifact rather than a real offset.
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
    let signed_distance = distance;

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

/// Compute the inset position of each original vertex for a PLANAR shell.
///
/// A shelled solid's inner faces must be inset to the cavity footprint — each
/// inner vertex sits at the intersection of the inward-offset planes of the
/// faces meeting at the original vertex. Offsetting each face's boundary along
/// only that face's own normal (the historical behaviour) reaches the right
/// plane but never insets in-plane, so the inner faces were the wrong size and
/// self-intersected (the "10×10×10 shell encloses 1640 vs 424" bug). Solving
/// the per-vertex plane intersection is what trims them.
///
/// Returns a map `original_vertex → inset_point`, covering only vertices whose
/// incident faces are ALL planar (and there are ≥ 3 of them). Vertices touching
/// any non-planar face are omitted; callers fall back to the per-face normal
/// offset there, preserving the previous behaviour for curved shells.
///
/// Removed faces (the shell openings) contribute their ORIGINAL plane rather
/// than the inset plane, so a rim vertex stays at the opening along the
/// removed-face normal while still insetting in the other directions.
fn compute_vertex_insets(
    model: &BRepModel,
    solid: &Solid,
    faces_to_remove: &[FaceId],
    thickness: f64,
) -> OperationResult<HashMap<VertexId, Point3>> {
    use crate::math::{linear_solver, svd, STRICT_TOLERANCE};
    use crate::primitives::surface::SurfaceType;

    let shell = model
        .shells
        .get(solid.outer_shell)
        .ok_or_else(|| OperationError::InvalidGeometry("Shell not found".to_string()))?;

    // vertex → the distinct faces of the outer shell that touch it.
    let mut vertex_faces: HashMap<VertexId, Vec<FaceId>> = HashMap::new();
    for &face_id in &shell.faces {
        let face = model
            .faces
            .get(face_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;
        let lp = model
            .loops
            .get(face.outer_loop)
            .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?;
        let mut seen = std::collections::HashSet::new();
        for &edge_id in &lp.edges {
            let edge = match model.edges.get(edge_id) {
                Some(e) => e,
                None => continue,
            };
            for v in [edge.start_vertex, edge.end_vertex] {
                if seen.insert(v) {
                    vertex_faces.entry(v).or_default().push(face_id);
                }
            }
        }
    }

    let removed: std::collections::HashSet<FaceId> = faces_to_remove.iter().copied().collect();
    let mut insets = HashMap::new();

    for (&vid, faces) in &vertex_faces {
        if faces.len() < 3 {
            continue;
        }
        let vpos = match model.vertices.get(vid) {
            Some(v) => v.point(),
            None => continue,
        };

        // One plane constraint per incident face:
        //   non-removed: n·x = n·V − t   (inset plane)
        //   removed:     n·x = n·V       (original plane — pins the rim)
        let mut a: Vec<Vec<f64>> = Vec::with_capacity(faces.len());
        let mut b: Vec<f64> = Vec::with_capacity(faces.len());
        let mut all_planar = true;
        for &face_id in faces {
            let face = match model.faces.get(face_id) {
                Some(f) => f,
                None => {
                    all_planar = false;
                    break;
                }
            };
            let surface = match model.surfaces.get(face.surface_id) {
                Some(s) => s,
                None => {
                    all_planar = false;
                    break;
                }
            };
            // Accept any GEOMETRICALLY planar face, not only the `Plane`
            // primitive. The live user path builds a box by extruding a
            // sketched rectangle (api-server `extrude_sketch` → `extrude_face`),
            // whose four side walls are flat `RuledSurface`s (a ruled surface
            // between two parallel straight edges is planar) rather than `Plane`
            // primitives. Gating on `SurfaceType::Plane` alone omitted every
            // rim-corner vertex of the extruded box from the inset solve, so the
            // shell walls fell back to per-face offsets and minted two divergent
            // inset corners per rim vertex — leaving 8 dangling corner edges
            // (open B-Rep, V−E+F=6, genus −2) that the primitive-box path never
            // hit. `is_planar` samples the surface normal across its parameter
            // domain and confirms a single constant normal, so the plane
            // constraint `n·x = n·V − t` below is exact for these flat walls.
            // A genuinely curved wall still fails `is_planar` and is omitted,
            // preserving the per-face-offset fallback for curved shells.
            let planar =
                surface.surface_type() == SurfaceType::Plane || surface.is_planar(STRICT_TOLERANCE);
            if !planar {
                all_planar = false;
                break;
            }
            // Outward face normal at the vertex. The inset-plane RHS below
            // (`n·V − t`) shifts the plane a distance `t` along `−n`, so for the
            // inset to move INTO the material `n` must point OUT of it. The
            // surface's intrinsic `normal_at` follows its own u×v
            // parameterisation — for a `Plane` box face that is already outward,
            // but a `RuledSurface` extrude wall can be parameterised either way,
            // so honour the face orientation (matching `create_shell_walls`'
            // `removed_normal` handling). Sign errors here would inset the corner
            // OUTWARD and reintroduce the divergent-corner gap.
            let face_orientation = face.orientation;
            let mut n = compute_surface_normal_at_point(surface, vpos)?;
            if matches!(
                face_orientation,
                crate::primitives::face::FaceOrientation::Backward
            ) {
                n = -n;
            }
            let n_dot_v = n.x * vpos.x + n.y * vpos.y + n.z * vpos.z;
            let rhs = if removed.contains(&face_id) {
                n_dot_v
            } else {
                n_dot_v - thickness
            };
            a.push(vec![n.x, n.y, n.z]);
            b.push(rhs);
        }
        if !all_planar {
            continue;
        }

        // Exactly three planes → exact 3×3 solve; more (rare high-valence
        // vertex) → least-squares best-fit corner via the SVD pseudo-inverse.
        let solved = if a.len() == 3 {
            linear_solver::gaussian_elimination(a, b, STRICT_TOLERANCE)
        } else {
            svd::solve_least_squares_svd(a, &b, STRICT_TOLERANCE)
        };
        if let Ok(x) = solved {
            if x.len() == 3 && x.iter().all(|c| c.is_finite()) {
                insets.insert(vid, Point3::new(x[0], x[1], x[2]));
            }
        }
        // A singular system (degenerate/parallel planes) just omits the vertex
        // → caller falls back to the per-face normal offset there.
    }

    Ok(insets)
}

/// Create interior offset faces for shell.
///
/// Returns the new face IDs alongside a `(source_edge_id ->
/// offset_edge_id)` map covering every edge that participated in any
/// non-removed face's outer loop. The map lets `create_shell_walls`
/// reuse offset edges as wall bottoms, sharing topology between walls
/// and interior faces — without it, the wall and the interior face
/// each create their own edge along the same boundary segment and the
/// resulting solid is non-manifold even when its vertices have been
/// dedup'd. Each removed-face boundary edge is shared with exactly
/// one non-removed adjacent face on a well-formed input solid, so the
/// mapping is unambiguous (last-writer-wins on the rare degenerate
/// case where an edge bounds multiple non-removed faces).
fn create_interior_offset_faces(
    model: &mut BRepModel,
    solid: &Solid,
    thickness: f64,
    faces_to_remove: &[FaceId],
    vertex_insets: &HashMap<VertexId, Point3>,
    options: &OffsetOptions,
) -> OperationResult<(
    Vec<FaceId>,
    std::collections::HashMap<EdgeId, EdgeId>,
    std::collections::HashMap<EdgeId, u32>,
)> {
    let shell = model
        .shells
        .get(solid.outer_shell)
        .ok_or_else(|| OperationError::InvalidGeometry("Shell not found".to_string()))?
        .clone();

    let mut interior_faces = Vec::new();
    let mut original_to_offset = std::collections::HashMap::new();
    let mut source_to_offset_surface: std::collections::HashMap<EdgeId, u32> =
        std::collections::HashMap::new();

    let offset_options = OffsetOptions {
        common: options.common.clone(),
        offset_type: OffsetType::Distance(thickness),
        intersection_handling: options.intersection_handling,
        max_deviation: options.max_deviation,
    };

    for &face_id in &shell.faces {
        // Skip faces that will be removed (openings)
        if faces_to_remove.contains(&face_id) {
            continue;
        }

        validate_offset_face_inputs(model, face_id, &offset_options)?;
        let face = model
            .faces
            .get(face_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?
            .clone();

        // Create offset surface and loop, capturing the per-edge map.
        //
        // `thickness` here is the inward offset magnitude (`-thickness.abs()`,
        // set by the caller). For a PLANAR face the per-surface offset helper
        // (`create_offset_plane`, and the trait `offset` for a flat ruled wall)
        // shifts along the surface's RAW intrinsic normal, which equals the
        // outward normal only when the face is `Forward`. A `Backward` planar
        // face — e.g. the bottom cap of an EXTRUDED box, whose profile-plane
        // normal is +Z but whose outward normal is −Z — would then offset to the
        // WRONG side (the bottom interior surface fell to z=−1 while its inset
        // loop sat at z=+1, a 2·t = 2.0 mismatch the validator flagged as "edge
        // lies off face"). Flip the signed distance for `Backward` PLANAR faces
        // so the offset is taken along the outward normal, matching the
        // orientation-corrected inset loop.
        //
        // Analytic CURVED surfaces (cylinder / sphere / cone / torus / NURBS /
        // curved ruled / revolution) are NOT flipped: their offset helpers
        // encode the outward direction in the sign of `distance` relative to the
        // canonical outward (radial) normal — `create_offset_cylinder` does
        // `radius + distance`, so `-|t|` shrinks the wall inward regardless of
        // the face-orientation flag. Flipping them by orientation would GROW the
        // radius outward and break curved shells (cylinder / revolved tube).
        let surface_is_planar = {
            let surf = model.surfaces.get(face.surface_id).ok_or_else(|| {
                OperationError::InvalidGeometry("Face surface not found".to_string())
            })?;
            surf.surface_type() == crate::primitives::surface::SurfaceType::Plane
                || surf.is_planar(crate::math::STRICT_TOLERANCE)
        };
        let oriented_thickness = match (surface_is_planar, face.orientation) {
            (true, crate::primitives::face::FaceOrientation::Backward) => -thickness,
            _ => thickness,
        };
        let offset_surface = create_offset_surface(model, &face, oriented_thickness)?;
        let surface_id = model.surfaces.add(offset_surface);

        // `original_to_offset` doubles as the cross-face dedup map: an edge
        // shared by two kept faces (a cap rim shared by a cap and the lateral,
        // a box edge shared by two side faces) is offset once and reused, so
        // the two offset faces meet on ONE shared edge (2-manifold) instead of
        // two coincident single-use edges.
        let (offset_loop, edge_map) = create_offset_loop(
            model,
            &face,
            thickness,
            vertex_insets,
            &offset_options,
            &mut original_to_offset,
        )?;
        let loop_id = model.loops.add(offset_loop);

        // The interior face bounds the cavity, so its outward-from-MATERIAL
        // normal points INTO the void — the opposite of the source face's
        // outward normal. The offset surface keeps the source normal direction,
        // so flip the face orientation to invert the effective normal (and the
        // tessellation winding). Without this the inner faces face outward like
        // the originals and the divergence adds the cavity instead of
        // subtracting it (the "1362 vs 424" residual after the inset trim).
        use crate::primitives::face::FaceOrientation;
        let inner_orientation = match face.orientation {
            FaceOrientation::Forward => FaceOrientation::Backward,
            FaceOrientation::Backward => FaceOrientation::Forward,
        };
        let offset_face_obj = Face::new(
            0, // ID assigned by store
            surface_id,
            loop_id,
            inner_orientation,
        );
        let offset_face_id = model.faces.add(offset_face_obj);
        interior_faces.push(offset_face_id);

        // `create_offset_loop` already recorded every newly created offset
        // edge in `original_to_offset` (the shared dedup map). Record this
        // face's offset SURFACE against each source edge it offset, so a
        // curved cap wall can rebuild the offset rim as that surface's exact
        // iso-curve (the cap rim is offset exactly once — by its adjacent kept
        // lateral — so the mapping is unambiguous for the rim).
        for (source_edge, _offset_edge) in &edge_map {
            source_to_offset_surface.insert(*source_edge, surface_id);
        }
    }

    Ok((interior_faces, original_to_offset, source_to_offset_surface))
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
    original_to_offset_edge: &std::collections::HashMap<EdgeId, EdgeId>,
    source_to_offset_surface: &std::collections::HashMap<EdgeId, u32>,
    vertex_insets: &HashMap<VertexId, Point3>,
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

    // Corner side-edges are shared by adjacent walls. Track them across EVERY
    // removed face's rims (keyed on the unordered vertex pair) so two walls
    // that meet at a corner reuse one edge instead of each minting a dangling
    // duplicate — the difference between a closed watertight cup and an open,
    // Euler≠2 husk.
    let mut corner_edges: std::collections::HashMap<(VertexId, VertexId), EdgeId> =
        std::collections::HashMap::new();

    for &face_id in faces_to_remove {
        // Get boundary edges of removed face
        let face = model
            .faces
            .get(face_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?
            .clone();

        // Process the outer loop AND every inner loop (hole) of the removed
        // face. An annular cap (a washer / flange / tube end) has BOTH an
        // outer rim and an inner rim, each a closed circle that needs its own
        // ruled wall; iterating only the outer loop would leave the inner rim
        // open.
        let mut rim_loops: Vec<Loop> = Vec::with_capacity(1 + face.inner_loops.len());
        rim_loops.push(
            model
                .loops
                .get(face.outer_loop)
                .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?
                .clone(),
        );
        for &inner_id in &face.inner_loops {
            rim_loops.push(
                model
                    .loops
                    .get(inner_id)
                    .ok_or_else(|| {
                        OperationError::InvalidGeometry("Inner loop not found".to_string())
                    })?
                    .clone(),
            );
        }

        // Compute the removed face's outward normal — the wall offset
        // direction is `-thickness * outward_normal` so walls hang inward
        // (matching the inward direction used by the interior offset
        // faces). Using a global-axis cross product as in the previous
        // implementation produced walls perpendicular to the wrong plane
        // for any face that wasn't axis-aligned.
        let removed_surface = model.surfaces.get(face.surface_id).ok_or_else(|| {
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

        // A closed cap rim (cylinder / cone / sphere / revolved / lofted /
        // NURBS cap) is ONE closed edge whose start_vertex == end_vertex at
        // the seam, or a curved (non-Line) edge. A straight quad wall cannot
        // span it — the "wall" between a closed outer rim and its offset
        // (inner) rim is an ANNULAR / ruled face: outer loop = the rim, inner
        // loop (hole) = the offset rim. Detect that case per edge and build
        // the ruled wall; otherwise keep the straight-quad path so box shells
        // (straight Line rim edges) do not regress.
        let tol = options.common.tolerance.distance();
        for loop_data in &rim_loops {
            for (i, &edge_id) in loop_data.edges.iter().enumerate() {
                let forward = loop_data.orientations[i];
                if edge_is_closed_or_curved(model, edge_id, tol)? {
                    let wall_face = create_curved_rim_wall(
                        model,
                        edge_id,
                        face.surface_id,
                        removed_normal,
                        original_to_offset_edge,
                        source_to_offset_surface,
                    )?;
                    wall_faces.push(wall_face);
                } else {
                    let wall_face = create_wall_face(
                        model,
                        edge_id,
                        thickness,
                        forward,
                        removed_normal,
                        tol,
                        original_to_offset_edge,
                        vertex_insets,
                        &mut corner_edges,
                    )?;
                    wall_faces.push(wall_face);
                }
            }
        }
    }

    Ok(wall_faces)
}

/// True when a removed-face boundary edge cannot be spanned by a straight
/// planar quad wall — i.e. it is a CLOSED edge (`start_vertex ==
/// end_vertex`, the cap-rim seam case) or a non-linear (curved) edge.
/// Straight `Line` edges (box rims) return `false` and keep the
/// quad-wall path.
fn edge_is_closed_or_curved(model: &BRepModel, edge_id: EdgeId, tol: f64) -> OperationResult<bool> {
    let edge = model.edges.get(edge_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "edge_is_closed_or_curved: edge {edge_id} not found"
        ))
    })?;
    if edge.is_loop() {
        return Ok(true);
    }
    let curve = model.curves.get(edge.curve_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "edge_is_closed_or_curved: curve {} of edge {edge_id} not found",
            edge.curve_id
        ))
    })?;
    Ok(!curve.is_linear(Tolerance::new(tol, tol)))
}

/// Characteristic radius of a closed rim edge: the mean distance from its
/// sampled curve points to their centroid. Used to decide which of two
/// concentric rims (a rim and its in-plane offset) bounds the annular wall
/// from outside.
fn closed_edge_mean_radius(model: &BRepModel, edge_id: EdgeId) -> OperationResult<f64> {
    const SAMPLES: usize = 24;
    let edge = model.edges.get(edge_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "closed_edge_mean_radius: edge {edge_id} not found"
        ))
    })?;
    let curve = model.curves.get(edge.curve_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "closed_edge_mean_radius: curve {} not found",
            edge.curve_id
        ))
    })?;
    let mut pts: Vec<Point3> = Vec::with_capacity(SAMPLES);
    for i in 0..SAMPLES {
        let t = i as f64 / SAMPLES as f64;
        if let Ok(p) = curve.point_at(t) {
            pts.push(p);
        }
    }
    if pts.is_empty() {
        return Ok(0.0);
    }
    let mut c = Point3::new(0.0, 0.0, 0.0);
    for p in &pts {
        c = c + *p;
    }
    c = c * (1.0 / pts.len() as f64);
    let mean = pts.iter().map(|p| (*p - c).magnitude()).sum::<f64>() / pts.len() as f64;
    Ok(mean)
}

/// Create the ruled / annular wall for a CLOSED or CURVED cap-rim edge.
///
/// A planar cap removed from a cylinder / cone / sphere / revolved / lofted
/// / NURBS solid leaves a rim that is a single closed curve (the cap's outer
/// loop edge, `start_vertex == end_vertex` at the seam). The "wall" joining
/// that outer rim to the inward-offset interior is therefore an ANNULAR face
/// lying in the cap's own plane: its outer loop is the rim edge, its inner
/// loop (hole) is the OFFSET rim edge that the adjacent interior offset face
/// already created. Sharing that offset edge is what makes the result a
/// closed 2-manifold — the offset rim edge is then used by exactly two faces
/// (this wall + the interior offset surface).
///
/// The wall reuses the removed cap's own surface (`cap_surface_id`), so it is
/// exactly coplanar with the rim. The inner-loop hole's curve is rebuilt as a
/// true in-plane inset of the rim curve (toward the cap interior) so it is
/// geometrically consistent with its already-correctly-inset endpoint vertex
/// — and, since the interior offset surface shares this same edge, that
/// rebuild makes the offset rim sound on both faces.
fn create_curved_rim_wall(
    model: &mut BRepModel,
    rim_edge_id: EdgeId,
    cap_surface_id: u32,
    removed_face_outward_normal: Vector3,
    original_to_offset_edge: &std::collections::HashMap<EdgeId, EdgeId>,
    source_to_offset_surface: &std::collections::HashMap<EdgeId, u32>,
) -> OperationResult<FaceId> {
    use crate::primitives::face::FaceOrientation;
    use crate::primitives::r#loop::LoopType;

    use super::orientation::orient_face_for_outward;

    // The adjacent interior offset face (e.g. the offset lateral cylinder /
    // NURBS surface) created the offset rim edge while it was being built.
    // Reuse it verbatim as the inner-loop boundary — that shared edge is the
    // manifold seam between this wall and the interior surface.
    let &offset_rim_edge_id = original_to_offset_edge.get(&rim_edge_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "create_curved_rim_wall: no offset edge for cap rim edge {rim_edge_id}; \
             the adjacent interior face was not offset (the cap rim must be shared \
             with a kept, offset face)"
        ))
    })?;

    // Decide the wall topology by whether the offset rim lies in the cap plane.
    //
    // For an ANALYTIC offset surface (cylinder / revolved cap) the rim is offset
    // radially with no axial component, so the offset rim stays in the cap plane
    // and the wall is a flat annulus on the cap's own surface. For a NURBS
    // lateral the offset is a control-net normal-push: at a barrel end the
    // inward push has an axial component, so the offset rim sits OFF the cap
    // plane (a small slanted collar) and the offset surface never reaches the
    // cap plane at all. Forcing such a rim onto the planar cap surface makes the
    // wall's planar tessellation degenerate and leaks the rim (~200 slivers).
    //
    // Test the AS-CREATED offset rim (the surface-normal offset, before any
    // reshape): its planarity relative to the cap plane is the discriminator.
    let normal = removed_face_outward_normal
        .normalize()
        .unwrap_or(removed_face_outward_normal);
    let cap_point = {
        let rim_edge = model.edges.get(rim_edge_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "create_curved_rim_wall: rim edge {rim_edge_id} not found"
            ))
        })?;
        let rim_curve = model.curves.get(rim_edge.curve_id).ok_or_else(|| {
            OperationError::InvalidGeometry("create_curved_rim_wall: rim curve not found".into())
        })?;
        rim_curve.point_at(0.0).map_err(|e| {
            OperationError::InvalidGeometry(format!("create_curved_rim_wall: rim eval: {e}"))
        })?
    };
    let offset_in_cap_plane =
        offset_rim_lies_in_plane(model, offset_rim_edge_id, cap_point, normal)?;

    if offset_in_cap_plane {
        // ---- Flat annular wall on the cap plane (analytic-offset case). ----
        //
        // The offset rim is already in the cap plane; reshape it to the exact
        // analytic in-plane inset of the cap rim (a no-op when the curve cannot
        // be analytically offset) so the flat annulus is geometrically clean.
        rebuild_offset_rim_curve(
            model,
            rim_edge_id,
            offset_rim_edge_id,
            removed_face_outward_normal,
        )?;
        //
        // Decide which rim bounds the annulus from outside. Inset of a convex
        // cap's OUTER rim shrinks it (rim is outer, offset is the hole); but on
        // the INNER rim of an annular cap (washer / tube end) the material lies
        // OUTSIDE the original rim — the offset rim is larger and must be the
        // outer loop. Compare the two closed curves' characteristic radius
        // about their shared centroid and put the larger one on the outer loop.
        let rim_radius = closed_edge_mean_radius(model, rim_edge_id)?;
        let offset_radius = closed_edge_mean_radius(model, offset_rim_edge_id)?;
        let (outer_edge, inner_edge) = if offset_radius > rim_radius {
            (offset_rim_edge_id, rim_edge_id)
        } else {
            (rim_edge_id, offset_rim_edge_id)
        };

        let mut outer_loop = Loop::new(0, LoopType::Outer);
        outer_loop.add_edge(outer_edge, true);
        let outer_loop_id = model.loops.add(outer_loop);

        let mut inner_loop = Loop::new(0, LoopType::Inner);
        inner_loop.add_edge(inner_edge, false);
        let inner_loop_id = model.loops.add(inner_loop);

        let cap_surface = model.surfaces.get(cap_surface_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "create_curved_rim_wall: cap surface {cap_surface_id} not found"
            ))
        })?;
        let orientation = orient_face_for_outward(cap_surface, removed_face_outward_normal)
            .unwrap_or(FaceOrientation::Forward);

        let mut wall_face = Face::new(0, cap_surface_id, outer_loop_id, orientation);
        wall_face.add_inner_loop(inner_loop_id);
        return Ok(model.faces.add(wall_face));
    }

    // ---- Ruled-band collar wall (off-plane offset rim, e.g. NURBS barrel). ----
    //
    // Rebuild the offset rim as the offset SURFACE's exact iso-curve at this cap
    // end. The as-created offset rim is a single-direction translate of the cap
    // rim (`Curve::offset(d, fixed_dir)`), which on a closed ring is a sideways
    // SHIFT (radius oscillates ±d) rather than a clean concentric ring — the
    // resulting collar self-overlaps and disagrees with the interior offset
    // face's own surface boundary. The offset surface's iso-curve is the true,
    // clean ring AND is exactly what the interior face tessellates to at its
    // rim, so installing it makes the collar non-self-intersecting and the seam
    // bit-coincident with the interior lateral. (Best-effort: if the iso-curve
    // is unavailable the as-created rim is kept.)
    if let Some(&offset_surface_id) = source_to_offset_surface.get(&rim_edge_id) {
        rebuild_offset_rim_from_iso_curve(
            model,
            rim_edge_id,
            offset_rim_edge_id,
            offset_surface_id,
            normal,
        )?;
    }

    // The wall surface is the ruled surface between the two closed rims; the
    // dedicated annular-band tessellator stitches the original-rim ring to the
    // offset-rim ring (both drawn from the shared edge cache). Outer loop = the
    // original cap rim, inner loop = the offset rim — the same two-loop
    // structure the flat wall uses, so the ruled-band tessellator and the
    // shell-validator both see one closed edge per loop.
    create_ruled_band_wall(
        model,
        rim_edge_id,
        offset_rim_edge_id,
        removed_face_outward_normal,
    )
}

/// Replace the offset rim edge's curve with the offset SURFACE's iso-curve at
/// the cap end this rim belongs to (the v-end of the offset NURBS lateral whose
/// 3D position is nearest the cap plane through `cap_normal`·rim). This is the
/// true ring the interior offset face tessellates to at its boundary, so the
/// collar's inner rim and the interior face's rim become bit-coincident, and
/// the ring is clean (no single-direction-translate radius oscillation).
fn rebuild_offset_rim_from_iso_curve(
    model: &mut BRepModel,
    rim_edge_id: EdgeId,
    offset_rim_edge_id: EdgeId,
    offset_surface_id: u32,
    cap_normal: Vector3,
) -> OperationResult<()> {
    use crate::primitives::curve::{Curve, NurbsCurve};
    use crate::primitives::surface::GeneralNurbsSurface;

    // Cap-plane reference point (a point on the original rim).
    let cap_point = {
        let rim_edge = model.edges.get(rim_edge_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "rebuild_offset_rim_from_iso_curve: rim edge {rim_edge_id} not found"
            ))
        })?;
        let rim_curve = model.curves.get(rim_edge.curve_id).ok_or_else(|| {
            OperationError::InvalidGeometry(
                "rebuild_offset_rim_from_iso_curve: rim curve not found".into(),
            )
        })?;
        match rim_curve.point_at(0.0) {
            Ok(p) => p,
            Err(_) => return Ok(()),
        }
    };

    // Extract the NURBS iso-curve at whichever v-end (0 or 1) of the offset
    // surface sits on this cap's side.
    let surface = model.surfaces.get(offset_surface_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "rebuild_offset_rim_from_iso_curve: offset surface {offset_surface_id} not found"
        ))
    })?;
    let nurbs = match surface.as_any().downcast_ref::<GeneralNurbsSurface>() {
        Some(g) => &g.nurbs,
        // Non-NURBS offset surface (e.g. analytic) — keep the as-created rim.
        None => return Ok(()),
    };
    // Signed distance of each v-end's seam point to the cap plane; the smaller
    // |distance| identifies this cap's end.
    let end_for = |v: f64| -> Option<f64> {
        surface
            .point_at(0.0, v)
            .ok()
            .map(|p| ((p - cap_point).dot(&cap_normal)).abs())
    };
    let v_end = match (end_for(0.0), end_for(1.0)) {
        (Some(d0), Some(d1)) => {
            if d0 <= d1 {
                0.0
            } else {
                1.0
            }
        }
        _ => return Ok(()),
    };

    let iso = match nurbs.iso_curve_v(v_end) {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };
    let iso_prim = match NurbsCurve::new(
        iso.degree,
        iso.control_points,
        iso.weights,
        iso.knots.to_vec(),
    ) {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };
    // Seam point of the iso-curve → snap the closed offset rim edge's vertex.
    let seam = match iso_prim.point_at(0.0) {
        Ok(p) => p,
        Err(_) => return Ok(()),
    };
    let new_curve_id = model.curves.add(Box::new(iso_prim));
    let (sv, ev) = {
        let e = model.edges.get(offset_rim_edge_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "rebuild_offset_rim_from_iso_curve: offset edge {offset_rim_edge_id} not found"
            ))
        })?;
        (e.start_vertex, e.end_vertex)
    };
    model.vertices.set_position(sv, seam.x, seam.y, seam.z);
    if ev != sv {
        model.vertices.set_position(ev, seam.x, seam.y, seam.z);
    }
    if let Some(e) = model.edges.get_mut(offset_rim_edge_id) {
        e.curve_id = new_curve_id;
    }
    Ok(())
}

/// True when the closed offset-rim edge lies entirely within the plane through
/// `plane_point` with unit `plane_normal` (every sample's signed distance is
/// below a relative tolerance). Used to choose between the flat annular wall
/// (rim in the cap plane) and the ruled-band collar (rim off the plane).
fn offset_rim_lies_in_plane(
    model: &BRepModel,
    offset_rim_edge_id: EdgeId,
    plane_point: Point3,
    plane_normal: Vector3,
) -> OperationResult<bool> {
    const SAMPLES: usize = 24;
    let edge = model.edges.get(offset_rim_edge_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "offset_rim_lies_in_plane: edge {offset_rim_edge_id} not found"
        ))
    })?;
    let curve = model.curves.get(edge.curve_id).ok_or_else(|| {
        OperationError::InvalidGeometry("offset_rim_lies_in_plane: curve not found".into())
    })?;
    let mut max_d = 0.0_f64;
    let mut mean_r = 0.0_f64;
    let mut count = 0usize;
    for i in 0..SAMPLES {
        let t = i as f64 / SAMPLES as f64;
        if let Ok(p) = curve.point_at(t) {
            let d = (p - plane_point).dot(&plane_normal).abs();
            max_d = max_d.max(d);
            mean_r += (p - plane_point).magnitude();
            count += 1;
        }
    }
    if count == 0 {
        return Ok(false);
    }
    mean_r /= count as f64;
    // Relative tolerance: the rim is "in plane" only if its out-of-plane drift
    // is a negligible fraction of its own extent. 1e-6 keeps the analytic
    // cylinder / revolved caps (drift ~0) on the flat path while routing the
    // NURBS barrel collar (drift ≫ that) to the ruled band.
    Ok(max_d <= mean_r.max(1.0) * 1e-6)
}

/// Build a ruled-band collar wall between a closed cap rim and its (off-plane)
/// offset rim. The wall surface is `RuledSurface(rim_curve, offset_rim_curve)`;
/// outer loop = the original rim, inner loop = the offset rim. The dedicated
/// `tessellate_ruled_annular_band` path stitches the two cached rings into a
/// watertight collar. Both rims are shared edges, so the collar coincides with
/// the adjacent exterior / interior laterals at every rim sample.
fn create_ruled_band_wall(
    model: &mut BRepModel,
    rim_edge_id: EdgeId,
    offset_rim_edge_id: EdgeId,
    removed_face_outward_normal: Vector3,
) -> OperationResult<FaceId> {
    use crate::primitives::face::FaceOrientation;
    use crate::primitives::r#loop::LoopType;
    use crate::primitives::surface::RuledSurface;

    use super::orientation::orient_face_for_outward;

    let rim_curve = {
        let e = model.edges.get(rim_edge_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "create_ruled_band_wall: rim edge {rim_edge_id} not found"
            ))
        })?;
        let c = model.curves.get(e.curve_id).ok_or_else(|| {
            OperationError::InvalidGeometry("create_ruled_band_wall: rim curve not found".into())
        })?;
        c.clone_box()
    };
    let offset_curve = {
        let e = model.edges.get(offset_rim_edge_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "create_ruled_band_wall: offset edge {offset_rim_edge_id} not found"
            ))
        })?;
        let c = model.curves.get(e.curve_id).ok_or_else(|| {
            OperationError::InvalidGeometry("create_ruled_band_wall: offset curve not found".into())
        })?;
        c.clone_box()
    };

    let ruled = RuledSurface::new(rim_curve, offset_curve);
    let surface_id = model.surfaces.add(Box::new(ruled));

    // Outer loop = original rim (forward); inner loop = offset rim (backward),
    // the standard hole winding. The tessellator reads the two closed loop
    // edges directly, so winding only needs to satisfy the B-Rep validator.
    let mut outer_loop = Loop::new(0, LoopType::Outer);
    outer_loop.add_edge(rim_edge_id, true);
    let outer_loop_id = model.loops.add(outer_loop);

    let mut inner_loop = Loop::new(0, LoopType::Inner);
    inner_loop.add_edge(offset_rim_edge_id, false);
    let inner_loop_id = model.loops.add(inner_loop);

    let surface_ref = model.surfaces.get(surface_id).ok_or_else(|| {
        OperationError::InvalidGeometry("create_ruled_band_wall: surface vanished".into())
    })?;
    let orientation = orient_face_for_outward(surface_ref, removed_face_outward_normal)
        .unwrap_or(FaceOrientation::Forward);

    let mut wall_face = Face::new(0, surface_id, outer_loop_id, orientation);
    wall_face.add_inner_loop(inner_loop_id);
    Ok(model.faces.add(wall_face))
}

/// Replace the curve on `offset_rim_edge_id` with a geometrically exact
/// in-plane inset of the cap rim curve, so the offset rim is consistent with
/// its (already inset) endpoint vertex and lies on the cap plane.
///
/// `cap_normal` is the removed cap's outward normal. The inset direction is
/// chosen in-plane, pointing from the rim toward its interior (the cap
/// centroid), so `Curve::offset` shrinks the closed rim by the same distance
/// the endpoint vertex was inset.
fn rebuild_offset_rim_curve(
    model: &mut BRepModel,
    rim_edge_id: EdgeId,
    offset_rim_edge_id: EdgeId,
    cap_normal: Vector3,
) -> OperationResult<()> {
    // Original rim curve + its parametric midpoint (the rim's far side).
    let rim_edge = model.edges.get(rim_edge_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "rebuild_offset_rim_curve: rim edge {rim_edge_id} not found"
        ))
    })?;
    let rim_curve_id = rim_edge.curve_id;
    let rim_curve = model.curves.get(rim_curve_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "rebuild_offset_rim_curve: rim curve {rim_curve_id} not found"
        ))
    })?;
    let rim_start = rim_curve.point_at(0.0).map_err(|e| {
        OperationError::InvalidGeometry(format!("rebuild_offset_rim_curve: rim eval: {e}"))
    })?;

    // The offset edge's stored endpoint is the correctly-inset rim point.
    let offset_edge = model.edges.get(offset_rim_edge_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "rebuild_offset_rim_curve: offset edge {offset_rim_edge_id} not found"
        ))
    })?;
    let off_start_arr = model
        .vertices
        .get_position(offset_edge.start_vertex)
        .ok_or_else(|| {
            OperationError::InvalidGeometry(
                "rebuild_offset_rim_curve: offset start vertex not found".into(),
            )
        })?;
    let off_start = Point3::new(off_start_arr[0], off_start_arr[1], off_start_arr[2]);

    // In-plane inset direction at the seam: project (offset_start − rim_start)
    // onto the cap plane. Its magnitude is the inset distance; its direction
    // points toward the cap interior. A near-zero projection (degenerate /
    // already coincident) leaves the existing offset curve untouched.
    let raw = off_start - rim_start;
    let n = match cap_normal.normalize() {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };
    let in_plane = raw - n * raw.dot(&n);
    let inset = in_plane.magnitude();
    if inset <= f64::EPSILON {
        return Ok(());
    }
    let inset_dir = in_plane * (1.0 / inset);

    // Offset the rim curve in-plane toward the interior by `inset`. For a
    // closed analytic rim (circle / arc) this is an exact concentric inset.
    let offset_curve = match rim_curve.offset(inset, &inset_dir) {
        Ok(c) => c,
        // If the curve cannot be analytically offset (degenerate radius, an
        // unsupported curve type), keep the interior face's existing offset
        // curve — the shared edge still makes the wall manifold.
        Err(_) => return Ok(()),
    };

    // Verify the rebuilt curve actually passes through the inset endpoint
    // before committing; an offset whose sign went the wrong way would land
    // on the far (grown) side. Only replace when it matches.
    if let Ok(test) = offset_curve.point_at(0.0) {
        if (test - off_start).magnitude() <= inset.max(1.0) * 1e-3 {
            let new_curve_id = model.curves.add(offset_curve);
            if let Some(e) = model.edges.get_mut(offset_rim_edge_id) {
                e.curve_id = new_curve_id;
            }
        }
    }

    Ok(())
}

/// Add (or reuse) the straight corner edge shared by two adjacent walls.
///
/// Two walls meet at every removed-face boundary vertex; the corner edge
/// running from that boundary vertex to its inset partner is common to BOTH
/// walls. `corner_edges` (keyed on the unordered vertex pair, scoped to the
/// shell op) ensures the edge is created exactly once and reused by the second
/// wall — without it each wall makes its own coincident edge and every corner
/// edge is used by a single face, leaving the shell open.
///
/// Returns `(edge_id, forward)` where `forward` is the loop-orientation flag:
/// `true` when the stored edge already runs `start → end`, `false` when the
/// caller traverses it in reverse (the reusing wall walks the shared edge the
/// opposite way). The reused edge's straight curve is geometrically identical
/// to the one the caller would have built (same two endpoints), so reusing it
/// is exact.
fn add_or_find_corner_edge(
    model: &mut BRepModel,
    corner_edges: &mut std::collections::HashMap<(VertexId, VertexId), EdgeId>,
    start: VertexId,
    end: VertexId,
    p_start: Vector3,
    p_end: Vector3,
) -> OperationResult<(EdgeId, bool)> {
    use crate::primitives::curve::Line;
    use crate::primitives::edge::EdgeOrientation;

    let key = if start <= end {
        (start, end)
    } else {
        (end, start)
    };
    if let Some(&existing) = corner_edges.get(&key) {
        let edge = model.edges.get(existing).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "add_or_find_corner_edge: cached corner edge {existing} not found"
            ))
        })?;
        // The shared edge has a fixed stored orientation. If it already runs
        // start→end this wall traverses it forward; otherwise backward.
        let forward = edge.start_vertex == start && edge.end_vertex == end;
        return Ok((existing, forward));
    }

    let line = Line::new(p_start, p_end);
    let curve_id = model.curves.add(Box::new(line));
    let edge_id = model.edges.add(Edge::new_auto_range(
        0,
        start,
        end,
        curve_id,
        EdgeOrientation::Forward,
    ));
    corner_edges.insert(key, edge_id);
    Ok((edge_id, true))
}

/// Create a wall face between outer and inner edges.
///
/// `removed_face_outward_normal` is the outward normal of the face being
/// removed (i.e., the opening). Walls extend along `-thickness *
/// outward_normal` so they meet the inward-offset interior faces in a
/// coplanar fashion at the boundary loop.
#[allow(clippy::too_many_arguments)]
fn create_wall_face(
    model: &mut BRepModel,
    outer_edge_id: EdgeId,
    thickness: f64,
    forward: bool,
    removed_face_outward_normal: Vector3,
    tol: f64,
    original_to_offset_edge: &std::collections::HashMap<EdgeId, EdgeId>,
    vertex_insets: &HashMap<VertexId, Point3>,
    corner_edges: &mut std::collections::HashMap<(VertexId, VertexId), EdgeId>,
) -> OperationResult<FaceId> {
    use crate::primitives::curve::Line;
    use crate::primitives::edge::EdgeOrientation;
    use crate::primitives::r#loop::LoopType;
    use crate::primitives::surface::Plane;

    use super::orientation::orient_face_for_outward;

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

    // Wall offset direction lies IN the plane of the removed face,
    // perpendicular to the boundary edge, pointing toward the face
    // interior. CCW outer loop convention (viewed from +n) means
    // `loop_edge_dir × n` points OUTward from the face interior; the
    // inward in-plane perpendicular is therefore `n × loop_edge_dir`.
    //
    // The wall is a quad co-planar with the removed face, going from
    // the outer rim to the offset rim — which is exactly where the
    // inward-offset interior faces meet the opening boundary, so wall
    // and interior faces share their boundary edge.
    //
    // The earlier `(-removed_face_outward_normal)` formulation made
    // walls extend perpendicular to the removed face *into* the solid,
    // which left a dangling wall not connected to any interior offset
    // face — manifold regression caught while wiring 42-C edge sharing.
    let loop_edge_dir = if forward { edge_dir } else { -edge_dir };
    let offset_dir = removed_face_outward_normal
        .cross(&loop_edge_dir)
        .normalize()?;

    // Rim corners. Prefer the shared corner insets (intersection of the inward-
    // offset planes meeting at each vertex) so the rim is correctly mitered AND
    // its corners coincide with the interior offset edges (dedup-merge). The
    // in-plane single-edge offset is the fallback for vertices without an inset
    // (e.g. on a non-planar face).
    let offset = offset_dir * thickness.abs();
    let p3 = match vertex_insets.get(&outer_edge.end_vertex) {
        Some(&p) => Vector3::new(p.x, p.y, p.z),
        None => p2 + offset,
    };
    let p4 = match vertex_insets.get(&outer_edge.start_vertex) {
        Some(&p) => Vector3::new(p.x, p.y, p.z),
        None => p1 + offset,
    };

    // The wall plane's normal is perpendicular to both the edge
    // direction and the in-plane offset direction; this is the same
    // convention the wall loop construction below assumes (CCW in the
    // wall plane from the +wall_normal side).
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

    // Create four edges for the rectangular face.
    //
    // The two SIDE edges (e_right / e_left) run from a removed-face boundary
    // vertex (e.g. a box corner) to its inset partner. EVERY such side edge is
    // shared by the TWO adjacent walls meeting at that corner — wall A's
    // e_right and the neighbour wall B's e_left span the identical vertex pair.
    // The boundary-loop vertices are already dedup'd (the rim vertices via the
    // shared outer loop, the insets via `add_or_find` below), so the two walls
    // request the SAME (start,end) vertex pair. Creating a fresh edge per wall
    // left each corner edge used by exactly one face — 8 dangling boundary
    // edges on a top-removed cube, an OPEN (non-watertight, Euler≠2) B-Rep.
    //
    // Deduplicate the side edges through `corner_edges`, keyed on the
    // unordered vertex pair and scoped to this shell op. The first wall to
    // reach a corner creates the edge; the second reuses it, traversing it in
    // whichever direction its loop needs (the stored edge has a fixed
    // start→end orientation, so the reuse carries the matching loop flag).
    let e_top = outer_edge_id; // reuse outer edge
    let (e_right, e_right_forward) =
        add_or_find_corner_edge(model, corner_edges, outer_edge.end_vertex, v3, p2, p3)?;

    // If the adjacent face was offset, the interior offset face has already
    // created the edge that runs along the wall/interior boundary. Reuse
    // that edge so wall and interior face share their meeting boundary,
    // giving a topologically watertight shell along the opening.
    //
    // The interior offset edge was created with start_vertex =
    // offset(orig.start_vertex) (positionally p4) and end_vertex =
    // offset(orig.end_vertex) (positionally p3), so its stored direction
    // is p4→p3. The wall's bottom traverses p3→p4, i.e., the reverse
    // direction — hence `e_bottom_forward = false` on the reused edge.
    //
    // Vertex dedup via `add_or_find(.., tol)` above guarantees v3 equals
    // the offset edge's end_vertex and v4 equals its start_vertex.
    let (e_bottom, e_bottom_forward) =
        if let Some(&offset_edge_id) = original_to_offset_edge.get(&outer_edge_id) {
            (offset_edge_id, false)
        } else {
            let line_bottom = Line::new(p3, p4);
            let c_bottom = model.curves.add(Box::new(line_bottom));
            let edge = model.edges.add(Edge::new_auto_range(
                0,
                v3,
                v4,
                c_bottom,
                EdgeOrientation::Forward,
            ));
            (edge, true)
        };

    let (e_left, e_left_forward) =
        add_or_find_corner_edge(model, corner_edges, v4, outer_edge.start_vertex, p4, p1)?;

    // Create loop
    let mut wall_loop = Loop::new(0, LoopType::Outer);
    wall_loop.add_edge(e_top, forward);
    wall_loop.add_edge(e_right, e_right_forward);
    wall_loop.add_edge(e_bottom, e_bottom_forward);
    wall_loop.add_edge(e_left, e_left_forward);
    let loop_id = model.loops.add(wall_loop);

    // The wall lies in the plane of the removed face, so its outward
    // direction (out of the resulting hollow solid) is the removed
    // face's outward normal itself. Slice 2 of the comprehensive
    // face-orientation fix.
    let wall_surface_ref = model
        .surfaces
        .get(surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Wall surface not found".into()))?;
    let orientation = orient_face_for_outward(wall_surface_ref, removed_face_outward_normal)?;

    // Create face
    let face = Face::new(0, surface_id, loop_id, orientation);
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
    // #29 — scope verdict to the shelled solid (see validate_solid_scoped).
    let result = crate::primitives::validation::validate_solid_scoped(
        model,
        solid_id,
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

    // ------------------------------------------------------------------
    // Shell-op (offset_solid) tests.
    //
    // The wall-direction regression discussed in c39bfd6 / 71bdc1d /
    // 8140327 boils down to a single observable invariant: walls erected
    // at the boundary of a removed face must lie IN the plane of that
    // face, not perpendicular to it. These tests check that invariant
    // empirically against the real B-Rep produced by offset_solid, so a
    // future direction regression cannot silently land.
    // ------------------------------------------------------------------

    use crate::operations::recorder::{OperationRecorder, RecordedOperation, RecorderError};
    use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
    use std::sync::{Arc, Mutex};

    /// Build a 10×10×10 box centred at the origin and locate the +Z
    /// (top) face by surface normal. Returns (model, solid_id, top_face_id).
    fn box_with_top_face() -> (BRepModel, SolidId, FaceId) {
        let mut model = BRepModel::new();
        let mut builder = TopologyBuilder::new(&mut model);
        let solid_id = match builder
            .create_box_3d(10.0, 10.0, 10.0)
            .expect("create_box_3d should succeed for positive dimensions")
        {
            GeometryId::Solid(id) => id,
            other => panic!("create_box_3d must return Solid, got {:?}", other),
        };

        // The +Z face is identified by surface normal ≈ (0,0,1) AND the
        // face plane evaluating at +hd on Z. Iterating faces directly
        // avoids depending on the box-face index ordering.
        let solid = model
            .solids
            .get(solid_id)
            .expect("solid must exist")
            .clone();
        let shell = model
            .shells
            .get(solid.outer_shell)
            .expect("outer shell must exist")
            .clone();
        let mut top_face = None;
        for &face_id in &shell.faces {
            let face = model.faces.get(face_id).expect("face must exist");
            let surface = model.surfaces.get(face.surface_id).expect("surface");
            let n = surface
                .normal_at(0.5, 0.5)
                .expect("normal_at must succeed for a planar box face");
            if (n.z - 1.0).abs() < 1e-9 && n.x.abs() < 1e-9 && n.y.abs() < 1e-9 {
                top_face = Some(face_id);
                break;
            }
        }
        let top_face_id = top_face.expect("box must have exactly one +Z face");
        (model, solid_id, top_face_id)
    }

    /// Build a 10×10×10 box via the LIVE user construction path: a sketched
    /// rectangle profile face extruded to a prism, NOT the `create_box_3d`
    /// primitive. This is the exact kernel sequence that `extrude_sketch`
    /// (api-server) drives: four `Line` edges → `create_face_from_profile_
    /// with_plane` → `extrude_face`. The resulting B-Rep is what the live
    /// `POST /api/geometry/shell` actually shells, so it is the only faithful
    /// reproduction of the live failure. Returns (model, solid_id,
    /// top_face_id) with the +Z cap located by surface normal.
    fn extruded_box_with_top_face() -> (BRepModel, SolidId, FaceId) {
        use crate::operations::extrude::{
            create_face_from_profile_with_plane, extrude_face, ExtrudeOptions,
        };
        use crate::primitives::curve::Line;
        use crate::primitives::edge::EdgeOrientation;

        let mut model = BRepModel::new();

        // Rectangle [0,10]×[0,10] on the XY plane, four corner vertices
        // shared between adjacent edges (each corner used by exactly two
        // edges — the same sharing the api-server `build_loop_edges` emits).
        let tol = Tolerance::default().distance();
        let v0 = model.vertices.add_or_find(0.0, 0.0, 0.0, tol);
        let v1 = model.vertices.add_or_find(10.0, 0.0, 0.0, tol);
        let v2 = model.vertices.add_or_find(10.0, 10.0, 0.0, tol);
        let v3 = model.vertices.add_or_find(0.0, 10.0, 0.0, tol);
        let mut add_line = |model: &mut BRepModel, a: VertexId, b: VertexId| -> EdgeId {
            let pa = model.vertices.get_position(a).expect("vertex a");
            let pb = model.vertices.get_position(b).expect("vertex b");
            let line = Line::new(
                Point3::new(pa[0], pa[1], pa[2]),
                Point3::new(pb[0], pb[1], pb[2]),
            );
            let curve_id = model.curves.add(Box::new(line));
            model.edges.add(Edge::new_auto_range(
                0,
                a,
                b,
                curve_id,
                EdgeOrientation::Forward,
            ))
        };
        let profile_edges = vec![
            add_line(&mut model, v0, v1),
            add_line(&mut model, v1, v2),
            add_line(&mut model, v2, v3),
            add_line(&mut model, v3, v0),
        ];

        let face_id = create_face_from_profile_with_plane(
            &mut model,
            profile_edges,
            Point3::new(0.0, 0.0, 0.0),
            Vector3::Z,
        )
        .expect("profile face construction must succeed");

        let opts = ExtrudeOptions {
            distance: 10.0,
            direction: Vector3::Z,
            cap_ends: true,
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            ..Default::default()
        };
        let solid_id =
            extrude_face(&mut model, face_id, opts).expect("extrude_face on profile must succeed");

        let solid = model.solids.get(solid_id).expect("solid").clone();
        let shell = model
            .shells
            .get(solid.outer_shell)
            .expect("outer shell")
            .clone();
        let mut top_face = None;
        for &fid in &shell.faces {
            let face = model.faces.get(fid).expect("face");
            let surface = model.surfaces.get(face.surface_id).expect("surface");
            let mut n = surface.normal_at(0.5, 0.5).expect("normal_at");
            if matches!(
                face.orientation,
                crate::primitives::face::FaceOrientation::Backward
            ) {
                n = -n;
            }
            // The +Z cap sits at z = 10; pick the face whose outward normal
            // is +Z and whose plane passes through the top.
            if (n.z - 1.0).abs() < 1e-6 && n.x.abs() < 1e-6 && n.y.abs() < 1e-6 {
                top_face = Some(fid);
                break;
            }
        }
        let top_face_id = top_face.expect("extruded box must have a +Z cap face");
        (model, solid_id, top_face_id)
    }

    /// Collect the 3D positions of every distinct vertex referenced by
    /// the outer loop of `face_id`. Walks the loop's edges, deduplicating
    /// by VertexId.
    fn loop_vertex_positions(model: &BRepModel, face_id: FaceId) -> Vec<Point3> {
        let face = model.faces.get(face_id).expect("face");
        let outer = model.loops.get(face.outer_loop).expect("outer loop");
        let mut seen = std::collections::HashSet::new();
        let mut positions = Vec::new();
        for &edge_id in &outer.edges {
            let edge = model.edges.get(edge_id).expect("edge");
            for vid in [edge.start_vertex, edge.end_vertex] {
                if seen.insert(vid) {
                    let p = model.vertices.get_position(vid).expect("vertex position");
                    positions.push(Point3::new(p[0], p[1], p[2]));
                }
            }
        }
        positions
    }

    /// Walls erected at a removed face's boundary must be coplanar with
    /// the removed face — every wall vertex sits on z = +hd for a top-
    /// face-removed cube. The earlier (-removed_face_outward_normal)
    /// formulation extruded walls to z = +hd - thickness instead, which
    /// disconnected them from the inward-offset interior faces.
    #[test]
    fn shell_top_face_removed_walls_lie_in_removed_face_plane() {
        let (mut model, solid_id, top_face_id) = box_with_top_face();
        let original_top_z = 5.0; // +hd for a 10x10x10 box centred at origin.
        let thickness = 1.0;

        let options = OffsetOptions {
            common: CommonOptions {
                // Skip post-shell validation: that path runs the full
                // B-Rep validator, which has independent open work
                // unrelated to the wall-direction invariant under test.
                validate_result: false,
                ..CommonOptions::default()
            },
            offset_type: OffsetType::Distance(thickness),
            intersection_handling: IntersectionHandling::Trim,
            max_deviation: 1e-3,
        };

        let hollow_id = offset_solid(&mut model, solid_id, thickness, vec![top_face_id], options)
            .expect("offset_solid on top-removed cube must succeed");

        // Identify wall faces in the resulting hollow solid:
        // - Their surface normal is parallel to the removed face's
        //   normal (±Z), AND
        // - Every vertex sits on z = +hd.
        let hollow = model.solids.get(hollow_id).expect("hollow solid").clone();
        let hollow_shell = model
            .shells
            .get(hollow.outer_shell)
            .expect("hollow outer shell")
            .clone();

        let mut wall_count = 0usize;
        for &face_id in &hollow_shell.faces {
            let face = model.faces.get(face_id).expect("face");
            let surface = model.surfaces.get(face.surface_id).expect("surface");
            let n = surface.normal_at(0.5, 0.5).expect("planar surface normal");
            // Only consider faces parallel to the removed-face plane.
            if !(n.x.abs() < 1e-6 && n.y.abs() < 1e-6 && (n.z.abs() - 1.0).abs() < 1e-6) {
                continue;
            }
            let positions = loop_vertex_positions(&model, face_id);
            // Detect by elevation: wall vertices all sit at z = +hd.
            // Original bottom face / interior bottom offset face also
            // satisfy the normal check but live at z = -hd or
            // z = -hd + thickness.
            let on_top_plane = positions
                .iter()
                .all(|p| (p.z - original_top_z).abs() < 1e-6);
            if !on_top_plane {
                continue;
            }
            wall_count += 1;
            // Each wall is a quad — exactly 4 distinct vertices.
            assert_eq!(
                positions.len(),
                4,
                "wall face {} has {} vertices, expected 4",
                face_id,
                positions.len()
            );
        }

        // Top face's outer loop has 4 edges → exactly 4 wall faces.
        assert_eq!(
            wall_count, 4,
            "expected 4 wall faces in the plane of the removed top face, found {}",
            wall_count
        );
    }

    /// Walls' inner edges must coincide with the inward-offset side
    /// faces' top edges so the new shell is manifold along the opening
    /// rim. Concretely: every wall has two vertices at z=+hd that are
    /// inset from the box boundary by exactly `thickness`. They must
    /// match the top edges of the (now offset) side faces.
    #[test]
    fn shell_top_face_removed_walls_meet_inner_offset_at_inset_distance() {
        let (mut model, solid_id, top_face_id) = box_with_top_face();
        let thickness = 1.0;
        let half_extent = 5.0;

        let options = OffsetOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            offset_type: OffsetType::Distance(thickness),
            intersection_handling: IntersectionHandling::Trim,
            max_deviation: 1e-3,
        };

        let hollow_id = offset_solid(&mut model, solid_id, thickness, vec![top_face_id], options)
            .expect("offset_solid must succeed");

        let hollow = model.solids.get(hollow_id).expect("hollow").clone();
        let hollow_shell = model.shells.get(hollow.outer_shell).expect("shell").clone();

        // Each rim (top-frame) wall is a quad: 2 OUTER corners on the cube
        // rim (the original box top corners, |x| = |y| = half_extent) and 2
        // INNER corners that are MITERED — inset by `thickness` on BOTH axes
        // (|x| = |y| = half_extent − thickness), where the two inner side walls
        // meet. (The corners are the shared `compute_vertex_insets` corners;
        // before that fix they were inset on only one axis, which left the
        // inner faces untrimmed — the 1640-vs-424 bug.)
        let inset = half_extent - thickness;
        let mut walls_checked = 0usize;
        for &face_id in &hollow_shell.faces {
            let face = model.faces.get(face_id).expect("face");
            let surface = model.surfaces.get(face.surface_id).expect("surface");
            let n = surface.normal_at(0.5, 0.5).expect("normal");
            if !(n.x.abs() < 1e-6 && n.y.abs() < 1e-6 && (n.z.abs() - 1.0).abs() < 1e-6) {
                continue;
            }
            let positions = loop_vertex_positions(&model, face_id);
            if !positions.iter().all(|p| (p.z - half_extent).abs() < 1e-6) {
                continue;
            }
            let mut outer = 0;
            let mut inner = 0;
            for p in &positions {
                let on_x_rim = (p.x.abs() - half_extent).abs() < 1e-6;
                let on_y_rim = (p.y.abs() - half_extent).abs() < 1e-6;
                let inset_x = (p.x.abs() - inset).abs() < 1e-6;
                let inset_y = (p.y.abs() - inset).abs() < 1e-6;
                if on_x_rim && on_y_rim {
                    outer += 1; // a box top corner
                } else if inset_x && inset_y {
                    inner += 1; // a mitered inner corner
                }
            }
            assert_eq!(
                outer, 2,
                "wall face {} should have 2 outer corners on the cube rim, got {}",
                face_id, outer
            );
            assert_eq!(
                inner, 2,
                "wall face {} should have 2 mitered inner corners (inset on both axes), got {}",
                face_id, inner
            );
            walls_checked += 1;
        }

        assert_eq!(walls_checked, 4, "expected to verify 4 walls");
    }

    /// Slice 42-C edge-sharing invariant: along the opening rim of a
    /// hollow solid, every wall face's bottom edge must be the SAME
    /// `EdgeId` as the adjacent interior offset face's boundary edge,
    /// not a fresh duplicate planted at the same position. This makes
    /// the resulting shell manifold along the opening — each rim edge
    /// is bordered by exactly two faces (one wall + one interior).
    ///
    /// Without sharing, the wall bottom and interior top would be two
    /// distinct edges at the same position, leaving each rim "slot"
    /// bordered by only 1 face per id and breaking the manifold
    /// invariant the validator (and downstream booleans) rely on.
    #[test]
    fn shell_wall_and_interior_share_rim_edge() {
        let (mut model, solid_id, top_face_id) = box_with_top_face();
        let thickness = 1.0;
        let half_extent = 5.0;
        let inset = half_extent - thickness; // = 4.0

        let options = OffsetOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            offset_type: OffsetType::Distance(thickness),
            intersection_handling: IntersectionHandling::Trim,
            max_deviation: 1e-3,
        };

        let hollow_id = offset_solid(&mut model, solid_id, thickness, vec![top_face_id], options)
            .expect("offset_solid must succeed");

        let hollow = model.solids.get(hollow_id).expect("hollow").clone();
        let hollow_shell = model.shells.get(hollow.outer_shell).expect("shell").clone();

        // Build edge → faces adjacency over every face in the hollow
        // shell. A manifold rim edge appears in exactly 2 of these.
        let mut edge_to_faces: std::collections::HashMap<EdgeId, Vec<FaceId>> =
            std::collections::HashMap::new();
        for &face_id in &hollow_shell.faces {
            let face = model.faces.get(face_id).expect("face");
            let outer = model.loops.get(face.outer_loop).expect("outer loop");
            for &edge_id in &outer.edges {
                edge_to_faces.entry(edge_id).or_default().push(face_id);
            }
        }

        // Locate the rim edges: both endpoints sit at z = +half_extent
        // AND each endpoint sits on the inset square (|x| ≈ inset or
        // |y| ≈ inset). The opening of a top-removed cube is a square
        // with 4 such edges.
        let mut rim_edges: Vec<(EdgeId, Vec<FaceId>)> = Vec::new();
        for (&edge_id, faces) in &edge_to_faces {
            let edge = model.edges.get(edge_id).expect("edge");
            let s = model
                .vertices
                .get_position(edge.start_vertex)
                .expect("start vertex position");
            let e = model
                .vertices
                .get_position(edge.end_vertex)
                .expect("end vertex position");
            let on_top_plane =
                (s[2] - half_extent).abs() < 1e-6 && (e[2] - half_extent).abs() < 1e-6;
            if !on_top_plane {
                continue;
            }
            let s_on_inset = (s[0].abs() - inset).abs() < 1e-6 || (s[1].abs() - inset).abs() < 1e-6;
            let e_on_inset = (e[0].abs() - inset).abs() < 1e-6 || (e[1].abs() - inset).abs() < 1e-6;
            if s_on_inset && e_on_inset {
                rim_edges.push((edge_id, faces.clone()));
            }
        }

        assert_eq!(
            rim_edges.len(),
            4,
            "expected exactly 4 rim edges along the opening, found {}",
            rim_edges.len()
        );
        for (edge_id, faces) in &rim_edges {
            assert_eq!(
                faces.len(),
                2,
                "rim edge {} must border exactly 2 faces \
                 (wall + interior); found {}: {:?}. \
                 If this is 1, walls and interior faces are using \
                 separate edges at the rim and the shell is not \
                 manifold along the opening (slice 42-C regression).",
                edge_id,
                faces.len(),
                faces
            );
        }
    }

    /// `offset_solid` must emit a `RecordedOperation` so the timeline
    /// can replay shell operations alongside booleans / extrudes /
    /// fillets. Per CLAUDE.md every kernel entry point that mutates
    /// topology records on success.
    #[test]
    fn shell_offset_solid_records_operation() {
        #[derive(Debug, Default)]
        struct CapturingRecorder(Mutex<Vec<RecordedOperation>>);
        impl OperationRecorder for CapturingRecorder {
            fn record(&self, op: RecordedOperation) -> Result<(), RecorderError> {
                self.0
                    .lock()
                    .map_err(|e| RecorderError::Other(e.to_string()))?
                    .push(op);
                Ok(())
            }
        }

        let recorder = Arc::new(CapturingRecorder::default());
        let (mut model, solid_id, top_face_id) = box_with_top_face();
        model.attach_recorder(Some(recorder.clone()));

        let _hollow_id = offset_solid(
            &mut model,
            solid_id,
            1.0,
            vec![top_face_id],
            OffsetOptions {
                common: CommonOptions {
                    validate_result: false,
                    ..CommonOptions::default()
                },
                offset_type: OffsetType::Distance(1.0),
                intersection_handling: IntersectionHandling::Trim,
                max_deviation: 1e-3,
            },
        )
        .expect("offset_solid must succeed");

        let recorded = recorder.0.lock().expect("recorder mutex").clone();
        let shell_records: Vec<_> = recorded
            .iter()
            .filter(|op| op.kind == "offset_solid")
            .collect();
        assert_eq!(
            shell_records.len(),
            1,
            "expected exactly one offset_solid recording, got {}: {:?}",
            shell_records.len(),
            recorded.iter().map(|r| &r.kind).collect::<Vec<_>>()
        );
        let rec = shell_records[0];
        assert!(
            rec.inputs.contains(&format!("solid:{}", solid_id)),
            "recording inputs must include the source solid"
        );
        assert!(
            rec.inputs.contains(&format!("face:{}", top_face_id)),
            "recording inputs must include the removed face"
        );
        assert_eq!(
            rec.outputs.len(),
            1,
            "shell op produces a single output solid"
        );
    }

    /// `create_offset_loop` must offset every edge of a kept face to
    /// the same side, regardless of the loop's per-edge `forward` flag.
    /// In a B-Rep, an outer loop traverses some edges in their natural
    /// direction (forward=true) and others reversed (forward=false) —
    /// that flag describes loop topology, NOT which side of the curve
    /// to offset to.
    ///
    /// For the +X side face of a 10×10×10 cube, the box-faces helper
    /// gives a loop with `[true, true, false, false]`. With
    /// `signed_distance = if forward { d } else { -d }`, two edges
    /// would offset inward (-X) and two outward (+X), producing a
    /// disconnected interior offset face. The full inward-offset
    /// face must instead live on the single plane x = hw - thickness.
    #[test]
    fn shell_interior_offset_face_loop_is_coplanar() {
        let (mut model, solid_id, top_face_id) = box_with_top_face();
        let half_extent = 5.0;
        let thickness = 1.0;
        let inset = half_extent - thickness; // = 4.0

        let options = OffsetOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            offset_type: OffsetType::Distance(thickness),
            intersection_handling: IntersectionHandling::Trim,
            max_deviation: 1e-3,
        };

        let hollow_id = offset_solid(&mut model, solid_id, thickness, vec![top_face_id], options)
            .expect("offset_solid must succeed");

        // The +X kept face's interior offset is the (single) face in
        // the result with surface normal ±X whose face center is at
        // x = inset. Find it via face-vertex inspection.
        let hollow = model.solids.get(hollow_id).expect("hollow").clone();
        let hollow_shell = model.shells.get(hollow.outer_shell).expect("shell").clone();

        let mut x_offset_faces = Vec::new();
        for &face_id in &hollow_shell.faces {
            let face = model.faces.get(face_id).expect("face");
            let surface = model.surfaces.get(face.surface_id).expect("surface");
            let n = surface.normal_at(0.5, 0.5).expect("normal");
            // Faces with normal ±X
            if !((n.x.abs() - 1.0).abs() < 1e-6 && n.y.abs() < 1e-6 && n.z.abs() < 1e-6) {
                continue;
            }
            let positions = loop_vertex_positions(&model, face_id);
            // Inward-offset +X face: every vertex sits at x = +inset.
            // Inward-offset -X face: every vertex sits at x = -inset.
            // Original kept faces are at x = ±half_extent.
            let all_at_pos_inset = positions.iter().all(|p| (p.x - inset).abs() < 1e-6);
            let all_at_neg_inset = positions.iter().all(|p| (p.x - (-inset)).abs() < 1e-6);
            if all_at_pos_inset || all_at_neg_inset {
                x_offset_faces.push((face_id, positions));
            }
        }

        // We expect exactly one inward-offset +X face and one inward-
        // offset -X face. With the per-edge sign-flip bug, each kept
        // X-side face produces a loop where two edges are at x=hw-t
        // and two at x=hw+t — so neither all_at_pos_inset nor
        // all_at_neg_inset holds, and this assertion fails.
        assert_eq!(
            x_offset_faces.len(),
            2,
            "expected 2 X-axis interior offset faces (one per ±X side), found {} \
             — likely a per-edge offset-direction inconsistency",
            x_offset_faces.len()
        );

        for (face_id, positions) in x_offset_faces {
            assert_eq!(
                positions.len(),
                4,
                "interior offset face {} should have 4 distinct vertices, got {}",
                face_id,
                positions.len()
            );
        }
    }

    /// Negative thickness is rejected by `validate_shell_inputs`'s
    /// |thickness| check — but that check uses `< 1e-10`, so a small
    /// negative number close to zero must still be caught. Pin the
    /// precondition.
    #[test]
    fn shell_rejects_zero_thickness() {
        let (mut model, solid_id, top_face_id) = box_with_top_face();
        let result = offset_solid(
            &mut model,
            solid_id,
            0.0,
            vec![top_face_id],
            OffsetOptions::default(),
        );
        assert!(result.is_err(), "zero thickness must be rejected");
    }

    /// Removing a face that doesn't belong to the solid's outer shell
    /// must fail loudly — otherwise wall topology would point at edges
    /// that aren't on the boundary loop, producing non-manifold output.
    #[test]
    fn shell_rejects_face_not_on_outer_shell() {
        let (mut model, solid_id, _top) = box_with_top_face();
        // Build a second box; use one of its faces as the "remove"
        // target, which definitely isn't on the first solid's shell.
        let mut builder2 = TopologyBuilder::new(&mut model);
        let other_solid = match builder2.create_box_3d(2.0, 2.0, 2.0).expect("box2") {
            GeometryId::Solid(id) => id,
            other => panic!("expected Solid, got {:?}", other),
        };
        let other_face = *model
            .shells
            .get(model.solids.get(other_solid).expect("solid").outer_shell)
            .expect("shell")
            .faces
            .first()
            .expect("at least one face");

        let result = offset_solid(
            &mut model,
            solid_id,
            0.5,
            vec![other_face],
            OffsetOptions {
                common: CommonOptions {
                    validate_result: false,
                    ..CommonOptions::default()
                },
                offset_type: OffsetType::Distance(0.5),
                intersection_handling: IntersectionHandling::Trim,
                max_deviation: 1e-3,
            },
        );
        assert!(
            result.is_err(),
            "face from a different solid must not be accepted for removal"
        );
    }

    /// B1 GATE: shelling a box with the top face removed (an open tray /
    /// cup) must produce a topologically CLOSED, watertight solid shell.
    ///
    /// "Open as a container, closed as a solid": the opening is bounded by
    /// rim walls that join the outer box walls to the inward-offset inner
    /// walls, so every edge is used by exactly two faces. Before the corner
    /// side-edge dedup fix, each of the four walls minted its own copy of the
    /// two corner edges it shares with its neighbours → 8 single-use boundary
    /// edges → an OPEN B-Rep with Euler ≠ 2 and `sound = false`.
    ///
    /// This gate asserts the full ground-truth path: B-Rep validity (the
    /// `validate_solid_scoped` Standard sweep, which fails on any boundary
    /// edge), watertightness + Euler = 2 (via `validate_shell_closure`'s exact
    /// per-edge tally), and the self-certified `ValidityCertificate` soundness
    /// flag.
    #[test]
    fn shell_top_removed_tray_is_closed_watertight_euler2_sound() {
        use crate::primitives::validation::{
            validate_shell_closure, validate_solid_scoped, ValidationLevel,
        };

        let (mut model, solid_id, top_face_id) = box_with_top_face();
        let thickness = 1.0;

        // Run the op WITH result validation on: a non-stitched rim makes
        // `validate_shell_solid` reject the result, so success here already
        // proves the rim is closed at the B-Rep level.
        let options = OffsetOptions {
            common: CommonOptions {
                validate_result: true,
                ..CommonOptions::default()
            },
            offset_type: OffsetType::Distance(thickness),
            intersection_handling: IntersectionHandling::Trim,
            max_deviation: 1e-3,
        };
        let hollow_id = offset_solid(&mut model, solid_id, thickness, vec![top_face_id], options)
            .expect("offset_solid on a top-removed cube must produce a valid closed shell");

        let hollow = model.solids.get(hollow_id).expect("hollow solid").clone();

        // (1) Exact per-edge closure: zero boundary AND zero non-manifold
        //     edges. This is the direct watertight / manifold oracle.
        let closure_errors = validate_shell_closure(&model, hollow.outer_shell);
        assert!(
            closure_errors.is_empty(),
            "shelled tray is not watertight — {} unstitched/non-manifold edge(s): {:?}",
            closure_errors.len(),
            closure_errors
                .iter()
                .take(8)
                .map(|e| format!("{e:?}"))
                .collect::<Vec<_>>()
        );

        // (2) Euler characteristic V − E + F = 2 (genus-0 closed shell). Tally
        //     the distinct vertices / edges / faces over the shell's faces.
        let shell = model.shells.get(hollow.outer_shell).expect("shell").clone();
        let mut vset = std::collections::HashSet::new();
        let mut eset = std::collections::HashSet::new();
        for &fid in &shell.faces {
            let face = model.faces.get(fid).expect("face");
            let mut loops = vec![face.outer_loop];
            loops.extend(face.inner_loops.iter().copied());
            for lid in loops {
                let lp = model.loops.get(lid).expect("loop");
                for &eid in &lp.edges {
                    eset.insert(eid);
                    if let Some(edge) = model.edges.get(eid) {
                        vset.insert(edge.start_vertex);
                        vset.insert(edge.end_vertex);
                    }
                }
            }
        }
        let euler = vset.len() as i64 - eset.len() as i64 + shell.faces.len() as i64;
        assert_eq!(
            euler,
            2,
            "shelled tray Euler V−E+F = {} (V={}, E={}, F={}), expected 2",
            euler,
            vset.len(),
            eset.len(),
            shell.faces.len()
        );

        // (3) Full B-Rep validity (Standard) scoped to the shelled solid —
        //     the same sweep `validate_shell_solid` runs. No boundary-edge gaps.
        let result = validate_solid_scoped(
            &model,
            hollow_id,
            Tolerance::default(),
            ValidationLevel::Standard,
        );
        assert!(
            result.is_valid,
            "shelled tray failed B-Rep validation ({} errors): {:?}",
            result.errors.len(),
            result
                .errors
                .iter()
                .take(5)
                .map(|e| format!("{e:?}"))
                .collect::<Vec<_>>()
        );

        // (4) Self-certified ground truth: the validity certificate must mark
        //     the shelled solid SOUND (brep_valid + watertight + manifold +
        //     self-intersection-free + tessellation/mesh clean).
        let cert = model.certify_solid(hollow_id);
        assert!(
            cert.is_sound(),
            "shelled tray ValidityCertificate is not sound: {cert:?}"
        );
    }

    /// LIVE-PATH gate: shell an EXTRUDED box (sketched rectangle → `extrude_
    /// face`), the exact B-Rep the user's `POST /api/geometry/shell` operates
    /// on, with the top cap removed and thickness 1. The primitive-box gate
    /// above passes on `create_box_3d`, but the extrude path produces a
    /// different vertex/edge structure, and a corner-edge dedup that worked
    /// for the primitive box can still leave dangling boundary edges here.
    ///
    /// This asserts the same four ground-truth pillars (per-edge closure,
    /// Euler = 2, Standard B-Rep validity, ValidityCertificate soundness) on
    /// the extrude-path tray. It is NON-VACUOUS: before the root fix it fails
    /// with boundary edges and Euler ≠ 2 (the live "17 errors / negative
    /// genus −2" failure).
    #[test]
    fn shell_top_removed_extruded_tray_is_closed_watertight_euler2_sound() {
        use crate::primitives::validation::{
            validate_shell_closure, validate_solid_scoped, ValidationLevel,
        };

        let (mut model, solid_id, top_face_id) = extruded_box_with_top_face();
        let thickness = 1.0;

        let options = OffsetOptions {
            common: CommonOptions {
                validate_result: true,
                ..CommonOptions::default()
            },
            offset_type: OffsetType::Distance(thickness),
            intersection_handling: IntersectionHandling::Trim,
            max_deviation: 1e-3,
        };
        let hollow_id = offset_solid(&mut model, solid_id, thickness, vec![top_face_id], options)
            .expect("offset_solid on a top-removed EXTRUDED box must produce a valid closed shell");

        let hollow = model.solids.get(hollow_id).expect("hollow solid").clone();

        // (1) Exact per-edge closure: zero boundary AND zero non-manifold edges.
        let closure_errors = validate_shell_closure(&model, hollow.outer_shell);
        assert!(
            closure_errors.is_empty(),
            "extruded tray is not watertight — {} unstitched/non-manifold edge(s): {:?}",
            closure_errors.len(),
            closure_errors
                .iter()
                .take(8)
                .map(|e| format!("{e:?}"))
                .collect::<Vec<_>>()
        );

        // (2) Euler characteristic V − E + F = 2 (genus-0 closed shell).
        let shell = model.shells.get(hollow.outer_shell).expect("shell").clone();
        let mut vset = std::collections::HashSet::new();
        let mut eset = std::collections::HashSet::new();
        for &fid in &shell.faces {
            let face = model.faces.get(fid).expect("face");
            let mut loops = vec![face.outer_loop];
            loops.extend(face.inner_loops.iter().copied());
            for lid in loops {
                let lp = model.loops.get(lid).expect("loop");
                for &eid in &lp.edges {
                    eset.insert(eid);
                    if let Some(edge) = model.edges.get(eid) {
                        vset.insert(edge.start_vertex);
                        vset.insert(edge.end_vertex);
                    }
                }
            }
        }
        let euler = vset.len() as i64 - eset.len() as i64 + shell.faces.len() as i64;
        assert_eq!(
            euler,
            2,
            "extruded tray Euler V−E+F = {} (V={}, E={}, F={}), expected 2",
            euler,
            vset.len(),
            eset.len(),
            shell.faces.len()
        );

        // (3) Full B-Rep validity (Standard) scoped to the shelled solid.
        let result = validate_solid_scoped(
            &model,
            hollow_id,
            Tolerance::default(),
            ValidationLevel::Standard,
        );
        assert!(
            result.is_valid,
            "extruded tray failed B-Rep validation ({} errors): {:?}",
            result.errors.len(),
            result
                .errors
                .iter()
                .take(5)
                .map(|e| format!("{e:?}"))
                .collect::<Vec<_>>()
        );

        // (4) Self-certified ground truth: SOUND.
        let cert = model.certify_solid(hollow_id);
        assert!(
            cert.is_sound(),
            "extruded tray ValidityCertificate is not sound: {cert:?}"
        );
    }
}
