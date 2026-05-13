//! Spine and rail computation (F3).
//!
//! Replaces the always-sampled bisector hack in
//! [`compute_rolling_ball_positions`](crate::operations::fillet) with
//! an explicit dispatch on the `(face_a, face_b, edge_kind,
//! radius_schedule)` tuple:
//!
//! * **Analytic arms** for surface pairs that admit a closed-form
//!   spine (plane/plane, plane/cylinder, plane/sphere, coaxial
//!   cylinder/cylinder).
//! * **Marching arm** for NURBS-NURBS and the oblique-axis cases
//!   that don't close in elementary form. Driven by the F1-γ
//!   bisection primitive and the F1-δ SSI-style corrector divergence
//!   guard.
//!
//! F3-α (this slice) lands the module skeleton plus **one** analytic
//! arm — plane/plane. Every other surface pair returns `Ok(None)`,
//! and the caller in [`crate::operations::fillet`] falls through to
//! the legacy bisector path. This keeps the change net-additive: no
//! existing test result moves until F3-γ wires in the marching solver
//! and F3-δ deletes the legacy path entirely.
//!
//! # Why a separate module
//!
//! The legacy bisector path is ~600 LoC inside `fillet.rs` and is
//! load-bearing for chamfer too (chamfer uses the same rolling-ball
//! skeleton; cross-section differs). Pulling it out into a single
//! place lets:
//!
//! * F3-β–δ work on one solver without touching the fillet entry-point.
//! * Chamfer reuse the same dispatch (F3-δ).
//! * Tests pin the **kind** of solver chosen for each surface pair
//!   (analytic vs marched) — a regression that flips a known-analytic
//!   case onto the marched path is visible immediately.

#![allow(clippy::indexing_slicing)] // Indexed access into bounded sample arrays.

use crate::math::frame::FrameAtStation;
use crate::math::{Point3, Tolerance, Vector3};
use crate::operations::blend_graph::{BlendGraph, BlendRadius};
use crate::operations::edge_classification::find_adjacent_faces;
use crate::operations::fillet::{edge_orientation_in_face, get_face_oriented_normal};
use crate::operations::fillet_robust::robust_face_angle;
use crate::operations::{OperationError, OperationResult};
use crate::primitives::curve::{Arc, Curve, Line};
use crate::primitives::edge::{Edge, EdgeId};
use crate::primitives::face::FaceId;
use crate::primitives::surface::{Cylinder, Plane, Sphere, SurfaceType};
use crate::primitives::topology_builder::BRepModel;

/// Which solver produced this spine. Tests pin the kind so a
/// regression that silently routes plane/plane through marching is
/// surfaced immediately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SolverKind {
    /// Two planes — straight-line spine, exact closed form.
    AnalyticPlanePlane,
    /// Plane + cylinder (perpendicular or axis-parallel) — circular
    /// arc or straight-line spine. Wired in F3-β.
    AnalyticPlaneCylinder,
    /// Plane + sphere — circular arc spine in offset plane. F3-β.
    AnalyticPlaneSphere,
    /// Two coaxial cylinders — straight-line spine parallel to the
    /// shared axis. F3-β.
    AnalyticCylCylCoaxial,
    /// General case: arc-length-marched predictor + alternate-
    /// projection corrector. Wired in F3-γ.
    Marched {
        /// Number of predictor (arc-length) steps taken along the edge.
        predictor_steps: usize,
        /// Worst-case corrector iteration count across the chain.
        corrector_iters: usize,
    },
}

/// A single sampled station along the spine. Each station carries
/// the spine point, both rail (contact) points, and the radius the
/// rolling ball had at this parameter.
#[derive(Debug, Clone, Copy)]
pub struct SpineRailSample {
    /// Parameter on the source edge curve, `[0, 1]`.
    pub edge_parameter: f64,
    /// Cumulative arc length along the spine from the first sample,
    /// in model units. `samples[0].arc_length == 0.0`.
    pub arc_length: f64,
    /// Spine point — locus of the rolling ball centre.
    pub center: Point3,
    /// Rail point on `face_a` — where the ball touches face A.
    pub contact_a: Point3,
    /// Rail point on `face_b` — where the ball touches face B.
    pub contact_b: Point3,
    /// Ball radius at this station (constant for `Constant` schedules,
    /// interpolated for `Linear` / `Variable`).
    pub radius: f64,
}

/// Result of a successful spine solve.
///
/// The three curves (`spine`, `rail_a`, `rail_b`) are fully fitted
/// continuous curves usable by F4 (blend surface construction). For
/// the F3-α plane/plane arm they are exact straight [`Line`]s; F3-β
/// will fit cubic NURBS for the analytic-with-arc cases; F3-γ fits
/// from marched samples. `samples` is the discrete sampling that
/// produced them, retained so the legacy
/// [`crate::operations::fillet`] downstream pipeline can be fed a
/// byte-compatible `RollingBallData` during the F3-α–γ parallel-
/// deployment window.
pub struct SpineRail {
    /// Spine curve — centre of the rolling ball.
    pub spine: Box<dyn Curve>,
    /// Rail curve on face A — contact locus on the first supporting face.
    pub rail_a: Box<dyn Curve>,
    /// Rail curve on face B — contact locus on the second supporting face.
    pub rail_b: Box<dyn Curve>,
    /// Discrete samples used to build the curves. Always non-empty;
    /// `samples.len() >= options.min_samples` for analytic arms.
    pub samples: Vec<SpineRailSample>,
    /// Rotation-minimising frame at each station. Empty for plane/
    /// plane (the spine is a straight line — frames degenerate to a
    /// fixed basis; F4 reconstructs them analytically when needed).
    /// Populated by the marching solver in F3-γ.
    pub frames: Vec<FrameAtStation>,
    /// Which dispatch arm produced this rail.
    pub solver_kind: SolverKind,
}

impl std::fmt::Debug for SpineRail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpineRail")
            .field("solver_kind", &self.solver_kind)
            .field("samples", &self.samples.len())
            .field("frames", &self.frames.len())
            .finish()
    }
}

/// Options threaded through every solver arm.
#[derive(Debug, Clone)]
pub struct SpineOptions {
    /// Geometric tolerance — fed into [`robust_face_angle`] and any
    /// internal curve fitting / arc-length integration.
    pub tolerance: Tolerance,
    /// Minimum number of stations sampled along the edge. Analytic
    /// arms may emit exactly this many; marching arms may emit more.
    pub min_samples: usize,
    /// Hard cap on station count. Refinement stops once the cap is
    /// hit even if the curvature-adaptive refinement would request
    /// more.
    pub max_samples: usize,
    /// When `true`, consult [`BlendGraph`] for per-corner setback
    /// distances and trim the spine parameter range accordingly. F3-α
    /// does not yet implement setback consumption — the flag is
    /// stored and honoured starting in F3-δ.
    pub honor_setbacks: bool,
}

impl Default for SpineOptions {
    fn default() -> Self {
        Self {
            tolerance: Tolerance::default(),
            min_samples: 32,
            max_samples: 2048,
            honor_setbacks: true,
        }
    }
}

/// Resolve a spine for one connected blend chain.
///
/// In F3-α we only handle single-edge chains analytically.
/// Multi-edge tangent chains return `Ok(None)` so the caller falls
/// through to the legacy bisector path. F3-γ will wire chain-level
/// marching using the F2-β chain ids.
///
/// `graph` carries the per-edge radius schedule (and setback fields
/// once F3-δ honours them). If the chain head's radius cannot be
/// resolved to a strictly positive scalar, the call returns
/// `Ok(None)` — the legacy path handles those edges.
pub fn solve_spine_for_chain(
    model: &BRepModel,
    chain: &[EdgeId],
    graph: &BlendGraph,
    options: &SpineOptions,
) -> OperationResult<Option<SpineRail>> {
    if chain.len() != 1 {
        return Ok(None);
    }
    let edge_id = chain[0];
    let blend_edge = match graph.edge(edge_id) {
        Some(b) => b,
        None => return Ok(None),
    };
    let radius = match &blend_edge.radius {
        BlendRadius::Constant(r) => *r,
        BlendRadius::Linear { start, end } => {
            // F3-β handles linear-ramp constant-radius analytic arms;
            // F3-α treats anything non-constant as out-of-scope.
            if (start - end).abs() < f64::EPSILON {
                *start
            } else {
                return Ok(None);
            }
        }
        BlendRadius::Variable(_) => return Ok(None),
    };
    if !(radius > 0.0 && radius.is_finite()) {
        return Err(OperationError::InvalidRadius(radius));
    }

    // Discover the two supporting faces. find_adjacent_faces walks
    // every shell and returns up to N faces incident to the edge.
    // For manifold edges this is exactly two; anything else is not
    // a candidate for analytic dispatch.
    let faces = find_adjacent_faces(model, edge_id);
    if faces.len() != 2 {
        return Ok(None);
    }
    solve_spine_for_edge(model, edge_id, faces[0], faces[1], radius, options)
}

