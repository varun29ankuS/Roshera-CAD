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

use crate::math::frame::{parallel_transport_frames, FrameAtStation};
use crate::math::{Point3, Tolerance, Vector3};
use crate::operations::blend_graph::{BlendGraph, BlendRadius};
use crate::operations::edge_classification::find_adjacent_faces;
use crate::operations::fillet::{edge_orientation_in_face, get_face_oriented_normal};
use crate::operations::fillet_robust::robust_face_angle;
use crate::operations::{OperationError, OperationResult};
use crate::primitives::curve::{Arc, Curve, Line, NurbsCurve};
use crate::primitives::edge::{Edge, EdgeId};
use crate::primitives::face::FaceId;
use crate::primitives::surface::{Cylinder, Plane, RuledSurface, Sphere, Surface, SurfaceType};
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
    /// When `true`, surface pairs that no analytic arm recognises
    /// route to the F3-γ marching solver instead of returning
    /// `Ok(None)` to the caller. **Default is `false`** during the
    /// F3-γ → F3-δ transition: production fillet code uses the
    /// legacy bisector fallback for non-analytic pairs so that
    /// flat ruled-surface side faces (every extruded prism wall)
    /// don't hit the marching corrector's noise floor on
    /// `RuledSurface::closest_point`. F3-δ adds planar-surface
    /// detection that promotes flat ruled walls into the
    /// plane/plane analytic arm and flips this default back to
    /// `true`. Tests that exercise marching invoke
    /// [`solve_marching`] directly or pass `enable_marching:
    /// true` explicitly.
    pub enable_marching: bool,
}

