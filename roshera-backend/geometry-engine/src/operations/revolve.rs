//! Revolution/Sweep Operations for B-Rep Models
//!
//! Creates solids of revolution by rotating profiles around an axis.
//! Supports partial revolutions, multiple profiles, and helical paths.

use super::lifecycle::{self, OpSpec};
use super::orientation::orient_face_for_outward;
use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Matrix4, Point3, Vector3};
use crate::primitives::{
    curve::ParameterRange,
    edge::{Edge, EdgeId, EdgeOrientation},
    face::{Face, FaceId},
    r#loop::Loop,
    shell::{Shell, ShellType},
    solid::{Solid, SolidId},
    surface::Surface,
    topology_builder::BRepModel,
    vertex::VertexId,
};

/// Options for revolution operations
#[derive(Debug, Clone)]
pub struct RevolveOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Axis origin point
    pub axis_origin: Point3,

    /// Axis direction (will be normalized)
    pub axis_direction: Vector3,

    /// Revolution angle in radians (2π for full revolution)
    pub angle: f64,

    /// Whether revolution is symmetric (extends in both directions from axis)
    pub symmetric: bool,

    /// Number of segments for discretization
    pub segments: u32,

    /// Helical pitch (0 for pure rotation)
    pub pitch: f64,

    /// Whether to create end caps for partial revolutions
    pub cap_ends: bool,
}

impl Default for RevolveOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            axis_origin: Point3::ZERO,
            axis_direction: Vector3::Z,
            angle: std::f64::consts::TAU,
            symmetric: false,
            segments: 32,
            pitch: 0.0,
            cap_ends: true,
        }
    }
}