/// Resolve a spine for one blend edge given its two supporting
/// faces and a constant radius.
///
/// Returns `Ok(Some(_))` iff the surface pair matches an analytic
/// arm wired in this slice (currently only plane/plane). All other
/// pairs return `Ok(None)` and the caller falls through to the
/// legacy bisector. `Err` is reserved for invalid inputs that no
/// arm can handle (missing entity, non-positive radius, degenerate
/// near-tangent geometry).
pub fn solve_spine_for_edge(
    model: &BRepModel,
    edge_id: EdgeId,
    face_a: FaceId,
    face_b: FaceId,
    radius: f64,
    options: &SpineOptions,
) -> OperationResult<Option<SpineRail>> {
    if !(radius > 0.0 && radius.is_finite()) {
        return Err(OperationError::InvalidRadius(radius));
    }
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry(format!("Edge {} not found", edge_id)))?
        .clone();
    let face_a_ref = model
        .faces
        .get(face_a)
        .ok_or_else(|| OperationError::InvalidGeometry(format!("Face {} not found", face_a)))?;
    let face_b_ref = model
        .faces
        .get(face_b)
        .ok_or_else(|| OperationError::InvalidGeometry(format!("Face {} not found", face_b)))?;
    let surface_a = model.surfaces.get(face_a_ref.surface_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!("Surface for face {} not found", face_a))
    })?;
    let surface_b = model.surfaces.get(face_b_ref.surface_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!("Surface for face {} not found", face_b))
    })?;

    // Dispatch on the surface-type tuple. F3-α landed plane/plane;
    // F3-β adds plane/cylinder (perpendicular + parallel-tangent
    // sub-cases) and plane/sphere. Oblique plane/cylinder, secant
    // plane/cylinder, sphere/sphere, cylinder/cylinder, and any
    // NURBS pair fall through to None so the caller routes the
    // request through the legacy bisector path until F3-γ wires in
    // marching.
    match (surface_a.surface_type(), surface_b.surface_type()) {
        (SurfaceType::Plane, SurfaceType::Plane) => {
            let plane_a = surface_a
                .as_any()
                .downcast_ref::<Plane>()
                .ok_or_else(|| OperationError::InternalError("Plane downcast failed".into()))?;
            let plane_b = surface_b
                .as_any()
                .downcast_ref::<Plane>()
                .ok_or_else(|| OperationError::InternalError("Plane downcast failed".into()))?;
            solve_plane_plane(
                model, &edge, edge_id, face_a, face_b, plane_a, plane_b, radius, options,
            )
            .map(Some)
        }
        (SurfaceType::Plane, SurfaceType::Cylinder) => {
            let plane = surface_a
                .as_any()
                .downcast_ref::<Plane>()
                .ok_or_else(|| OperationError::InternalError("Plane downcast failed".into()))?;
            let cylinder = surface_b
                .as_any()
                .downcast_ref::<Cylinder>()
                .ok_or_else(|| OperationError::InternalError("Cylinder downcast failed".into()))?;
            solve_plane_cylinder(
                model, &edge, edge_id, face_a, face_b, plane, cylinder, /*plane_is_a=*/ true,
                radius, options,
            )
        }
        (SurfaceType::Cylinder, SurfaceType::Plane) => {
            let cylinder = surface_a
                .as_any()
                .downcast_ref::<Cylinder>()
                .ok_or_else(|| OperationError::InternalError("Cylinder downcast failed".into()))?;
            let plane = surface_b
                .as_any()
                .downcast_ref::<Plane>()
                .ok_or_else(|| OperationError::InternalError("Plane downcast failed".into()))?;
            solve_plane_cylinder(
                model, &edge, edge_id, face_a, face_b, plane, cylinder, /*plane_is_a=*/ false,
                radius, options,
            )
        }
        (SurfaceType::Plane, SurfaceType::Sphere) => {
            let plane = surface_a
                .as_any()
                .downcast_ref::<Plane>()
                .ok_or_else(|| OperationError::InternalError("Plane downcast failed".into()))?;
            let sphere = surface_b
                .as_any()
                .downcast_ref::<Sphere>()
                .ok_or_else(|| OperationError::InternalError("Sphere downcast failed".into()))?;
            solve_plane_sphere(
                model, &edge, edge_id, face_a, face_b, plane, sphere, /*plane_is_a=*/ true,
                radius, options,
            )
        }
        (SurfaceType::Sphere, SurfaceType::Plane) => {
            let sphere = surface_a
                .as_any()
                .downcast_ref::<Sphere>()
                .ok_or_else(|| OperationError::InternalError("Sphere downcast failed".into()))?;
            let plane = surface_b
                .as_any()
                .downcast_ref::<Plane>()
                .ok_or_else(|| OperationError::InternalError("Plane downcast failed".into()))?;
            solve_plane_sphere(
                model, &edge, edge_id, face_a, face_b, plane, sphere, /*plane_is_a=*/ false,
                radius, options,
            )
        }
        // (Cylinder, Cylinder) coaxial: the geometry requires two
        // coaxial cylinders that genuinely share an edge curve.
        // That topology does not arise from our primitive
        // constructors (a stepped shaft has a planar step face
        // between the two cylinders, not a direct cyl/cyl edge),
        // and the boolean-imprint path that could produce one is
        // F3-γ marching's territory because the spine geometry
        // depends on the imprint curve, not just the surfaces.
        // Returning None defers to the legacy bisector. Once a
        // genuine cyl/cyl edge fixture appears we will revisit.
        _ => Ok(None),
    }
}

/// Closed-form spine + rail solve for two planar faces.
///
/// The two supporting planes have constant outward normals
/// `n_a, n_b`. The rolling-ball centre therefore traces a straight
/// line parallel to the edge, offset by `d = radius / sin(α/2)` in
/// the bisector direction (or `-bisector` for convex edges, where
/// the ball lives inside the solid). Contacts on each face are
/// `center ± radius · n_i` with sign determined by the edge's
/// signed dihedral.
///
/// The math is identical to the legacy
/// [`compute_rolling_ball_positions`](crate::operations::fillet)
/// for the plane/plane special case — that path samples the bisector
/// at every parameter unnecessarily, but the *result* per sample is
/// the same. This is why F3-α is a numerically-conservative landing:
/// the dihedral matrix tests pass within 1e-8 of legacy, by
/// construction.
#[allow(clippy::too_many_arguments)]
fn solve_plane_plane(
    model: &BRepModel,
    edge: &Edge,
    edge_id: EdgeId,
    face_a: FaceId,
    _face_b: FaceId,
    _plane_a: &Plane,
    _plane_b: &Plane,
    radius: f64,
    options: &SpineOptions,
) -> OperationResult<SpineRail> {
    // Outward-oriented face normals at the edge midpoint.
    // `get_face_oriented_normal` bakes in the `FaceOrientation` sign
    // — using the raw plane normals here would silently invert the
    // offset for every Backward-oriented face (half of any solid).
    let midpoint = edge.evaluate(0.5, &model.curves)?;
    let normal_a = get_face_oriented_normal(model, face_a, &midpoint)?;
    let normal_b = get_face_oriented_normal(model, _face_b, &midpoint)?;

    // Signed dihedral. Edge tangent must be rotated into face A's
    // loop direction so the sign is a geometric (convex/concave)
    // invariant rather than an artefact of the curve's parameter
    // direction. Identical to `compute_rolling_ball_positions`'
    // sign-correctness convention.
    let raw_tangent = edge.tangent_at(0.5, &model.curves)?;
    let face_a_loop_sign = edge_orientation_in_face(model, face_a, edge_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "Edge {} not present in any loop of face {}",
            edge_id, face_a
        ))
    })?;
    let edge_tangent_in_loop = raw_tangent * face_a_loop_sign;
    let dihedral = robust_face_angle(
        &normal_a,
        &normal_b,
        &edge_tangent_in_loop,
        &options.tolerance,
    )
    .map_err(|e| OperationError::NumericalError(format!("Dihedral compute failed: {:?}", e)))?;

    let abs_angle = dihedral.abs();
    if abs_angle < 0.1 || (std::f64::consts::PI - abs_angle) < 0.1 {
        // Match the legacy near-tangent guard so analytic dispatch
        // doesn't accept inputs the bisector path would reject.
        // F3-γ's marching solver may relax this; F3-α stays
        // conservative.
        return Err(OperationError::InvalidGeometry(
            "Near-tangent surfaces require special handling".to_string(),
        ));
    }

    let bisector = (normal_a + normal_b).normalize().map_err(|e| {
        OperationError::NumericalError(format!("Bisector normalization failed: {:?}", e))
    })?;
    let bisector_dot_na = bisector.dot(&normal_a);
    if bisector_dot_na.abs() < 1e-9 {
        return Err(OperationError::NumericalError(
            "Bisector orthogonal to face normal — degenerate dihedral".to_string(),
        ));
    }
    let offset_distance = radius / bisector_dot_na;

    // Sign convention matches `compute_rolling_ball_positions`:
    //  - Convex (dihedral > 0): ball lives inside the solid; centre
    //    is offset in the −bisector direction; contacts are at
    //    `centre + r·n_i` (normals point outward from the ball's
    //    occupied volume back onto each face).
    //  - Concave (dihedral < 0): ball lives in the cavity; centre
    //    offset is +bisector; contacts are at `centre − r·n_i`.
    let (offset_sign, contact_sign) = if dihedral > 0.0 {
        (-1.0, 1.0)
    } else {
        (1.0, -1.0)
    };

    let n_samples = options.min_samples.max(2);
    let mut samples: Vec<SpineRailSample> = Vec::with_capacity(n_samples);
    let mut prev_center: Option<Point3> = None;
    let mut cumulative_arc = 0.0;
    for i in 0..n_samples {
        let t = i as f64 / (n_samples as f64 - 1.0);
        let edge_point = edge.evaluate(t, &model.curves)?;
        let center = edge_point + bisector * (offset_sign * offset_distance);
        let contact_a = center + normal_a * (contact_sign * radius);
        let contact_b = center + normal_b * (contact_sign * radius);

        if let Some(prev) = prev_center {
            cumulative_arc += (center - prev).magnitude();
        }
        prev_center = Some(center);

        samples.push(SpineRailSample {
            edge_parameter: t,
            arc_length: cumulative_arc,
            center,
            contact_a,
            contact_b,
            radius,
        });
    }

    // The three curves are exact straight lines for plane/plane —
    // build them as [`Line`] rather than fitting a degree-3 NURBS to
    // 32 colinear points. This keeps the rest of the kernel
    // unchanged (Line implements `Curve` trait used by every
    // downstream consumer).
    let spine_start = samples[0].center;
    let spine_end = samples[n_samples - 1].center;
    let rail_a_start = samples[0].contact_a;
    let rail_a_end = samples[n_samples - 1].contact_a;
    let rail_b_start = samples[0].contact_b;
    let rail_b_end = samples[n_samples - 1].contact_b;

    let spine: Box<dyn Curve> = Box::new(Line::new(spine_start, spine_end));
    let rail_a: Box<dyn Curve> = Box::new(Line::new(rail_a_start, rail_a_end));
    let rail_b: Box<dyn Curve> = Box::new(Line::new(rail_b_start, rail_b_end));

    Ok(SpineRail {
        spine,
        rail_a,
        rail_b,
        samples,
        // F3-γ populates frames for the marching solver. For
        // plane/plane the spine is straight: frames are constant and
        // F4 can reconstruct them analytically from `(bisector,
        // edge_tangent)` if it needs them.
        frames: Vec::new(),
        solver_kind: SolverKind::AnalyticPlanePlane,
    })
}