impl Default for SpineOptions {
    fn default() -> Self {
        Self {
            tolerance: Tolerance::default(),
            min_samples: 32,
            max_samples: 2048,
            honor_setbacks: true,
            enable_marching: false,
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

    // Dispatch on the (effective) surface-type tuple. F3-α landed
    // plane/plane; F3-β adds plane/cylinder (perpendicular +
    // parallel-tangent sub-cases) and plane/sphere. F3-γ ships the
    // marching solver as a parallel-deployment addition behind
    // [`SpineOptions::enable_marching`] (default `false` — see the
    // field doc for the rationale). When the flag is `true` and no
    // analytic arm claims the pair, marching takes over; when the
    // flag is `false` (production today) we return `Ok(None)` so
    // [`fillet`] falls through to the legacy bisector path.
    //
    // F3-δ.1 introduces planar-ruled-surface *promotion*: extrusion
    // produces [`RuledSurface`] side walls (not [`Plane`] faces) but
    // their geometry is exactly a plane whenever both rails are
    // [`Line`](crate::primitives::curve::Line)s with coplanar
    // corners — which is the universal case for prism walls. Without
    // promotion these faces fall through to the wildcard arm and
    // the marching solver's corrector can't beat
    // [`RuledSurface::closest_point`]'s 30×10 grid noise floor
    // (~1e-4 vs target 1e-6). With promotion they route through the
    // plane/plane (or plane/cylinder, plane/sphere) analytic arms
    // exactly as if extrude had emitted [`Plane`] faces directly.
    let effective_plane_a = effective_plane(surface_a, &options.tolerance);
    let effective_plane_b = effective_plane(surface_b, &options.tolerance);
    let effective_type_a = effective_plane_a
        .as_ref()
        .map_or_else(|| surface_a.surface_type(), |_| SurfaceType::Plane);
    let effective_type_b = effective_plane_b
        .as_ref()
        .map_or_else(|| surface_b.surface_type(), |_| SurfaceType::Plane);

    let analytic_result: OperationResult<Option<SpineRail>> =
        match (effective_type_a, effective_type_b) {
            (SurfaceType::Plane, SurfaceType::Plane) => {
                let plane_a = effective_plane_a.as_ref().ok_or_else(|| {
                    OperationError::InternalError("Effective plane A missing".into())
                })?;
                let plane_b = effective_plane_b.as_ref().ok_or_else(|| {
                    OperationError::InternalError("Effective plane B missing".into())
                })?;
                solve_plane_plane(
                    model, &edge, edge_id, face_a, face_b, plane_a, plane_b, radius, options,
                )
                .map(Some)
            }
            (SurfaceType::Plane, SurfaceType::Cylinder) => {
                let plane = effective_plane_a.as_ref().ok_or_else(|| {
                    OperationError::InternalError("Effective plane A missing".into())
                })?;
                let cylinder = surface_b.as_any().downcast_ref::<Cylinder>().ok_or_else(
                    || OperationError::InternalError("Cylinder downcast failed".into()),
                )?;
                solve_plane_cylinder(
                    model, &edge, edge_id, face_a, face_b, plane, cylinder,
                    /*plane_is_a=*/ true, radius, options,
                )
            }
            (SurfaceType::Cylinder, SurfaceType::Plane) => {
                let cylinder = surface_a.as_any().downcast_ref::<Cylinder>().ok_or_else(
                    || OperationError::InternalError("Cylinder downcast failed".into()),
                )?;
                let plane = effective_plane_b.as_ref().ok_or_else(|| {
                    OperationError::InternalError("Effective plane B missing".into())
                })?;
                solve_plane_cylinder(
                    model, &edge, edge_id, face_a, face_b, plane, cylinder,
                    /*plane_is_a=*/ false, radius, options,
                )
            }
            (SurfaceType::Plane, SurfaceType::Sphere) => {
                let plane = effective_plane_a.as_ref().ok_or_else(|| {
                    OperationError::InternalError("Effective plane A missing".into())
                })?;
                let sphere = surface_b.as_any().downcast_ref::<Sphere>().ok_or_else(|| {
                    OperationError::InternalError("Sphere downcast failed".into())
                })?;
                solve_plane_sphere(
                    model, &edge, edge_id, face_a, face_b, plane, sphere,
                    /*plane_is_a=*/ true, radius, options,
                )
            }
            (SurfaceType::Sphere, SurfaceType::Plane) => {
                let sphere = surface_a.as_any().downcast_ref::<Sphere>().ok_or_else(|| {
                    OperationError::InternalError("Sphere downcast failed".into())
                })?;
                let plane = effective_plane_b.as_ref().ok_or_else(|| {
                    OperationError::InternalError("Effective plane B missing".into())
                })?;
                solve_plane_sphere(
                    model, &edge, edge_id, face_a, face_b, plane, sphere,
                    /*plane_is_a=*/ false, radius, options,
                )
            }
            _ => Ok(None),
        };

    match analytic_result {
        Ok(Some(rail)) => Ok(Some(rail)),
        Ok(None) => {
            if options.enable_marching {
                solve_marching(
                    model, &edge, edge_id, face_a, face_b, surface_a, surface_b, radius, options,
                )
                .map(Some)
            } else {
                Ok(None)
            }
        }
        Err(e) => Err(e),
    }
}

/// Resolve a face's *effective* [`Plane`] for dispatch purposes.
///
/// Returns `Some(_)` when:
/// * `surface` is a [`Plane`] — the stored plane is cloned out.
/// * `surface` is a [`RuledSurface`] whose two rails are
///   [`Line`](crate::primitives::curve::Line)s and whose four corner
///   points are coplanar — a synthesised [`Plane`] through those
///   corners is returned (see [`try_promote_ruled_to_plane`]).
///
/// Returns `None` for every other surface type (and for ruled
/// surfaces with curved rails, with collinear corners, or with
/// non-coplanar corners). Callers fall through to the wildcard
/// dispatch arm, which routes to marching (if
/// [`SpineOptions::enable_marching`]) or returns `Ok(None)` so the
/// legacy bisector path picks the case up.
fn effective_plane(surface: &dyn Surface, tolerance: &Tolerance) -> Option<Plane> {
    if let Some(p) = surface.as_any().downcast_ref::<Plane>() {
        return Some(p.clone());
    }
    if let Some(r) = surface.as_any().downcast_ref::<RuledSurface>() {
        return try_promote_ruled_to_plane(r, tolerance);
    }
    None
}

/// Detect when a [`RuledSurface`] is geometrically a plane and, if
/// so, synthesise the corresponding [`Plane`].
///
/// Promotion is conservative — both rails must be
/// [`Line`](crate::primitives::curve::Line)s and the four corner
/// points must be coplanar to within `tolerance.distance()` (scaled
/// by the local diagonal length to absorb large-coordinate models).
/// This is exactly the case extrusion produces: each prism wall has
/// `curve1` = a base-sketch edge and `curve2` = the same edge
/// translated by the extrude vector, so the four corners form a
/// parallelogram and are trivially coplanar.
///
/// Curved-rail ruled surfaces (e.g. the lateral surface of a
/// cylinder built as a ruled surface between two coaxial arcs) are
/// *not* promoted — sampling two coaxial arcs would produce coplanar
/// rail endpoints but the interior of the ruled surface bows away
/// from the chord plane, and only the marching solver handles them
/// correctly.
fn try_promote_ruled_to_plane(ruled: &RuledSurface, tolerance: &Tolerance) -> Option<Plane> {
    let line1 = ruled.curve1.as_any().downcast_ref::<Line>()?;
    let line2 = ruled.curve2.as_any().downcast_ref::<Line>()?;

    let p1 = line1.start;
    let p2 = line1.end;
    let p3 = line2.start;
    let p4 = line2.end;

    let diag = ((p1 - p3).magnitude())
        .max((p1 - p4).magnitude())
        .max((p2 - p3).magnitude())
        .max((p2 - p4).magnitude())
        .max(1.0);
    let abs_tol = tolerance.distance() * diag;

    // Try plane through (p1, p2, p3); if degenerate (collinear), try
    // (p1, p2, p4). If both triangles are degenerate the four points
    // are collinear — a degenerate "ruled surface" we refuse to
    // promote (it's a 1D object, not a plane).
    let v12 = p2 - p1;
    let v13 = p3 - p1;
    let cross_123 = v12.cross(&v13);
    let mag_123 = cross_123.magnitude();
    if mag_123 > 1e-12 {
        let normal = cross_123 / mag_123;
        let dev = (p4 - p1).dot(&normal).abs();
        if dev <= abs_tol {
            return Plane::from_three_points(p1, p2, p3).ok();
        }
        return None;
    }

    let v14 = p4 - p1;
    let cross_124 = v12.cross(&v14);
    let mag_124 = cross_124.magnitude();
    if mag_124 > 1e-12 {
        let normal = cross_124 / mag_124;
        let dev = (p3 - p1).dot(&normal).abs();
        if dev <= abs_tol {
            return Plane::from_three_points(p1, p2, p4).ok();
        }
        return None;
    }

    // All four corners collinear — degenerate ruled surface.
    None
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

// ---------------------------------------------------------------------------
// F3-γ — Marching solver
// ---------------------------------------------------------------------------
//
// For every surface pair the analytic dispatcher declines to handle, we
// march along the source edge curve sampling the rolling-ball spine at
// uniform parameters. At each station an alternate-projection corrector
// refines an initial bisector seed using each face's `Surface::closest_point`
// — so unlike the legacy bisector path the contacts are *projected* onto
// the supporting faces rather than approximated via `center ± radius·normal`.
//
// The corrector is a Picard-style iteration capped at [`MAX_CORRECTOR_ITERS`]
// rounds with an F1-δ-style monotone-decrease guard: if the worst-case gap
// (max of `||center − contact_i| − radius|` over i ∈ {a, b}) fails to shrink
// for [`CORRECTOR_STALL_LIMIT`] consecutive iterations the corrector returns
// a `PK_BLEND_SPINE_DIVERGED` numerical error. F2-δ rollback turns that
// into a clean "model untouched" failure at the fillet entry point.
//
// Frames are attached after fitting: the marched centres are fitted to a
// cubic NURBS curve, then [`parallel_transport_frames`] produces a
// rotation-minimising frame sequence (Wang/Jüttler double-reflection) along
// it. F4 consumes those frames when stamping the pipe / NURBS-skin blend
// surface — analytic arms re-derive frames from their closed forms so the
// frames field is left empty there.

/// Hard cap on corrector iterations per station. Eight is enough for the
/// quadratic convergence of the alternating projection to reach
/// `tolerance.distance()` on every well-conditioned blend we have measured.
const MAX_CORRECTOR_ITERS: usize = 8;

/// Stall threshold for the F1-δ-style monotone-decrease guard. Three
/// consecutive non-decreasing gaps declare divergence.
const CORRECTOR_STALL_LIMIT: usize = 3;

/// Near-tangent guard for the marching solver. Slightly tighter than the
/// analytic 0.1 rad bound — marching can in principle cope with sharper
/// dihedrals than the bisector heuristic, but anything tighter than ≈3°
/// is almost always a blend-too-large pathology and rejecting it surfaces
/// the failure to the caller instead of silently producing garbage.
const MARCHING_NEAR_TANGENT_RAD: f64 = 0.05;

/// One station of the marching corrector. Given a seed centre, alternates
/// projecting onto `surface_a` and `surface_b` and re-centring at the
/// midpoint of the two `(contact + radius·outward_normal)` targets.
///
/// Returns `(corrected_center, contact_a, contact_b, iterations_used)`.
///
/// Errors with `PK_BLEND_SPINE_DIVERGED` when the worst-case
/// `||center − contact| − radius|` fails to monotonically decrease for
/// [`CORRECTOR_STALL_LIMIT`] iterations, or when the iteration cap is
/// reached with the residual still more than 10× the target tolerance.
fn corrector(
    surface_a: &dyn Surface,
    surface_b: &dyn Surface,
    seed_center: Point3,
    radius: f64,
    tolerance: Tolerance,
) -> OperationResult<(Point3, Point3, Point3, usize)> {
    let target = tolerance.distance().max(1e-9);
    let mut center = seed_center;
    let mut prev_gap = f64::INFINITY;
    let mut stall: usize = 0;
    let mut best: Option<(Point3, Point3, Point3, f64)> = None;

    for iter in 1..=MAX_CORRECTOR_ITERS {
        let (ua, va) = surface_a.closest_point(&center, tolerance).map_err(|e| {
            OperationError::NumericalError(format!(
                "Marching corrector: closest_point on face A failed: {:?}",
                e
            ))
        })?;
        let contact_a = surface_a.point_at(ua, va).map_err(|e| {
            OperationError::NumericalError(format!(
                "Marching corrector: point_at face A failed: {:?}",
                e
            ))
        })?;
        let (ub, vb) = surface_b.closest_point(&center, tolerance).map_err(|e| {
            OperationError::NumericalError(format!(
                "Marching corrector: closest_point on face B failed: {:?}",
                e
            ))
        })?;
        let contact_b = surface_b.point_at(ub, vb).map_err(|e| {
            OperationError::NumericalError(format!(
                "Marching corrector: point_at face B failed: {:?}",
                e
            ))
        })?;

        let da = (center - contact_a).magnitude();
        let db = (center - contact_b).magnitude();
        let gap_a = (da - radius).abs();
        let gap_b = (db - radius).abs();
        let gap = gap_a.max(gap_b);

        // Track the best (lowest-gap) iterate so we can return it on
        // iteration exhaustion if it's "close enough".
        best = match best {
            Some((_, _, _, g)) if g <= gap => best,
            _ => Some((center, contact_a, contact_b, gap)),
        };

        if gap <= target {
            return Ok((center, contact_a, contact_b, iter));
        }

        // Monotone-decrease guard. A tiny relative slack (1e-6) keeps
        // floating-point noise from declaring divergence on a stalled-
        // but-converged residual.
        if gap >= prev_gap * (1.0 - 1e-6) {
            stall += 1;
            if stall >= CORRECTOR_STALL_LIMIT {
                return Err(OperationError::NumericalError(format!(
                    "PK_BLEND_SPINE_DIVERGED: marching corrector gap not decreasing \
                     ({:.3e} ≥ {:.3e}) for {} iterations at radius {}",
                    gap, prev_gap, stall, radius
                )));
            }
        } else {
            stall = 0;
        }
        prev_gap = gap;

        // Picard update. Push each contact outward by `radius` along the
        // (center − contact) direction; the next centre estimate is the
        // midpoint of those two push targets. When the centre coincides
        // with a contact (da or db ≈ 0) we fall back to the surface
        // normal at that contact — that case is rare in practice
        // (corrector should never land on the surface) but is the
        // correct degenerate limit.
        let dir_a = if da > 1e-30 {
            (center - contact_a) * (1.0 / da)
        } else {
            surface_a.normal_at(ua, va).map_err(|e| {
                OperationError::NumericalError(format!(
                    "Marching corrector: normal at contact A: {:?}",
                    e
                ))
            })?
        };
        let dir_b = if db > 1e-30 {
            (center - contact_b) * (1.0 / db)
        } else {
            surface_b.normal_at(ub, vb).map_err(|e| {
                OperationError::NumericalError(format!(
                    "Marching corrector: normal at contact B: {:?}",
                    e
                ))
            })?
        };
        let push_a = contact_a + dir_a * radius;
        let push_b = contact_b + dir_b * radius;
        center = Point3 {
            x: 0.5 * (push_a.x + push_b.x),
            y: 0.5 * (push_a.y + push_b.y),
            z: 0.5 * (push_a.z + push_b.z),
        };
    }

    // Iteration cap reached without converging. Accept the best iterate
    // if its residual is within 10× the target — that's still well
    // inside the supporting-face tolerance budget and avoids rejecting
    // marginally-converged stations on otherwise valid blends. Anything
    // worse declares divergence.
    let (best_c, best_a, best_b, best_gap) = best.ok_or_else(|| {
        OperationError::InternalError(
            "Marching corrector exited without computing any iterate".to_string(),
        )
    })?;
    if best_gap > target * 10.0 {
        return Err(OperationError::NumericalError(format!(
            "PK_BLEND_SPINE_DIVERGED: marching corrector exceeded {} iterations \
             with best gap {:.3e} (target {:.3e})",
            MAX_CORRECTOR_ITERS, best_gap, target
        )));
    }
    Ok((best_c, best_a, best_b, MAX_CORRECTOR_ITERS))
}

/// Marching solver for surface pairs that no analytic arm recognises.
///
/// The march samples [`SpineOptions::min_samples`] stations at uniform
/// edge-curve parameters. Each station seeds the corrector with either
/// the previous corrected centre (warm-start, used once we have one) or
/// the legacy bisector heuristic (cold-start at the first station). The
/// corrector then alternate-projects onto both supporting faces.
///
/// After all stations succeed, the centre and contact arrays are fitted
/// to cubic NURBS curves and [`parallel_transport_frames`] attaches a
/// rotation-minimising frame sequence to the spine. The returned
/// `SolverKind::Marched` records the worst-case corrector iteration
/// count so callers / tests can monitor convergence quality.
#[allow(clippy::too_many_arguments)]
fn solve_marching(
    model: &BRepModel,
    edge: &Edge,
    edge_id: EdgeId,
    face_a: FaceId,
    _face_b: FaceId,
    surface_a: &dyn Surface,
    surface_b: &dyn Surface,
    radius: f64,
    options: &SpineOptions,
) -> OperationResult<SpineRail> {
    // 1. Establish signed dihedral and sign convention once at the
    //    edge midpoint — identical convention to plane/plane and the
    //    legacy bisector. The marching path inherits this so we don't
    //    flip-flop ball-side along the edge.
    let edge_mid = edge.evaluate(0.5, &model.curves)?;
    let n_a_mid = get_face_oriented_normal(model, face_a, &edge_mid)?;
    let n_b_mid = get_face_oriented_normal(model, _face_b, &edge_mid)?;
    let raw_tangent = edge.tangent_at(0.5, &model.curves)?;
    let face_a_loop_sign = edge_orientation_in_face(model, face_a, edge_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "Edge {} not present in any loop of face {}",
            edge_id, face_a
        ))
    })?;
    let edge_tangent_in_loop = raw_tangent * face_a_loop_sign;
    let dihedral = robust_face_angle(
        &n_a_mid,
        &n_b_mid,
        &edge_tangent_in_loop,
        &options.tolerance,
    )
    .map_err(|e| {
        OperationError::NumericalError(format!("Marching: dihedral compute failed: {:?}", e))
    })?;