/// Revolve a face around an axis to create a solid
pub fn revolve_face(
    model: &mut BRepModel,
    face_id: FaceId,
    options: RevolveOptions,
) -> OperationResult<SolidId> {
    // F2-δ pre-flight.
    if options.common.validate_before {
        lifecycle::validate_can_apply(model, OpSpec::RevolveFace { face_id })?;
    }

    lifecycle::with_rollback(model, move |model| {
        // Validate inputs
        validate_revolve_inputs(model, face_id, &options)?;

        // Normalize axis direction
        let axis_dir = options.axis_direction.normalize()?;

        // Get the face to revolve
        let face = model
            .faces
            .get(face_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?
            .clone();

        // Check if face intersects the axis (would create self-intersection)
        if face_intersects_axis(model, &face, options.axis_origin, axis_dir)? {
            return Err(OperationError::SelfIntersection);
        }

        // Create revolved solid
        let solid_id = if options.pitch.abs() < 1e-10 {
            // Pure revolution
            create_revolution(model, &face, face_id, &options)?
        } else {
            // Helical sweep
            create_helical_sweep(model, &face, face_id, &options)?
        };

        // PILLAR 1: a revolution is a designed surface.
        model.set_solid_provenance(
            solid_id,
            crate::primitives::provenance::OperationKind::Revolve,
            Vec::new(),
        );

        // Validate result if requested
        if options.common.validate_result {
            validate_revolved_solid(model, solid_id)?;
        }

        // Record for attached recorders.
        model.record_operation(
            crate::operations::recorder::RecordedOperation::new("revolve_face")
                .with_parameters(serde_json::json!({
                    "face_id": face_id,
                    "axis_origin": [
                        options.axis_origin.x,
                        options.axis_origin.y,
                        options.axis_origin.z,
                    ],
                    "axis_direction": [
                        options.axis_direction.x,
                        options.axis_direction.y,
                        options.axis_direction.z,
                    ],
                    "angle": options.angle,
                    "pitch": options.pitch,
                    "segments": options.segments,
                    "cap_ends": options.cap_ends,
                }))
                .with_input_faces([face_id as u64])
                .with_output_solids([solid_id as u64]),
        );

        Ok(solid_id)
    })
}

/// Revolve a wire/profile to create a solid
pub fn revolve_profile(
    model: &mut BRepModel,
    profile_edges: Vec<EdgeId>,
    options: RevolveOptions,
) -> OperationResult<SolidId> {
    if options.common.validate_before {
        lifecycle::validate_can_apply(
            model,
            OpSpec::RevolveProfile {
                profile_edges: &profile_edges,
            },
        )?;
    }
    lifecycle::with_rollback(model, move |model| {
        let face_id = create_face_from_profile(model, profile_edges)?;
        revolve_face(model, face_id, options)
    })
}

/// Revolve a set of sketch-plane profile regions (typed analytic loops
/// and/or chord-sampled polygons) about an IN-PLANE axis — the csketch
/// route's kernel entry AND the timeline `sketch_revolve` replay arm
/// (SKETCH-DCM #45 follow-ups B, item 5). One code path, no
/// live-vs-replay drift — mirrors `extrude_profile_regions`.
///
/// Frame: `lift(p) = origin + u_axis·p[0] + v_axis·p[1]`; the axis is
/// given in the SAME plane coordinates (`axis_origin_2d` +
/// `axis_direction_2d`) and lifted through the same map, so the whole
/// event payload stays 2D and self-contained.
///
/// Loop payloads route through the SAME builders as extrude:
/// typed `Line`/`Arc`/`Nurbs` edges become exact kernel curves (the
/// closed-NURBS seam split included), so the analytic-band revolve
/// path (#19/#21) sees true geometry — axis-parallel lines → Cylinder
/// bands, axis-perpendicular → planar annuli, sloped → Cone bands,
/// curved edges → ONE `SurfaceOfRevolution` face each.
///
/// Honest refusal (typed): a full-circle profile edge revolved about
/// an external axis is a TORUS lateral, which the revolve builder has
/// no analytic band for — callers sample such loops explicitly
/// (counted, never silently approximated).
///
/// Holes are revolved separately and SUBTRACTED (the proven
/// click-draft region scheme); disjoint regions are Union-folded.
#[allow(clippy::too_many_arguments)] // Reason: mirrors extrude_profile_regions' established frame+payload signature (plus the in-plane axis pair); bundling would diverge the two shared kernel entries for no clarity gain.
pub fn revolve_profile_regions(
    model: &mut BRepModel,
    origin: Point3,
    u_axis: Vector3,
    v_axis: Vector3,
    regions: &[crate::operations::extrude::ProfileRegion],
    axis_origin_2d: [f64; 2],
    axis_direction_2d: [f64; 2],
    angle: f64,
    segments: u32,
    tolerance: crate::math::Tolerance,
) -> OperationResult<SolidId> {
    use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use crate::operations::extrude::{
        build_analytic_loop, build_polygon_loop, create_face_from_profile_with_plane,
        lift_plane_point, ProfileLoop,
    };
    use crate::sketch2d::sketch_topology::ProfileEdge;

    if regions.is_empty() {
        return Err(OperationError::InvalidGeometry(
            "revolve_profile_regions: no regions supplied".to_string(),
        ));
    }
    if !angle.is_finite() || angle.abs() < 1e-9 {
        return Err(OperationError::InvalidGeometry(format!(
            "revolve angle must be non-zero and finite (got {angle})"
        )));
    }
    let normal = u_axis.cross(&v_axis);

    // Lift the in-plane axis through the SAME frame map as the profile
    // points, so axis and profile can never disagree about the plane.
    let axis_origin = lift_plane_point(origin, u_axis, v_axis, axis_origin_2d);
    let axis_direction = u_axis * axis_direction_2d[0] + v_axis * axis_direction_2d[1];
    let axis_direction = axis_direction.normalize().map_err(|e| {
        OperationError::NumericalError(format!("revolve axis normalization: {e:?}"))
    })?;

    // Typed refusal BEFORE any kernel mutation: a full-circle profile
    // edge would revolve to a torus lateral.
    for region in regions {
        for lp in std::iter::once(&region.outer).chain(region.holes.iter()) {
            if let ProfileLoop::Edges(edges) = lp {
                if edges
                    .iter()
                    .any(|e| matches!(e, ProfileEdge::Circle { .. }))
                {
                    return Err(OperationError::InvalidGeometry(
                        "analytic full-circle profiles have no typed revolve path yet \
                         (a revolved circle's lateral is a TORUS band the analytic-band \
                         builder does not emit); sample the circle into a polygon"
                            .to_string(),
                    ));
                }
            }
        }
    }

    let build = |model: &mut BRepModel, lp: &ProfileLoop| -> OperationResult<Vec<EdgeId>> {
        match lp {
            ProfileLoop::Polygon(poly) => {
                let lifted: Vec<Point3> = poly
                    .iter()
                    .map(|p| lift_plane_point(origin, u_axis, v_axis, *p))
                    .collect();
                build_polygon_loop(model, &lifted, tolerance.distance())
            }
            ProfileLoop::Edges(edges) => build_analytic_loop(
                model,
                origin,
                u_axis,
                v_axis,
                normal,
                // The circle/oblique guard inside `build_analytic_loop`
                // keys on the extrude direction; revolve has none, and
                // circle edges were already refused above — pass the
                // plane normal so the guard is inert.
                normal,
                edges,
                tolerance.distance(),
            ),
        }
    };

    let options = RevolveOptions {
        axis_origin,
        axis_direction,
        angle,
        segments,
        ..RevolveOptions::default()
    };

    let mut region_solids: Vec<SolidId> = Vec::with_capacity(regions.len());
    for region in regions {
        let outer_edges = build(model, &region.outer)?;
        let outer_face = create_face_from_profile_with_plane(model, outer_edges, origin, normal)?;
        let mut region_solid = revolve_face(model, outer_face, options.clone())?;

        // Holes: revolve the hole profile about the SAME axis and
        // subtract — the annular void is exactly the hole's solid of
        // revolution (the proven click-draft scheme; the analytic-band
        // revolve consumes single-loop faces, so cap-hole topology is
        // expressed through the boolean, not inner loops).
        for hole in &region.holes {
            let hole_edges = build(model, hole)?;
            let hole_face = create_face_from_profile_with_plane(model, hole_edges, origin, normal)?;
            let hole_solid = revolve_face(model, hole_face, options.clone())?;
            region_solid = boolean_operation(
                model,
                region_solid,
                hole_solid,
                BooleanOp::Difference,
                BooleanOptions::default(),
            )?;
        }
        region_solids.push(region_solid);
    }

    let mut region_iter = region_solids.into_iter();
    let mut accumulator = region_iter.next().ok_or_else(|| {
        OperationError::InvalidGeometry(
            "revolve_profile_regions: no region solids built".to_string(),
        )
    })?;
    for sid in region_iter {
        accumulator = boolean_operation(
            model,
            accumulator,
            sid,
            BooleanOp::Union,
            BooleanOptions::default(),
        )?;
    }
    Ok(accumulator)
}

/// Orthonormal `(â, ê1, ê2)` frame of a revolution axis, with `ê1` the canonical
/// radial reference direction at which a `(r, z)` meridian's `r` is laid out.
///
/// `ê1` follows the SAME convention as [`crate::primitives::curve::Arc::new`]'s
/// `x_axis` (±Z→+X, ±X→+Y, ±Y→+Z, otherwise `axis.perpendicular()`), so that:
///   * the default axis (`+Z`) gives `ê1 = +X` — a `(r, z)` point lifts to the
///     world `(r, 0, z)` half-plane, IDENTICAL to the original behaviour (no
///     regression for the overwhelmingly common default-axis case), and
///   * the radial reference lines up with the seam direction the analytic-band
///     and meridian-arc paths already anchor to (`Circle::x_axis()`), so the
///     lifted profile, its ring seams, and the revolved surface all agree.
///
/// `ê2 = â × ê1` completes a right-handed frame. The result is the foundation of
/// the fix for a NON-ORIGIN / NON-Z axis: `(r, z)` is (radius-from-axis,
/// height-along-axis) in THIS frame, never world coordinates — so `axis_origin`
/// translates the whole solid and `r` stays a pure radius, instead of the axis
/// offset leaking into the radius/bbox.
fn axis_frame(axis_direction: Vector3) -> OperationResult<(Vector3, Vector3, Vector3)> {
    let a = axis_direction.normalize()?;
    const AXIS_EPS: f64 = 1e-10;
    let e1 = if (a - Vector3::Z).magnitude() < AXIS_EPS || (a + Vector3::Z).magnitude() < AXIS_EPS {
        Vector3::X
    } else if (a - Vector3::X).magnitude() < AXIS_EPS || (a + Vector3::X).magnitude() < AXIS_EPS {
        Vector3::Y
    } else if (a - Vector3::Y).magnitude() < AXIS_EPS || (a + Vector3::Y).magnitude() < AXIS_EPS {
        Vector3::Z
    } else {
        a.perpendicular().normalize()?
    };
    let e2 = a.cross(&e1).normalize()?;
    Ok((a, e1, e2))
}

/// Lift a `(r, z)` meridian point into world space in the axis frame:
/// `axis_origin + z·â + r·ê1`. This is the single point where the half-plane
/// profile becomes a world point; every revolve builder routes its profile
/// vertices AND its retained construction geometry through it, so the geometry
/// and the consistency check always agree on where the profile lives.
fn lift_meridian(axis_origin: Point3, axis: Vector3, e1: Vector3, r: f64, z: f64) -> Point3 {
    axis_origin + axis * z + e1 * r
}

/// Recover the `(r, z)` of a lifted world meridian point — the inverse of
/// [`lift_meridian`]: `z = (p − origin)·â`, `r = |(p − origin) − z·â|`. Used by
/// [`get_revolve_meridian`] so a part built about ANY axis still reads back its
/// editable half-plane profile.
fn unlift_meridian(axis_origin: Point3, axis: Vector3, p: Point3) -> (f64, f64) {
    let rel = p - axis_origin;
    let z = rel.dot(&axis);
    let r = (rel - axis * z).magnitude();
    (r, z)
}

/// Parametric revolve from a MERIDIAN profile — the scientist-facing form of a
/// solid of revolution. `profile_rz` is the `(r, z)` half-plane meridian (r =
/// radius from the axis, z = height along it); it is lifted into the axis frame
/// (`axis_origin + z·â + r·ê1`), joined by line edges (auto-closed last→first)
/// and revolved. For the default axis this is the world `(r, 0, z)` plane; for an
/// offset/rotated axis the profile rides the axis so `axis_origin` translates the
/// whole solid and `r` stays a pure radius (the offset never leaks into the
/// radius/bbox). The generating meridian is RETAINED as the solid's construction
/// geometry, so the part remembers how it was made — the foundation of the
/// edit→regenerate workflow (#25): an edit recovers this profile, changes it, and
/// re-revolves.
pub fn revolve_meridian(
    model: &mut BRepModel,
    profile_rz: &[(f64, f64)],
    options: RevolveOptions,
) -> OperationResult<SolidId> {
    use crate::primitives::curve::{Line, ParameterRange};
    use crate::primitives::edge::{Edge, EdgeOrientation};

    if profile_rz.len() < 3 {
        return Err(OperationError::InvalidGeometry(
            "revolve_meridian: need at least 3 meridian points".to_string(),
        ));
    }

    let axis_origin = options.axis_origin;
    let (axis, e1, _e2) = axis_frame(options.axis_direction)?;
    let n = profile_rz.len();
    let lifted: Vec<Point3> = profile_rz
        .iter()
        .map(|&(r, z)| lift_meridian(axis_origin, axis, e1, r, z))
        .collect();
    let verts: Vec<_> = lifted
        .iter()
        .map(|p| model.vertices.add(p.x, p.y, p.z))
        .collect();
    let mut edges = Vec::with_capacity(n);
    let mut meridian_pts = Vec::with_capacity(n);
    for i in 0..n {
        let j = (i + 1) % n;
        meridian_pts.push(lifted[i]);
        let line = Line::new(lifted[i], lifted[j]);
        let cid = model.curves.add(Box::new(line));
        edges.push(model.edges.add(Edge::new(
            0,
            verts[i],
            verts[j],
            cid,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )));
    }

    let solid = revolve_profile(model, edges, options)?;

    // Retain the generating meridian as construction geometry (the source-profile
    // link, consistency-checked + carried through transforms) so the part
    // remembers its profile for the edit→regenerate workflow.
    model.set_solid_construction(
        solid,
        crate::primitives::provenance::ConstructionGeometry::revolution(
            axis_origin,
            axis,
            meridian_pts,
        ),
    );
    Ok(solid)
}

/// Recover the `(r, z)` meridian a revolved part was built from, read back from
/// its retained construction geometry — the editable profile a scientist opens,
/// changes, and re-revolves with [`revolve_meridian`]. The stored points are the
/// world-frame lift of the half-plane profile; the EXACT revolution axis is read
/// from the construction geometry (`plane_origin` = axis origin,
/// `revolution_axis` = unit direction) and each world point is projected back to
/// `(r, z)` (radius from axis, height along it) via [`unlift_meridian`]. For the
/// default `+Z`-through-origin axis this reduces to `r = x`, `z = z`. Returns
/// `None` when the solid carries no retained REVOLVED meridian (it was not built
/// by a revolve meridian builder), completing the edit→regenerate loop's recover
/// step (#25). A transform rotates the stored axis in lock-step, so the readback
/// stays exact after the part is moved.
pub fn get_revolve_meridian(model: &BRepModel, solid: SolidId) -> Option<Vec<(f64, f64)>> {
    let cg = model.solid_construction(solid)?;
    // Only a revolved part carries a recorded revolution axis; a sketch-derived
    // construction record (extrude) has none and is not an editable meridian.
    let axis = cg.revolution_axis?;
    let axis_origin = cg.plane_origin;
    Some(
        cg.profile_points
            .iter()
            .map(|&p| unlift_meridian(axis_origin, axis, p))
            .collect(),
    )
}

/// Parametric revolve of a SMOOTH (NURBS-spline) walled TUBE — the smooth-meridian
/// form for nozzles, vases and bell profiles (#9 + #25). `wall_rz` is the OUTER
/// wall meridian (the `(r, z)` points, bottom→top, every `r > bore_radius`); it
/// is interpolated by a degree-3 NURBS so the revolved outer wall is ONE smooth
/// `SurfaceOfRevolution` (not a faceted polyline of `P × segments` tiny faces —
/// the original nozzle complaint). The meridian closes through a cylindrical
/// `bore_radius` bore and two annular caps, so it never touches the axis (which
/// would force the per-segment grid fallback) — the hollow form a real nozzle
/// takes. The wall points are retained as construction geometry for the
/// edit→regenerate loop.
pub fn revolve_spline_meridian(
    model: &mut BRepModel,
    wall_rz: &[(f64, f64)],
    bore_radius: f64,
    options: RevolveOptions,
) -> OperationResult<SolidId> {
    use crate::math::nurbs::{interpolate_nurbs_curve, ParameterizationType};
    use crate::primitives::curve::{Line, NurbsCurve, ParameterRange};
    use crate::primitives::edge::{Edge, EdgeOrientation};

    if wall_rz.len() < 4 {
        return Err(OperationError::InvalidGeometry(
            "revolve_spline_meridian: need at least 4 wall points for a cubic spline".to_string(),
        ));
    }
    if !(bore_radius > 0.0) {
        return Err(OperationError::InvalidGeometry(
            "revolve_spline_meridian: bore_radius must be > 0".to_string(),
        ));
    }
    if wall_rz.iter().any(|&(r, _)| r <= bore_radius) {
        return Err(OperationError::InvalidGeometry(
            "revolve_spline_meridian: every wall radius must exceed bore_radius".to_string(),
        ));
    }

    let axis_origin = options.axis_origin;
    let (axis, e1, _e2) = axis_frame(options.axis_direction)?;
    let lift = |r: f64, z: f64| lift_meridian(axis_origin, axis, e1, r, z);
    let (r0, z0) = wall_rz[0];
    let (r1, z1) = wall_rz[wall_rz.len() - 1];

    // Fit a degree-3 NURBS through the outer wall meridian (lifted into the axis
    // frame) — the single smooth wall curve.
    let wall_pts: Vec<Point3> = wall_rz.iter().map(|&(r, z)| lift(r, z)).collect();
    let wall_math = interpolate_nurbs_curve(&wall_pts, 3, ParameterizationType::ChordLength)
        .map_err(|e| OperationError::NumericalError(format!("wall spline fit: {e}")))?;
    let wall_curve = NurbsCurve::new(
        wall_math.degree,
        wall_math.control_points,
        wall_math.weights,
        wall_math.knots.to_vec(),
    )
    .map_err(|e| OperationError::NumericalError(format!("wall curve: {e:?}")))?;

    // Tube meridian (same winding as the analytic-band tube: outer-bottom →
    // outer-top → inner-top → inner-bottom). The outer wall is the smooth NURBS;
    // the bore is a straight cylinder; the caps are annular planes. No vertex sits
    // on the axis, so the analytic one-surface band path applies to the wall.
    let p_out_bot = lift(r0, z0);
    let p_out_top = lift(r1, z1);
    let p_in_top = lift(bore_radius, z1);
    let p_in_bot = lift(bore_radius, z0);
    let v_out_bot = model.vertices.add(p_out_bot.x, p_out_bot.y, p_out_bot.z);
    let v_out_top = model.vertices.add(p_out_top.x, p_out_top.y, p_out_top.z);
    let v_in_top = model.vertices.add(p_in_top.x, p_in_top.y, p_in_top.z);
    let v_in_bot = model.vertices.add(p_in_bot.x, p_in_bot.y, p_in_bot.z);

    let c_wall = model.curves.add(Box::new(wall_curve));
    let c_top = model.curves.add(Box::new(Line::new(p_out_top, p_in_top)));
    let c_bore = model.curves.add(Box::new(Line::new(p_in_top, p_in_bot)));
    let c_bottom = model.curves.add(Box::new(Line::new(p_in_bot, p_out_bot)));

    let edges = vec![
        model.edges.add(Edge::new(
            0,
            v_out_bot,
            v_out_top,
            c_wall,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )),
        model.edges.add(Edge::new(
            0,
            v_out_top,
            v_in_top,
            c_top,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )),
        model.edges.add(Edge::new(
            0,
            v_in_top,
            v_in_bot,
            c_bore,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )),
        model.edges.add(Edge::new(
            0,
            v_in_bot,
            v_out_bot,
            c_bottom,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )),
    ];

    let solid = revolve_profile(model, edges, options)?;

    // Retain the outer wall meridian (lifted construction geometry) for
    // edit→regenerate, co-located with the solid for the consistency check.
    let meridian_pts: Vec<Point3> = wall_rz.iter().map(|&(r, z)| lift(r, z)).collect();
    model.set_solid_construction(
        solid,
        crate::primitives::provenance::ConstructionGeometry::revolution(
            axis_origin,
            axis,
            meridian_pts,
        ),
    );
    Ok(solid)
}

/// Parametric revolve of a SMOOTH SOLID of revolution — the solid analogue of
/// `revolve_spline_meridian` (which is hollow). Fits ONE degree-3 NURBS through
/// the wall meridian and revolves it to a SINGLE `SurfaceOfRevolution` face, so a
/// nose cone / dome / teardrop / bullet is ONE smooth wall + flat cap(s) with
/// ZERO meridian band rings — not `P` cone-frustum bands welded rim-to-rim.
///
/// `profile_rz` is the meridian `(r, z)`; a leading on-axis point (the base
/// centre) is dropped and rebuilt as the disc closure. The wall runs base-rim →
/// tip; an APEX tip (`r ≈ 0`) closes straight down the axis (the pole the
/// analytic revolve + pole-fan tessellation now handle), a FINITE tip gets a
/// second disc. The wall meridian is retained as construction geometry.
pub fn revolve_smooth_solid(
    model: &mut BRepModel,
    profile_rz: &[(f64, f64)],
    options: RevolveOptions,
) -> OperationResult<SolidId> {
    use crate::math::nurbs::{interpolate_nurbs_curve, ParameterizationType};
    use crate::primitives::curve::{Line, NurbsCurve, ParameterRange};
    use crate::primitives::edge::{Edge, EdgeOrientation};

    let eps = 1e-4;
    // Drop a leading on-axis (base-centre) pole; the wall is the rest.
    let wall: &[(f64, f64)] = match profile_rz.first() {
        Some(&(r, _)) if r < eps => &profile_rz[1..],
        _ => profile_rz,
    };
    if wall.len() < 4 {
        return Err(OperationError::InvalidGeometry(
            "revolve_smooth_solid: need at least 4 wall points for a cubic spline".to_string(),
        ));
    }
    if wall[0].0 < eps {
        return Err(OperationError::InvalidGeometry(
            "revolve_smooth_solid: wall must start at a finite base rim".to_string(),
        ));
    }

    let axis_origin = options.axis_origin;
    let (axis, e1, _e2) = axis_frame(options.axis_direction)?;
    let lift = |r: f64, z: f64| lift_meridian(axis_origin, axis, e1, r, z);

    // ONE degree-3 NURBS through the wall meridian (lifted into the axis frame).
    let wall_pts: Vec<Point3> = wall.iter().map(|&(r, z)| lift(r, z)).collect();
    let wall_math = interpolate_nurbs_curve(&wall_pts, 3, ParameterizationType::ChordLength)
        .map_err(|e| OperationError::NumericalError(format!("wall spline fit: {e}")))?;
    let wall_curve = NurbsCurve::new(
        wall_math.degree,
        wall_math.control_points,
        wall_math.weights,
        wall_math.knots.to_vec(),
    )
    .map_err(|e| OperationError::NumericalError(format!("wall curve: {e:?}")))?;

    let (r_rim, z_rim) = wall[0];
    let (r_tip, z_tip) = wall[wall.len() - 1];
    let p_rim = lift(r_rim, z_rim);
    let p_tip = lift(r_tip, z_tip);
    let p_base_c = lift(0.0, z_rim); // base centre on the axis
    let v_rim = model.vertices.add(p_rim.x, p_rim.y, p_rim.z);
    let v_tip = model.vertices.add(p_tip.x, p_tip.y, p_tip.z);
    let v_base_c = model.vertices.add(p_base_c.x, p_base_c.y, p_base_c.z);

    let c_base = model.curves.add(Box::new(Line::new(p_base_c, p_rim))); // base disc radius
    let c_wall = model.curves.add(Box::new(wall_curve)); // the smooth wall

    let mut edges = vec![
        model.edges.add(Edge::new(
            0,
            v_base_c,
            v_rim,
            c_base,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )),
        model.edges.add(Edge::new(
            0,
            v_rim,
            v_tip,
            c_wall,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )),
    ];
    if r_tip < eps {
        // Apex on the axis → close the meridian straight down the axis.
        let c_axis = model.curves.add(Box::new(Line::new(p_tip, p_base_c)));
        edges.push(model.edges.add(Edge::new(
            0,
            v_tip,
            v_base_c,
            c_axis,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )));
    } else {
        // Finite (blunt) tip → a second disc cap, then the axis closure.
        let p_tip_c = lift(0.0, z_tip);
        let v_tip_c = model.vertices.add(p_tip_c.x, p_tip_c.y, p_tip_c.z);
        let c_tip = model.curves.add(Box::new(Line::new(p_tip, p_tip_c)));
        let c_axis = model.curves.add(Box::new(Line::new(p_tip_c, p_base_c)));
        edges.push(model.edges.add(Edge::new(
            0,
            v_tip,
            v_tip_c,
            c_tip,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )));
        edges.push(model.edges.add(Edge::new(
            0,
            v_tip_c,
            v_base_c,
            c_axis,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )));
    }

    let solid = revolve_profile(model, edges, options)?;

    let meridian_pts: Vec<Point3> = wall.iter().map(|&(r, z)| lift(r, z)).collect();
    model.set_solid_construction(
        solid,
        crate::primitives::provenance::ConstructionGeometry::revolution(
            axis_origin,
            axis,
            meridian_pts,
        ),
    );
    Ok(solid)
}

/// Parametric revolve of a SMOOTH constant-thickness AXISYMMETRIC SHELL — the
/// correct primitive for a Rao bell nozzle (or any contoured vessel). `inner_rz`
/// is the FLOW/inner meridian (the `(r, z)` contour, inlet→exit, every `r > 0`);
/// the outer wall is `inner` offset radially by `wall_thickness`. BOTH contours
/// are interpolated by degree-3 NURBS and revolved, so the inner AND outer walls
/// are each ONE smooth `SurfaceOfRevolution` — EXACT circular cross-sections AND
/// a smooth contour, with NO band rings (the faceted-revolve artifact) and NO
/// loft seam (the nurbs_loft notch). Two planar annular rims close it into a
/// watertight wall. The inner contour is retained as construction geometry for
/// the edit→regenerate loop.
pub fn revolve_smooth_nozzle(
    model: &mut BRepModel,
    inner_rz: &[(f64, f64)],
    wall_thickness: f64,
    options: RevolveOptions,
) -> OperationResult<SolidId> {
    use crate::math::nurbs::{interpolate_nurbs_curve, ParameterizationType};
    use crate::primitives::curve::{Line, NurbsCurve, ParameterRange};
    use crate::primitives::edge::{Edge, EdgeOrientation};

    if inner_rz.len() < 4 {
        return Err(OperationError::InvalidGeometry(
            "revolve_smooth_nozzle: need at least 4 inner contour points".to_string(),
        ));
    }
    if !(wall_thickness > 0.0) {
        return Err(OperationError::InvalidGeometry(
            "revolve_smooth_nozzle: wall_thickness must be > 0".to_string(),
        ));
    }
    if inner_rz.iter().any(|&(r, _)| r <= 0.0) {
        return Err(OperationError::InvalidGeometry(
            "revolve_smooth_nozzle: every inner radius must be > 0".to_string(),
        ));
    }

    let axis_origin = options.axis_origin;
    let (axis, e1, _e2) = axis_frame(options.axis_direction)?;
    let lift = |r: f64, z: f64| lift_meridian(axis_origin, axis, e1, r, z);
    let (ir0, z0) = inner_rz[0]; // inner inlet
    let (ir1, z1) = inner_rz[inner_rz.len() - 1]; // inner exit
    let (or0, or1) = (ir0 + wall_thickness, ir1 + wall_thickness);

    // Fit a degree-3 NURBS through the `(r, z)` contour, clamp the control-point
    // RADII in the half-plane, THEN lift the (clamped) control points into the
    // axis frame. The radius clamp MUST happen in `(r, z)` space — `r` is the
    // radial coordinate regardless of where the axis sits — so it stays correct
    // for an offset/rotated axis (the old `cp.x` clamp assumed a world-X radius).
    let fit = |pts: Vec<(f64, f64)>| -> OperationResult<NurbsCurve> {
        let rmin = pts.iter().map(|&(r, _)| r).fold(f64::INFINITY, f64::min);
        let rmax = pts
            .iter()
            .map(|&(r, _)| r)
            .fold(f64::NEG_INFINITY, f64::max);
        // Fit in the canonical `(r, 0, z)` half-plane (so the clamp axis is X),
        // identical numerics to the original for any axis.
        let p: Vec<Point3> = pts.iter().map(|&(r, z)| Point3::new(r, 0.0, z)).collect();
        let m = interpolate_nurbs_curve(&p, 3, ParameterizationType::ChordLength)
            .map_err(|e| OperationError::NumericalError(format!("nozzle spline fit: {e}")))?;
        // Clamp the control-point RADII to the contour's [rmin, rmax]. A B-spline
        // lies inside its control hull, so a control point whose radius exceeds the
        // data range bulges the curve past the contour — cubic interpolation
        // overshoot at the sharp chamber/throat corners (chord-length overshot to
        // r≈60, centripetal to r≈66, from a max contour radius of 52), and the
        // revolved seam captured the spike → a non-rotationally-symmetric wall.
        // Clamping the radius guarantees no overshoot; the throat (rmin) and exit
        // (rmax) sit at the bounds so they are preserved. Lift each clamped CP into
        // the axis frame (`(r, z)` → `axis_origin + z·â + r·ê1`).
        let cps: Vec<Point3> = m
            .control_points
            .iter()
            .map(|&cp| lift(cp.x.clamp(rmin, rmax), cp.z))
            .collect();
        NurbsCurve::new(m.degree, cps, m.weights, m.knots.to_vec())
            .map_err(|e| OperationError::NumericalError(format!("nozzle curve: {e:?}")))
    };

    // Outer wall = inner offset radially by wall_thickness (so the inlet/exit rims
    // stay planar annuli). Inner is reversed (exit→inlet) so its forward edge runs
    // the inner-exit→inner-inlet leg of the tube loop.
    let outer_curve = fit(inner_rz
        .iter()
        .map(|&(r, z)| (r + wall_thickness, z))
        .collect())?;
    let inner_curve = fit(inner_rz.iter().rev().cloned().collect())?;

    // Tube winding: outer-inlet → outer-exit → inner-exit → inner-inlet.
    let p_out_bot = lift(or0, z0);
    let p_out_top = lift(or1, z1);
    let p_in_top = lift(ir1, z1);
    let p_in_bot = lift(ir0, z0);
    let v_out_bot = model.vertices.add(p_out_bot.x, p_out_bot.y, p_out_bot.z);
    let v_out_top = model.vertices.add(p_out_top.x, p_out_top.y, p_out_top.z);
    let v_in_top = model.vertices.add(p_in_top.x, p_in_top.y, p_in_top.z);
    let v_in_bot = model.vertices.add(p_in_bot.x, p_in_bot.y, p_in_bot.z);

    let c_outer = model.curves.add(Box::new(outer_curve));
    let c_exit = model.curves.add(Box::new(Line::new(p_out_top, p_in_top)));
    let c_inner = model.curves.add(Box::new(inner_curve));
    let c_inlet = model.curves.add(Box::new(Line::new(p_in_bot, p_out_bot)));

    let edges = vec![
        model.edges.add(Edge::new(
            0,
            v_out_bot,
            v_out_top,
            c_outer,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )),
        model.edges.add(Edge::new(
            0,
            v_out_top,
            v_in_top,
            c_exit,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )),
        model.edges.add(Edge::new(
            0,
            v_in_top,
            v_in_bot,
            c_inner,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )),
        model.edges.add(Edge::new(
            0,
            v_in_bot,
            v_out_bot,
            c_inlet,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )),
    ];

    let solid = revolve_profile(model, edges, options)?;

    let meridian_pts: Vec<Point3> = inner_rz.iter().map(|&(r, z)| lift(r, z)).collect();
    model.set_solid_construction(
        solid,
        crate::primitives::provenance::ConstructionGeometry::revolution(
            axis_origin,
            axis,
            meridian_pts,
        ),
    );
    Ok(solid)
}

/// Create a pure revolution (no helical component) as a watertight B-Rep.
///
/// Builds a SHARED vertex/edge grid rather than independent per-quad islands:
///   * one ring of vertices per profile vertex (station 0 reuses the original
///     profile vertex; a full revolution wraps station `segments` back to 0; a
///     profile vertex on the axis collapses to a single shared apex),
///   * shared meridian arcs between angular stations, and
///   * shared profile-arc edges at each station.
/// Every quad face then shares all four borders with its neighbours, so the
/// shell is a closed 2-manifold. For a partial revolution the start/end caps
/// are rebuilt from the SAME station-0 / station-`segments` arcs (not fresh
/// geometry) so the caps seal watertight too.
///
/// The previous implementation created fresh vertices/edges for every quad,
/// leaving every edge single-use — a non-manifold shell with a broken Euler
/// characteristic. It only ever "passed" because the call sites set
/// `validate_result = false`.
fn create_revolution(
    model: &mut BRepModel,
    base_face: &Face,
    base_face_id: FaceId,
    options: &RevolveOptions,
) -> OperationResult<SolidId> {
    use crate::primitives::r#loop::LoopType;
    use std::collections::HashMap;

    let is_full = (options.angle - std::f64::consts::TAU).abs() < 1e-10;
    let segments = options.segments.max(3);
    let axis_origin = options.axis_origin;
    let axis = options.axis_direction.normalize()?;
    let seg_angle = options.angle / segments as f64;

    // #19: analytic-band fast path. A full revolution of a straight-line
    // (rectilinear v1) meridian profile is a stack of coaxial analytic bands —
    // Cylinder walls + annular Plane caps sharing ring-circle edges — exactly the
    // watertight structure `create_cylinder_topology` uses. Self-verifying: it
    // builds the minimal analytic faces, checks the result is a valid watertight
    // solid, and on ANY failure rolls back so the proven per-segment grid path
    // below runs on a clean model (zero regression by construction).
    if is_full {
        if let Some(sid) = try_analytic_band_revolution(model, base_face, base_face_id, options)? {
            return Ok(sid);
        }
    }

    let base_loop = model
        .loops
        .get(base_face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?
        .clone();
    let base_surface_id = base_face.surface_id;

    // Rotation about the axis line (through axis_origin), reused throughout.
    let rot_about_axis = |angle: f64| -> OperationResult<Matrix4> {
        let to_origin = Matrix4::from_translation(&-axis_origin);
        let from_origin = Matrix4::from_translation(&axis_origin);
        Ok(from_origin * Matrix4::from_axis_angle(&axis, angle)? * to_origin)
    };

    // Profile edges in loop order, endpoints honouring loop orientation.
    let mut prof: Vec<(u32, VertexId, VertexId)> = Vec::new();
    for (idx, &eid) in base_loop.edges.iter().enumerate() {
        let e = model.edges.get(eid).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "revolve: profile edge {eid} (slot {idx}) not found"
            ))
        })?;
        let fwd = base_loop.orientations.get(idx).copied().unwrap_or(true);
        let (s, en) = if fwd {
            (e.start_vertex, e.end_vertex)
        } else {
            (e.end_vertex, e.start_vertex)
        };
        prof.push((e.curve_id, s, en));
    }

    // Unique profile vertices, first-seen order.
    let mut uniq: Vec<VertexId> = Vec::new();
    for &(_, s, en) in &prof {
        if !uniq.contains(&s) {
            uniq.push(s);
        }
        if !uniq.contains(&en) {
            uniq.push(en);
        }
    }

    // Distinct angular stations: full wraps (segments), partial is open (segments+1).
    let n_stations = if is_full { segments } else { segments + 1 };

    // Vertex ring per profile vertex (single shared apex when on the axis).
    let mut rings: HashMap<VertexId, Vec<VertexId>> = HashMap::new();
    let mut apex: HashMap<VertexId, bool> = HashMap::new();
    for &pv in &uniq {
        let pos = model.vertices.get_position(pv).ok_or_else(|| {
            OperationError::InvalidGeometry(format!("revolve: profile vertex {pv} not found"))
        })?;
        let p = Vector3::new(pos[0], pos[1], pos[2]);
        let rel = p - axis_origin;
        let radial = rel - axis * rel.dot(&axis);
        if radial.magnitude() < 1e-9 {
            apex.insert(pv, true);
            rings.insert(pv, vec![pv]);
        } else {
            apex.insert(pv, false);
            let mut ring = Vec::with_capacity(n_stations as usize);
            ring.push(pv);
            for s in 1..n_stations {
                let rp = rot_about_axis(seg_angle * s as f64)?.transform_point(&p);
                ring.push(model.vertices.add(rp.x, rp.y, rp.z));
            }
            rings.insert(pv, ring);
        }
    }

    let vid_at = |pv: VertexId, s: u32| -> VertexId {
        if apex[&pv] {
            return rings[&pv][0];
        }
        let ring = &rings[&pv];
        let idx = if is_full {
            (s % segments) as usize
        } else {
            s as usize
        };
        ring[idx]
    };

    // Meridian arcs per (profile vertex, segment). Apex vertices contribute none
    // (their faces degenerate to triangles).
    let mut merid: HashMap<(VertexId, u32), EdgeId> = HashMap::new();
    for &pv in &uniq {
        if apex[&pv] {
            continue;
        }
        for s in 0..segments {
            let a = vid_at(pv, s);
            let b = vid_at(pv, s + 1);
            let eid = create_meridian_edge(
                model,
                a,
                b,
                axis_origin,
                axis,
                seg_angle * s as f64,
                seg_angle * (s + 1) as f64,
            )?;
            merid.insert((pv, s), eid);
        }
    }

    // Profile-arc edges: a rotated copy of each profile edge at each station,
    // sharing the ring vertices. Full revolution wraps station `segments` → 0.
    //
    // A profile edge whose BOTH endpoints lie on the axis runs ALONG the axis:
    // every rotated copy is the SAME zero-radius axis segment between the same two
    // apex vertices. Such an edge bounds no wall face (its wall faces are skipped),
    // and for a PARTIAL revolution it is the axis seam SHARED by the start and end
    // caps. Building a fresh per-station copy would give each cap its OWN axis edge
    // — single-use on both → two boundary edges + an odd Euler χ (the
    // partial-angle axis-seam leak). Instead build ONE shared seam edge and route
    // both caps through it (forward at the start cap, reversed at the end cap), so
    // the axis seam is used by exactly two faces — watertight + manifold. The full
    // revolution has no caps, so this map stays empty there.
    let mut arcs: HashMap<(usize, u32), EdgeId> = HashMap::new();
    let mut axis_seam: HashMap<usize, EdgeId> = HashMap::new();
    for (e_idx, &(curve_id, sp, ep)) in prof.iter().enumerate() {
        if apex[&sp] && apex[&ep] {
            // Single shared axis-seam edge (apex→apex line, station-invariant).
            let curve = model.curves.get(curve_id).ok_or_else(|| {
                OperationError::InvalidGeometry("revolve: profile curve not found".to_string())
            })?;
            let new_cid = model.curves.add(curve.clone_box());
            let edge = Edge::new_auto_range(
                0,
                vid_at(sp, 0),
                vid_at(ep, 0),
                new_cid,
                EdgeOrientation::Forward,
            );
            axis_seam.insert(e_idx, model.edges.add(edge));
            continue;
        }
        for s in 0..n_stations {
            let xf = rot_about_axis(seg_angle * s as f64)?;
            let curve = model.curves.get(curve_id).ok_or_else(|| {
                OperationError::InvalidGeometry("revolve: profile curve not found".to_string())
            })?;
            let rotated = curve.transform(&xf);
            let new_cid = model.curves.add(rotated);
            let edge = Edge::new_auto_range(
                0,
                vid_at(sp, s),
                vid_at(ep, s),
                new_cid,
                EdgeOrientation::Forward,
            );
            arcs.insert((e_idx, s), model.edges.add(edge));
        }
    }
    let arc_at = |e_idx: usize, s: u32| -> EdgeId {
        let st = if is_full { s % segments } else { s };
        arcs[&(e_idx, st)]
    };

    // Side faces: one per (profile edge, angular segment), boundary
    // bottom-arc(fwd) · right-meridian(fwd) · top-arc(bwd) · left-meridian(bwd).
    let mut shell_faces: Vec<FaceId> = Vec::new();
    for (e_idx, &(curve_id, sp, ep)) in prof.iter().enumerate() {
        // A profile edge whose BOTH endpoints lie on the axis (both poles) runs
        // ALONG the axis: rotating it gives the same axis line, so every angular
        // copy is a zero-area "fin". Such a face has no meridian sides (both are
        // skipped just below), collapsing its loop to a 2-edge bigon the
        // tessellator cannot mesh — it falls to curved-CDT, emits nothing, and
        // leaves the fin's axis edges uncovered (REVOLVE-POLE 2b: 147 open on the
        // pole dome). The axis segment bounds no surface, so skip its wall faces
        // entirely; the real walls (dome bands + base disc) already converge to
        // the shared pole vertices and seal without it. The arc copies built above
        // remain available for a partial revolution's start/end caps.
        if apex[&sp] && apex[&ep] {
            continue;
        }
        // Outward normal of this profile edge at angle 0 = the right-hand
        // normal of the (CCW) profile loop, which points OUT of the solid.
        // edge dir d, profile-plane normal n_p = axis × r̂ (the angular tangent
        // at angle 0); outward = n_p × d. (Using `radial` for every edge — the
        // old behaviour — inverts the inner wall and the caps, which is why the
        // divergence-theorem volume came out at ⅓.) Per segment it is rotated
        // to the segment's mid-angle.
        let sp0 = model.vertices.get_position(vid_at(sp, 0)).ok_or_else(|| {
            OperationError::InvalidGeometry("revolve: profile start vertex not found".to_string())
        })?;
        let ep0 = model.vertices.get_position(vid_at(ep, 0)).ok_or_else(|| {
            OperationError::InvalidGeometry("revolve: profile end vertex not found".to_string())
        })?;
        let sp0 = Vector3::new(sp0[0], sp0[1], sp0[2]);
        let ep0 = Vector3::new(ep0[0], ep0[1], ep0[2]);
        let d = ep0 - sp0;
        let mid = (sp0 + ep0) * 0.5;
        let mrel = mid - axis_origin;
        let rhat = mrel - axis * mrel.dot(&axis);
        let outward0 = if rhat.magnitude_squared() > 1e-20 {
            let n_p = axis.cross(&rhat.normalize()?);
            n_p.cross(&d).normalize().unwrap_or(rhat)
        } else {
            // Edge centroid on the axis (apex-touching): fall back to ±axis
            // by the edge's axial direction.
            axis
        };

        for s in 0..segments {
            let mid_angle = seg_angle * (s as f64 + 0.5);
            let outward_target =
                Matrix4::from_axis_angle(&axis, mid_angle)?.transform_vector(&outward0);

            let mut fl = Loop::new(0, LoopType::Outer);
            fl.add_edge(arc_at(e_idx, s), true);
            if !apex[&ep] {
                fl.add_edge(merid[&(ep, s)], true);
            }
            fl.add_edge(arc_at(e_idx, s + 1), false);
            if !apex[&sp] {
                fl.add_edge(merid[&(sp, s)], false);
            }
            let loop_id = model.loops.add(fl);

            // Surface for THIS segment patch only: the profile rotated to the
            // segment's start angle, swept by `seg_angle`. SurfaceOfRevolution
            // always spans u ∈ [0, angle] from the given profile, so a single
            // full-TAU surface shared by every segment makes the tessellator
            // re-mesh the entire revolution per face (~32× overdraw + wrong
            // divergence volume). A per-segment surface whose domain is exactly
            // the wedge meshes only the wedge.
            let start_xf = rot_about_axis(seg_angle * s as f64)?;
            let rotated_profile = model
                .curves
                .get(curve_id)
                .ok_or_else(|| {
                    OperationError::InvalidGeometry("revolve: profile curve not found".to_string())
                })?
                .clone_box()
                .transform(&start_xf);
            let surface: Box<dyn Surface> = Box::new(
                crate::primitives::surface::SurfaceOfRevolution::new(
                    axis_origin,
                    axis,
                    rotated_profile,
                    seg_angle,
                )
                .map_err(|e| OperationError::NumericalError(format!("revolution surface: {e}")))?,
            );
            let orientation = orient_face_for_outward(surface.as_ref(), outward_target)?;
            let surf_id = model.surfaces.add(surface);
            shell_faces.push(model.faces.add(Face::new(0, surf_id, loop_id, orientation)));
        }
    }

    // Caps for a partial revolution: the profile face at station 0 and at
    // station `segments`, rebuilt from the shared station arcs so they seal.
    if !is_full && options.cap_ends {
        shell_faces.push(build_revolution_cap(
            model,
            &prof,
            &arcs,
            &axis_seam,
            0,
            base_surface_id,
            axis_origin,
            axis,
            &rot_about_axis(0.0)?,
            true, // start cap faces back along -sweep
        )?);
        shell_faces.push(build_revolution_cap(
            model,
            &prof,
            &arcs,
            &axis_seam,
            segments,
            base_surface_id,
            axis_origin,
            axis,
            &rot_about_axis(seg_angle * segments as f64)?,
            false, // end cap faces along +sweep
        )?);
    }

    // Remove the scratch profile face (the revolve INPUT). It is not part of
    // the result — the caps are rebuilt from the shared station arcs — so left
    // in place it would linger as an orphaned, single-use boundary that the
    // whole-model validator flags. Its vertices are retained: they are reused
    // as the station-0 ring. The original profile edges are not reused (fresh
    // station arcs replace them), so they are removed too.
    for &eid in &base_loop.edges {
        model.edges.remove(eid);
    }
    for &il in &base_face.inner_loops {
        if let Some(l) = model.loops.get(il).cloned() {
            for &eid in &l.edges {
                model.edges.remove(eid);
            }
        }
        model.loops.remove(il);
    }
    model.loops.remove(base_face.outer_loop);
    model.faces.remove(base_face_id);

    let shell_type = if is_full || options.cap_ends {
        ShellType::Closed
    } else {
        ShellType::Open
    };
    let mut shell = Shell::new(0, shell_type);
    for &fid in &shell_faces {
        shell.add_face(fid);
    }
    let shell_id = model.shells.add(shell);
    let solid = Solid::new(0, shell_id);
    Ok(model.solids.add(solid))
}