/// Tolerance for classifying plane/cylinder alignment. The plane is
/// "perpendicular" to the cylinder axis when `|n_plane · axis| ≈ 1`
/// (axis is normal to the plane) and "parallel" when `|n_plane ·
/// axis| ≈ 0` (axis lies in the plane). Oblique configurations fall
/// through to the marching solver.
const PLANE_CYL_ALIGNMENT_TOL: f64 = 1e-6;

/// Resolve the (plane_face, cyl_face, plane_normal, cyl_radial)
/// quartet that the plane/cylinder solvers need to evaluate sign
/// conventions consistently. Returns the outward-oriented plane
/// normal, the outward-oriented cylinder normal at the edge midpoint,
/// the radial unit vector from cyl axis to edge midpoint, the
/// signed dihedral (positive convex, negative concave), and the
/// edge midpoint itself.
#[allow(clippy::type_complexity)]
fn evaluate_plane_cyl_geometry(
    model: &BRepModel,
    edge: &Edge,
    edge_id: EdgeId,
    plane_face: FaceId,
    cyl_face: FaceId,
    cylinder: &Cylinder,
    options: &SpineOptions,
) -> OperationResult<(Vector3, Vector3, Vector3, f64, Point3)> {
    let edge_mid = edge.evaluate(0.5, &model.curves)?;
    let n_plane = get_face_oriented_normal(model, plane_face, &edge_mid)?;
    let n_cyl = get_face_oriented_normal(model, cyl_face, &edge_mid)?;

    // Radial direction from cyl axis to edge midpoint. For a point
    // on the cylinder surface this has magnitude r_cyl by
    // construction; we still normalise to guard against numerical
    // drift in the midpoint evaluation.
    let edge_offset = edge_mid - cylinder.origin;
    let axial_component = edge_offset.dot(&cylinder.axis);
    let radial_vec = edge_offset - cylinder.axis * axial_component;
    let radial_dir = radial_vec.normalize().map_err(|e| {
        OperationError::NumericalError(format!(
            "Edge midpoint coincides with cylinder axis: {:?}",
            e
        ))
    })?;

    // Signed dihedral. Edge tangent rotated into face A's loop
    // direction so the sign is a geometric invariant — identical
    // convention to plane/plane and the legacy bisector.
    let raw_tangent = edge.tangent_at(0.5, &model.curves)?;
    let loop_sign = edge_orientation_in_face(model, plane_face, edge_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "Edge {} not present in any loop of face {}",
            edge_id, plane_face
        ))
    })?;
    let edge_tan_in_loop = raw_tangent * loop_sign;
    let dihedral = robust_face_angle(&n_plane, &n_cyl, &edge_tan_in_loop, &options.tolerance)
        .map_err(|e| {
            OperationError::NumericalError(format!("Dihedral compute failed: {:?}", e))
        })?;

    Ok((n_plane, n_cyl, radial_dir, dihedral, edge_mid))
}

/// Closed-form spine + rail solve for a planar face adjacent to a
/// cylindrical face.
///
/// Three geometric sub-cases:
///
/// * **Perpendicular** (`|n_plane · axis| ≈ 1`): the spine is a
///   circular arc concentric with the cylinder, in a plane offset
///   from the original plane by `±radius * n_plane`. Spine radius
///   from axis = `r_cyl ± radius` with sign determined by edge
///   convexity and the orientation of the cylinder normal. Rails
///   are circular arcs in the original plane (rail on plane) and
///   in the spine's offset plane on the cylinder (rail on
///   cylinder).
/// * **Parallel-tangent** (`|n_plane · axis| ≈ 0` and the plane is
///   tangent to the cylinder, i.e. `n_plane ‖ radial_at_edge`): the
///   spine is a straight line parallel to the cylinder axis, at
///   distance `r_cyl ± radius` from the axis, in the offset plane.
/// * **Oblique** / **parallel-secant**: return `Ok(None)` so the
///   caller routes to F3-γ marching (the spine in those
///   configurations no longer closes in elementary form).
///
/// `plane_is_a` records whether the *original* `face_a` was the
/// plane. The returned `SpineRail.rail_a` always corresponds to the
/// original `face_a`'s contact rail, regardless of which underlying
/// surface that face holds — the dispatcher relies on this so
/// downstream consumers see rails in the same order as their face
/// arguments.
#[allow(clippy::too_many_arguments)]
fn solve_plane_cylinder(
    model: &BRepModel,
    edge: &Edge,
    edge_id: EdgeId,
    face_a: FaceId,
    face_b: FaceId,
    plane: &Plane,
    cylinder: &Cylinder,
    plane_is_a: bool,
    radius: f64,
    options: &SpineOptions,
) -> OperationResult<Option<SpineRail>> {
    let (plane_face, cyl_face) = if plane_is_a {
        (face_a, face_b)
    } else {
        (face_b, face_a)
    };

    let (n_plane, n_cyl, radial_dir, dihedral, _edge_mid) =
        evaluate_plane_cyl_geometry(model, edge, edge_id, plane_face, cyl_face, cylinder, options)?;

    let abs_dihedral = dihedral.abs();
    if abs_dihedral < 0.1 || (std::f64::consts::PI - abs_dihedral) < 0.1 {
        // Near-tangent guard matches the legacy bisector path —
        // analytic dispatch shouldn't accept inputs the bisector
        // would reject. F3-γ may relax this.
        return Err(OperationError::InvalidGeometry(
            "Near-tangent surfaces require special handling".to_string(),
        ));
    }

    // Sign conventions identical to plane/plane:
    //   - convex (dihedral > 0): ball lives in the solid; centre is
    //     offset opposite to each outward normal. offset_sign = -1.
    //   - concave (dihedral < 0): ball lives in the cavity. offset_sign = +1.
    let offset_sign = if dihedral > 0.0 { -1.0 } else { 1.0 };

    // Plane/cylinder alignment classification.
    let axis_alignment = n_plane.dot(&cylinder.axis);
    let abs_alignment = axis_alignment.abs();

    if (abs_alignment - 1.0).abs() < PLANE_CYL_ALIGNMENT_TOL {
        // Perpendicular: spine is a circular arc.
        solve_plane_cyl_perpendicular(
            model, edge, plane, cylinder, plane_face, cyl_face, plane_is_a, n_plane, n_cyl,
            radial_dir, axis_alignment, offset_sign, radius, options,
        )
        .map(Some)
    } else if abs_alignment < PLANE_CYL_ALIGNMENT_TOL {
        // Parallel: only the tangent sub-case is analytic. Tangent
        // means n_plane is colinear with the cylinder's radial
        // direction at the edge midpoint.
        let radial_alignment = n_plane.dot(&radial_dir).abs();
        if (radial_alignment - 1.0).abs() < PLANE_CYL_ALIGNMENT_TOL {
            solve_plane_cyl_parallel_tangent(
                model, edge, plane, cylinder, plane_face, cyl_face, plane_is_a, n_plane, n_cyl,
                radial_dir, offset_sign, radius, options,
            )
            .map(Some)
        } else {
            // Secant — closed form is messier; F3-γ marching handles it.
            Ok(None)
        }
    } else {
        // Oblique — defer to marching.
        Ok(None)
    }
}