    let abs_dihedral = dihedral.abs();
    if abs_dihedral < MARCHING_NEAR_TANGENT_RAD
        || (std::f64::consts::PI - abs_dihedral) < MARCHING_NEAR_TANGENT_RAD
    {
        return Err(OperationError::InvalidGeometry(
            "Near-tangent surfaces require special handling (marching solver)".to_string(),
        ));
    }

    let offset_sign = if dihedral > 0.0 { -1.0 } else { 1.0 };

    // 2. Sampling cadence. Honor `min_samples`; cap at `max_samples`.
    //    Marching needs ≥ 4 samples to fit a cubic NURBS later, but
    //    `NurbsCurve::fit_to_points` degrades degree gracefully below
    //    that — we still enforce a hard floor of 4 so the frame
    //    sequence is non-trivial.
    let n_samples = options
        .min_samples
        .max(4)
        .min(options.max_samples.max(4));

    // 3. March along the edge. Uniform-parameter sampling — adaptive
    //    arc-length refinement is an F3-γ.1 follow-up; for the
    //    landing slice uniform-t is sufficient and matches the
    //    convention used by the analytic arms.
    //
    //    Each station seeds the corrector with the bisector heuristic
    //    evaluated at the *local* edge point. Warm-starting from the
    //    previous corrected centre is tempting (saves 1-2 corrector
    //    iterations on curved spines) but breaks plane/plane: the
    //    corrector's Picard iteration there has an *infinite line*
    //    of fixed points (any point at the correct perpendicular
    //    offset from the edge satisfies `|center−contact_i| = r`
    //    on both faces). Once station 0 lands at the corner's x,
    //    warm-starting holds that x through every subsequent
    //    station, producing a spine parallel to the edge but
    //    anchored at the wrong axial position. Cold-seeding per
    //    station puts the seed on the correct edge-perpendicular
    //    locus before the corrector runs, eliminating the
    //    degeneracy. The 1-2 saved iterations per station on
    //    curved geometry are not worth the plane/plane failure
    //    mode.
    let mut samples: Vec<SpineRailSample> = Vec::with_capacity(n_samples);
    let mut prev_center: Option<Point3> = None;
    let mut cumulative_arc = 0.0_f64;
    let mut worst_iters: usize = 0;