/// Ring geometry of a profile vertex: `(center_on_axis, radius, axial_param)`.
fn ring_geometry(
    model: &BRepModel,
    vid: VertexId,
    axis_origin: Point3,
    axis: Vector3,
) -> OperationResult<(Point3, f64, f64)> {
    let pos = model.vertices.get_position(vid).ok_or_else(|| {
        OperationError::InvalidGeometry(format!("revolve: vertex {vid} not found"))
    })?;
    let p = Vector3::new(pos[0], pos[1], pos[2]);
    let rel = p - axis_origin;
    let axial = rel.dot(&axis);
    let radius = (rel - axis * axial).magnitude();
    let center = axis_origin + axis * axial;
    Ok((Point3::new(center.x, center.y, center.z), radius, axial))
}

/// #19 analytic-band revolve (v1: Cylinder walls + annular Plane caps).
///
/// Returns `Some(solid)` when the profile is a full-revolution rectilinear
/// (axis-aligned) closed meridian with every radius `> 1e-4` — emitting ONE
/// analytic face per band instead of `segments` `SurfaceOfRevolution` patches —
/// and `None` (model unchanged) for any other profile, so the caller falls back
/// to the proven per-segment grid path. The build runs inside a nested
/// `with_rollback` whose closure returns `Err` if the result is not a valid
/// watertight solid, so a failed analytic attempt leaves a clean model.
fn try_analytic_band_revolution(
    model: &mut BRepModel,
    base_face: &Face,
    base_face_id: FaceId,
    options: &RevolveOptions,
) -> OperationResult<Option<SolidId>> {
    let axis = options.axis_direction.normalize()?;
    let axis_origin = options.axis_origin;

    if !base_face.inner_loops.is_empty() {
        return Ok(None);
    }
    let base_loop = model
        .loops
        .get(base_face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("revolve: base loop not found".into()))?
        .clone();

    // Oriented profile vertex pairs, each carrying its curve id + whether it is
    // linear. Linear edges → Cylinder / Cone / annular-Plane bands; a CURVED edge
    // → ONE `SurfaceOfRevolution` band for the whole 360° (the #21 fix: a smooth
    // revolved wall is ONE analytic face, not `segments` angular patches). A bad
    // curved-band attempt fails the self-check below and rolls back to the proven
    // grid path, so this never regresses a working revolve.
    let mut prof: Vec<(VertexId, VertexId, u32, bool)> = Vec::new();
    for (idx, &eid) in base_loop.edges.iter().enumerate() {
        let e = model
            .edges
            .get(eid)
            .ok_or_else(|| OperationError::InvalidGeometry("revolve: profile edge".into()))?;
        let curve = model
            .curves
            .get(e.curve_id)
            .ok_or_else(|| OperationError::InvalidGeometry("revolve: profile curve".into()))?;
        let is_linear = curve.is_linear(crate::math::Tolerance::default());
        let curve_id = e.curve_id;
        let fwd = base_loop.orientations.get(idx).copied().unwrap_or(true);
        let (s, en) = if fwd {
            (e.start_vertex, e.end_vertex)
        } else {
            (e.end_vertex, e.start_vertex)
        };
        prof.push((s, en, curve_id, is_linear));
    }

    // Eligibility: a full-revolution closed meridian. Pole-touching vertices
    // (radius ≈ 0) are now first-class — a band that closes to the axis becomes
    // ONE cone / SurfaceOfRevolution face to the apex, a full disc when the cap is
    // horizontal, and the pure axis segment bounds no face. (Before this, ANY r≈0
    // vertex — every nose cone, dome, and pressure-vessel cap — bailed to the grid
    // path and exploded to profile×`segments` faces.) build_analytic_bands
    // self-checks and rolls back to the grid path on any failure, so an apex
    // profile the analytic path can't yet seal never regresses a working revolve.

    // Build + self-check inside a rollback: Err restores the model so the grid
    // path runs clean.
    let attempt = lifecycle::with_rollback(model, move |model| {
        build_analytic_bands(
            model,
            base_face,
            base_face_id,
            &base_loop,
            &prof,
            axis_origin,
            axis,
        )
    });
    Ok(attempt.ok())
}