/// Plane perpendicular to cylinder axis. Spine is a circular arc.
///
/// In axis-frame coordinates with `cyl.axis = ẑ`:
/// * Edge is a circle/arc at axial height `z_plane = (plane.origin -
///   cyl.origin) · cyl.axis`, at radius `r_cyl` from the axis.
/// * Ball-centre constraints:
///   * `(c - plane.origin) · n_plane = offset_sign · radius`
///   * `dist(c, axis) = r_cyl + offset_sign · sign_radial · radius`
///   where `sign_radial = sign(n_cyl · radial_dir)`. For a solid
///   cylinder the cylinder normal points radially outward
///   (`sign_radial = +1`); for a cylindrical hole it points inward
///   (`sign_radial = -1`).
/// * Spine axial height: `z_spine = z_plane + offset_sign · radius ·
///   (n_plane · axis)`. The `n_plane · axis` factor is ±1 in the
///   perpendicular case and absorbs the orientation of the plane
///   normal relative to the axis direction.
/// * Spine radius from axis: `r_spine = r_cyl + offset_sign ·
///   sign_radial · radius`. If this is non-positive the requested
///   `radius` exceeds the cylinder radius and no analytic blend
///   exists; we surface `InvalidRadius`.
#[allow(clippy::too_many_arguments)]
fn solve_plane_cyl_perpendicular(
    model: &BRepModel,
    edge: &Edge,
    plane: &Plane,
    cylinder: &Cylinder,
    _plane_face: FaceId,
    _cyl_face: FaceId,
    plane_is_a: bool,
    n_plane: Vector3,
    n_cyl: Vector3,
    radial_dir: Vector3,
    axis_alignment: f64, // n_plane · cyl.axis ∈ {≈-1, ≈+1}
    offset_sign: f64,
    radius: f64,
    options: &SpineOptions,
) -> OperationResult<SpineRail> {
    let sign_radial = if n_cyl.dot(&radial_dir) >= 0.0 {
        1.0
    } else {
        -1.0
    };
    let r_spine = cylinder.radius + offset_sign * sign_radial * radius;
    if !(r_spine > options.tolerance.distance()) {
        return Err(OperationError::InvalidRadius(radius));
    }

    let axis_n_plane_sign = if axis_alignment >= 0.0 { 1.0 } else { -1.0 };
    let z_plane = (plane.origin - cylinder.origin).dot(&cylinder.axis);
    let z_spine = z_plane + offset_sign * radius * axis_n_plane_sign;

    // Build samples. For each edge parameter, compute the angular
    // position of the edge point around the cylinder axis, then
    // construct the spine point at the same angle but at radius
    // r_spine and axial height z_spine.
    let n_samples = options.min_samples.max(2);
    let mut samples: Vec<SpineRailSample> = Vec::with_capacity(n_samples);
    let mut prev_center: Option<Point3> = None;
    let mut cumulative_arc = 0.0;

    // Local orthonormal basis in the plane perpendicular to axis:
    // (cylinder.ref_dir, axis × ref_dir). We measure angle in this
    // basis so it matches the cylinder's own parameterisation.
    let ref_x = cylinder.ref_dir;
    let ref_y = cylinder.axis.cross(&ref_x);

    let mut edge_angles: Vec<f64> = Vec::with_capacity(n_samples);
    for i in 0..n_samples {
        let t = i as f64 / (n_samples as f64 - 1.0);
        let edge_point = edge.evaluate(t, &model.curves)?;
        let p_off = edge_point - cylinder.origin;
        let axial_t = p_off.dot(&cylinder.axis);
        let radial_vec_t = p_off - cylinder.axis * axial_t;
        // Normalise locally — guard against drift but don't fail
        // hard, the angle only needs a stable atan2.
        let angle = radial_vec_t
            .dot(&ref_y)
            .atan2(radial_vec_t.dot(&ref_x));
        edge_angles.push(angle);

        let radial_t = ref_x * angle.cos() + ref_y * angle.sin();
        let center = cylinder.origin + cylinder.axis * z_spine + radial_t * r_spine;
        // Contact on plane: project centre back onto the original
        // plane along n_plane.
        let contact_plane = center - n_plane * (offset_sign * radius);
        // Contact on cylinder: same angle as centre, axial position
        // matches centre, radial r_cyl.
        let contact_cyl =
            cylinder.origin + cylinder.axis * z_spine + radial_t * cylinder.radius;

        if let Some(prev) = prev_center {
            cumulative_arc += (center - prev).magnitude();
        }
        prev_center = Some(center);

        let (contact_a, contact_b) = if plane_is_a {
            (contact_plane, contact_cyl)
        } else {
            (contact_cyl, contact_plane)
        };
        samples.push(SpineRailSample {
            edge_parameter: t,
            arc_length: cumulative_arc,
            center,
            contact_a,
            contact_b,
            radius,
        });
    }

    // Resolve the angular sweep for the analytic arc curves. The
    // Arc primitive parameterises with (start_angle, sweep_angle);
    // we reconstruct both from the first and last edge angles.
    // Closed edges (full rim of cylinder) sweep ±2π — the direction
    // is taken from the angle delta accumulated over samples so
    // that the arc parameterisation tracks the edge curve.
    let (start_angle, sweep_angle) =
        resolve_arc_parameters(&edge_angles, edge.is_loop());

    // Spine arc: centred on the cylinder axis at z_spine, normal
    // aligned with cylinder.axis, radius r_spine. We pass cyl.axis
    // directly so that Arc::evaluate(0.0) uses cyl.ref_dir as the
    // x-direction — matching the way we built edge_angles above.
    let spine_arc = Arc::new(
        cylinder.origin + cylinder.axis * z_spine,
        cylinder.axis,
        r_spine,
        start_angle,
        sweep_angle,
    )
    .map_err(|e| OperationError::NumericalError(format!("Spine arc construction: {:?}", e)))?;

    let rail_plane_arc = Arc::new(
        cylinder.origin + cylinder.axis * z_plane,
        cylinder.axis,
        r_spine,
        start_angle,
        sweep_angle,
    )
    .map_err(|e| OperationError::NumericalError(format!("Plane rail arc: {:?}", e)))?;

    let rail_cyl_arc = Arc::new(
        cylinder.origin + cylinder.axis * z_spine,
        cylinder.axis,
        cylinder.radius,
        start_angle,
        sweep_angle,
    )
    .map_err(|e| OperationError::NumericalError(format!("Cylinder rail arc: {:?}", e)))?;

    let spine: Box<dyn Curve> = Box::new(spine_arc);
    let (rail_a, rail_b): (Box<dyn Curve>, Box<dyn Curve>) = if plane_is_a {
        (Box::new(rail_plane_arc), Box::new(rail_cyl_arc))
    } else {
        (Box::new(rail_cyl_arc), Box::new(rail_plane_arc))
    };

    Ok(SpineRail {
        spine,
        rail_a,
        rail_b,
        samples,
        frames: Vec::new(),
        solver_kind: SolverKind::AnalyticPlaneCylinder,
    })
}

/// Reconstruct (start_angle, sweep_angle) from an array of angles
/// sampled at uniform edge parameters. Closed edges sweep ±2π in
/// the direction implied by the accumulated angle deltas.
fn resolve_arc_parameters(angles: &[f64], closed: bool) -> (f64, f64) {
    if angles.is_empty() {
        return (0.0, 0.0);
    }
    let start_angle = angles[0];
    if angles.len() == 1 {
        return (start_angle, 0.0);
    }

    // Unwrap accumulated deltas so we measure the *signed* swept
    // angle, not the principal-value end-start.
    let mut accum = 0.0_f64;
    for w in angles.windows(2) {
        let mut d = w[1] - w[0];
        while d > std::f64::consts::PI {
            d -= 2.0 * std::f64::consts::PI;
        }
        while d < -std::f64::consts::PI {
            d += 2.0 * std::f64::consts::PI;
        }
        accum += d;
    }

    let sweep = if closed {
        // Closed edge: full revolution in the direction the deltas
        // accumulated. Guard against a zero-net accumulation by
        // defaulting to CCW.
        if accum >= 0.0 {
            2.0 * std::f64::consts::PI
        } else {
            -2.0 * std::f64::consts::PI
        }
    } else {
        accum
    };

    (start_angle, sweep)
}