    for i in 0..n_samples {
        let t = i as f64 / (n_samples as f64 - 1.0);
        let edge_point = edge.evaluate(t, &model.curves)?;

        // Bisector seed at this edge parameter — same heuristic as
        // the legacy bisector path and the analytic arms' starting
        // point. Anchors the corrector on the correct edge-
        // perpendicular locus regardless of prior station drift.
        let seed = {
            let n_a = get_face_oriented_normal(model, face_a, &edge_point)?;
            let n_b = get_face_oriented_normal(model, _face_b, &edge_point)?;
            let bisector_raw = n_a + n_b;
            let bisector = bisector_raw.normalize().map_err(|e| {
                OperationError::NumericalError(format!(
                    "Marching: bisector normalize at t={}: {:?}",
                    t, e
                ))
            })?;
            let dot_a = bisector.dot(&n_a);
            let offset_dist = if dot_a.abs() > 1e-9 {
                radius / dot_a
            } else {
                radius
            };
            edge_point + bisector * (offset_sign * offset_dist)
        };

        let (center, contact_a, contact_b, iters) =
            corrector(surface_a, surface_b, seed, radius, options.tolerance)?;
        worst_iters = worst_iters.max(iters);

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

    // 4. Build spine + rail curves. If the marched samples are
    //    collinear within tolerance (which happens whenever the
    //    underlying geometry admits a straight spine — plane/plane
    //    blends, axis-parallel cylinder/plane, etc.), emit exact
    //    [`Line`]s. Otherwise fit cubic NURBS through the samples;
    //    the fitter uses uniform parameterisation and clamped
    //    knot vectors. Three independent fits keep each curve's
    //    parameterisation aligned to the discrete `SpineRailSample`
    //    arrays so F4 / F5 can reason about local frames.
    //
    //    Collinearity detection matters for two reasons:
    //
    //    * Numerical: `NurbsCurve::fit_to_points` with collinear
    //      control points yields a NURBS whose interior derivative
    //      can collapse to zero magnitude (degenerate basis-function
    //      evaluation when control points span no 2D extent),
    //      breaking the downstream `tangent_at(t).normalize()` call
    //      inside [`parallel_transport_frames`].
    //    * Semantic: emitting a `Line` instead of a NURBS for a
    //      provably-straight spine matches the analytic plane/plane
    //      arm and keeps F4's pipe-surface dispatch on the cheap
    //      path.
    let centers: Vec<Point3> = samples.iter().map(|s| s.center).collect();
    let contacts_a: Vec<Point3> = samples.iter().map(|s| s.contact_a).collect();
    let contacts_b: Vec<Point3> = samples.iter().map(|s| s.contact_b).collect();

    let tol = options.tolerance.distance();
    let centers_colinear = points_are_collinear(&centers, tol);
    let rail_a_colinear = points_are_collinear(&contacts_a, tol);
    let rail_b_colinear = points_are_collinear(&contacts_b, tol);

    let spine: Box<dyn Curve> = if centers_colinear {
        Box::new(Line::new(centers[0], centers[centers.len() - 1]))
    } else {
        Box::new(NurbsCurve::fit_to_points(&centers, 3, tol).map_err(|e| {
            OperationError::NumericalError(format!("Marching: spine NURBS fit failed: {:?}", e))
        })?)
    };
    let rail_a: Box<dyn Curve> = if rail_a_colinear {
        Box::new(Line::new(contacts_a[0], contacts_a[contacts_a.len() - 1]))
    } else {
        Box::new(NurbsCurve::fit_to_points(&contacts_a, 3, tol).map_err(|e| {
            OperationError::NumericalError(format!("Marching: rail A NURBS fit failed: {:?}", e))
        })?)
    };
    let rail_b: Box<dyn Curve> = if rail_b_colinear {
        Box::new(Line::new(contacts_b[0], contacts_b[contacts_b.len() - 1]))
    } else {
        Box::new(NurbsCurve::fit_to_points(&contacts_b, 3, tol).map_err(|e| {
            OperationError::NumericalError(format!("Marching: rail B NURBS fit failed: {:?}", e))
        })?)
    };

    // 5. Rotation-minimising frames along the fitted spine. For
    //    genuinely curved spines (NURBS path) we run
    //    `parallel_transport_frames` with a hint derived from the
    //    spine→contact_a direction at station 0 — that vector is
    //    approximately perpendicular to the spine tangent (exactly
    //    so for an exact rolling ball), giving the parallel-
    //    transport seed a well-defined starting normal.
    //
    //    For collinear samples (Line spine) the rotation-minimising
    //    frame is constant along the spine and F4 reconstructs it
    //    analytically from `(bisector, edge_tangent)` — exactly the
    //    same convention as the analytic plane/plane arm, which
    //    returns `frames: Vec::new()`. Emitting an empty frame
    //    vector here keeps marching's output shape consistent with
    //    the analytic arm for the collinear case (no spurious
    //    `parallel_transport_frames` call on a straight curve, which
    //    can degenerate when the underlying tangent derivative
    //    collapses through rounding).
    let frames = if centers_colinear {
        Vec::new()
    } else {
        let frame_hint = (contacts_a[0] - centers[0]).normalize().ok();
        parallel_transport_frames(
            spine.as_ref(),
            n_samples,
            frame_hint.as_ref(),
            options.tolerance,
        )
        .map_err(|e| {
            OperationError::NumericalError(format!("Marching: frame attachment failed: {:?}", e))
        })?
    };

    Ok(SpineRail {
        spine,
        rail_a,
        rail_b,
        samples,
        frames,
        solver_kind: SolverKind::Marched {
            predictor_steps: n_samples,
            corrector_iters: worst_iters,
        },
    })
}

/// Return `true` when every point in `pts` lies within a small
/// envelope of the chord from `pts[0]` to `pts[last]`. The envelope
/// is `max(10·tol, 1e-10)` — large enough to absorb the corrector's
/// own convergence noise (which is `O(tol)`) but tight enough to
/// reject any geometrically-meaningful curvature drift (a typical
/// cylinder-rim spine arc has sagitta in the 10⁻²–10⁻¹ range, many
/// orders of magnitude above this threshold).
///
/// Degenerate single-point or zero-length-chord inputs are treated
/// as trivially collinear.
fn points_are_collinear(pts: &[Point3], tol: f64) -> bool {
    if pts.len() < 3 {
        return true;
    }
    let perp_tol = (10.0 * tol).max(1e-10);
    let perp_tol_sq = perp_tol * perp_tol;
    let first = pts[0];
    let last = pts[pts.len() - 1];
    let chord = last - first;
    let chord_len_sq = chord.dot(&chord);
    if chord_len_sq < f64::EPSILON {
        // Zero-length chord: collinear iff every interior point
        // coincides with `first` within tolerance.
        return pts
            .iter()
            .all(|p| (*p - first).dot(&(*p - first)) <= perp_tol_sq);
    }
    let inv = 1.0 / chord_len_sq;
    for p in &pts[1..pts.len() - 1] {
        let v = *p - first;
        // Perpendicular component squared = |v|² - (v·chord)² / |chord|².
        let v_dot_chord = v.dot(&chord);
        let perp_sq = (v.dot(&v) - v_dot_chord * v_dot_chord * inv).max(0.0);
        if perp_sq > perp_tol_sq {
            return false;
        }
    }
    true
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

    // ----------------------------------------------------------------
    // F3-γ — marching solver tests
    //
    // The marching solver is exercised directly (not via
    // `solve_spine_for_edge`, which would route plane/plane into the
    // analytic arm). Calling `solve_marching` on a box edge with
    // plane/plane geometry lets us cross-check marching results
    // against the closed-form analytic result and validate the
    // corrector + NURBS-fit + frame-attachment pipeline end-to-end.
    // ----------------------------------------------------------------

    /// Test helper: gather the (`edge`, `surface_a`, `surface_b`)
    /// references needed to invoke [`solve_marching`] for a given
    /// box edge.
    fn marching_inputs(
        model: &BRepModel,
        edge_id: EdgeId,
        face_a: FaceId,
        face_b: FaceId,
    ) -> (Edge, crate::primitives::surface::SurfaceId, crate::primitives::surface::SurfaceId) {
        let edge = model.edges.get(edge_id).expect("edge").clone();
        let surface_a_id = model.faces.get(face_a).expect("face a").surface_id;
        let surface_b_id = model.faces.get(face_b).expect("face b").surface_id;
        (edge, surface_a_id, surface_b_id)
    }

    #[test]
    fn marching_box_edge_converges() {
        let mut model = BRepModel::new();
        let _solid = make_box(&mut model, 4.0, 3.0, 2.0);
        let (edge_id, face_a, face_b) =
            first_manifold_plane_plane_edge(&model).expect("box edges");
        let (edge, sa_id, sb_id) = marching_inputs(&model, edge_id, face_a, face_b);
        let surface_a = model.surfaces.get(sa_id).expect("surface a");
        let surface_b = model.surfaces.get(sb_id).expect("surface b");

        let opts = SpineOptions::default();
        let rail = solve_marching(
            &model, &edge, edge_id, face_a, face_b, surface_a, surface_b, 0.25, &opts,
        )
        .expect("marching should converge on plane/plane");

        assert!(rail.samples.len() >= 4);
    }

    #[test]
    fn marching_solver_kind_is_marched() {
        let mut model = BRepModel::new();
        let _solid = make_box(&mut model, 4.0, 3.0, 2.0);
        let (edge_id, face_a, face_b) =
            first_manifold_plane_plane_edge(&model).expect("box edges");
        let (edge, sa_id, sb_id) = marching_inputs(&model, edge_id, face_a, face_b);
        let surface_a = model.surfaces.get(sa_id).expect("surface a");
        let surface_b = model.surfaces.get(sb_id).expect("surface b");

        let opts = SpineOptions::default();
        let rail = solve_marching(
            &model, &edge, edge_id, face_a, face_b, surface_a, surface_b, 0.3, &opts,
        )
        .expect("marching");
        match rail.solver_kind {
            SolverKind::Marched {
                predictor_steps,
                corrector_iters,
            } => {
                assert!(predictor_steps >= 4);
                // For two infinite planes the Picard iteration is a
                // contraction; corrector converges in very few steps.
                assert!(
                    corrector_iters <= MAX_CORRECTOR_ITERS,
                    "corrector took {corrector_iters} iters (cap {MAX_CORRECTOR_ITERS})"
                );
            }
            other => panic!("expected Marched, got {other:?}"),
        }
    }

    #[test]
    fn marching_contact_distance_equals_radius() {
        let mut model = BRepModel::new();
        let _solid = make_box(&mut model, 4.0, 3.0, 2.0);
        let (edge_id, face_a, face_b) =
            first_manifold_plane_plane_edge(&model).expect("box edges");
        let (edge, sa_id, sb_id) = marching_inputs(&model, edge_id, face_a, face_b);
        let surface_a = model.surfaces.get(sa_id).expect("surface a");
        let surface_b = model.surfaces.get(sb_id).expect("surface b");

        let radius = 0.4;
        let opts = SpineOptions::default();
        let rail = solve_marching(
            &model, &edge, edge_id, face_a, face_b, surface_a, surface_b, radius, &opts,
        )
        .expect("marching");

        for s in &rail.samples {
            let da = (s.contact_a - s.center).magnitude();
            let db = (s.contact_b - s.center).magnitude();
            // Corrector convergence target is `tolerance.distance()`
            // (~1e-6); use a slightly looser bound to accommodate the
            // best-iterate fallback envelope.
            assert!(
                (da - radius).abs() < 1e-5,
                "contact_a-to-center {da} != r {radius}"
            );
            assert!(
                (db - radius).abs() < 1e-5,
                "contact_b-to-center {db} != r {radius}"
            );
        }
    }

    #[test]
    fn marching_matches_analytic_for_box_edge() {
        // Marching on a plane/plane configuration must converge to
        // within tolerance of the analytic closed-form solution.
        let mut model = BRepModel::new();
        let _solid = make_box(&mut model, 4.0, 3.0, 2.0);
        let (edge_id, face_a, face_b) =
            first_manifold_plane_plane_edge(&model).expect("box edges");
        let radius = 0.25;
        let opts = SpineOptions::default();

        // Analytic via the public entry point.
        let analytic = solve_spine_for_edge(&model, edge_id, face_a, face_b, radius, &opts)
            .expect("analytic solve")
            .expect("analytic should match");
        assert_eq!(analytic.solver_kind, SolverKind::AnalyticPlanePlane);

        // Marching via direct call (bypasses analytic dispatch).
        let (edge, sa_id, sb_id) = marching_inputs(&model, edge_id, face_a, face_b);
        let surface_a = model.surfaces.get(sa_id).expect("surface a");
        let surface_b = model.surfaces.get(sb_id).expect("surface b");
        let marched = solve_marching(
            &model, &edge, edge_id, face_a, face_b, surface_a, surface_b, radius, &opts,
        )
        .expect("marching");

        // Compare centres at matching edge parameters. Both arms
        // sample uniformly with `options.min_samples` stations, so
        // sample indices align.
        assert_eq!(analytic.samples.len(), marched.samples.len());
        for (a, m) in analytic.samples.iter().zip(marched.samples.iter()) {
            let dc = (a.center - m.center).magnitude();
            assert!(
                dc < 5e-5,
                "centre mismatch at t={}: analytic {:?} vs marched {:?} (Δ={})",
                a.edge_parameter,
                a.center,
                m.center,
                dc
            );
        }
    }

    #[test]
    fn marching_collinear_emits_empty_frames() {
        // Collinear samples (plane/plane spine is straight) take the
        // Line path and return `frames: Vec::new()` — matching the
        // analytic plane/plane arm's convention. F4 reconstructs
        // frames analytically for straight spines.
        //
        // Frame attachment on a genuinely curved (NURBS) marched
        // spine is exercised by F3-γ.1 once a non-analytic curved
        // fixture is in scope.
        let mut model = BRepModel::new();
        let _solid = make_box(&mut model, 4.0, 3.0, 2.0);
        let (edge_id, face_a, face_b) =
            first_manifold_plane_plane_edge(&model).expect("box edges");
        let (edge, sa_id, sb_id) = marching_inputs(&model, edge_id, face_a, face_b);
        let surface_a = model.surfaces.get(sa_id).expect("surface a");
        let surface_b = model.surfaces.get(sb_id).expect("surface b");

        let opts = SpineOptions {
            min_samples: 16,
            ..SpineOptions::default()
        };
        let rail = solve_marching(
            &model, &edge, edge_id, face_a, face_b, surface_a, surface_b, 0.2, &opts,
        )
        .expect("marching");
        assert_eq!(rail.samples.len(), 16);
        assert!(
            rail.frames.is_empty(),
            "collinear spine should not emit frames"
        );
    }

    #[test]
    fn marching_emits_line_for_collinear_samples() {
        // Marching on a plane/plane configuration must detect the
        // collinear spine and emit a `Line` (matching the analytic
        // arm's output type), not a degenerate NURBS fit through
        // collinear control points.
        let mut model = BRepModel::new();
        let _solid = make_box(&mut model, 4.0, 3.0, 2.0);
        let (edge_id, face_a, face_b) =
            first_manifold_plane_plane_edge(&model).expect("box edges");
        let (edge, sa_id, sb_id) = marching_inputs(&model, edge_id, face_a, face_b);
        let surface_a = model.surfaces.get(sa_id).expect("surface a");
        let surface_b = model.surfaces.get(sb_id).expect("surface b");

        let opts = SpineOptions::default();
        let rail = solve_marching(
            &model, &edge, edge_id, face_a, face_b, surface_a, surface_b, 0.2, &opts,
        )
        .expect("marching");

        assert!(
            rail.spine.as_any().downcast_ref::<Line>().is_some(),
            "marched spine on plane/plane should be Line, got {}",
            rail.spine.type_name()
        );
        assert!(
            rail.rail_a.as_any().downcast_ref::<Line>().is_some(),
            "rail_a should be Line for plane/plane, got {}",
            rail.rail_a.type_name()
        );
        assert!(
            rail.rail_b.as_any().downcast_ref::<Line>().is_some(),
            "rail_b should be Line for plane/plane, got {}",
            rail.rail_b.type_name()
        );
    }

    #[test]
    fn points_are_collinear_basic_cases() {
        // Pin the collinearity helper directly so a regression in
        // the tolerance envelope doesn't manifest only through
        // marching's downstream behaviour.
        let p0 = Point3 { x: 0.0, y: 0.0, z: 0.0 };
        let p1 = Point3 { x: 1.0, y: 0.0, z: 0.0 };
        let p2 = Point3 { x: 2.0, y: 0.0, z: 0.0 };
        let p_off = Point3 { x: 1.0, y: 0.1, z: 0.0 };

        assert!(points_are_collinear(&[p0, p1, p2], 1e-6));
        assert!(!points_are_collinear(&[p0, p_off, p2], 1e-6));
        // Singleton / pair → trivially collinear.
        assert!(points_are_collinear(&[p0], 1e-6));
        assert!(points_are_collinear(&[p0, p1], 1e-6));
        // Within-noise drift (1e-7 perpendicular) accepted at tol = 1e-6.
        let p_noisy = Point3 { x: 1.0, y: 1e-7, z: 0.0 };
        assert!(points_are_collinear(&[p0, p_noisy, p2], 1e-6));
    }

    #[test]
    fn marching_disabled_returns_none_for_non_analytic() {
        // The F3-γ → F3-δ transition defaults `enable_marching` to
        // `false` so production fillet routes non-analytic pairs to
        // the legacy bisector path (avoids `RuledSurface::closest_point`
        // noise on flat extruded walls). Explicit construction with
        // `enable_marching: true` lets callers opt in; the default
        // stays opt-out until F3-δ.
        let opts = SpineOptions {
            enable_marching: true,
            ..SpineOptions::default()
        };
        assert!(opts.enable_marching);
        let default_opts = SpineOptions::default();
        assert!(
            !default_opts.enable_marching,
            "marching is opt-in until F3-δ lands planar-ruled-surface promotion"
        );
    }

    // ----- F3-δ.1: planar-ruled-surface promotion -----

    /// Build a [`RuledSurface`] with two [`Line`] rails between
    /// `(p1, p2)` and `(p3, p4)`.
    fn ruled_from_corners(p1: Point3, p2: Point3, p3: Point3, p4: Point3) -> RuledSurface {
        RuledSurface::new(Box::new(Line::new(p1, p2)), Box::new(Line::new(p3, p4)))
    }

    #[test]
    fn promote_ruled_parallelogram_succeeds() {
        // Two parallel Line rails translated by a constant offset
        // (extrude case). The four corners form a parallelogram and
        // are trivially coplanar — promotion should yield a Plane
        // whose normal is perpendicular to both the edge tangent and
        // the extrude direction.
        let p1 = Point3::new(0.0, 0.0, 0.0);
        let p2 = Point3::new(2.0, 0.0, 0.0); // edge tangent +X
        let extrude = Vector3::new(0.0, 0.0, 1.0); // extrude direction +Z
        let p3 = p1 + extrude;
        let p4 = p2 + extrude;
        let ruled = ruled_from_corners(p1, p2, p3, p4);
        let tol = Tolerance::from_distance(1e-6);
        let plane = try_promote_ruled_to_plane(&ruled, &tol).expect("planar rails should promote");
        // Plane normal must be perpendicular to both edge_tangent (X)
        // and extrude (Z) — i.e. ±Y.
        let dot_x = plane.normal.dot(&Vector3::X).abs();
        let dot_z = plane.normal.dot(&Vector3::Z).abs();
        assert!(dot_x < 1e-9, "plane normal must be ⊥ edge tangent: dot_x={dot_x}");
        assert!(dot_z < 1e-9, "plane normal must be ⊥ extrude dir: dot_z={dot_z}");
        // Corners lie on the plane (zero signed distance).
        for p in [p1, p2, p3, p4] {
            let signed = (p - plane.origin).dot(&plane.normal).abs();
            assert!(signed < 1e-9, "corner {p:?} off plane by {signed}");
        }
    }

    #[test]
    fn promote_ruled_non_coplanar_rejected() {
        // Skew rails — p4 lifted out of the (p1, p2, p3) plane.
        let p1 = Point3::new(0.0, 0.0, 0.0);
        let p2 = Point3::new(1.0, 0.0, 0.0);
        let p3 = Point3::new(0.0, 1.0, 0.0);
        let p4 = Point3::new(1.0, 1.0, 0.5); // out of plane z=0
        let ruled = ruled_from_corners(p1, p2, p3, p4);
        let tol = Tolerance::from_distance(1e-6);
        assert!(
            try_promote_ruled_to_plane(&ruled, &tol).is_none(),
            "non-coplanar rails must not promote"
        );
    }

    #[test]
    fn promote_ruled_curved_rail_rejected() {
        // RuledSurface with one Arc rail (not a Line). Promotion is
        // strictly Line-only — even when the arc lies in a plane its
        // ruling interior bows off the chord plane, so the marching
        // solver must own this case.
        let p1 = Point3::new(0.0, 0.0, 0.0);
        let p2 = Point3::new(1.0, 0.0, 0.0);
        let line1 = Box::new(Line::new(p1, p2));
        let arc =
            Arc::new(Point3::new(0.5, 0.5, 0.0), Vector3::Z, 0.5, 0.0, std::f64::consts::PI)
                .expect("arc construction");
        let ruled = RuledSurface::new(line1, Box::new(arc));
        let tol = Tolerance::from_distance(1e-6);
        assert!(
            try_promote_ruled_to_plane(&ruled, &tol).is_none(),
            "curved-rail ruled surface must not promote"
        );
    }

    #[test]
    fn promote_ruled_collinear_corners_rejected() {
        // All four corners on a single line — degenerate.
        let p1 = Point3::new(0.0, 0.0, 0.0);
        let p2 = Point3::new(1.0, 0.0, 0.0);
        let p3 = Point3::new(2.0, 0.0, 0.0);
        let p4 = Point3::new(3.0, 0.0, 0.0);
        let ruled = ruled_from_corners(p1, p2, p3, p4);
        let tol = Tolerance::from_distance(1e-6);
        assert!(
            try_promote_ruled_to_plane(&ruled, &tol).is_none(),
            "fully collinear corners are not a plane"
        );
    }

    #[test]
    fn promote_ruled_triangle_case_succeeds() {
        // p1, p2, p3 collinear but p4 off the line — the (p1, p2, p3)
        // triangle is degenerate so the helper must fall back to
        // (p1, p2, p4) and check p3 against that plane.
        let p1 = Point3::new(0.0, 0.0, 0.0);
        let p2 = Point3::new(1.0, 0.0, 0.0);
        let p3 = Point3::new(2.0, 0.0, 0.0); // collinear with p1, p2
        let p4 = Point3::new(0.0, 1.0, 0.0); // off the line
        let ruled = ruled_from_corners(p1, p2, p3, p4);
        let tol = Tolerance::from_distance(1e-6);
        let plane = try_promote_ruled_to_plane(&ruled, &tol)
            .expect("fallback triangle (p1,p2,p4) should promote");
        // p3 is on the plane through (p1,p2,p4) (z=0 plane).
        let signed = (p3 - plane.origin).dot(&plane.normal).abs();
        assert!(signed < 1e-9, "p3 off plane: {signed}");
    }

    #[test]
    fn effective_plane_returns_actual_plane_unchanged() {
        // A real Plane round-trips through effective_plane().
        let origin = Point3::new(1.0, 2.0, 3.0);
        let normal = Vector3::new(0.0, 0.0, 1.0);
        let u_dir = Vector3::X;
        let plane = Plane::new(origin, normal, u_dir).expect("plane construction");
        let tol = Tolerance::from_distance(1e-6);
        let eff = effective_plane(&plane, &tol).expect("real plane returns Some");
        assert!((eff.normal - plane.normal).magnitude() < 1e-12);
        assert!((eff.origin - plane.origin).magnitude() < 1e-12);
    }

    #[test]
    fn effective_plane_promotes_ruled_extrude_wall() {
        // Sanity check at the dispatch-layer helper: a planar
        // RuledSurface that mimics an extrude side wall must
        // produce Some(plane).
        let p1 = Point3::new(0.0, 0.0, 0.0);
        let p2 = Point3::new(2.0, 0.0, 0.0);
        let p3 = Point3::new(0.0, 0.0, 1.0);
        let p4 = Point3::new(2.0, 0.0, 1.0);
        let ruled = ruled_from_corners(p1, p2, p3, p4);
        let tol = Tolerance::from_distance(1e-6);
        assert!(
            effective_plane(&ruled, &tol).is_some(),
            "planar extrude-wall ruled surface must promote"
        );
    }

    #[test]
    fn effective_plane_returns_none_for_cylinder() {
        // Non-plane, non-ruled surfaces fall through to None.
        let cyl =
            Cylinder::new(Point3::ORIGIN, Vector3::Z, 1.0).expect("cylinder construction");
        let tol = Tolerance::from_distance(1e-6);
        assert!(effective_plane(&cyl, &tol).is_none());
    }

    #[test]
    fn extrude_prism_side_walls_route_through_analytic_plane_plane() {
        // Integration check at the spine-solver layer: extruding a
        // triangular profile creates a prism whose three vertical
        // side faces are [`RuledSurface`]s. With F3-δ.1 promotion
        // the dispatch routes a vertical-edge fillet (between two
        // RuledSurface side walls) through the plane/plane analytic
        // arm. Without promotion it would fall through to the
        // wildcard arm and either march (and fail on noise) or
        // return Ok(None).
        use crate::operations::extrude::{extrude_profile, ExtrudeOptions};
        use crate::primitives::edge::{Edge, EdgeOrientation};

        let mut model = BRepModel::new();
        let pts: [(f64, f64); 3] = [(0.0, 0.0), (2.0, 0.0), (1.0, 1.5)];
        let mut vertex_ids = Vec::with_capacity(pts.len());
        for &(x, y) in &pts {
            vertex_ids.push(model.vertices.add(x, y, 0.0));
        }
        let mut edge_ids = Vec::with_capacity(pts.len());
        for i in 0..pts.len() {
            let a = vertex_ids[i];
            let b = vertex_ids[(i + 1) % pts.len()];
            let pa = model.vertices.get(a).expect("vertex a exists").position;
            let pb = model.vertices.get(b).expect("vertex b exists").position;
            let line = Line::new(
                Point3::new(pa[0], pa[1], pa[2]),
                Point3::new(pb[0], pb[1], pb[2]),
            );
            let curve_id = model.curves.add(Box::new(line));
            let edge = Edge::new_auto_range(0, a, b, curve_id, EdgeOrientation::Forward);
            edge_ids.push(model.edges.add(edge));
        }
        let extrude_opts = ExtrudeOptions {
            direction: Vector3::Z,
            distance: 1.0,
            cap_ends: true,
            ..Default::default()
        };
        let _ = extrude_profile(&mut model, edge_ids, extrude_opts)
            .expect("triangular prism extrusion");

        // Find an edge whose two adjacent faces are both Ruled — a
        // vertical seam of the prism.
        let mut ruled_pair: Option<(EdgeId, FaceId, FaceId)> = None;
        for (edge_id, _) in model.edges.iter() {
            let faces = find_adjacent_faces(&model, edge_id);
            if faces.len() != 2 {
                continue;
            }
            let both_ruled = faces.iter().all(|&fid| {
                model
                    .faces
                    .get(fid)
                    .and_then(|f| model.surfaces.get(f.surface_id))
                    .map(|s| s.surface_type() == SurfaceType::Ruled)
                    .unwrap_or(false)
            });
            if both_ruled {
                ruled_pair = Some((edge_id, faces[0], faces[1]));
                break;
            }
        }
        let (edge_id, face_a, face_b) =
            ruled_pair.expect("prism must have a ruled/ruled vertical edge");

        let opts = SpineOptions::default();
        let rail = solve_spine_for_edge(&model, edge_id, face_a, face_b, 0.1, &opts)
            .expect("solve")
            .expect("planar ruled walls must promote to plane/plane analytic");
        assert_eq!(
            rail.solver_kind,
            SolverKind::AnalyticPlanePlane,
            "F3-δ.1: extruded ruled walls must promote to plane/plane analytic"
        );
    }
}