/// Emit the analytic band faces (shared ring-circle edges), clean up the scratch
/// profile face, assemble the solid, and self-verify. Returns `Err` (→ rollback)
/// if the result is not a valid watertight solid.
#[allow(clippy::too_many_arguments)]
fn build_analytic_bands(
    model: &mut BRepModel,
    base_face: &Face,
    base_face_id: FaceId,
    base_loop: &Loop,
    prof: &[(VertexId, VertexId, u32, bool)],
    axis_origin: Point3,
    axis: Vector3,
) -> OperationResult<SolidId> {
    use super::orientation::orient_face_for_outward;
    use crate::primitives::curve::{Circle, Line};
    use crate::primitives::r#loop::LoopType;
    use crate::primitives::surface::{Cylinder, Plane};
    use std::collections::HashMap;
    use std::f64::consts::PI;

    let eps = 1e-7;
    let tol = model.tolerance();

    // Canonical seam direction, shared by every ring (a full revolution is
    // rotationally symmetric, so anchoring all seams to the canonical x-axis
    // lines the seam meridians up → watertight).
    let unit_circle = Circle::new(axis_origin, axis, 1.0)
        .map_err(|e| OperationError::NumericalError(format!("revolve seam circle: {e}")))?;
    let ref_dir = unit_circle.x_axis();

    // One SHARED closed circle edge per unique profile vertex.
    let mut uniq: Vec<VertexId> = Vec::new();
    for &(s, en, _, _) in prof {
        for v in [s, en] {
            if !uniq.contains(&v) {
                uniq.push(v);
            }
        }
    }
    // A vertex within `apex_eps` of the axis is a POLE — a single point, not a
    // ring circle. Its seam vertex is the axis point itself; adjacent bands'
    // seam meridians terminate there.
    let apex_eps = 1e-4;
    let mut ring_edge: HashMap<VertexId, EdgeId> = HashMap::new();
    let mut ring_seamv: HashMap<VertexId, VertexId> = HashMap::new();
    let mut ring_geo: HashMap<VertexId, (Point3, f64, f64)> = HashMap::new();
    let mut is_apex: HashMap<VertexId, bool> = HashMap::new();
    for &v in &uniq {
        let (center, radius, axial) = ring_geometry(model, v, axis_origin, axis)?;
        ring_geo.insert(v, (center, radius, axial));
        if radius < apex_eps {
            is_apex.insert(v, true);
            let av = model
                .vertices
                .add_or_find(center.x, center.y, center.z, tol.distance());
            ring_seamv.insert(v, av);
            continue;
        }
        is_apex.insert(v, false);
        let seam_pos = Point3::new(
            center.x + ref_dir.x * radius,
            center.y + ref_dir.y * radius,
            center.z + ref_dir.z * radius,
        );
        let seam_v = model
            .vertices
            .add_or_find(seam_pos.x, seam_pos.y, seam_pos.z, tol.distance());
        let circle = Circle::new(center, axis, radius)
            .map_err(|e| OperationError::NumericalError(format!("revolve ring circle: {e}")))?;
        let cid = model.curves.add(Box::new(circle));
        let edge = model.edges.add(Edge::new(
            0,
            seam_v,
            seam_v,
            cid,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ));
        ring_edge.insert(v, edge);
        ring_seamv.insert(v, seam_v);
    }

    // One analytic face per band.
    let mut faces: Vec<FaceId> = Vec::new();
    for &(s, en, curve_id, is_linear) in prof {
        let apex_s = is_apex.get(&s).copied().unwrap_or(false);
        let apex_en = is_apex.get(&en).copied().unwrap_or(false);
        // A profile edge running ALONG the axis (both endpoints poles) bounds no
        // surface — the adjacent bands' seams already meet at the shared apex.
        if apex_s && apex_en {
            continue;
        }
        let (c0, r0, t0) = ring_geo[&s];
        let (c1, r1, t1) = ring_geo[&en];
        let sp0 = model
            .vertices
            .get_position(s)
            .ok_or_else(|| OperationError::InvalidGeometry("revolve: band start vertex".into()))?;
        let ep0 = model
            .vertices
            .get_position(en)
            .ok_or_else(|| OperationError::InvalidGeometry("revolve: band end vertex".into()))?;
        let sp0 = Vector3::new(sp0[0], sp0[1], sp0[2]);
        let ep0 = Vector3::new(ep0[0], ep0[1], ep0[2]);
        // Proven outward rule (matches the grid path, fixed the ⅓-volume bug).
        let d = ep0 - sp0;
        let mid = (sp0 + ep0) * 0.5;
        let mrel = mid - axis_origin;
        let rhat = mrel - axis * mrel.dot(&axis);
        let outward0 = if rhat.magnitude_squared() > 1e-20 {
            let n_p = axis.cross(&rhat.normalize()?);
            n_p.cross(&d).normalize().unwrap_or(rhat)
        } else {
            axis
        };
        // Azimuth of the PROFILE (where outward0 is evaluated) measured
        // from the canonical ring ref_dir. The band surfaces are all
        // anchored at ref_dir with u ∈ [0, 2π], so
        // `orient_face_for_outward` samples their normal at azimuth π
        // FROM REF_DIR — the historical rotate-by-π target below was
        // only correct for profiles lying in the ref_dir half-plane
        // (θ_p = 0), which every meridian builder guarantees by
        // construction. Sketch-plane profiles (`revolve_profile_regions`,
        // SKETCH-DCM #45 follow-ups B item 5) sit at arbitrary azimuth;
        // the correct rotation is (π − θ_p), which reduces to π exactly
        // when θ_p = 0 — zero behaviour change for existing callers.
        let theta_p = if rhat.magnitude_squared() > 1e-20 {
            let rn = rhat.normalize()?;
            let cos_t = ref_dir.dot(&rn).clamp(-1.0, 1.0);
            let sin_t = axis.dot(&ref_dir.cross(&rn));
            sin_t.atan2(cos_t)
        } else {
            0.0
        };

        let vertical = (r0 - r1).abs() < eps;
        let horizontal = (t0 - t1).abs() < eps;

        if !is_linear {
            // CURVED profile edge → ONE SurfaceOfRevolution face for the full
            // 360° band (#21: a smooth revolved wall is one analytic face, not
            // `segments` patches). The seam meridian is the profile curve rotated
            // onto the canonical ref_dir so its endpoints meet the shared ring
            // seam vertices; the surface revolves that same curve through 2π. A
            // misaligned seam fails the watertight self-check → rollback to grid.
            let pv = model
                .vertices
                .get_position(if r0 > 1e-4 { s } else { en })
                .ok_or_else(|| {
                    OperationError::InvalidGeometry("revolve: curved band angle vertex".into())
                })?;
            let pv = Vector3::new(pv[0], pv[1], pv[2]);
            let rel = pv - axis_origin;
            let radial = rel - axis * rel.dot(&axis);
            let rhat = radial.normalize()?;
            let cos_t = ref_dir.dot(&rhat).clamp(-1.0, 1.0);
            let sin_t = axis.dot(&ref_dir.cross(&rhat));
            let theta = sin_t.atan2(cos_t);
            let rot = Matrix4::from_axis_angle(&axis, -theta)?;

            let base_curve = model.curves.get(curve_id).ok_or_else(|| {
                OperationError::InvalidGeometry("revolve: curved profile curve".into())
            })?;
            let mut seam_curve = base_curve.clone_box().transform(&rot);
            // Orient the seam from ring_seamv[s] → ring_seamv[en].
            let c0p = seam_curve
                .evaluate(0.0)
                .map_err(|e| OperationError::NumericalError(format!("revolve curved seam: {e}")))?
                .position;
            let want = model.vertices.get_position(ring_seamv[&s]).ok_or_else(|| {
                OperationError::InvalidGeometry("revolve: curved seam start".into())
            })?;
            let want = Point3::new(want[0], want[1], want[2]);
            if (c0p - want).magnitude() > tol.distance() {
                seam_curve = seam_curve.reversed();
            }

            let sor = crate::primitives::surface::SurfaceOfRevolution::new(
                axis_origin,
                axis,
                seam_curve.clone_box(),
                std::f64::consts::TAU,
            )
            .map_err(|e| OperationError::NumericalError(format!("revolve sor: {e}")))?;
            let surf_id = model.surfaces.add(Box::new(sor));
            // Lateral loop. A pole end → the revolution surface closes to its
            // apex: the ONLY boundary is the rim circle (the apex is the surface's
            // singularity, not topology — mirrors ConePrimitive's single-edge apex
            // loop), no seam. Two finite ends → a periodic band with a seam
            // meridian: bottom_circle(fwd) seam(fwd) top_circle(bwd) seam(bwd).
            let lp_id = if apex_s || apex_en {
                let rim_v = if apex_s { en } else { s };
                let mut lp = Loop::new(0, LoopType::Outer);
                lp.add_edge(ring_edge[&rim_v], true);
                model.loops.add(lp)
            } else {
                let seam_cid = model.curves.add(seam_curve);
                let seam_eid = model.edges.add(Edge::new(
                    0,
                    ring_seamv[&s],
                    ring_seamv[&en],
                    seam_cid,
                    EdgeOrientation::Forward,
                    ParameterRange::new(0.0, 1.0),
                ));
                let mut lp = Loop::new(0, LoopType::Outer);
                lp.add_edge(ring_edge[&s], true);
                lp.add_edge(seam_eid, true);
                lp.add_edge(ring_edge[&en], false);
                lp.add_edge(seam_eid, false);
                model.loops.add(lp)
            };

            // The SoR is anchored at ref_dir; the outward sample point
            // (u_mid = π) sits at azimuth π − θ_p from the profile.
            let target = Matrix4::from_axis_angle(&axis, PI - theta_p)?.transform_vector(&outward0);
            let surf = model
                .surfaces
                .get(surf_id)
                .ok_or_else(|| OperationError::InvalidGeometry("revolve: sor surface".into()))?;
            let orient = orient_face_for_outward(surf, target)?;
            let mut f = Face::new(0, surf_id, lp_id, orient);
            f.outer_loop = lp_id;
            faces.push(model.faces.add(f));
        } else if horizontal {
            // Plane cap at constant axial. A pole end → full DISC (outer circle
            // only); two finite radii → annular ring (outer rim + inner hole).
            let plane = Plane::from_point_normal(c0, axis)
                .map_err(|e| OperationError::NumericalError(format!("revolve plane: {e}")))?;
            let surf_id = model.surfaces.add(Box::new(plane));
            let rim_v = if r0 >= r1 { s } else { en };

            let mut lp = Loop::new(0, LoopType::Outer);
            lp.add_edge(ring_edge[&rim_v], true);
            let lp_id = model.loops.add(lp);

            // Plane outward0 is constant ±axis over the whole face → sample midpoint.
            let surf = model
                .surfaces
                .get(surf_id)
                .ok_or_else(|| OperationError::InvalidGeometry("revolve: plane surface".into()))?;
            let orient = orient_face_for_outward(surf, outward0)?;
            let mut f = Face::new(0, surf_id, lp_id, orient);
            f.outer_loop = lp_id;
            if !(apex_s || apex_en) {
                // Annular ring: the smaller-radius circle is the central hole.
                let inner_v = if r0 >= r1 { en } else { s };
                let mut inner = Loop::new(0, LoopType::Inner);
                inner.add_edge(ring_edge[&inner_v], true);
                let inner_id = model.loops.add(inner);
                f.add_inner_loop(inner_id);
            }
            faces.push(model.faces.add(f));
        } else {
            // Periodic seam band: Cylinder (vertical edge) or Cone frustum
            // (sloped edge). Both share the seam meridian + rectangular loop.
            // Geometry uses the axial-ordered base ring; the loop/orientation use
            // profile order (decoupled, same as the grid path).
            let (base_c, base_r, top_c, top_r, height) = if t0 <= t1 {
                (c0, r0, c1, r1, t1 - t0)
            } else {
                (c1, r1, c0, r0, t0 - t1)
            };
            let surf_id = if vertical {
                let mut cyl = Cylinder::new_finite(base_c, axis, base_r, height).map_err(|e| {
                    OperationError::NumericalError(format!("revolve cylinder: {e}"))
                })?;
                cyl.ref_dir = ref_dir;
                model.surfaces.add(Box::new(cyl))
            } else {
                // Cone frustum band (mirrors create_frustum_topology) with the
                // seam anchored to the shared ring ref_dir (cone-rim-seam-alignment).
                let tan_half = (base_r - top_r).abs() / height;
                let half_angle = tan_half.atan();
                let (apex, cone_axis) = if base_r > top_r {
                    (base_c + axis * (base_r / tan_half), -axis)
                } else {
                    (base_c - axis * (base_r / tan_half), axis)
                };
                let v_base_d = (base_c - apex).dot(&cone_axis);
                let v_top_d = (top_c - apex).dot(&cone_axis);
                let cone = crate::primitives::surface::Cone {
                    apex,
                    axis: cone_axis,
                    half_angle,
                    ref_dir,
                    height_limits: Some([v_base_d.min(v_top_d), v_base_d.max(v_top_d)]),
                    angle_limits: None,
                };
                model.surfaces.add(Box::new(cone))
            };

            // Lateral loop. A pole end → a cone closing to its apex: its ONLY
            // boundary is the rim circle (the apex is the surface's singularity,
            // not topology — mirrors ConePrimitive's single-edge apex loop), no
            // seam. Two finite ends → a periodic band with a seam meridian:
            // bottom_circle(fwd) seam(fwd) top_circle(bwd) seam(bwd).
            let lp_id = if apex_s || apex_en {
                let rim_v = if apex_s { en } else { s };
                let mut lp = Loop::new(0, LoopType::Outer);
                lp.add_edge(ring_edge[&rim_v], true);
                model.loops.add(lp)
            } else {
                let svs = model
                    .vertices
                    .get_position(ring_seamv[&s])
                    .ok_or_else(|| OperationError::InvalidGeometry("revolve: seam vtx s".into()))?;
                let sve = model
                    .vertices
                    .get_position(ring_seamv[&en])
                    .ok_or_else(|| OperationError::InvalidGeometry("revolve: seam vtx e".into()))?;
                let seam_line = Line::new(
                    Point3::new(svs[0], svs[1], svs[2]),
                    Point3::new(sve[0], sve[1], sve[2]),
                );
                let seam_cid = model.curves.add(Box::new(seam_line));
                let seam_eid = model.edges.add(Edge::new(
                    0,
                    ring_seamv[&s],
                    ring_seamv[&en],
                    seam_cid,
                    EdgeOrientation::Forward,
                    ParameterRange::new(0.0, 1.0),
                ));
                let mut lp = Loop::new(0, LoopType::Outer);
                lp.add_edge(ring_edge[&s], true);
                lp.add_edge(seam_eid, true);
                lp.add_edge(ring_edge[&en], false);
                lp.add_edge(seam_eid, false);
                model.loops.add(lp)
            };

            // orient_face_for_outward samples u = π from ref_dir; the
            // profile (where outward0 lives) sits at azimuth θ_p, so
            // rotate by π − θ_p (= π for the historical θ_p = 0 case).
            let target = Matrix4::from_axis_angle(&axis, PI - theta_p)?.transform_vector(&outward0);
            let surf = model
                .surfaces
                .get(surf_id)
                .ok_or_else(|| OperationError::InvalidGeometry("revolve: band surface".into()))?;
            let orient = orient_face_for_outward(surf, target)?;
            let mut f = Face::new(0, surf_id, lp_id, orient);
            f.outer_loop = lp_id;
            faces.push(model.faces.add(f));
        }
    }

    // Remove the scratch profile face + its edges/loop (mirrors create_revolution).
    for &eid in &base_loop.edges {
        model.edges.remove(eid);
    }
    model.loops.remove(base_face.outer_loop);
    model.faces.remove(base_face_id);

    // Shell + solid.
    let mut shell = Shell::new(0, ShellType::Closed);
    for &fid in &faces {
        shell.add_face(fid);
    }
    let shell_id = model.shells.add(shell);
    let solid = Solid::new(0, shell_id);
    let sid = model.solids.add(solid);

    // Persistent-id lineage (#11 slice 40-E): the result solid roots on the
    // revolve event; each analytic band face (1:1 with the profile edges, in
    // order) derives from the profile edge it was revolved from. So "the band
    // over profile edge i" keeps its PID across a dimension mould — even if the
    // edit morphs that band cylinder↔cone↔annulus. If the self-check below fails
    // and rolls back, these PIDs roll back with it (snapshot-captured).
    {
        use crate::primitives::persistent_id::{PersistentId, Role};
        let solid_pid = PersistentId::root(&model.next_root_seed("revolve"));
        model.set_solid_pid(sid, solid_pid);
        for (i, &face_id) in faces.iter().enumerate() {
            let base_edge_pid = match base_loop.edges.get(i).and_then(|&e| model.edge_pid(e)) {
                Some(p) => p,
                None => PersistentId::derive(
                    &[solid_pid],
                    "revolve_profile_edge",
                    &Role::Generic {
                        source_pid: solid_pid,
                        label: format!("e{i}"),
                    },
                ),
            };
            let fpid = PersistentId::derive(
                &[solid_pid, base_edge_pid],
                "revolve",
                &Role::RevolveBand { base_edge_pid },
            );
            model.set_face_pid(face_id, fpid);
        }
    }

    // Self-check: a valid, closed, manifold, watertight solid — else Err → rollback.
    let v = crate::primitives::validation::validate_solid_scoped(
        model,
        sid,
        tol,
        crate::primitives::validation::ValidationLevel::Standard,
    );
    if !v.is_valid {
        return Err(OperationError::InvalidGeometry(format!(
            "analytic revolve invalid: {:?}",
            v.errors
        )));
    }
    match crate::harness::watertight::manifold_report(model, sid, 0.1, 1e-6) {
        Some(rep) if rep.boundary_edges == 0 && rep.closed && rep.manifold => Ok(sid),
        Some(rep) => Err(OperationError::InvalidGeometry(format!(
            "analytic revolve not watertight: boundary={} closed={} manifold={}",
            rep.boundary_edges, rep.closed, rep.manifold
        ))),
        None => Err(OperationError::InvalidGeometry(
            "analytic revolve tessellation empty".into(),
        )),
    }
}

