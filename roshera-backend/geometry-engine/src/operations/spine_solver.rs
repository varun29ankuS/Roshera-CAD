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
use crate::math::{Point3, Tolerance};
use crate::operations::blend_graph::{BlendGraph, BlendRadius};
use crate::operations::edge_classification::find_adjacent_faces;
use crate::operations::fillet::{edge_orientation_in_face, get_face_oriented_normal};
use crate::operations::fillet_robust::robust_face_angle;
use crate::operations::{OperationError, OperationResult};
use crate::primitives::curve::{Curve, Line};
use crate::primitives::edge::{Edge, EdgeId};
use crate::primitives::face::FaceId;
use crate::primitives::surface::{Plane, SurfaceType};
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

    // Dispatch on the surface-type tuple. F3-α implements only the
    // (Plane, Plane) arm; everything else falls through to None so
    // the caller routes the request through the legacy path.
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

    #[test]
    fn solve_returns_none_for_curved_face_pair() {
        // Cylinder-on-cylinder etc. is F3-β territory. Until then,
        // any non-plane/plane request falls through to None so the
        // caller routes to the legacy bisector.
        let mut model = BRepModel::new();
        let mut builder = TopologyBuilder::new(&mut model);
        let geom = builder
            .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, 2.0, 5.0)
            .expect("cylinder creation");
        let _solid = match geom {
            GeometryId::Solid(id) => id,
            other => panic!("expected solid, got {other:?}"),
        };

        // Find any edge whose two adjacent faces are not both planar.
        let mut found = false;
        for (edge_id, _e) in model.edges.iter() {
            let faces = find_adjacent_faces(&model, edge_id);
            if faces.len() != 2 {
                continue;
            }
            let s0 = model
                .surfaces
                .get(model.faces.get(faces[0]).expect("face").surface_id)
                .expect("surf");
            let s1 = model
                .surfaces
                .get(model.faces.get(faces[1]).expect("face").surface_id)
                .expect("surf");
            if s0.surface_type() != SurfaceType::Plane
                || s1.surface_type() != SurfaceType::Plane
            {
                let opts = SpineOptions::default();
                let res = solve_spine_for_edge(&model, edge_id, faces[0], faces[1], 0.1, &opts)
                    .expect("solve");
                assert!(res.is_none(), "non-plane/plane should return None");
                found = true;
                break;
            }
        }
        assert!(found, "cylinder should have at least one non-plane/plane edge pair");
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