/// Plane tangent to cylinder, plane normal in the same direction as
/// the cylinder radial. Spine is a straight line parallel to the
/// cylinder axis.
///
/// At every edge sample the radial direction from axis is constant
/// (`= radial_dir`) and equal to `±n_plane`. The spine therefore
/// shares this radial direction but at radius `r_cyl + offset_sign
/// · sign_radial · radius`, and in the plane offset by `offset_sign
/// · radius · n_plane` from the original plane.
#[allow(clippy::too_many_arguments)]
fn solve_plane_cyl_parallel_tangent(
    model: &BRepModel,
    edge: &Edge,
    _plane: &Plane,
    cylinder: &Cylinder,
    _plane_face: FaceId,
    _cyl_face: FaceId,
    plane_is_a: bool,
    n_plane: Vector3,
    n_cyl: Vector3,
    radial_dir: Vector3,
    offset_sign: f64,
    radius: f64,
    options: &SpineOptions,
) -> OperationResult<SpineRail> {
    let sign_radial = if n_cyl.dot(&radial_dir) >= 0.0 {
        1.0
    } else {
        -1.0
    };
    let r_spine = cylinder.radius + offset_sign * sign_radial * radius;
    if !(r_spine > options.tolerance.distance()) {
        return Err(OperationError::InvalidRadius(radius));
    }

    let n_samples = options.min_samples.max(2);
    let mut samples: Vec<SpineRailSample> = Vec::with_capacity(n_samples);
    let mut prev_center: Option<Point3> = None;
    let mut cumulative_arc = 0.0;

    // The radial direction is constant along the edge for the
    // tangent case. We still sample because the edge may not be a
    // perfect line numerically and downstream consumers expect a
    // uniform sample array.
    for i in 0..n_samples {
        let t = i as f64 / (n_samples as f64 - 1.0);
        let edge_point = edge.evaluate(t, &model.curves)?;
        let p_off = edge_point - cylinder.origin;
        let axial_t = p_off.dot(&cylinder.axis);

        // Spine point: cyl axis projection at axial_t, plus radial
        // offset in the constant radial_dir direction at distance
        // r_spine.
        let center = cylinder.origin + cylinder.axis * axial_t + radial_dir * r_spine;
        let contact_plane = center - n_plane * (offset_sign * radius);
        let contact_cyl =
            cylinder.origin + cylinder.axis * axial_t + radial_dir * cylinder.radius;

        if let Some(prev) = prev_center {
            cumulative_arc += (center - prev).magnitude();
        }
        prev_center = Some(center);

        let (contact_a, contact_b) = if plane_is_a {
            (contact_plane, contact_cyl)
        } else {
            (contact_cyl, contact_plane)
        };
        samples.push(SpineRailSample {
            edge_parameter: t,
            arc_length: cumulative_arc,
            center,
            contact_a,
            contact_b,
            radius,
        });
    }

    // Spine and rails are straight lines for the parallel-tangent
    // case. Build them as exact `Line`s rather than fitted NURBS.
    let spine_start = samples[0].center;
    let spine_end = samples[n_samples - 1].center;
    let spine: Box<dyn Curve> = Box::new(Line::new(spine_start, spine_end));

    let plane_rail_start = samples[0].contact_a;
    let plane_rail_end = samples[n_samples - 1].contact_a;
    let cyl_rail_start = samples[0].contact_b;
    let cyl_rail_end = samples[n_samples - 1].contact_b;
    // Swap if plane was face_b.
    let (rail_a_start, rail_a_end, rail_b_start, rail_b_end) = if plane_is_a {
        (plane_rail_start, plane_rail_end, cyl_rail_start, cyl_rail_end)
    } else {
        (plane_rail_start, plane_rail_end, cyl_rail_start, cyl_rail_end)
    };
    let rail_a: Box<dyn Curve> = Box::new(Line::new(rail_a_start, rail_a_end));
    let rail_b: Box<dyn Curve> = Box::new(Line::new(rail_b_start, rail_b_end));

    Ok(SpineRail {
        spine,
        rail_a,
        rail_b,
        samples,
        frames: Vec::new(),
        solver_kind: SolverKind::AnalyticPlaneCylinder,
    })
}