/// Build a revolution end-cap face at a given angular `station`, reusing the
/// shared profile-arc edges so it shares its boundary with the adjacent ring of
/// side faces (watertight). The cap surface is the base profile surface rotated
/// to the station; orientation is chosen so the normal points out of the body
/// (opposite the sweep at the start cap, along it at the end cap).
#[allow(clippy::too_many_arguments)]
fn build_revolution_cap(
    model: &mut BRepModel,
    prof: &[(u32, VertexId, VertexId)],
    arcs: &std::collections::HashMap<(usize, u32), EdgeId>,
    axis_seam: &std::collections::HashMap<usize, EdgeId>,
    station: u32,
    base_surface_id: u32,
    axis_origin: Point3,
    axis: Vector3,
    station_xform: &Matrix4,
    is_start: bool,
) -> OperationResult<FaceId> {
    use crate::primitives::r#loop::LoopType;

    // Closed loop of the profile arcs at this station (profile order). A profile
    // edge that runs ALONG the axis (both endpoints on it) contributes the SHARED
    // axis-seam edge instead of a per-station arc — the seam where the start and
    // end caps meet. Both caps add it in profile order (its endpoints are the same
    // axis apex vertices at every station, so each cap loop still closes
    // v0→…→v3→v0); the two caps carry OPPOSITE face orientations, so the seam's two
    // uses are opposite in 3D — manifold + watertight. Without the shared seam the
    // two caps each own a single-use axis edge → a boundary-edge leak + odd
    // Euler χ (the partial-angle axis-seam defect).
    let mut fl = Loop::new(0, LoopType::Outer);
    let mut centroid = Vector3::ZERO;
    let mut n = 0.0;
    for (e_idx, &(_, sp, _)) in prof.iter().enumerate() {
        let eid = if let Some(&seam) = axis_seam.get(&e_idx) {
            seam
        } else {
            *arcs.get(&(e_idx, station)).ok_or_else(|| {
                OperationError::InvalidGeometry("revolve: cap arc not found".to_string())
            })?
        };
        fl.add_edge(eid, true);
        let _ = sp;
        if let Some(q) = model.edges.get(eid).and_then(|e| {
            let v = e.start_vertex;
            model.vertices.get_position(v)
        }) {
            centroid = centroid + Vector3::new(q[0], q[1], q[2]);
            n += 1.0;
        }
    }
    if n > 0.0 {
        centroid = centroid * (1.0 / n);
    }
    let loop_id = model.loops.add(fl);

    // Cap surface: base profile surface rotated to this station.
    let base_surf = model
        .surfaces
        .get(base_surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("revolve: base surface not found".into()))?;
    let cap_surface = base_surf.transform(station_xform);

    // Outward = ± the angular tangent (axis × radial) at the cap centroid.
    let rel = centroid - axis_origin;
    let radial = rel - axis * rel.dot(&axis);
    let tangent = axis.cross(&radial);
    let outward_target = if tangent.magnitude_squared() > 1e-20 {
        if is_start {
            tangent * -1.0
        } else {
            tangent
        }
    } else if is_start {
        axis * -1.0
    } else {
        axis
    };
    let orientation = orient_face_for_outward(cap_surface.as_ref(), outward_target)?;
    let surf_id = model.surfaces.add(cap_surface);
    Ok(model.faces.add(Face::new(0, surf_id, loop_id, orientation)))
}

/// Create a helical sweep — revolve with axial translation (pitch per revolution)
fn create_helical_sweep(
    model: &mut BRepModel,
    base_face: &Face,
    _base_face_id: FaceId,
    options: &RevolveOptions,
) -> OperationResult<SolidId> {
    let segments = options.segments.max(4);
    let angle_step = options.angle / segments as f64;
    // Axial translation per angle step
    let pitch_step = options.pitch * (angle_step / (2.0 * std::f64::consts::PI));

    let outer_loop = model
        .loops
        .get(base_face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Face loop not found".into()))?
        .clone();

    let mut shell_faces = Vec::new();

    // Generate faces for each segment by composing rotation + translation
    for seg in 0..segments {
        let angle = angle_step * seg as f64;
        let next_angle = angle_step * (seg + 1) as f64;
        let axial_offset = pitch_step * seg as f64;
        let next_axial = pitch_step * (seg + 1) as f64;

        // Build combined transforms: rotate then translate along axis
        let rot = Matrix4::from_axis_angle(&options.axis_direction, angle)?;
        let next_rot = Matrix4::from_axis_angle(&options.axis_direction, next_angle)?;
        let translate = Matrix4::from_translation(&(options.axis_direction * axial_offset));
        let next_translate = Matrix4::from_translation(&(options.axis_direction * next_axial));
        let xform = translate * rot;
        let next_xform = next_translate * next_rot;

        // Create faces for each edge in the profile loop. The loop index
        // `i` is folded into error messages so revolve failures point to a
        // specific profile edge rather than the abstract "edge not found".
        for (i, &edge_id) in outer_loop.edges.iter().enumerate() {
            let edge = model
                .edges
                .get(edge_id)
                .ok_or_else(|| {
                    OperationError::InvalidGeometry(format!(
                        "revolve: edge {} (profile slot {}) not found",
                        edge_id, i
                    ))
                })?
                .clone();

            // Get edge endpoints and transform them
            let ps_arr = model
                .vertices
                .get_position(edge.start_vertex)
                .ok_or_else(|| {
                    OperationError::InvalidGeometry(format!(
                        "revolve: start vertex {} of edge {} (profile slot {}) not found",
                        edge.start_vertex, edge_id, i
                    ))
                })?;
            let pe_arr = model
                .vertices
                .get_position(edge.end_vertex)
                .ok_or_else(|| {
                    OperationError::InvalidGeometry(format!(
                        "revolve: end vertex {} of edge {} (profile slot {}) not found",
                        edge.end_vertex, edge_id, i
                    ))
                })?;
            let p_start = Vector3::new(ps_arr[0], ps_arr[1], ps_arr[2]);
            let p_end = Vector3::new(pe_arr[0], pe_arr[1], pe_arr[2]);

            let p1 = xform.transform_point(&p_start);
            let p2 = xform.transform_point(&p_end);
            let p3 = next_xform.transform_point(&p_end);
            let p4 = next_xform.transform_point(&p_start);

            // Create quad face from these 4 points
            let v1 = model.vertices.add(p1.x, p1.y, p1.z);
            let v2 = model.vertices.add(p2.x, p2.y, p2.z);
            let v3 = model.vertices.add(p3.x, p3.y, p3.z);
            let v4 = model.vertices.add(p4.x, p4.y, p4.z);

            use crate::primitives::curve::Line;
            use crate::primitives::edge::EdgeOrientation;
            use crate::primitives::r#loop::LoopType;
            use crate::primitives::surface::Plane;

            let l1 = model.curves.add(Box::new(Line::new(p1, p2)));
            let l2 = model.curves.add(Box::new(Line::new(p2, p3)));
            let l3 = model.curves.add(Box::new(Line::new(p3, p4)));
            let l4 = model.curves.add(Box::new(Line::new(p4, p1)));

            let e1 = model.edges.add(Edge::new_auto_range(
                0,
                v1,
                v2,
                l1,
                EdgeOrientation::Forward,
            ));
            let e2 = model.edges.add(Edge::new_auto_range(
                0,
                v2,
                v3,
                l2,
                EdgeOrientation::Forward,
            ));
            let e3 = model.edges.add(Edge::new_auto_range(
                0,
                v3,
                v4,
                l3,
                EdgeOrientation::Forward,
            ));
            let e4 = model.edges.add(Edge::new_auto_range(
                0,
                v4,
                v1,
                l4,
                EdgeOrientation::Forward,
            ));

            let mut face_loop = Loop::new(0, LoopType::Outer);
            face_loop.add_edge(e1, true);
            face_loop.add_edge(e2, true);
            face_loop.add_edge(e3, true);
            face_loop.add_edge(e4, true);
            let loop_id = model.loops.add(face_loop);

            // Create planar surface from the quad normal
            let n = (p2 - p1).cross(&(p4 - p1));
            let normal = if n.magnitude_squared() > 1e-20 {
                n.normalize()?
            } else {
                Vector3::Z
            };
            let surf = Plane::from_point_normal(p1, normal)?;

            // Outward target: radially outward from the revolution axis
            // at the quad centroid. Project the centroid onto the axis
            // line; the perpendicular component is the radial outward
            // direction. Fall back to `normal` if the quad straddles the
            // axis (radius below tolerance) so the orientation pick
            // degrades gracefully.
            let centroid = (p1 + p2 + p3 + p4) * 0.25;
            let to_centroid = centroid - options.axis_origin;
            let axial = to_centroid.dot(&options.axis_direction) * options.axis_direction;
            let radial = to_centroid - axial;
            let outward_target = if radial.magnitude_squared() > 1e-20 {
                radial
            } else {
                normal
            };
            let surf_box: Box<dyn Surface> = Box::new(surf);
            let orientation = orient_face_for_outward(surf_box.as_ref(), outward_target)?;
            let surf_id = model.surfaces.add(surf_box);

            let face = Face::new(0, surf_id, loop_id, orientation);
            shell_faces.push(model.faces.add(face));
        }
    }

    // Build shell and solid
    let shell_type = if options.cap_ends {
        ShellType::Closed
    } else {
        ShellType::Open
    };
    let mut shell = Shell::new(0, shell_type);
    for &fid in &shell_faces {
        shell.add_face(fid);
    }
    let shell_id = model.shells.add(shell);
    let solid = Solid::new(0, shell_id);
    Ok(model.solids.add(solid))
}

/// Create surface(s) by revolving an edge
fn create_revolved_edge_surface(
    model: &mut BRepModel,
    edge_id: EdgeId,
    edge_forward: bool,
    axis_origin: Point3,
    axis_direction: Vector3,
    angle: f64,
    segments: u32,
) -> OperationResult<Vec<FaceId>> {
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
        .clone();

    let mut faces = Vec::new();
    let segment_angle = angle / segments as f64;

    // Create faces for each segment
    for i in 0..segments {
        let start_angle = i as f64 * segment_angle;
        let end_angle = (i + 1) as f64 * segment_angle;

        let face_id = create_revolution_segment_face(
            model,
            &edge,
            edge_forward,
            axis_origin,
            axis_direction,
            start_angle,
            end_angle,
        )?;
        faces.push(face_id);
    }

    Ok(faces)
}

/// Create a single face for a revolution segment
fn create_revolution_segment_face(
    model: &mut BRepModel,
    edge: &Edge,
    edge_forward: bool,
    axis_origin: Point3,
    axis_direction: Vector3,
    start_angle: f64,
    end_angle: f64,
) -> OperationResult<FaceId> {
    // Get edge endpoints
    let start_vertex = model
        .vertices
        .get(edge.start_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("Start vertex not found".to_string()))?;
    let end_vertex = model
        .vertices
        .get(edge.end_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("End vertex not found".to_string()))?;

    // Create rotated vertices
    let rotation_start = Matrix4::from_axis_angle(&axis_direction, start_angle)?;
    let rotation_end = Matrix4::from_axis_angle(&axis_direction, end_angle)?;

    let _v0 = edge.start_vertex;
    let _v1 = edge.end_vertex;
    let v2 = create_rotated_vertex(model, &end_vertex, axis_origin, rotation_start)?;
    let v3 = create_rotated_vertex(model, &end_vertex, axis_origin, rotation_end)?;
    let v4 = create_rotated_vertex(model, &start_vertex, axis_origin, rotation_end)?;
    let v5 = create_rotated_vertex(model, &start_vertex, axis_origin, rotation_start)?;

    // Create edges for the face
    let mut face_edges = Vec::new();

    // Edge 1: Original edge at start angle (or rotated copy if not at 0)
    if start_angle.abs() < 1e-10 {
        face_edges.push((edge.id, edge_forward));
    } else {
        let rotated_edge = create_rotated_edge(model, edge, axis_origin, rotation_start)?;
        face_edges.push((rotated_edge, edge_forward));
    }

    // Edge 2: Meridian from end of profile edge
    let meridian1 = create_meridian_edge(
        model,
        v2,
        v3,
        axis_origin,
        axis_direction,
        start_angle,
        end_angle,
    )?;
    face_edges.push((meridian1, true));

    // Edge 3: Rotated edge at end angle (reversed)
    let rotated_edge_end = create_rotated_edge(model, edge, axis_origin, rotation_end)?;
    face_edges.push((rotated_edge_end, !edge_forward));

    // Edge 4: Meridian from start of profile edge (reversed)
    let meridian2 = create_meridian_edge(
        model,
        v5,
        v4,
        axis_origin,
        axis_direction,
        start_angle,
        end_angle,
    )?;
    face_edges.push((meridian2, false));

    // Create loop
    let mut face_loop = Loop::new(0, crate::primitives::r#loop::LoopType::Outer); // ID will be assigned by store
    for (edge_id, forward) in face_edges {
        face_loop.add_edge(edge_id, forward);
    }
    let loop_id = model.loops.add(face_loop);

    // Create surface of revolution
    let surface = create_revolution_surface(model, edge.curve_id, axis_origin, axis_direction)?;

    // Outward target: radially outward from the axis at the midpoint
    // of the segment. Take the profile-edge midpoint, rotate it by the
    // mid-angle of the segment, project onto the axis, and the
    // perpendicular component is the radial outward direction. This
    // matches the geometric outward of any surface of revolution whose
    // profile sits on one side of the axis (the standard case; profiles
    // that cross the axis are rejected earlier by
    // `face_intersects_axis`). If the midpoint sits on the axis
    // (degenerate, only happens for an apex-touching profile that wasn't
    // caught above), default to `+axis_direction` so the orientation
    // pick is at least deterministic.
    let start_pos = Vector3::from(start_vertex.position);
    let end_pos = Vector3::from(end_vertex.position);
    let profile_mid = (start_pos + end_pos) * 0.5;
    let mid_angle = 0.5 * (start_angle + end_angle);
    let rotation_mid = Matrix4::from_axis_angle(&axis_direction, mid_angle)?;
    let mid_rel = profile_mid - axis_origin;
    let mid_world = rotation_mid.transform_point(&mid_rel) + axis_origin;
    let to_mid = mid_world - axis_origin;
    let axial = to_mid.dot(&axis_direction) * axis_direction;
    let radial = to_mid - axial;
    let outward_target = if radial.magnitude_squared() > 1e-20 {
        radial
    } else {
        axis_direction
    };
    let orientation = orient_face_for_outward(surface.as_ref(), outward_target)?;
    let surface_id = model.surfaces.add(surface);

    // Create face
    let face = Face::new(
        0, // ID will be assigned by store
        surface_id,
        loop_id,
        orientation,
    );
    let face_id = model.faces.add(face);

    Ok(face_id)
}

/// Create a rotated vertex
fn create_rotated_vertex(
    model: &mut BRepModel,
    vertex: &crate::primitives::vertex::Vertex,
    axis_origin: Point3,
    rotation: Matrix4,
) -> OperationResult<VertexId> {
    let pos = Vector3::from(vertex.position);
    let relative_pos = pos - axis_origin;
    let rotated_pos = rotation.transform_point(&relative_pos) + axis_origin;

    Ok(model
        .vertices
        .add(rotated_pos.x, rotated_pos.y, rotated_pos.z))
}

/// Create a meridian edge (arc on surface of revolution)
fn create_meridian_edge(
    model: &mut BRepModel,
    start_vertex: VertexId,
    end_vertex: VertexId,
    axis_origin: Point3,
    axis_direction: Vector3,
    start_angle: f64,
    end_angle: f64,
) -> OperationResult<EdgeId> {
    use crate::primitives::curve::Arc;

    // Get vertex position
    let vertex_pos = model
        .vertices
        .get(start_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("Vertex not found".to_string()))?
        .position;
    let point = Vector3::from(vertex_pos);

    // Project point to plane perpendicular to axis
    let to_point = point - axis_origin;
    let axis_component = to_point.dot(&axis_direction) * axis_direction;
    let radial_component = to_point - axis_component;
    let radius = radial_component.magnitude();

    if radius < 1e-10 {
        // Point is on axis, create degenerate edge
        return create_degenerate_edge(model, start_vertex, end_vertex);
    }

    // Create arc
    let center = axis_origin + axis_component;
    let arc = Arc::new(
        center,
        axis_direction,
        radius,
        start_angle,
        end_angle - start_angle,
    )?;
    let curve_id = model.curves.add(Box::new(arc));

    let edge = Edge::new_auto_range(
        0, // ID will be assigned by store
        start_vertex,
        end_vertex,
        curve_id,
        EdgeOrientation::Forward,
    );
    let edge_id = model.edges.add(edge);

    Ok(edge_id)
}

/// Create a rotated copy of an edge
fn create_rotated_edge(
    model: &mut BRepModel,
    edge: &Edge,
    axis_origin: Point3,
    rotation: Matrix4,
) -> OperationResult<EdgeId> {
    // Get original curve
    let curve = model
        .curves
        .get(edge.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;

    // Create transformation that rotates around axis
    let to_origin = Matrix4::from_translation(&-axis_origin);
    let from_origin = Matrix4::from_translation(&axis_origin);
    let transform = from_origin * rotation * to_origin;

    // Create transformed curve
    let rotated_curve = curve.transform(&transform);
    let new_curve_id = model.curves.add(rotated_curve);

    // Get rotated vertices
    let start_vertex = model
        .vertices
        .get(edge.start_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("Start vertex not found".to_string()))?;
    let end_vertex = model
        .vertices
        .get(edge.end_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("End vertex not found".to_string()))?;

    let new_start = create_rotated_vertex(model, &start_vertex, axis_origin, rotation)?;
    let new_end = create_rotated_vertex(model, &end_vertex, axis_origin, rotation)?;

    // Create new edge
    let new_edge = Edge::new(
        0, // ID will be assigned by store
        new_start,
        new_end,
        new_curve_id,
        edge.orientation,
        edge.param_range,
    );
    let edge_id = model.edges.add(new_edge);

    Ok(edge_id)
}

/// Create a surface of revolution from a profile curve rotated around an axis.
fn create_revolution_surface(
    model: &mut BRepModel,
    profile_curve_id: u32,
    axis_origin: Point3,
    axis_direction: Vector3,
) -> OperationResult<Box<dyn Surface>> {
    let curve = model
        .curves
        .get(profile_curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Profile curve not found".to_string()))?;

    let profile_clone = curve.clone_box();

    let revolution = crate::primitives::surface::SurfaceOfRevolution::new(
        axis_origin,
        axis_direction,
        profile_clone,
        std::f64::consts::TAU, // Full 360° revolution by default
    )
    .map_err(|e| {
        OperationError::NumericalError(format!("Failed to create revolution surface: {e}"))
    })?;

    Ok(Box::new(revolution))
}

/// Create a transformed copy of a face.
///
/// Transforms the surface, creates new vertices/edges/loops for the boundary,
/// and produces a new face referencing the transformed geometry.
fn create_transformed_face(
    model: &mut BRepModel,
    face: &Face,
    transform: Matrix4,
) -> OperationResult<FaceId> {
    // Transform the surface
    let surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;
    let new_surface = surface.transform(&transform);
    let new_surface_id = model.surfaces.add(new_surface);

    // Transform the outer loop
    let outer_loop = model
        .loops
        .get(face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Outer loop not found".to_string()))?
        .clone();

    let mut new_loop = Loop::new(0, crate::primitives::r#loop::LoopType::Outer);

    for (idx, &edge_id) in outer_loop.edges.iter().enumerate() {
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
            .clone();

        // Transform curve
        let curve = model
            .curves
            .get(edge.curve_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;
        let new_curve = curve.transform(&transform);
        let new_curve_id = model.curves.add(new_curve);

        // Transform vertices
        let sv = model
            .vertices
            .get(edge.start_vertex)
            .ok_or_else(|| OperationError::InvalidGeometry("Start vertex not found".to_string()))?;
        let ev = model
            .vertices
            .get(edge.end_vertex)
            .ok_or_else(|| OperationError::InvalidGeometry("End vertex not found".to_string()))?;

        let new_start_pos =
            transform.transform_point(&Point3::new(sv.position[0], sv.position[1], sv.position[2]));
        let new_end_pos =
            transform.transform_point(&Point3::new(ev.position[0], ev.position[1], ev.position[2]));

        let new_start =
            model
                .vertices
                .add_or_find(new_start_pos.x, new_start_pos.y, new_start_pos.z, 1e-6);
        let new_end = model
            .vertices
            .add_or_find(new_end_pos.x, new_end_pos.y, new_end_pos.z, 1e-6);

        let new_edge = Edge::new(
            0,
            new_start,
            new_end,
            new_curve_id,
            edge.orientation,
            edge.param_range,
        );
        let new_edge_id = model.edges.add(new_edge);

        let forward = outer_loop.orientations.get(idx).copied().unwrap_or(true);
        new_loop.add_edge(new_edge_id, forward);
    }

    let new_loop_id = model.loops.add(new_loop);

    let new_face = Face::new(0, new_surface_id, new_loop_id, face.orientation);
    let new_face_id = model.faces.add(new_face);

    Ok(new_face_id)
}

/// Create a face from a closed wire profile
fn create_face_from_profile(
    model: &mut BRepModel,
    profile_edges: Vec<EdgeId>,
) -> OperationResult<FaceId> {
    // Reuse from extrude module
    super::extrude::create_face_from_profile(model, profile_edges)
}

/// Create a degenerate edge (point edge)
fn create_degenerate_edge(
    model: &mut BRepModel,
    vertex: VertexId,
    _end_vertex: VertexId,
) -> OperationResult<EdgeId> {
    let vertex_data = model
        .vertices
        .get(vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("Vertex not found".to_string()))?;

    // Represent a point-edge with a zero-length Line (start == end). The kernel
    // does not maintain a dedicated Point curve type because every consumer of
    // a degenerate edge must also handle the zero-arc-length case on regular
    // curves; collapsing both paths to a single Line implementation keeps
    // intersection / projection / parameter-mapping logic uniform.
    use crate::primitives::curve::Line;
    let point = Vector3::from(vertex_data.position);
    let point_curve = Line::new(point, point);
    let curve_id = model.curves.add(Box::new(point_curve));

    let edge = Edge::new(
        0, // ID will be assigned by store
        vertex,
        vertex,
        curve_id,
        EdgeOrientation::Forward,
        ParameterRange::new(0.0, 1.0),
    );
    let edge_id = model.edges.add(edge);

    Ok(edge_id)
}

/// Check whether the revolution axis passes through the face.
///
/// Three conditions detect intersection:
///
///  1. **Vertex on axis** — any boundary vertex within `tolerance` radial
///     distance of the axis. Cheap, catches sketches drawn touching the
///     pivot.
///  2. **Edge crossing axis** — sampled radial distance falls below
///     `tolerance` along an edge, *or* the radial offset vector flips
///     sense between samples (sign change in a fixed orthogonal frame).
///     Catches edges that pass through the axis without endpointing on it.
///  3. **Axis pierces face interior** — for a planar face the revolution
///     axis line is intersected with the face plane; the resulting point
///     is then tested against the face's outer loop using a 2D
///     point-in-polygon parity test on the axis-projected polygon. Catches
///     the "axis goes straight through the middle of a flat face" case.
///
/// Non-planar surfaces fall back to (1) and (2) only — sufficient in
/// practice because revolution profiles are typically sketched on
/// planar sketch planes.
fn face_intersects_axis(
    model: &BRepModel,
    face: &Face,
    axis_origin: Point3,
    axis_direction: Vector3,
) -> OperationResult<bool> {
    use crate::primitives::surface::Plane;

    let tolerance = 1e-6;
    let axis_dir = axis_direction.normalize().unwrap_or(axis_direction);

    let loop_data = model
        .loops
        .get(face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?;

    // Helper: radial offset of a point from the infinite axis line.
    let radial_offset = |p: Point3| -> Vector3 {
        let to_p =
            Vector3::new(p.x, p.y, p.z) - Vector3::new(axis_origin.x, axis_origin.y, axis_origin.z);
        to_p - axis_dir * to_p.dot(&axis_dir)
    };

    // (1) + (2): walk the boundary loop, checking endpoints and edge interior.
    let mut radial_samples: Vec<Vector3> = Vec::new();
    for &edge_id in &loop_data.edges {
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

        // Endpoint check — fast path catches sketches touching the axis.
        for &vertex_id in &[edge.start_vertex, edge.end_vertex] {
            let vertex = model
                .vertices
                .get(vertex_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Vertex not found".to_string()))?;
            let point = Point3::new(vertex.position[0], vertex.position[1], vertex.position[2]);
            let r = radial_offset(point);
            // A boundary vertex ON the axis (r≈0) is a valid POLE / axis touch —
            // a quarter-disk's apex, or a rectangle edge on the axis that
            // revolves to a solid cylinder — NOT a self-intersection. Only an
            // interior CROSSING of the axis is rejected (the sign-flip test in
            // the edge loop + the planar pierce test below). REVOLVE-POLE.
            radial_samples.push(r);
        }

        // Edge-interior check: sample the curve and look for sub-tolerance
        // radial magnitude or sign-flip in the radial offset direction.
        if let Some(curve) = model.curves.get(edge.curve_id) {
            let pr = curve.parameter_range();
            let span = pr.end - pr.start;
            if span.abs() > 1e-12 {
                const N: usize = 8;
                let mut prev: Option<Vector3> = None;
                for i in 0..=N {
                    let t = pr.start + span * (i as f64 / N as f64);
                    if let Ok(p) = curve.point_at(t) {
                        let r = radial_offset(p);
                        // Drop the touch-reject: an edge sample at r≈0 is a valid
                        // axis-coincident edge (the pole's closing edge, or a
                        // cylinder's axis edge), not a crossing. Only flag a
                        // genuine CROSSING — both samples strictly OFF-axis AND
                        // pointing into opposite radial half-spaces (the profile
                        // passes through the axis interior). REVOLVE-POLE.
                        if let Some(prev_r) = prev {
                            if prev_r.magnitude() > tolerance
                                && r.magnitude() > tolerance
                                && prev_r.dot(&r) < 0.0
                            {
                                return Ok(true);
                            }
                        }
                        prev = Some(r);
                    }
                }
            }
        }
    }

    // (3) Planar-face interior pierce test.
    if let Some(surface) = model.surfaces.get(face.surface_id) {
        if let Some(plane) = surface.as_any().downcast_ref::<Plane>() {
            let n = plane.normal;
            let denom = n.dot(&axis_dir);
            if denom.abs() > 1e-12 {
                // Axis is not parallel to plane → unique intersection point.
                let plane_origin_v = Vector3::new(plane.origin.x, plane.origin.y, plane.origin.z);
                let axis_origin_v = Vector3::new(axis_origin.x, axis_origin.y, axis_origin.z);
                let t = n.dot(&(plane_origin_v - axis_origin_v)) / denom;
                let pierce = Point3::new(
                    axis_origin.x + axis_dir.x * t,
                    axis_origin.y + axis_dir.y * t,
                    axis_origin.z + axis_dir.z * t,
                );

                // Build a 2D frame on the plane to run point-in-polygon.
                let u_dir = if n.x.abs() < 0.9 {
                    n.cross(&Vector3::new(1.0, 0.0, 0.0))
                } else {
                    n.cross(&Vector3::new(0.0, 1.0, 0.0))
                }
                .normalize()
                .unwrap_or(Vector3::X);
                let v_dir = n.cross(&u_dir).normalize().unwrap_or(Vector3::Y);

                let project_2d = |p: Point3| -> (f64, f64) {
                    let d = Vector3::new(p.x, p.y, p.z) - plane_origin_v;
                    (d.dot(&u_dir), d.dot(&v_dir))
                };

                // Collect ordered boundary vertices.
                let mut polygon: Vec<(f64, f64)> = Vec::new();
                for &edge_id in &loop_data.edges {
                    if let Some(edge) = model.edges.get(edge_id) {
                        if let Some(vertex) = model.vertices.get(edge.start_vertex) {
                            let p = Point3::new(
                                vertex.position[0],
                                vertex.position[1],
                                vertex.position[2],
                            );
                            polygon.push(project_2d(p));
                        }
                    }
                }

                if let Some(&last) = polygon.last() {
                    if polygon.len() >= 3 {
                        let (px, py) = project_2d(pierce);
                        let mut inside = false;
                        let mut prev = last;
                        for &curr in &polygon {
                            let (xi, yi) = curr;
                            let (xj, yj) = prev;
                            let crosses = (yi > py) != (yj > py)
                                && px < (xj - xi) * (py - yi) / (yj - yi) + xi;
                            if crosses {
                                inside = !inside;
                            }
                            prev = curr;
                        }
                        if inside {
                            return Ok(true);
                        }
                    }
                }
            }
        }
    }

    Ok(false)
}

/// Validate inputs for revolution
fn validate_revolve_inputs(
    model: &BRepModel,
    face_id: FaceId,
    options: &RevolveOptions,
) -> OperationResult<()> {
    // Check face exists
    if model.faces.get(face_id).is_none() {
        return Err(OperationError::InvalidGeometry(
            "Face not found".to_string(),
        ));
    }

    // Check angle is valid
    if options.angle <= 0.0 || options.angle > std::f64::consts::TAU * 2.0 {
        return Err(OperationError::InvalidGeometry(
            "Invalid revolution angle".to_string(),
        ));
    }

    // Check axis direction is valid
    if options.axis_direction.magnitude() < options.common.tolerance.distance() {
        return Err(OperationError::InvalidGeometry(
            "Invalid axis direction".to_string(),
        ));
    }

    // Check segments is reasonable
    if options.segments < 3 {
        return Err(OperationError::InvalidGeometry(
            "Too few segments for revolution".to_string(),
        ));
    }

    Ok(())
}

/// Validate the revolved solid by running the full B-Rep validation suite.
fn validate_revolved_solid(model: &BRepModel, solid_id: SolidId) -> OperationResult<()> {
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidBRep("Solid not found".to_string()));
    }
    // #29 — scope verdict to the revolved solid (see validate_solid_scoped).
    let result = crate::primitives::validation::validate_solid_scoped(
        model,
        solid_id,
        crate::math::Tolerance::default(),
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
            "Revolved solid failed validation ({} errors): {}",
            result.errors.len(),
            summary
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::curve::Line;
    use crate::primitives::topology_builder::BRepModel;

    /// Add a Line curve + Edge between two existing vertices.
    fn add_line_edge(model: &mut BRepModel, v_start: VertexId, v_end: VertexId) -> EdgeId {
        let s = model.vertices.get(v_start).expect("start vertex");
        let e = model.vertices.get(v_end).expect("end vertex");
        let line = Line::new(Point3::from(s.position), Point3::from(e.position));
        let curve_id = model.curves.add(Box::new(line));
        let edge = Edge::new_auto_range(0, v_start, v_end, curve_id, EdgeOrientation::Forward);
        model.edges.add(edge)
    }

    /// Closed CCW rectangle in the XZ plane offset along +X (so the Z axis
    /// does NOT pierce it). Profile lives at y = 0.
    fn make_offset_rectangle(model: &mut BRepModel) -> Vec<EdgeId> {
        let v0 = model.vertices.add(2.0, 0.0, 0.0);
        let v1 = model.vertices.add(4.0, 0.0, 0.0);
        let v2 = model.vertices.add(4.0, 0.0, 1.0);
        let v3 = model.vertices.add(2.0, 0.0, 1.0);
        vec![
            add_line_edge(model, v0, v1),
            add_line_edge(model, v1, v2),
            add_line_edge(model, v2, v3),
            add_line_edge(model, v3, v0),
        ]
    }

    /// Closed CCW rectangle in the XZ plane that straddles the Z axis
    /// (one edge on the axis itself).
    fn make_on_axis_rectangle(model: &mut BRepModel) -> Vec<EdgeId> {
        let v0 = model.vertices.add(0.0, 0.0, 0.0);
        let v1 = model.vertices.add(2.0, 0.0, 0.0);
        let v2 = model.vertices.add(2.0, 0.0, 1.0);
        let v3 = model.vertices.add(0.0, 0.0, 1.0);
        vec![
            add_line_edge(model, v0, v1),
            add_line_edge(model, v1, v2),
            add_line_edge(model, v2, v3),
            add_line_edge(model, v3, v0),
        ]
    }

    // -------------------------------------------------------------------
    // RevolveOptions defaults
    // -------------------------------------------------------------------

    #[test]
    fn revolve_options_default_values_match_documentation() {
        let opts = RevolveOptions::default();
        assert_eq!(opts.axis_origin, Point3::ZERO);
        assert_eq!(opts.axis_direction, Vector3::Z);
        assert!((opts.angle - std::f64::consts::TAU).abs() < 1e-12);
        assert!(!opts.symmetric);
        assert_eq!(opts.segments, 32);
        assert_eq!(opts.pitch, 0.0);
        assert!(opts.cap_ends);
    }

    // -------------------------------------------------------------------
    // validate_revolve_inputs
    // -------------------------------------------------------------------

    #[test]
    fn validate_revolve_inputs_rejects_unknown_face() {
        let model = BRepModel::new();
        let result = validate_revolve_inputs(&model, 999, &RevolveOptions::default());
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn validate_revolve_inputs_rejects_zero_angle() {
        let mut model = BRepModel::new();
        let edges = make_offset_rectangle(&mut model);
        let face_id = create_face_from_profile(&mut model, edges).expect("face");
        let opts = RevolveOptions {
            angle: 0.0,
            ..Default::default()
        };
        let result = validate_revolve_inputs(&model, face_id, &opts);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn validate_revolve_inputs_rejects_negative_angle() {
        let mut model = BRepModel::new();
        let edges = make_offset_rectangle(&mut model);
        let face_id = create_face_from_profile(&mut model, edges).expect("face");
        let opts = RevolveOptions {
            angle: -std::f64::consts::PI,
            ..Default::default()
        };
        let result = validate_revolve_inputs(&model, face_id, &opts);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn validate_revolve_inputs_rejects_oversized_angle() {
        let mut model = BRepModel::new();
        let edges = make_offset_rectangle(&mut model);
        let face_id = create_face_from_profile(&mut model, edges).expect("face");
        let opts = RevolveOptions {
            // > 4π (TAU * 2.0)
            angle: std::f64::consts::TAU * 2.5,
            ..Default::default()
        };
        let result = validate_revolve_inputs(&model, face_id, &opts);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn validate_revolve_inputs_accepts_full_double_revolution() {
        let mut model = BRepModel::new();
        let edges = make_offset_rectangle(&mut model);
        let face_id = create_face_from_profile(&mut model, edges).expect("face");
        let opts = RevolveOptions {
            // Exactly 4π is the upper inclusive bound.
            angle: std::f64::consts::TAU * 2.0,
            ..Default::default()
        };
        assert!(validate_revolve_inputs(&model, face_id, &opts).is_ok());
    }

    #[test]
    fn validate_revolve_inputs_rejects_zero_axis_direction() {
        let mut model = BRepModel::new();
        let edges = make_offset_rectangle(&mut model);
        let face_id = create_face_from_profile(&mut model, edges).expect("face");
        let opts = RevolveOptions {
            axis_direction: Vector3::new(0.0, 0.0, 0.0),
            ..Default::default()
        };
        let result = validate_revolve_inputs(&model, face_id, &opts);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn validate_revolve_inputs_rejects_too_few_segments() {
        let mut model = BRepModel::new();
        let edges = make_offset_rectangle(&mut model);
        let face_id = create_face_from_profile(&mut model, edges).expect("face");
        let opts = RevolveOptions {
            segments: 2,
            ..Default::default()
        };
        let result = validate_revolve_inputs(&model, face_id, &opts);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn validate_revolve_inputs_accepts_minimum_segment_count() {
        let mut model = BRepModel::new();
        let edges = make_offset_rectangle(&mut model);
        let face_id = create_face_from_profile(&mut model, edges).expect("face");
        let opts = RevolveOptions {
            segments: 3,
            ..Default::default()
        };
        assert!(validate_revolve_inputs(&model, face_id, &opts).is_ok());
    }

    #[test]
    fn validate_revolve_inputs_accepts_default_options() {
        let mut model = BRepModel::new();
        let edges = make_offset_rectangle(&mut model);
        let face_id = create_face_from_profile(&mut model, edges).expect("face");
        assert!(validate_revolve_inputs(&model, face_id, &RevolveOptions::default()).is_ok());
    }

    // -------------------------------------------------------------------
    // face_intersects_axis
    // -------------------------------------------------------------------

    #[test]
    fn face_intersects_axis_offset_rectangle_does_not_intersect() {
        let mut model = BRepModel::new();
        let edges = make_offset_rectangle(&mut model);
        let face_id = create_face_from_profile(&mut model, edges).expect("face");
        let face = model.faces.get(face_id).expect("face").clone();
        let result =
            face_intersects_axis(&model, &face, Point3::ZERO, Vector3::Z).expect("intersect query");
        assert!(!result, "offset rectangle should not touch the Z axis");
    }

    #[test]
    fn face_intersects_axis_on_axis_rectangle_does_not_intersect() {
        // A rectangle with one edge ON the Z axis (r=0..2, z=0..1) revolves to a
        // VALID solid cylinder — the axis edge sweeps to the cylinder centerline.
        // After the REVOLVE-POLE relaxation this is an axis TOUCH, not a
        // self-intersection, so it must NOT register.
        let mut model = BRepModel::new();
        let edges = make_on_axis_rectangle(&mut model);
        let face_id = create_face_from_profile(&mut model, edges).expect("face");
        let face = model.faces.get(face_id).expect("face").clone();
        let result =
            face_intersects_axis(&model, &face, Point3::ZERO, Vector3::Z).expect("intersect query");
        assert!(
            !result,
            "on-axis rectangle revolves to a valid cylinder — axis touch, not a crossing"
        );
    }

    #[test]
    fn face_intersects_axis_pierce_through_interior_detects() {
        // Profile in XY plane straddling origin; revolution axis = Z pierces it.
        let mut model = BRepModel::new();
        let v0 = model.vertices.add(-2.0, -2.0, 0.0);
        let v1 = model.vertices.add(2.0, -2.0, 0.0);
        let v2 = model.vertices.add(2.0, 2.0, 0.0);
        let v3 = model.vertices.add(-2.0, 2.0, 0.0);
        let edges = vec![
            add_line_edge(&mut model, v0, v1),
            add_line_edge(&mut model, v1, v2),
            add_line_edge(&mut model, v2, v3),
            add_line_edge(&mut model, v3, v0),
        ];
        let face_id = create_face_from_profile(&mut model, edges).expect("face");
        let face = model.faces.get(face_id).expect("face").clone();
        let result =
            face_intersects_axis(&model, &face, Point3::ZERO, Vector3::Z).expect("intersect query");
        assert!(result, "Z axis pierces the centered XY rectangle");
    }

    #[test]
    fn face_intersects_axis_offset_xy_rectangle_does_not_intersect() {
        // XY rectangle far from the Z axis — no pierce, no boundary touch.
        let mut model = BRepModel::new();
        let v0 = model.vertices.add(10.0, 10.0, 0.0);
        let v1 = model.vertices.add(12.0, 10.0, 0.0);
        let v2 = model.vertices.add(12.0, 12.0, 0.0);
        let v3 = model.vertices.add(10.0, 12.0, 0.0);
        let edges = vec![
            add_line_edge(&mut model, v0, v1),
            add_line_edge(&mut model, v1, v2),
            add_line_edge(&mut model, v2, v3),
            add_line_edge(&mut model, v3, v0),
        ];
        let face_id = create_face_from_profile(&mut model, edges).expect("face");
        let face = model.faces.get(face_id).expect("face").clone();
        let result =
            face_intersects_axis(&model, &face, Point3::ZERO, Vector3::Z).expect("intersect query");
        assert!(
            !result,
            "offset XY rectangle far from axis should not intersect"
        );
    }

    // -------------------------------------------------------------------
    // revolve_face / revolve_profile
    // -------------------------------------------------------------------

    #[test]
    fn revolve_profile_full_revolution_creates_solid() {
        let mut model = BRepModel::new();
        let edges = make_offset_rectangle(&mut model);
        // validate_result defaults to true: the result must be a VALID manifold
        // (the grid revolve produces a watertight-topology shell; the old
        // island-per-quad shell was non-manifold and only "passed" with
        // validation disabled).
        let opts = RevolveOptions::default();
        let solid_id = revolve_profile(&mut model, edges, opts).expect("revolve");
        assert!(model.solids.get(solid_id).is_some());
    }

    #[test]
    fn revolve_profile_partial_revolution_creates_solid_with_caps() {
        let mut model = BRepModel::new();
        let edges = make_offset_rectangle(&mut model);
        let opts = RevolveOptions {
            angle: std::f64::consts::PI, // half revolution
            cap_ends: true,
            ..Default::default() // validate_result defaults true → manifold check
        };
        let solid_id = revolve_profile(&mut model, edges, opts).expect("revolve");
        let solid = model.solids.get(solid_id).expect("solid");
        let shell = model.shells.get(solid.outer_shell).expect("shell");
        // Side faces (4 edges × default 32 segments) plus 2 caps.
        assert_eq!(shell.faces.len(), 4 * 32 + 2);
    }

    #[test]
    fn revolve_face_rejects_unknown_face_id() {
        let mut model = BRepModel::new();
        let opts = RevolveOptions {
            common: CommonOptions {
                validate_result: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let result = revolve_face(&mut model, 9999, opts);
        // F2-δ: pre-flight resolves entity IDs and returns InvalidInput.
        assert!(matches!(result, Err(OperationError::InvalidInput { .. })));
    }

    #[test]
    fn revolve_face_accepts_on_axis_cylinder_profile() {
        // An on-axis rectangle (r=0..2, z=0..1) revolves to a VALID solid
        // cylinder — the axis edge is a pole/centerline touch, not a
        // self-intersection. After the REVOLVE-POLE relaxation revolve_face must
        // ACCEPT it (previously wrongly rejected as SelfIntersection). A genuine
        // axis CROSSING is still rejected — see
        // face_intersects_axis_pierce_through_interior_detects.
        let mut model = BRepModel::new();
        let edges = make_on_axis_rectangle(&mut model);
        let face_id = create_face_from_profile(&mut model, edges).expect("face");
        let opts = RevolveOptions {
            common: CommonOptions {
                validate_result: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let result = revolve_face(&mut model, face_id, opts);
        assert!(
            result.is_ok(),
            "on-axis rectangle must revolve to a valid cylinder, got {result:?}"
        );
    }

    #[test]
    fn revolve_face_rejects_zero_angle() {
        let mut model = BRepModel::new();
        let edges = make_offset_rectangle(&mut model);
        let face_id = create_face_from_profile(&mut model, edges).expect("face");
        let opts = RevolveOptions {
            angle: 0.0,
            common: CommonOptions {
                validate_result: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let result = revolve_face(&mut model, face_id, opts);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn revolve_profile_helical_sweep_creates_solid() {
        // Non-zero pitch routes through create_helical_sweep.
        let mut model = BRepModel::new();
        let edges = make_offset_rectangle(&mut model);
        let opts = RevolveOptions {
            angle: std::f64::consts::TAU,
            pitch: 1.0,
            segments: 8,
            cap_ends: false,
            common: CommonOptions {
                validate_result: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let solid_id = revolve_profile(&mut model, edges, opts).expect("helical");
        assert!(model.solids.get(solid_id).is_some());
    }

    #[test]
    fn revolve_profile_with_x_axis_creates_solid() {
        // Profile in YZ-plane offset along +Y, revolved around X axis.
        let mut model = BRepModel::new();
        let v0 = model.vertices.add(0.0, 2.0, 0.0);
        let v1 = model.vertices.add(0.0, 4.0, 0.0);
        let v2 = model.vertices.add(0.0, 4.0, 1.0);
        let v3 = model.vertices.add(0.0, 2.0, 1.0);
        let edges = vec![
            add_line_edge(&mut model, v0, v1),
            add_line_edge(&mut model, v1, v2),
            add_line_edge(&mut model, v2, v3),
            add_line_edge(&mut model, v3, v0),
        ];
        let opts = RevolveOptions {
            axis_direction: Vector3::X,
            common: CommonOptions {
                validate_result: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let solid_id = revolve_profile(&mut model, edges, opts).expect("revolve");
        assert!(model.solids.get(solid_id).is_some());
    }

    // -------------------------------------------------------------------
    // validate_revolved_solid
    // -------------------------------------------------------------------

    #[test]
    fn validate_revolved_solid_rejects_unknown_solid() {
        let model = BRepModel::new();
        let result = validate_revolved_solid(&model, 9999);
        assert!(matches!(result, Err(OperationError::InvalidBRep(_))));
    }

    // -------------------------------------------------------------------
    // create_face_from_profile (revolve thin wrapper)
    // -------------------------------------------------------------------

    #[test]
    fn create_face_from_profile_wraps_extrude_helper() {
        let mut model = BRepModel::new();
        let edges = make_offset_rectangle(&mut model);
        let face_id = create_face_from_profile(&mut model, edges).expect("face");
        assert!(model.faces.get(face_id).is_some());
    }

    #[test]
    fn create_face_from_empty_profile_is_error() {
        let mut model = BRepModel::new();
        let result = create_face_from_profile(&mut model, vec![]);
        assert!(result.is_err());
    }

    // -------------------------------------------------------------------
    // create_degenerate_edge
    // -------------------------------------------------------------------

    #[test]
    fn create_degenerate_edge_produces_zero_length_self_loop() {
        let mut model = BRepModel::new();
        let v = model.vertices.add(1.0, 2.0, 3.0);
        let edge_id = create_degenerate_edge(&mut model, v, v).expect("degenerate edge");
        let edge = model.edges.get(edge_id).expect("edge");
        assert_eq!(edge.start_vertex, v);
        assert_eq!(edge.end_vertex, v);
    }

    #[test]
    fn create_degenerate_edge_rejects_unknown_vertex() {
        let mut model = BRepModel::new();
        let result = create_degenerate_edge(&mut model, 9999, 9999);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }
}