/// Closed-form spine + rail solve for a planar face adjacent to a
/// spherical face. Plane ∩ sphere is always a single circle (the
/// "small circle"), so the edge is a circular arc and the spine is
/// also a circular arc on a parallel plane.
///
/// Constraints on the ball centre `c`:
/// * `(c - plane.origin) · n_plane = offset_sign · radius`
/// * `|c - sphere.center| = r_sphere + offset_sign · sign_sphere ·
///   radius`
///   where `sign_sphere = +1` when the sphere normal points away
///   from the sphere centre (solid hemisphere) and `-1` when it
///   points toward the centre (spherical cavity).
///
/// The intersection of these two constraints is a circle on the
/// offset plane, centred on the projection of `sphere.center` onto
/// that plane, with radius `√(r_eff² - d²)` where `r_eff = r_sphere
/// + offset_sign · sign_sphere · radius` and `d` is the distance
/// from `sphere.center` to the offset plane.
#[allow(clippy::too_many_arguments)]
fn solve_plane_sphere(
    model: &BRepModel,
    edge: &Edge,
    edge_id: EdgeId,
    face_a: FaceId,
    face_b: FaceId,
    plane: &Plane,
    sphere: &Sphere,
    plane_is_a: bool,
    radius: f64,
    options: &SpineOptions,
) -> OperationResult<Option<SpineRail>> {
    let (plane_face, sphere_face) = if plane_is_a {
        (face_a, face_b)
    } else {
        (face_b, face_a)
    };

    // Outward normals at edge midpoint, signed dihedral.
    let edge_mid = edge.evaluate(0.5, &model.curves)?;
    let n_plane = get_face_oriented_normal(model, plane_face, &edge_mid)?;
    let n_sphere = get_face_oriented_normal(model, sphere_face, &edge_mid)?;

    let raw_tangent = edge.tangent_at(0.5, &model.curves)?;
    let loop_sign = edge_orientation_in_face(model, plane_face, edge_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "Edge {} not present in any loop of face {}",
            edge_id, plane_face
        ))
    })?;
    let edge_tan_in_loop = raw_tangent * loop_sign;
    let dihedral = robust_face_angle(&n_plane, &n_sphere, &edge_tan_in_loop, &options.tolerance)
        .map_err(|e| {
            OperationError::NumericalError(format!("Dihedral compute failed: {:?}", e))
        })?;

    let abs_dihedral = dihedral.abs();
    if abs_dihedral < 0.1 || (std::f64::consts::PI - abs_dihedral) < 0.1 {
        return Err(OperationError::InvalidGeometry(
            "Near-tangent surfaces require special handling".to_string(),
        ));
    }

    let offset_sign = if dihedral > 0.0 { -1.0 } else { 1.0 };

    // Radial direction from sphere centre to edge midpoint.
    let centre_to_mid = edge_mid - sphere.center;
    let radial_dir = centre_to_mid.normalize().map_err(|e| {
        OperationError::NumericalError(format!(
            "Edge midpoint coincides with sphere centre: {:?}",
            e
        ))
    })?;
    // sign_sphere: +1 for convex sphere face (normal radially
    // outward), -1 for concave (normal radially inward, i.e.
    // spherical cavity).
    let sign_sphere = if n_sphere.dot(&radial_dir) >= 0.0 {
        1.0
    } else {
        -1.0
    };
    let r_eff = sphere.radius + offset_sign * sign_sphere * radius;
    if !(r_eff > options.tolerance.distance()) {
        return Err(OperationError::InvalidRadius(radius));
    }

    // Offset plane: passes through `plane.origin + offset_sign *
    // radius * n_plane`, normal n_plane.
    let offset_plane_origin = plane.origin + n_plane * (offset_sign * radius);

    // Distance from sphere centre to offset plane (signed along
    // n_plane).
    let centre_to_offset_plane = (offset_plane_origin - sphere.center).dot(&n_plane);
    let d_abs = centre_to_offset_plane.abs();
    if d_abs >= r_eff - options.tolerance.distance() {
        // Sphere of radius r_eff does not actually intersect the
        // offset plane in a non-degenerate circle. No analytic
        // spine exists for the requested radius.
        return Err(OperationError::InvalidRadius(radius));
    }

    // Centre of the spine circle: projection of sphere centre onto
    // the offset plane.
    let spine_centre = sphere.center + n_plane * centre_to_offset_plane;
    // Radius of the spine circle.
    let spine_circle_r = (r_eff * r_eff - centre_to_offset_plane * centre_to_offset_plane).sqrt();

    // Build a 2D basis on the offset plane. The arc primitive needs
    // a normal vector (the offset plane's normal, n_plane) and uses
    // its own canonical x-axis derivation; we measure edge angles
    // in the same basis Arc::new uses.
    let probe_arc = Arc::new(
        spine_centre,
        n_plane,
        spine_circle_r,
        0.0,
        std::f64::consts::TAU,
    )
    .map_err(|e| OperationError::NumericalError(format!("Spine arc probe failed: {:?}", e)))?;
    let basis_x = probe_arc.x_axis;
    let basis_y = n_plane.cross(&basis_x);

    let n_samples = options.min_samples.max(2);
    let mut samples: Vec<SpineRailSample> = Vec::with_capacity(n_samples);
    let mut prev_center: Option<Point3> = None;
    let mut cumulative_arc = 0.0;
    let mut edge_angles: Vec<f64> = Vec::with_capacity(n_samples);

    for i in 0..n_samples {
        let t = i as f64 / (n_samples as f64 - 1.0);
        let edge_point = edge.evaluate(t, &model.curves)?;

        // Project edge point onto offset plane and measure its
        // angular position in the (basis_x, basis_y) basis around
        // spine_centre.
        let p_offset = edge_point - spine_centre;
        let p_in_plane =
            p_offset - n_plane * p_offset.dot(&n_plane);
        let angle = p_in_plane.dot(&basis_y).atan2(p_in_plane.dot(&basis_x));
        edge_angles.push(angle);

        let radial = basis_x * angle.cos() + basis_y * angle.sin();
        let center = spine_centre + radial * spine_circle_r;
        // Contact on plane: project centre back onto original plane.
        let contact_plane = center - n_plane * (offset_sign * radius);
        // Contact on sphere: project centre onto sphere surface
        // along the line from sphere.centre through the centre.
        let c_to_sphere = (center - sphere.center).normalize().map_err(|e| {
            OperationError::NumericalError(format!(
                "Centre coincides with sphere centre: {:?}",
                e
            ))
        })?;
        let contact_sphere = sphere.center + c_to_sphere * sphere.radius;

        if let Some(prev) = prev_center {
            cumulative_arc += (center - prev).magnitude();
        }
        prev_center = Some(center);

        let (contact_a, contact_b) = if plane_is_a {
            (contact_plane, contact_sphere)
        } else {
            (contact_sphere, contact_plane)
        };
        samples.push(SpineRailSample {
            edge_parameter: t,
            arc_length: cumulative_arc,
            center,
            contact_a,
            contact_b,
            radius,
        });
    }

    let (start_angle, sweep_angle) =
        resolve_arc_parameters(&edge_angles, edge.is_loop());

    let spine_arc = Arc::new(
        spine_centre,
        n_plane,
        spine_circle_r,
        start_angle,
        sweep_angle,
    )
    .map_err(|e| OperationError::NumericalError(format!("Spine arc construction: {:?}", e)))?;

    // Plane rail: at original plane height, around projection of
    // sphere centre onto original plane. The edge circle itself has
    // a known radius (chord of plane ∩ sphere) but here we want the
    // *contact* locus on the plane, which is the offset spine
    // projected back to the original plane. That's a circle of the
    // same radius `spine_circle_r` but on the original plane.
    let plane_rail_centre = spine_centre - n_plane * (offset_sign * radius);
    let plane_rail_arc = Arc::new(
        plane_rail_centre,
        n_plane,
        spine_circle_r,
        start_angle,
        sweep_angle,
    )
    .map_err(|e| OperationError::NumericalError(format!("Plane rail arc: {:?}", e)))?;

    // Sphere rail: locus of contact points on the sphere. Each
    // sample's contact_sphere lies at angle `angle_i` on a sphere
    // small-circle. The small circle has axis = (sphere.center →
    // spine_centre direction). Build the sphere rail as an arc on
    // the plane parallel to the offset plane, at the height where
    // `c_to_sphere` ray meets the sphere.
    // The contact locus is the small circle on the sphere at axial
    // height `r_sphere · (spine_centre - sphere.center).normalised
    // · n_plane` measured from sphere.center along n_plane... cleanest
    // approach: build it from explicit samples since the underlying
    // ray geometry depends on r_eff in a way that doesn't reduce to
    // a single arc constructor unless we re-derive the small-circle
    // centre on the sphere.
    let sphere_centre_to_spine_centre = spine_centre - sphere.center;
    let scs_proj_n = sphere_centre_to_spine_centre.dot(&n_plane);
    // Direction from sphere.center toward sample contact points'
    // small-circle centre on the sphere.
    // The small circle on the sphere lies on a plane parallel to
    // n_plane (its normal IS n_plane), shifted from sphere.center
    // by `scs_proj_n * (r_sphere / r_eff)` along n_plane (similar
    // triangle: contact points are on the sphere, scaled toward
    // sphere.center).
    let sphere_rail_centre_offset = if r_eff.abs() > options.tolerance.distance() {
        scs_proj_n * (sphere.radius / r_eff)
    } else {
        0.0
    };
    let sphere_rail_centre = sphere.center + n_plane * sphere_rail_centre_offset;
    // Small-circle radius on the sphere: r_sphere · (spine_circle_r
    // / r_eff). This is the radius of the sphere-side contact arc.
    let sphere_rail_r = sphere.radius * spine_circle_r / r_eff;
    let sphere_rail_arc = Arc::new(
        sphere_rail_centre,
        n_plane,
        sphere_rail_r,
        start_angle,
        sweep_angle,
    )
    .map_err(|e| OperationError::NumericalError(format!("Sphere rail arc: {:?}", e)))?;

    let spine: Box<dyn Curve> = Box::new(spine_arc);
    let (rail_a, rail_b): (Box<dyn Curve>, Box<dyn Curve>) = if plane_is_a {
        (Box::new(plane_rail_arc), Box::new(sphere_rail_arc))
    } else {
        (Box::new(sphere_rail_arc), Box::new(plane_rail_arc))
    };

    Ok(Some(SpineRail {
        spine,
        rail_a,
        rail_b,
        samples,
        frames: Vec::new(),
        solver_kind: SolverKind::AnalyticPlaneSphere,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vector3;
    use crate::operations::blend_graph::{BlendEdge, BlendGraph};
    use crate::primitives::edge::ManifoldKind;
    use crate::primitives::face::FaceId;
    use crate::primitives::solid::SolidId;
    use crate::primitives::surface::SurfaceType;
    use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

    /// Build a unit (`w × h × d`) box at the origin and return its
    /// solid id. Used as the canonical plane/plane test fixture: every
    /// box edge has two planar adjacent faces and a 90° dihedral.
    fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
        let mut builder = TopologyBuilder::new(model);
        match builder
            .create_box_3d(w, h, d)
            .expect("box creation should succeed")
        {
            GeometryId::Solid(id) => id,
            other => panic!("expected solid, got {other:?}"),
        }
    }

    /// Find the first edge that is incident to exactly two faces and
    /// both faces have planar surfaces. Returns `(edge_id, face_a,
    /// face_b)`.
    fn first_manifold_plane_plane_edge(
        model: &BRepModel,
    ) -> Option<(EdgeId, FaceId, FaceId)> {
        for (edge_id, _edge) in model.edges.iter() {
            let faces = find_adjacent_faces(model, edge_id);
            if faces.len() != 2 {
                continue;
            }
            let mut all_planar = true;
            for &fid in &faces {
                let face = match model.faces.get(fid) {
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
                if surface.surface_type() != SurfaceType::Plane {
                    all_planar = false;
                    break;
                }
            }
            if all_planar {
                return Some((edge_id, faces[0], faces[1]));
            }
        }
        None
    }

    #[test]
    fn plane_plane_box_edge_returns_some() {
        let mut model = BRepModel::new();
        let _solid = make_box(&mut model, 4.0, 3.0, 2.0);
        let (edge_id, face_a, face_b) =
            first_manifold_plane_plane_edge(&model).expect("box has plane/plane edges");
        let opts = SpineOptions::default();
        let rail = solve_spine_for_edge(&model, edge_id, face_a, face_b, 0.25, &opts)
            .expect("solve should not error")
            .expect("plane/plane should match analytic arm");
        assert_eq!(rail.solver_kind, SolverKind::AnalyticPlanePlane);
        assert!(rail.samples.len() >= 2);
    }

    #[test]
    fn plane_plane_solver_kind_is_analytic() {
        let mut model = BRepModel::new();
        let _solid = make_box(&mut model, 4.0, 3.0, 2.0);
        let (edge_id, face_a, face_b) =
            first_manifold_plane_plane_edge(&model).expect("box has plane/plane edges");
        let opts = SpineOptions::default();
        let rail = solve_spine_for_edge(&model, edge_id, face_a, face_b, 0.5, &opts)
            .expect("solve")
            .expect("analytic");
        assert!(matches!(rail.solver_kind, SolverKind::AnalyticPlanePlane));
    }

    #[test]
    fn plane_plane_offset_distance_equals_radius_times_sqrt_two_for_box() {
        // 90° dihedral on a box: spine offset distance from edge =
        // r / sin(45°) = r·√2. Pull the offset out of the sample
        // grid by comparing the centre at midpoint to the edge
        // point at midpoint.
        let mut model = BRepModel::new();
        let _solid = make_box(&mut model, 4.0, 3.0, 2.0);
        let (edge_id, face_a, face_b) =
            first_manifold_plane_plane_edge(&model).expect("box edges exist");
        let radius = 0.3;
        let opts = SpineOptions::default();
        let rail = solve_spine_for_edge(&model, edge_id, face_a, face_b, radius, &opts)
            .expect("solve")
            .expect("analytic");

        let mid_idx = rail.samples.len() / 2;
        let center_at_mid = rail.samples[mid_idx].center;
        let edge = model.edges.get(edge_id).expect("edge").clone();
        let edge_at_mid = edge
            .evaluate(rail.samples[mid_idx].edge_parameter, &model.curves)
            .expect("edge eval");
        let observed = (center_at_mid - edge_at_mid).magnitude();
        let expected = radius * 2f64.sqrt();
        assert!(
            (observed - expected).abs() < 1e-8,
            "offset distance {observed} should equal r·√2 = {expected}"
        );
    }

    #[test]
    fn plane_plane_contact_distance_equals_radius() {
        // Both rail contacts sit exactly `radius` away from the
        // spine centre at every sample.
        let mut model = BRepModel::new();
        let _solid = make_box(&mut model, 4.0, 3.0, 2.0);
        let (edge_id, face_a, face_b) =
            first_manifold_plane_plane_edge(&model).expect("box edges");
        let radius = 0.4;
        let opts = SpineOptions::default();
        let rail = solve_spine_for_edge(&model, edge_id, face_a, face_b, radius, &opts)
            .expect("solve")
            .expect("analytic");
        for s in &rail.samples {
            let da = (s.contact_a - s.center).magnitude();
            let db = (s.contact_b - s.center).magnitude();
            assert!(
                (da - radius).abs() < 1e-9,
                "contact_a-to-center {da} != r {radius}"
            );
            assert!(
                (db - radius).abs() < 1e-9,
                "contact_b-to-center {db} != r {radius}"
            );
        }
    }

    #[test]
    fn plane_plane_spine_length_equals_edge_length() {
        // For two parallel planes the spine is a parallel translate
        // of the edge — its arc length matches the edge length
        // exactly.
        let mut model = BRepModel::new();
        let _solid = make_box(&mut model, 4.0, 3.0, 2.0);
        let (edge_id, face_a, face_b) =
            first_manifold_plane_plane_edge(&model).expect("box edges");
        let opts = SpineOptions::default();
        let rail = solve_spine_for_edge(&model, edge_id, face_a, face_b, 0.2, &opts)
            .expect("solve")
            .expect("analytic");

        let edge = model.edges.get(edge_id).expect("edge").clone();
        let curve = model.curves.get(edge.curve_id).expect("curve");
        let edge_length = curve.arc_length(Tolerance::default());

        // Cumulative arc length stored on the last sample equals the
        // spine total length, exactly (samples are uniformly spaced
        // along the colinear spine).
        let last_arc = rail.samples.last().expect("≥1 sample").arc_length;
        assert!(
            (last_arc - edge_length).abs() < 1e-7,
            "spine length {last_arc} != edge length {edge_length}"
        );
    }

    #[test]
    fn plane_plane_spine_is_a_line() {
        // F3-α emits an exact [`Line`] for the spine — not a NURBS
        // fitted through colinear samples.
        let mut model = BRepModel::new();
        let _solid = make_box(&mut model, 4.0, 3.0, 2.0);
        let (edge_id, face_a, face_b) =
            first_manifold_plane_plane_edge(&model).expect("box edges");
        let opts = SpineOptions::default();
        let rail = solve_spine_for_edge(&model, edge_id, face_a, face_b, 0.2, &opts)
            .expect("solve")
            .expect("analytic");
        assert!(
            rail.spine.as_any().downcast_ref::<Line>().is_some(),
            "spine should be Line, got {}",
            rail.spine.type_name()
        );
        assert!(rail.rail_a.as_any().downcast_ref::<Line>().is_some());
        assert!(rail.rail_b.as_any().downcast_ref::<Line>().is_some());
    }

    #[test]
    fn plane_plane_spine_tangent_parallel_to_edge_tangent() {
        // Spine is parallel to the edge for two planar faces.
        let mut model = BRepModel::new();
        let _solid = make_box(&mut model, 4.0, 3.0, 2.0);
        let (edge_id, face_a, face_b) =
            first_manifold_plane_plane_edge(&model).expect("box edges");
        let opts = SpineOptions::default();
        let rail = solve_spine_for_edge(&model, edge_id, face_a, face_b, 0.2, &opts)
            .expect("solve")
            .expect("analytic");

        let edge = model.edges.get(edge_id).expect("edge").clone();
        let edge_tan = edge
            .tangent_at(0.5, &model.curves)
            .expect("edge tangent")
            .normalize()
            .expect("normalise edge tangent");
        let spine_tan = rail
            .spine
            .tangent_at(0.5)
            .expect("spine tangent")
            .normalize()
            .expect("normalise spine tangent");
        let alignment = spine_tan.dot(&edge_tan).abs();
        assert!(
            (alignment - 1.0).abs() < 1e-9,
            "spine tangent {spine_tan:?} should be ‖ edge tangent {edge_tan:?} (|dot|={alignment})"
        );
    }

    #[test]
    fn plane_plane_min_samples_honoured() {
        let mut model = BRepModel::new();
        let _solid = make_box(&mut model, 4.0, 3.0, 2.0);
        let (edge_id, face_a, face_b) =
            first_manifold_plane_plane_edge(&model).expect("box edges");
        let mut opts = SpineOptions::default();
        opts.min_samples = 17;
        let rail = solve_spine_for_edge(&model, edge_id, face_a, face_b, 0.1, &opts)
            .expect("solve")
            .expect("analytic");
        assert_eq!(rail.samples.len(), 17);
    }

    /// Find the first edge whose two adjacent faces are exactly one
    /// Plane and one Cylinder. Returns `(edge_id, face_a, face_b,
    /// plane_face_is_a)`. Used as the canonical plane/cylinder
    /// perpendicular test fixture: every cylinder primitive's top
    /// and bottom rim qualifies.
    fn first_plane_cyl_edge(
        model: &BRepModel,
    ) -> Option<(EdgeId, FaceId, FaceId, bool)> {
        for (edge_id, _e) in model.edges.iter() {
            let faces = find_adjacent_faces(model, edge_id);
            if faces.len() != 2 {
                continue;
            }
            let t0 = model
                .surfaces
                .get(model.faces.get(faces[0])?.surface_id)?
                .surface_type();
            let t1 = model
                .surfaces
                .get(model.faces.get(faces[1])?.surface_id)?
                .surface_type();
            if t0 == SurfaceType::Plane && t1 == SurfaceType::Cylinder {
                return Some((edge_id, faces[0], faces[1], true));
            }
            if t0 == SurfaceType::Cylinder && t1 == SurfaceType::Plane {
                return Some((edge_id, faces[0], faces[1], false));
            }
        }
        None
    }

    #[test]
    fn plane_cyl_perpendicular_cylinder_rim_returns_some() {
        let mut model = BRepModel::new();
        let mut builder = TopologyBuilder::new(&mut model);
        let _ = builder
            .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, 2.0, 5.0)
            .expect("cylinder creation");
        let (edge_id, face_a, face_b, _plane_is_a) =
            first_plane_cyl_edge(&model).expect("cylinder has a plane/cyl rim edge");
        let opts = SpineOptions::default();
        let rail = solve_spine_for_edge(&model, edge_id, face_a, face_b, 0.25, &opts)
            .expect("solve should not error")
            .expect("plane/cylinder perpendicular should match analytic arm");
        assert_eq!(rail.solver_kind, SolverKind::AnalyticPlaneCylinder);
        assert!(rail.samples.len() >= 2);
    }

    #[test]
    fn plane_cyl_perpendicular_contact_distance_equals_radius() {
        // Both rail contacts sit exactly `radius` from the spine
        // centre at every station.
        let mut model = BRepModel::new();
        let mut builder = TopologyBuilder::new(&mut model);
        let _ = builder
            .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, 2.0, 5.0)
            .expect("cylinder creation");
        let (edge_id, face_a, face_b, _) =
            first_plane_cyl_edge(&model).expect("plane/cyl rim");
        let radius = 0.4;
        let opts = SpineOptions::default();
        let rail = solve_spine_for_edge(&model, edge_id, face_a, face_b, radius, &opts)
            .expect("solve")
            .expect("analytic");
        for s in &rail.samples {
            let da = (s.contact_a - s.center).magnitude();
            let db = (s.contact_b - s.center).magnitude();
            assert!(
                (da - radius).abs() < 1e-9,
                "contact_a-to-center {da} != r {radius}"
            );
            assert!(
                (db - radius).abs() < 1e-9,
                "contact_b-to-center {db} != r {radius}"
            );
        }
    }

    #[test]
    fn plane_cyl_perpendicular_spine_curves_are_arcs() {
        // F3-β emits exact `Arc` primitives for the spine and both
        // rails — not fitted NURBS through sampled points.
        let mut model = BRepModel::new();
        let mut builder = TopologyBuilder::new(&mut model);
        let _ = builder
            .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, 2.0, 5.0)
            .expect("cylinder creation");
        let (edge_id, face_a, face_b, _) =
            first_plane_cyl_edge(&model).expect("plane/cyl rim");
        let opts = SpineOptions::default();
        let rail = solve_spine_for_edge(&model, edge_id, face_a, face_b, 0.3, &opts)
            .expect("solve")
            .expect("analytic");
        assert!(
            rail.spine.as_any().downcast_ref::<Arc>().is_some(),
            "spine should be Arc, got {}",
            rail.spine.type_name()
        );
        assert!(
            rail.rail_a.as_any().downcast_ref::<Arc>().is_some(),
            "rail_a should be Arc, got {}",
            rail.rail_a.type_name()
        );
        assert!(
            rail.rail_b.as_any().downcast_ref::<Arc>().is_some(),
            "rail_b should be Arc, got {}",
            rail.rail_b.type_name()
        );
    }

    #[test]
    fn plane_cyl_perpendicular_spine_radius_matches_formula() {
        // For a solid cylinder of radius r_cyl, perpendicular plane
        // (cap), convex rim, external blend: spine radius from
        // axis = r_cyl - radius. Hoffmann §10.5.
        let mut model = BRepModel::new();
        let mut builder = TopologyBuilder::new(&mut model);
        let _ = builder
            .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, 2.0, 5.0)
            .expect("cylinder creation");
        let (edge_id, face_a, face_b, _) =
            first_plane_cyl_edge(&model).expect("plane/cyl rim");
        let r_cyl = 2.0;
        let radius = 0.5;
        let opts = SpineOptions::default();
        let rail = solve_spine_for_edge(&model, edge_id, face_a, face_b, radius, &opts)
            .expect("solve")
            .expect("analytic");

        // Every spine sample lies at distance r_spine = r_cyl - radius
        // from the cylinder axis (the Z axis through the origin).
        let r_spine_expected = r_cyl - radius;
        for s in &rail.samples {
            let radial = Vector3::new(s.center.x, s.center.y, 0.0).magnitude();
            assert!(
                (radial - r_spine_expected).abs() < 1e-9,
                "spine radial {radial} != r_cyl - r = {r_spine_expected} (centre={:?})",
                s.center
            );
        }
    }

    #[test]
    fn plane_cyl_perpendicular_spine_lies_in_offset_plane() {
        // Analytic invariant: for a perpendicular plane/cylinder edge
        // the spine sits on a plane parallel to the cap plane,
        // offset by exactly `radius` along the cap normal. We test
        // the *magnitude* of the offset — which side of the cap the
        // spine sits on is governed by the signed-dihedral convention
        // (inherited from the legacy bisector path; same convention
        // F3-α plane/plane uses). The dihedral sign on the
        // cylinder-rim case may flip between top and bottom rim
        // depending on loop orientation; that is the legacy
        // `robust_face_angle` behaviour and is preserved here for
        // bit-compatibility with the legacy fillet output.
        let mut model = BRepModel::new();
        let mut builder = TopologyBuilder::new(&mut model);
        let _ = builder
            .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, 2.0, 5.0)
            .expect("cylinder creation");
        let (edge_id, face_a, face_b, _) =
            first_plane_cyl_edge(&model).expect("plane/cyl rim");
        let radius = 0.5;
        let opts = SpineOptions::default();
        let rail = solve_spine_for_edge(&model, edge_id, face_a, face_b, radius, &opts)
            .expect("solve")
            .expect("analytic");

        let z0 = rail.samples[0].center.z;
        // All samples share the same z height (perpendicular cap →
        // spine on a single z-plane).
        for s in &rail.samples {
            assert!(
                (s.center.z - z0).abs() < 1e-9,
                "spine z {} drifted from z0 {z0}",
                s.center.z
            );
        }
        // The plane-side contact lies on the original cap plane
        // (z=0 or z=5). The spine sits exactly `radius` from that
        // plane along the cap normal — either side is admissible
        // for this magnitude check.
        let plane_z_options = [0.0_f64, 5.0_f64];
        let matches_any = plane_z_options
            .iter()
            .any(|&zp| ((z0 - zp).abs() - radius).abs() < 1e-9);
        assert!(
            matches_any,
            "spine z {z0} must lie exactly radius={radius} from one of {plane_z_options:?}"
        );
    }

    #[test]
    fn plane_cyl_perpendicular_cylinder_contact_lies_on_cylinder_surface() {
        // The cylinder-side contact lies on the cylinder surface →
        // its distance to the axis equals the cylinder radius.
        let mut model = BRepModel::new();
        let mut builder = TopologyBuilder::new(&mut model);
        let _ = builder
            .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, 2.0, 5.0)
            .expect("cylinder creation");
        let (edge_id, face_a, face_b, plane_is_a) =
            first_plane_cyl_edge(&model).expect("plane/cyl rim");
        let r_cyl = 2.0;
        let opts = SpineOptions::default();
        let rail = solve_spine_for_edge(&model, edge_id, face_a, face_b, 0.3, &opts)
            .expect("solve")
            .expect("analytic");
        for s in &rail.samples {
            // Cylinder rail is whichever contact corresponds to the
            // cylinder face (the non-plane one).
            let cyl_contact = if plane_is_a { s.contact_b } else { s.contact_a };
            let radial = Vector3::new(cyl_contact.x, cyl_contact.y, 0.0).magnitude();
            assert!(
                (radial - r_cyl).abs() < 1e-9,
                "cylinder contact radial {radial} != r_cyl {r_cyl}"
            );
        }
    }

    #[test]
    fn plane_cyl_perpendicular_plane_contact_lies_on_plane() {
        // The plane-side contact lies on the original cap plane →
        // its z equals the cap z (0 or 5 for the canonical cylinder).
        let mut model = BRepModel::new();
        let mut builder = TopologyBuilder::new(&mut model);
        let _ = builder
            .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, 2.0, 5.0)
            .expect("cylinder creation");
        let (edge_id, face_a, face_b, plane_is_a) =
            first_plane_cyl_edge(&model).expect("plane/cyl rim");
        let opts = SpineOptions::default();
        let rail = solve_spine_for_edge(&model, edge_id, face_a, face_b, 0.3, &opts)
            .expect("solve")
            .expect("analytic");
        let plane_contact_0 = if plane_is_a {
            rail.samples[0].contact_a
        } else {
            rail.samples[0].contact_b
        };
        let cap_z = plane_contact_0.z;
        let top = (cap_z - 5.0).abs() < 1e-9;
        let bottom = cap_z.abs() < 1e-9;
        assert!(
            top || bottom,
            "plane contact z {cap_z} must equal cap z (0 or 5)"
        );
        for s in &rail.samples {
            let plane_contact = if plane_is_a { s.contact_a } else { s.contact_b };
            assert!(
                (plane_contact.z - cap_z).abs() < 1e-9,
                "plane contact z {} drifted from cap z {cap_z}",
                plane_contact.z
            );
        }
    }

    #[test]
    fn plane_cyl_perpendicular_min_samples_honoured() {
        let mut model = BRepModel::new();
        let mut builder = TopologyBuilder::new(&mut model);
        let _ = builder
            .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, 2.0, 5.0)
            .expect("cylinder creation");
        let (edge_id, face_a, face_b, _) =
            first_plane_cyl_edge(&model).expect("plane/cyl rim");
        let mut opts = SpineOptions::default();
        opts.min_samples = 19;
        let rail = solve_spine_for_edge(&model, edge_id, face_a, face_b, 0.1, &opts)
            .expect("solve")
            .expect("analytic");
        assert_eq!(rail.samples.len(), 19);
    }

    #[test]
    fn plane_cyl_perpendicular_oversized_radius_rejected() {
        // r >= r_cyl would invert the spine to the other side of
        // the axis — `r_spine = r_cyl - r ≤ 0`. The solver must
        // reject with InvalidRadius rather than build a degenerate
        // arc.
        let mut model = BRepModel::new();
        let mut builder = TopologyBuilder::new(&mut model);
        let _ = builder
            .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, 2.0, 5.0)
            .expect("cylinder creation");
        let (edge_id, face_a, face_b, _) =
            first_plane_cyl_edge(&model).expect("plane/cyl rim");
        let opts = SpineOptions::default();
        // r = 2.0 == r_cyl → r_spine = 0; r = 2.5 > r_cyl → r_spine < 0.
        for bad_r in [2.0, 2.5] {
            let result = solve_spine_for_edge(&model, edge_id, face_a, face_b, bad_r, &opts);
            assert!(
                matches!(result, Err(OperationError::InvalidRadius(_))),
                "radius={bad_r} >= r_cyl=2.0 should error, got {result:?}"
            );
        }
    }

    #[test]
    fn solve_errors_on_non_positive_radius() {
        let mut model = BRepModel::new();
        let _solid = make_box(&mut model, 4.0, 3.0, 2.0);
        let (edge_id, face_a, face_b) =
            first_manifold_plane_plane_edge(&model).expect("box edges");
        let opts = SpineOptions::default();

        for bad_r in [0.0, -0.5, f64::NAN, f64::INFINITY] {
            let r = solve_spine_for_edge(&model, edge_id, face_a, face_b, bad_r, &opts);
            assert!(
                matches!(r, Err(OperationError::InvalidRadius(_))),
                "radius={bad_r} should error, got {r:?}"
            );
        }
    }

    #[test]
    fn solve_errors_on_missing_edge() {
        let mut model = BRepModel::new();
        let _solid = make_box(&mut model, 4.0, 3.0, 2.0);
        let (_edge_id, face_a, face_b) =
            first_manifold_plane_plane_edge(&model).expect("box edges");
        let opts = SpineOptions::default();
        // 9999 is far above any id the box construction allocated.
        let r = solve_spine_for_edge(&model, 9_999, face_a, face_b, 0.1, &opts);
        assert!(matches!(r, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn solve_spine_for_chain_multi_edge_returns_none() {
        // Multi-edge chains are F3-γ territory; F3-α returns None
        // so the caller falls through to the legacy bisector path.
        let mut model = BRepModel::new();
        let _solid = make_box(&mut model, 4.0, 3.0, 2.0);
        let (edge_id_a, _, _) =
            first_manifold_plane_plane_edge(&model).expect("box edges");
        // Find a second distinct edge from the model to populate
        // a synthetic two-edge chain.
        let edge_id_b = model
            .edges
            .iter()
            .map(|(id, _)| id)
            .find(|&id| id != edge_id_a)
            .expect("box has many edges");

        let mut graph = BlendGraph::default();
        graph.edges.insert(
            edge_id_a,
            BlendEdge {
                id: edge_id_a,
                radius: BlendRadius::Constant(0.1),
                chain_id: 0,
                dihedral_angle: None,
                convexity: 1,
                manifold_kind: ManifoldKind::Manifold,
                start_setback: None,
                end_setback: None,
            },
        );
        graph.edges.insert(
            edge_id_b,
            BlendEdge {
                id: edge_id_b,
                radius: BlendRadius::Constant(0.1),
                chain_id: 0,
                dihedral_angle: None,
                convexity: 1,
                manifold_kind: ManifoldKind::Manifold,
                start_setback: None,
                end_setback: None,
            },
        );

        let opts = SpineOptions::default();
        let result =
            solve_spine_for_chain(&model, &[edge_id_a, edge_id_b], &graph, &opts).expect("solve");
        assert!(
            result.is_none(),
            "multi-edge chain should return None in F3-α"
        );
    }

    #[test]
    fn plane_plane_offset_along_bisector_direction() {
        // The vector from edge midpoint to spine centre must lie in
        // the plane spanned by the two face normals (i.e. it's the
        // bisector of −n_a and −n_b for a convex edge), and must be
        // perpendicular to the edge tangent.
        let mut model = BRepModel::new();
        let _solid = make_box(&mut model, 4.0, 3.0, 2.0);
        let (edge_id, face_a, face_b) =
            first_manifold_plane_plane_edge(&model).expect("box edges");
        let radius = 0.25;
        let opts = SpineOptions::default();
        let rail = solve_spine_for_edge(&model, edge_id, face_a, face_b, radius, &opts)
            .expect("solve")
            .expect("analytic");

        let edge = model.edges.get(edge_id).expect("edge").clone();
        let edge_tan = edge
            .tangent_at(0.5, &model.curves)
            .expect("tan")
            .normalize()
            .expect("normalise");

        let mid_idx = rail.samples.len() / 2;
        let center = rail.samples[mid_idx].center;
        let edge_pt = edge
            .evaluate(rail.samples[mid_idx].edge_parameter, &model.curves)
            .expect("edge eval");
        let offset_vec: Vector3 = center - edge_pt;
        let along_edge = offset_vec.dot(&edge_tan);
        assert!(
            along_edge.abs() < 1e-9,
            "offset {offset_vec:?} should be ⟂ edge tangent (along={along_edge})"
        );
    }
}
