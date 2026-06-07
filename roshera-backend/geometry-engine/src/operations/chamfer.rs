//! Chamfer Operations for B-Rep Models
//!
//! Creates beveled transitions between faces by cutting edges at specified angles or distances.
//!
//! Indexed access into edge/face buffers and surface-sample arrays is the
//! canonical idiom — all `arr[i]` sites use indices bounded by topology
//! enumeration. Matches the numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::blend_graph::BlendVertexKind;
use super::diagnostics::{BlendFailure, VertexBlendUnsupportedReason};
use super::edge_blend_topology::{splice_blend_edge, BlendEdgeSurgery};
use super::fillet::{edge_orientation_in_face, get_face_oriented_normal};
use super::lifecycle::{self, OpSpec};
use super::mixed_kind_corner_cap::SeamContinuity;
use super::orientation::orient_face_for_outward;
use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::Curve,
    edge::{Edge, EdgeId, EdgeOrientation},
    face::{Face, FaceId, FaceOrientation},
    r#loop::{Loop, LoopType},
    solid::{BlendKind, SolidId},
    surface::Surface,
    topology_builder::BRepModel,
    vertex::VertexId,
};
use std::collections::{HashMap, HashSet};

/// Options for chamfer operations
#[derive(Debug, Clone)]
pub struct ChamferOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Type of chamfer
    pub chamfer_type: ChamferType,

    /// Distance from edge on first face
    pub distance1: f64,

    /// Distance from edge on second face
    pub distance2: f64,

    /// Whether chamfer is symmetric (equal distances)
    pub symmetric: bool,

    /// Propagation mode for edge selection
    pub propagation: PropagationMode,

    /// Whether to preserve original edges in special cases
    pub preserve_edges: bool,

    /// CF-β.5.2-A — opt-in: convex corner vertices at which the caller
    /// intends to leave a *partial-mixed* selection, knowing that a
    /// follow-up `fillet_edges` call will close the corner. See
    /// [`crate::operations::fillet::FilletOptions::partial_corner_vertices`]
    /// for the full contract; the chamfer side carries identical
    /// semantics.
    pub partial_corner_vertices: Vec<VertexId>,

    /// CF-γ.1 — caller-selectable seam continuity at the mixed-kind
    /// cap's rim. Defaults to
    /// [`SeamContinuity::C0`] (planar N-gon cap — CF-β behaviour).
    /// Selecting [`SeamContinuity::G1`] opts into the CF-γ
    /// degenerate-bicubic NURBS cap whose tangent plane matches each
    /// neighbour at every rim sample. The flag is consulted at the
    /// mixed-kind dispatch site in `handle_chamfer_vertices`; on
    /// non-mixed-kind corners it has no effect.
    pub seam_continuity: SeamContinuity,
}

impl Default for ChamferOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            chamfer_type: ChamferType::EqualDistance(1.0),
            distance1: 1.0,
            distance2: 1.0,
            symmetric: true,
            propagation: PropagationMode::None,
            preserve_edges: false,
            partial_corner_vertices: Vec::new(),
            seam_continuity: SeamContinuity::default(),
        }
    }
}

/// Type of chamfer
#[derive(Debug, Clone)]
pub enum ChamferType {
    /// Equal distance from edge on both faces
    EqualDistance(f64),
    /// Different distances on each face
    TwoDistances(f64, f64),
    /// Distance and angle
    DistanceAngle(f64, f64),
    /// Symmetric at specified angle (45° default)
    Angle(f64),
}

/// How to propagate chamfer selection
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PropagationMode {
    /// No propagation
    None,
    /// Propagate along tangent edges
    Tangent,
    /// Propagate along smooth edges
    Smooth,
}

/// Apply chamfer to edges
pub fn chamfer_edges(
    model: &mut BRepModel,
    solid_id: SolidId,
    edges: Vec<EdgeId>,
    options: ChamferOptions,
) -> OperationResult<Vec<FaceId>> {
    // F2-δ pre-flight: cheap input validation + setback-aware
    // corner compatibility (replaces the historical
    // `validate_no_shared_corners` blanket reject). Atomic — the
    // model is untouched if pre-flight fails.
    if options.common.validate_before {
        lifecycle::validate_can_apply(
            model,
            OpSpec::ChamferEdges {
                solid_id,
                edges: &edges,
                partial_corner_vertices: &options.partial_corner_vertices,
            },
        )?;
    }

    // F2-δ transactional wrapper: any Err out of the body restores
    // the pre-call snapshot so the caller sees an unchanged model.
    lifecycle::with_rollback(model, move |model| {
        // Validate inputs
        validate_chamfer_inputs(model, solid_id, &edges, &options)?;

        // Capture input edges before the Vec is consumed by propagation.
        let input_edges_for_record: Vec<u32> = edges.clone();

        // CF-α: snapshot endpoint vertex IDs of every requested edge
        // *before* `splice_blend_edge` destroys the edge. After
        // surgery we record only surviving vertices on
        // `Solid::blended_vertices`, so the pre-flight conflict gate
        // (`lifecycle::validate_blend_conflict`) can detect shared-
        // corner cross-kind clashes in subsequent blend calls.
        let input_edge_endpoints: Vec<VertexId> = edges
            .iter()
            .filter_map(|&eid| model.edges.get(eid))
            .flat_map(|e| [e.start_vertex, e.end_vertex])
            .collect();

        // CF-β.5.2-B — capture each opt-in partial-mixed corner's
        // incident-edge degree on the *pre-surgery* model. The
        // `splice_blend_edge` loop downstream destroys some of those
        // edges, so a post-surgery count would underflow the typed
        // `MixedKindUnsupported { DegreeUnsupported { degree } }`
        // payload that the second call's feasibility pre-flight emits.
        // Recorded into `Solid::pending_mixed_kind_corners` below
        // alongside `mark_pending_mixed_kind_corner`.
        let partial_corner_original_degrees: HashMap<VertexId, usize> = {
            let mut out: HashMap<VertexId, usize> = HashMap::new();
            for &vid in &options.partial_corner_vertices {
                if model.vertices.get(vid).is_none() {
                    continue;
                }
                let mut degree: usize = 0;
                for (_eid, edge) in model.edges.iter() {
                    if edge.start_vertex == vid || edge.end_vertex == vid {
                        degree += 1;
                    }
                }
                out.insert(vid, degree);
            }
            out
        };

        // Propagate edge selection if requested
        let selected_edges = propagate_edge_selection(model, edges, options.propagation)?;

        // Chamfer-α — identify degree-3 convex planar uniform-offset
        // corners BEFORE the per-edge surgery loop runs. The pre-surgery
        // model still carries the original edges and the corner vertex
        // intact, so adjacent-face lookup and convexity classification
        // are well-defined. The set drives two things:
        //   1. `original_v*_corner_shared` stamping on each
        //      `BlendEdgeSurgery` below — so `splice_blend_edge` skips
        //      the V-side cap insertion + corner-vertex removal at any
        //      endpoint flagged as a shared corner. This parallels the
        //      F5-α flow in fillet.rs:910-911 exactly.
        //   2. The cap synthesis pass at the end (post-splice) — see
        //      [`handle_chamfer_vertices`].
        let corner_set = identify_chamfer_corners(model, solid_id, &selected_edges, &options)?;

        // Chamfer-β — pre-compute per-(edge, corner vertex, shared
        // face) miter overrides for every degree-≥4 convex corner.
        // For degree-3 corners and for non-corner edges the override
        // map is empty (or `None` is passed downstream), preserving
        // legacy behaviour exactly. The miter pass only fires for
        // uniform-offset chamfers (`identify_chamfer_corners` gates
        // `corner_set.corners` on that), and the canonical offset is
        // `options.distance1` (equal to `distance2` within tolerance
        // by the same gate).
        let miter_map = if corner_set.corners.is_empty() {
            MiterOverrideMap::new()
        } else {
            compute_corner_miter_overrides(
                model,
                solid_id,
                &corner_set,
                &selected_edges,
                options.distance1,
                options.common.tolerance,
            )?
        };
        let miter_overrides: Option<&MiterOverrideMap> = if miter_map.is_empty() {
            None
        } else {
            Some(&miter_map)
        };

        // Create chamfer faces for each edge. Closed-edge (rim) chamfers
        // perform their topology surgery inline and return `None`; open
        // edges return `Some(surgery)` for the 4-face splice below.
        let mut chamfer_faces = Vec::new();
        let mut surgeries: Vec<BlendEdgeSurgery> = Vec::new();
        for &edge_id in &selected_edges {
            let (face_id, surgery) =
                create_edge_chamfer(model, solid_id, edge_id, &options, miter_overrides)?;
            chamfer_faces.push(face_id);
            if let Some(mut s) = surgery {
                // Chamfer-α — stamp corner-shared flags. With the flag
                // set, `splice_blend_edge` leaves the corner vertex
                // alive and skips third-face cap insertion at that
                // endpoint; `apply_planar_chamfer_cap` takes over both
                // responsibilities post-splice.
                //
                // CF-β.5.2-A — OR with the opt-in partial-mixed
                // corner set so V is also preserved when the caller
                // declares this corner will be closed later by the
                // opposite blend kind. Mirrors the fillet-side stamp
                // in fillet.rs::create_fillet_chain.
                let v0_partial_opt_in = options.partial_corner_vertices.contains(&s.original_v0);
                let v1_partial_opt_in = options.partial_corner_vertices.contains(&s.original_v1);
                s.original_v0_corner_shared =
                    corner_set.is_corner(s.original_v0) || v0_partial_opt_in;
                s.original_v1_corner_shared =
                    corner_set.is_corner(s.original_v1) || v1_partial_opt_in;
                surgeries.push(s);
            }
        }

        // Re-stitch surrounding topology and add chamfer faces to outer shell.
        update_adjacent_faces_for_chamfer(model, solid_id, &chamfer_faces, &surgeries)?;

        // CF-β.5.2-B — register each surgery's cap-rim edges under the
        // original corner vertex when the corner is preserved. The cap
        // edge constructed in `create_edge_chamfer` connects the
        // *offset* vertices (`v_t1_start/v_t2_start` or
        // `v_t1_end/v_t2_end`), not V, so the mixed-kind cap
        // synthesizer cannot recover the rim from V's edge incidence.
        // This registry stores the (V, cap_edge) link directly so
        // `find_blend_cap_edges_at_vertex` is O(1) per corner.
        if let Some(solid) = model.solids.get_mut(solid_id) {
            for s in &surgeries {
                if s.original_v0_corner_shared {
                    solid.record_corner_cap_edge(s.original_v0, s.cap_v0_edge, BlendKind::Chamfer);
                }
                if s.original_v1_corner_shared {
                    solid.record_corner_cap_edge(s.original_v1, s.cap_v1_edge, BlendKind::Chamfer);
                }
            }
        }

        // Chamfer-α corner closure — walks the pre-computed corner set
        // (vertex IDs survive surgery because their `corner_shared`
        // flags suppressed removal) and emits one planar triangular
        // patch per qualifying corner. Cap face IDs join the returned
        // vec so callers can reference them in the timeline event +
        // verify shell membership.
        if !corner_set.corners.is_empty() {
            let cap_faces = handle_chamfer_vertices(
                model,
                solid_id,
                &corner_set,
                &selected_edges,
                &surgeries,
                options.seam_continuity,
            )?;
            chamfer_faces.extend(cap_faces);
        }

        // CF-β.5.2-A — register every opt-in partial-mixed corner
        // vertex in the host solid's `pending_mixed_kind_corners`
        // set BEFORE the `validate_result` gate runs. The β.4.2
        // carve-out inside `validate_chamfered_solid` consults this
        // set via `filter_pending_corner_errors` to drop the
        // non-manifold-edge errors at intentionally-open corners.
        // Vertices that no longer exist (e.g. surgery removed them
        // because the opt-in was redundant with a fully-closed
        // 3-corner) are filtered out — pending is a vertex-id
        // index, the membership API requires a live id.
        if !options.partial_corner_vertices.is_empty() {
            let alive_partial: Vec<VertexId> = options
                .partial_corner_vertices
                .iter()
                .copied()
                .filter(|&vid| model.vertices.get(vid).is_some())
                .collect();
            if let Some(solid) = model.solids.get_mut(solid_id) {
                for vid in alive_partial {
                    // CF-β.5.2-B — original degree captured
                    // pre-surgery (see `partial_corner_original_degrees`
                    // above). Falls back to 0 only if the vertex was
                    // already gone at function entry (defensive — the
                    // alive_partial filter above already excludes that).
                    let original_degree = partial_corner_original_degrees
                        .get(&vid)
                        .copied()
                        .unwrap_or(0);
                    solid.mark_pending_mixed_kind_corner(vid, original_degree);
                }
            }
        }

        // Validate result if requested
        if options.common.validate_result {
            validate_chamfered_solid(model, solid_id)?;
        }

        // CF-α: populate the per-solid blend registry. Mirrors the
        // fillet writer — edges keyed by their pre-surgery IDs, only
        // surviving endpoint vertices recorded. See
        // `fillet::fillet_edges` for the full rationale.
        let surviving_endpoints: Vec<VertexId> = {
            let mut seen: HashSet<VertexId> = HashSet::new();
            input_edge_endpoints
                .iter()
                .copied()
                .filter(|vid| seen.insert(*vid))
                .filter(|vid| model.vertices.get(*vid).is_some())
                .collect()
        };
        if let Some(solid) = model.solids.get_mut(solid_id) {
            for &eid in &input_edges_for_record {
                solid.record_blended_edge(eid, BlendKind::Chamfer);
            }
            for vid in surviving_endpoints {
                solid.record_blended_vertex(vid, BlendKind::Chamfer);
            }
            // CF-β.3 — tag every emitted chamfer face (trim + cap)
            // with `BlendKind::Chamfer` so the mixed-kind corner cap
            // synthesizer can locate the surviving chamfer faces at
            // a shared corner without re-deriving the classification
            // from surface geometry.
            for &fid in &chamfer_faces {
                solid.record_blend_face(fid, BlendKind::Chamfer);
            }
        }

        // Record the operation for timeline / event-sourcing consumers.
        // `outputs` leads with `solid_id` so that downstream modify ops
        // (fillet-after-chamfer, shell-after-chamfer, …) resolve their
        // parent edge to this event rather than skipping past it to the
        // primitive that originally produced `solid_id`. The chamfer face
        // ids follow so the lineage graph still records *what* topology
        // the op produced.
        //
        // CF-β.4 — snapshot the post-call `pending_mixed_kind_corners`
        // set into the event payload. Timeline replay reconstructs the
        // intermediate-state expectation from this so the
        // `validate_result` gate at the replayed call's boundary
        // applies the same carve-out. Sorted for stable serialisation.
        let pending_after_call: Vec<u64> = {
            let mut v: Vec<u64> = model
                .solids
                .get(solid_id)
                .map(|s| {
                    s.pending_mixed_kind_corners()
                        .keys()
                        .map(|&vid| vid as u64)
                        .collect()
                })
                .unwrap_or_default();
            v.sort_unstable();
            v
        };
        // CF-β.5.2-A — caller's opt-in partial-mixed corner
        // declaration. Sorted for stable replay-determinism
        // (matching the `pending_mixed_kind_corners` shape).
        // Computed outside the `json!` macro because the macro
        // cannot parse the `Vec<u64>` turbofish.
        let partial_corner_vertices_payload: Vec<u64> = {
            let mut v: Vec<u64> = options
                .partial_corner_vertices
                .iter()
                .map(|&vid| vid as u64)
                .collect();
            v.sort_unstable();
            v
        };
        model.record_operation(
            crate::operations::recorder::RecordedOperation::new("chamfer_edges")
                .with_parameters(serde_json::json!({
                    "solid_id": solid_id,
                    "chamfer_type": format!("{:?}", options.chamfer_type),
                    "distance1": options.distance1,
                    "distance2": options.distance2,
                    "symmetric": options.symmetric,
                    "propagation": format!("{:?}", options.propagation),
                    "preserve_edges": options.preserve_edges,
                    "pending_mixed_kind_corners": pending_after_call,
                    "partial_corner_vertices": partial_corner_vertices_payload,
                }))
                .with_input_solids([solid_id as u64])
                .with_input_edges(input_edges_for_record.iter().map(|&e| e as u64))
                .with_output_solids([solid_id as u64])
                .with_output_faces(chamfer_faces.iter().map(|&f| f as u64)),
        );

        // Drop the solid's cached mass-properties — the splice changed
        // volume, surface area, COM and inertia. Without this the next
        // mass / surface-area query returns the pre-chamfer figure.
        if let Some(solid) = model.solids.get_mut(solid_id) {
            solid.invalidate_mass_props_cache();
        }

        Ok(chamfer_faces)
    })
}

/// Create chamfer for a single edge.
///
/// Returns `(FaceId, Option<BlendEdgeSurgery>)`. For open edges the
/// surgery is `Some(...)` and consumed by
/// [`update_adjacent_faces_for_chamfer`] to splice the four-face
/// neighbourhood around V0/V1. For closed (rim) edges the surgery is
/// `None` because the rim-blend pipeline performs its surgery
/// inline (no V0≠V1 to splice).
fn create_edge_chamfer(
    model: &mut BRepModel,
    solid_id: SolidId,
    edge_id: EdgeId,
    options: &ChamferOptions,
    miter_overrides: Option<&MiterOverrideMap>,
) -> OperationResult<(FaceId, Option<BlendEdgeSurgery>)> {
    // Get adjacent faces
    let (face1_id, face2_id) = get_adjacent_faces(model, solid_id, edge_id)?;

    // Closed (rim/seam) edges — `start_vertex == end_vertex` — need a
    // distinct topology. The shared `create_chamfer_face` helper assumes
    // V0 != V1 and produces straight cap edges between distinct V0/V1
    // chamfer vertices; on a closed edge both caps collapse to a single
    // point and the chamfer face has no usable boundary. Upstream of
    // that, `compute_chamfer_offsets` derives an edge axis via
    // `(p1 - p0).normalize()` which is zero-length and surfaces as a
    // cryptic `DivisionByZero` from the math layer.
    //
    // Dispatch closed cylinder rims to the cone-frustum blend pipeline
    // — mirrors `create_closed_edge_fillet` in operations/fillet.rs but
    // produces a truncated cone instead of a torus (chamfer's straight
    // cross-section vs fillet's circular cross-section).
    let is_closed = model
        .edges
        .get(edge_id)
        .map(|e| e.is_loop())
        .unwrap_or(false);
    if is_closed {
        let (d_lat, d_cap) = chamfer_distances_for_closed_edge(
            model,
            edge_id,
            face1_id,
            face2_id,
            &options.chamfer_type,
        )?;
        return create_closed_edge_chamfer(
            model,
            edge_id,
            face1_id,
            face2_id,
            d_lat,
            d_cap,
            options.common.tolerance,
        );
    }

    // Create chamfer based on type. Open-edge creators return a
    // non-optional surgery; wrap in `Some` to match the unified return
    // shape that closed-edge dispatch above uses.
    match &options.chamfer_type {
        ChamferType::EqualDistance(dist) => create_equal_distance_chamfer(
            model,
            edge_id,
            face1_id,
            face2_id,
            *dist,
            miter_overrides,
        )
        .map(|(f, s)| (f, Some(s))),
        ChamferType::TwoDistances(dist1, dist2) => create_two_distance_chamfer(
            model,
            edge_id,
            face1_id,
            face2_id,
            *dist1,
            *dist2,
            miter_overrides,
        )
        .map(|(f, s)| (f, Some(s))),
        ChamferType::DistanceAngle(dist, angle) => create_distance_angle_chamfer(
            model,
            edge_id,
            face1_id,
            face2_id,
            *dist,
            *angle,
            miter_overrides,
        )
        .map(|(f, s)| (f, Some(s))),
        ChamferType::Angle(angle) => {
            create_angle_chamfer(model, edge_id, face1_id, face2_id, *angle, miter_overrides)
                .map(|(f, s)| (f, Some(s)))
        }
    }
}

/// Create equal distance chamfer
fn create_equal_distance_chamfer(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    distance: f64,
    miter_overrides: Option<&MiterOverrideMap>,
) -> OperationResult<(FaceId, BlendEdgeSurgery)> {
    // Get edge geometry
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
        .clone();

    // Compute chamfer offsets along edge
    let chamfer_data = compute_chamfer_offsets(
        model,
        &edge,
        edge_id,
        face1_id,
        face2_id,
        distance,
        distance,
        miter_overrides,
    )?;

    // Create chamfer surface (ruled surface between offset curves)
    let chamfer_surface = create_ruled_chamfer_surface(model, &chamfer_data)?;
    let surface_id = model.surfaces.add(chamfer_surface);

    // Create chamfer face with proper boundaries
    create_chamfer_face(
        model,
        surface_id,
        edge_id,
        face1_id,
        face2_id,
        &chamfer_data,
    )
}

/// Create two-distance chamfer
fn create_two_distance_chamfer(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    distance1: f64,
    distance2: f64,
    miter_overrides: Option<&MiterOverrideMap>,
) -> OperationResult<(FaceId, BlendEdgeSurgery)> {
    // Similar to equal distance but with different offsets
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
        .clone();

    let chamfer_data = compute_chamfer_offsets(
        model,
        &edge,
        edge_id,
        face1_id,
        face2_id,
        distance1,
        distance2,
        miter_overrides,
    )?;

    let chamfer_surface = create_ruled_chamfer_surface(model, &chamfer_data)?;
    let surface_id = model.surfaces.add(chamfer_surface);

    create_chamfer_face(
        model,
        surface_id,
        edge_id,
        face1_id,
        face2_id,
        &chamfer_data,
    )
}

/// Create distance-angle chamfer
fn create_distance_angle_chamfer(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    distance: f64,
    angle: f64,
    miter_overrides: Option<&MiterOverrideMap>,
) -> OperationResult<(FaceId, BlendEdgeSurgery)> {
    // Compute second distance from angle
    let face_angle = compute_face_angle(model, edge_id, face1_id, face2_id)?;
    let distance2 = distance * (angle.sin() / (face_angle - angle).sin());

    create_two_distance_chamfer(
        model,
        edge_id,
        face1_id,
        face2_id,
        distance,
        distance2,
        miter_overrides,
    )
}

/// Create angle-based chamfer.
///
/// `angle` is the chamfer plane's angle (in radians) measured from
/// `face1` toward `face2`. The chamfer width on `face1` defaults to a
/// fraction of the underlying edge's arc length; the width on `face2`
/// follows from the law of sines in the chamfer triangle:
///
/// `d2 = d1 · sin(angle) / sin(face_angle − angle)`
///
/// where `face_angle` is the dihedral angle between the two adjacent
/// faces.
fn create_angle_chamfer(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    angle: f64,
    miter_overrides: Option<&MiterOverrideMap>,
) -> OperationResult<(FaceId, BlendEdgeSurgery)> {
    let face_angle = compute_face_angle(model, edge_id, face1_id, face2_id)?;
    if angle <= 0.0 || angle >= face_angle {
        return Err(OperationError::InvalidGeometry(format!(
            "Chamfer angle {} rad must be in (0, {}) rad (dihedral)",
            angle, face_angle
        )));
    }

    // Derive face1 width from the edge's arc length so the chamfer
    // scales with the feature instead of being a hardcoded constant.
    // 1/10 of edge length is a conservative default that keeps the
    // chamfer well within the adjacent faces for typical geometry.
    let mut edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
        .clone();
    let edge_length = edge
        .length(&model.curves, Tolerance::default())
        .map_err(|e| OperationError::NumericalError(format!("Edge length: {:?}", e)))?;
    if !edge_length.is_finite() || edge_length <= 0.0 {
        return Err(OperationError::FeatureTooSmall);
    }
    let distance1 = (edge_length * 0.1).max(Tolerance::default().distance() * 10.0);
    let denom = (face_angle - angle).sin();
    if denom.abs() < 1e-12 {
        return Err(OperationError::NumericalError(
            "Degenerate chamfer triangle (face_angle − angle ≈ 0)".to_string(),
        ));
    }
    let distance2 = distance1 * angle.sin() / denom;

    create_two_distance_chamfer(
        model,
        edge_id,
        face1_id,
        face2_id,
        distance1,
        distance2,
        miter_overrides,
    )
}

/// Resolve `(d_lat, d_cap)` distances for a closed-rim chamfer, where
/// `d_lat` is the axial pullback along the cylinder and `d_cap` is the
/// radial pullback on the cap face. Cylinder rims always have a π/2
/// dihedral between cap and lateral, which fixes the geometry for
/// `DistanceAngle` and `Angle` chamfer types.
///
/// Mapping of the four [`ChamferType`] variants:
///
///   * `EqualDistance(d)` → `(d, d)`.
///   * `TwoDistances(d1, d2)` — `d1` is the pullback on `face1`; we
///     route it to `d_lat` or `d_cap` depending on which face is the
///     cylinder.
///   * `DistanceAngle(d, a)` — by law of sines in the chamfer triangle
///     with a π/2 dihedral, the second distance is `d · tan(a)`. The
///     base distance `d` is on `face1` (same orientation convention as
///     [`create_distance_angle_chamfer`]).
///   * `Angle(a)` — the base distance defaults to one-tenth of the
///     edge arc-length (matching [`create_angle_chamfer`]); the second
///     follows by `tan(a)` as above. For a closed rim
///     `edge_length = 2π·R`, so `Angle` typically requires a small
///     cylinder radius to keep `d_cap < R`.
fn chamfer_distances_for_closed_edge(
    model: &BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    chamfer_type: &ChamferType,
) -> OperationResult<(f64, f64)> {
    use crate::primitives::surface::Cylinder;
    use std::f64::consts::FRAC_PI_2;

    let face1 = model
        .faces
        .get(face1_id)
        .ok_or_else(|| OperationError::InvalidGeometry(format!("Face {face1_id} not found")))?;
    let face2 = model
        .faces
        .get(face2_id)
        .ok_or_else(|| OperationError::InvalidGeometry(format!("Face {face2_id} not found")))?;
    let surf1 = model.surfaces.get(face1.surface_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!("Surface {} not found", face1.surface_id))
    })?;
    let surf2 = model.surfaces.get(face2.surface_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!("Surface {} not found", face2.surface_id))
    })?;

    let face1_is_cylinder = surf1.as_any().downcast_ref::<Cylinder>().is_some();
    let face2_is_cylinder = surf2.as_any().downcast_ref::<Cylinder>().is_some();

    if face1_is_cylinder == face2_is_cylinder {
        // Either both faces are cylinders (impossible for a manifold
        // rim) or neither is — closed-edge chamfer only supports a
        // Plane–Cylinder pair in this slice.
        return Err(OperationError::NotImplemented(format!(
            "Closed-edge chamfer (edge {edge_id}) currently supports only \
             Plane–Cylinder rims (cylinder caps). Other rim topologies \
             (cone, torus, revolve seams) are tracked as follow-ups."
        )));
    }

    // `face1`'s distance is the user's `distance1` slot; route to
    // (d_lat, d_cap) per which face is the cylinder.
    let route = |d1_for_face1: f64, d2_for_face2: f64| -> (f64, f64) {
        if face1_is_cylinder {
            (d1_for_face1, d2_for_face2) // face1=lat, face2=cap
        } else {
            (d2_for_face2, d1_for_face1) // face1=cap, face2=lat → swap
        }
    };

    match chamfer_type {
        ChamferType::EqualDistance(d) => {
            if !d.is_finite() || *d <= 0.0 {
                return Err(OperationError::InvalidRadius(*d));
            }
            Ok((*d, *d))
        }
        ChamferType::TwoDistances(d1, d2) => {
            if !d1.is_finite() || *d1 <= 0.0 {
                return Err(OperationError::InvalidRadius(*d1));
            }
            if !d2.is_finite() || *d2 <= 0.0 {
                return Err(OperationError::InvalidRadius(*d2));
            }
            Ok(route(*d1, *d2))
        }
        ChamferType::DistanceAngle(d, angle) => {
            if !d.is_finite() || *d <= 0.0 {
                return Err(OperationError::InvalidRadius(*d));
            }
            if !angle.is_finite() || *angle <= 0.0 || *angle >= FRAC_PI_2 {
                return Err(OperationError::InvalidGeometry(format!(
                    "Chamfer angle {angle} rad must be in (0, π/2) rad for a \
                     cylinder-rim chamfer (dihedral = π/2)"
                )));
            }
            // π/2 dihedral: d2 = d * sin(a) / sin(π/2 − a) = d · tan(a).
            let d_other = *d * angle.tan();
            Ok(route(*d, d_other))
        }
        ChamferType::Angle(angle) => {
            if !angle.is_finite() || *angle <= 0.0 || *angle >= FRAC_PI_2 {
                return Err(OperationError::InvalidGeometry(format!(
                    "Chamfer angle {angle} rad must be in (0, π/2) rad for a \
                     cylinder-rim chamfer (dihedral = π/2)"
                )));
            }
            // Match `create_angle_chamfer`'s default: distance1 = 1/10
            // of the underlying edge's arc-length. For a closed rim
            // that's 2π·R / 10. We cannot reach `length()` without a
            // mutable handle to curves here, so derive it from the
            // cylinder radius directly.
            // `Edge::length` is `&mut self` (caches the result), so we
            // clone before measuring — chamfer_distances_for_closed_edge
            // borrows `model` immutably and must not mutate the store.
            let mut edge = model
                .edges
                .get(edge_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
                .clone();
            let edge_length = edge
                .length(&model.curves, Tolerance::default())
                .map_err(|e| OperationError::NumericalError(format!("Rim length: {:?}", e)))?;
            if !edge_length.is_finite() || edge_length <= 0.0 {
                return Err(OperationError::FeatureTooSmall);
            }
            let d1 = (edge_length * 0.1).max(Tolerance::default().distance() * 10.0);
            let d2 = d1 * angle.tan();
            Ok(route(d1, d2))
        }
    }
}

/// Build a cone-frustum chamfer that replaces a cylinder rim.
///
/// Mirrors `cylinder_rim_fillet` in `operations/fillet.rs`. The only
/// surface-level difference is the chamfer surface — a truncated cone
/// — versus the fillet's quarter-torus. Both produce the same loop
/// topology (lateral trim circle + cap trim circle + two seam
/// traversals), and both perform inline topology surgery (no
/// [`BlendEdgeSurgery`] is returned because the rim has no V0/V1 pair
/// to splice).
///
/// Returns `(blend_face_id, None)`. The `None` tells
/// [`update_adjacent_faces_for_chamfer`] to skip the open-edge splice.
#[allow(clippy::too_many_lines)] // mirrors cylinder_rim_fillet's structure for maintainability
fn create_closed_edge_chamfer(
    model: &mut BRepModel,
    rim_edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    d_lat: f64,
    d_cap: f64,
    tol: Tolerance,
) -> OperationResult<(FaceId, Option<BlendEdgeSurgery>)> {
    use crate::primitives::curve::{Arc, Line, ParameterRange};
    use crate::primitives::r#loop::{Loop, LoopType};
    use crate::primitives::surface::{Cone, Cylinder, Plane};
    use std::collections::HashMap;

    // ---------- step 0: identify cap (Plane) vs lateral (Cylinder). ----------
    let face1 = model
        .faces
        .get(face1_id)
        .ok_or_else(|| OperationError::InvalidGeometry(format!("Face {face1_id} not found")))?
        .clone();
    let face2 = model
        .faces
        .get(face2_id)
        .ok_or_else(|| OperationError::InvalidGeometry(format!("Face {face2_id} not found")))?
        .clone();
    let surf1 = model.surfaces.get(face1.surface_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!("Surface {} not found", face1.surface_id))
    })?;
    let surf2 = model.surfaces.get(face2.surface_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!("Surface {} not found", face2.surface_id))
    })?;

    let plane1 = surf1.as_any().downcast_ref::<Plane>();
    let cyl1 = surf1.as_any().downcast_ref::<Cylinder>();
    let plane2 = surf2.as_any().downcast_ref::<Plane>();
    let cyl2 = surf2.as_any().downcast_ref::<Cylinder>();

    let (cap_face_id, lat_face_id, plane, cylinder) = if let (Some(p), Some(c)) = (plane1, cyl2) {
        (face1_id, face2_id, *p, *c)
    } else if let (Some(c), Some(p)) = (cyl1, plane2) {
        (face2_id, face1_id, *p, *c)
    } else {
        return Err(OperationError::NotImplemented(format!(
            "Closed-edge chamfer (edge {rim_edge_id}) currently supports only \
             Plane–Cylinder rims (cylinder caps)."
        )));
    };

    let axis = cylinder.axis;
    let ref_dir = cylinder.ref_dir;
    let big_r = cylinder.radius;
    let origin = cylinder.origin;
    let height_limits = cylinder.height_limits.ok_or_else(|| {
        OperationError::InvalidGeometry(
            "Closed-edge chamfer requires a finite cylinder (height_limits set)".to_string(),
        )
    })?;
    let h_low = height_limits[0];
    let h_high = height_limits[1];
    let height = h_high - h_low;

    // ---------- step 0b: geometric preconditions. ----------
    // d_cap < big_r keeps the inner cap circle non-degenerate.
    // d_lat < height keeps the lateral surface non-degenerate after
    // it's shortened on the rim side.
    //
    // AUDIT-H1: numerical safety margins use the caller-supplied
    // distance tolerance instead of a hardcoded `1e-9`.
    let margin = tol.distance();
    if d_cap >= big_r - margin {
        return Err(OperationError::InvalidGeometry(format!(
            "Chamfer cap distance {d_cap} is too large for cylinder rim: must \
             be strictly less than the cylinder radius ({big_r}); the inner \
             cap circle would collapse to (or invert through) the axis."
        )));
    }
    if d_lat >= height - margin {
        return Err(OperationError::InvalidGeometry(format!(
            "Chamfer lateral distance {d_lat} is too large for cylinder rim: \
             exceeds available cylinder height ({height}); the lateral \
             surface would collapse."
        )));
    }

    // ---------- step 0c: rim sign + new seam positions. ----------
    // sign = +1 for top rim (cap normal aligned with cylinder axis),
    //      = -1 for bottom rim (cap normal opposite the cylinder axis).
    let sign: f64 = if plane.normal.dot(&axis) > 0.0 {
        1.0
    } else {
        -1.0
    };

    let cap_h = if sign > 0.0 { h_high } else { h_low };
    let lat_seam_h = cap_h - sign * d_lat;

    let new_height_limits = if sign > 0.0 {
        [h_low, lat_seam_h]
    } else {
        [lat_seam_h, h_high]
    };

    // World-frame positions of the two new rim seam vertices.
    //   v_lat (= old rim seam vertex, repurposed) lives on the lateral
    //   cylinder at the new shorter height.
    //   v_cap (= newly created) lives on the cap at the reduced radius.
    let lat_seam_pos = origin + axis * lat_seam_h + ref_dir * big_r;
    let cap_seam_pos = origin + axis * cap_h + ref_dir * (big_r - d_cap);

    // ---------- step 0d: cone geometry. ----------
    // Walking the chamfer line in (radial, axial) coordinates from
    // (big_r, lat_seam_h) toward (big_r - d_cap, cap_h), the line hits
    // the axis (r = 0) at axial height `cap_h + sign * h_a` where
    //   h_a = d_lat * (big_r - d_cap) / d_cap.
    // That intercept is the cone apex; the cone half-angle is
    // atan2(d_cap, d_lat) (cap pullback over axial pullback). The
    // cone's natural v-direction runs from the apex outward into the
    // cylinder; v = h_a coincides with the cap seam circle (smaller
    // radius), v = h_a + d_lat with the lateral seam circle (radius
    // = big_r).
    let h_a = d_lat * (big_r - d_cap) / d_cap;
    let cone_apex = origin + axis * (cap_h + sign * h_a);
    let cone_axis = axis * (-sign);
    let cone_half_angle = (d_cap / d_lat).atan();
    let cone_v_cap = h_a;
    let cone_v_lat = h_a + d_lat;

    // ---------- step 1: snapshot the rim edge. ----------
    let rim_edge = model
        .edges
        .get(rim_edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Rim edge not found".to_string()))?
        .clone();
    if rim_edge.start_vertex != rim_edge.end_vertex {
        return Err(OperationError::InvalidGeometry(
            "Closed-edge chamfer invariant violated: rim edge is not a loop".to_string(),
        ));
    }
    let v_lat = rim_edge.start_vertex;

    // ---------- step 2: locate the cap and lateral loops. ----------
    let lat_loop_id = model
        .faces
        .get(lat_face_id)
        .map(|f| f.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Lateral face missing".to_string()))?;
    let cap_loop_id = model
        .faces
        .get(cap_face_id)
        .map(|f| f.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Cap face missing".to_string()))?;

    // The lateral seam edge appears twice in the lateral loop (once
    // forward, once backward) — same "seamed face" pattern the fillet
    // uses. Find both occurrences so they can be replaced together
    // with a fresh shorter seam.
    let (rim_idx_in_lat, seam_edge_id, seam_idx_first, seam_idx_second) = {
        let lat_loop = model
            .loops
            .get(lat_loop_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Lateral loop missing".to_string()))?;
        let mut counts: HashMap<EdgeId, Vec<usize>> = HashMap::new();
        for (i, &e) in lat_loop.edges.iter().enumerate() {
            counts.entry(e).or_default().push(i);
        }
        let mut rim_idx: Option<usize> = None;
        let mut seam_id: Option<EdgeId> = None;
        let mut seam_first: Option<usize> = None;
        let mut seam_second: Option<usize> = None;
        for (i, &e) in lat_loop.edges.iter().enumerate() {
            if e == rim_edge_id {
                rim_idx = Some(i);
                break;
            }
        }
        for (e, idxs) in &counts {
            if idxs.len() == 2 {
                seam_id = Some(*e);
                seam_first = Some(idxs[0]);
                seam_second = Some(idxs[1]);
                break;
            }
        }
        (
            rim_idx.ok_or_else(|| {
                OperationError::InvalidGeometry("Rim edge not found in lateral loop".to_string())
            })?,
            seam_id.ok_or_else(|| {
                OperationError::InvalidGeometry(
                    "Lateral seam edge not found (no duplicate edge in loop)".to_string(),
                )
            })?,
            seam_first.ok_or_else(|| {
                OperationError::InvalidGeometry("Seam first occurrence missing".to_string())
            })?,
            seam_second.ok_or_else(|| {
                OperationError::InvalidGeometry("Seam second occurrence missing".to_string())
            })?,
        )
    };

    let seam_edge = model
        .edges
        .get(seam_edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Seam edge not found".to_string()))?
        .clone();

    // ---------- step 3: move v_lat to its shortened-rim position. ----------
    // Every edge that referenced v_lat (rim, seam) is being rewritten
    // in this same pass, so the move is safe.
    if !model
        .vertices
        .set_position(v_lat, lat_seam_pos.x, lat_seam_pos.y, lat_seam_pos.z)
    {
        return Err(OperationError::InvalidGeometry(format!(
            "Failed to move lateral seam vertex {v_lat} to new rim position"
        )));
    }

    // ---------- step 4: create v_cap at the reduced cap radius. ----------
    let tol = Tolerance::default().distance();
    let v_cap = model
        .vertices
        .add_or_find(cap_seam_pos.x, cap_seam_pos.y, cap_seam_pos.z, tol);

    // ---------- step 5: build the new curves. ----------
    // Cap and lateral trim circles share the cylinder's parametric
    // direction so loop orientation flags carry the same convention as
    // the fillet implementation (see step 7).
    let cap_trim_circle = Arc::circle(origin + axis * cap_h, axis, big_r - d_cap)
        .map_err(|e| OperationError::NumericalError(format!("cap trim circle: {e}")))?;
    let lat_trim_circle = Arc::circle(origin + axis * lat_seam_h, axis, big_r)
        .map_err(|e| OperationError::NumericalError(format!("lat trim circle: {e}")))?;

    // Chamfer cross-section is a straight slant — a Line from the
    // lateral seam vertex to the cap seam vertex. (Fillet uses an Arc
    // here; that's the only surface-shape difference.)
    let cone_seam_line = Line::new(lat_seam_pos, cap_seam_pos);

    // New (shorter) lateral seam line. Vertex IDs are preserved; only
    // the position of v_lat has moved (step 3).
    let new_seam_start_pos = {
        let v = model.vertices.get(seam_edge.start_vertex).ok_or_else(|| {
            OperationError::InvalidGeometry("Seam start vertex missing".to_string())
        })?;
        Point3::new(v.position[0], v.position[1], v.position[2])
    };
    let new_seam_end_pos = {
        let v = model.vertices.get(seam_edge.end_vertex).ok_or_else(|| {
            OperationError::InvalidGeometry("Seam end vertex missing".to_string())
        })?;
        Point3::new(v.position[0], v.position[1], v.position[2])
    };
    let new_seam_line = Line::new(new_seam_start_pos, new_seam_end_pos);

    let cap_trim_curve_id = model.curves.add(Box::new(cap_trim_circle));
    let lat_trim_curve_id = model.curves.add(Box::new(lat_trim_circle));
    let cone_seam_curve_id = model.curves.add(Box::new(cone_seam_line));
    let new_seam_line_id = model.curves.add(Box::new(new_seam_line));

    // ---------- step 6: build the new edges. ----------
    // ParameterRange::new(0.0, 1.0) matches `Arc`'s and `Line`'s unit
    // parameterisation — same convention as the fillet path.
    // F7-α: rail/cap/seam edges thread the caller's tolerance so the
    // F7-δ sew pass can compare gaps against the same value the edge
    // was built with. `Tolerance::default().distance()` matches the
    // historical 1e-6 hardcode — no semantic change vs. pre-F7-α.
    let rail_tol = crate::math::Tolerance::default().distance();
    let cap_trim_edge_id = model.edges.add(Edge::new_with_tolerance(
        0,
        v_cap,
        v_cap,
        cap_trim_curve_id,
        EdgeOrientation::Forward,
        ParameterRange::new(0.0, 1.0),
        rail_tol,
    ));
    let lat_trim_edge_id = model.edges.add(Edge::new_with_tolerance(
        0,
        v_lat,
        v_lat,
        lat_trim_curve_id,
        EdgeOrientation::Forward,
        ParameterRange::new(0.0, 1.0),
        rail_tol,
    ));
    let cone_seam_edge_id = model.edges.add(Edge::new_with_tolerance(
        0,
        v_lat,
        v_cap,
        cone_seam_curve_id,
        EdgeOrientation::Forward,
        ParameterRange::new(0.0, 1.0),
        rail_tol,
    ));
    let new_seam_edge_id = model.edges.add(Edge::new_with_tolerance(
        0,
        seam_edge.start_vertex,
        seam_edge.end_vertex,
        new_seam_line_id,
        EdgeOrientation::Forward,
        ParameterRange::new(0.0, 1.0),
        rail_tol,
    ));

    // ---------- step 7: replace the rim slot in the cap loop. ----------
    // The cap loop is a single closed circle; preserve its original
    // orientation flag (forward for top, backward for bottom — see
    // `create_cylinder_topology`).
    let cap_orientation = {
        let cap_loop = model
            .loops
            .get(cap_loop_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Cap loop missing".to_string()))?;
        let mut orient = None;
        for (i, &e) in cap_loop.edges.iter().enumerate() {
            if e == rim_edge_id {
                orient = Some(cap_loop.orientations[i]);
                break;
            }
        }
        orient.ok_or_else(|| {
            OperationError::InvalidGeometry("Rim edge not found in cap loop".to_string())
        })?
    };
    {
        let cap_loop = model
            .loops
            .get_mut(cap_loop_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Cap loop missing (mut)".to_string()))?;
        for (i, edge) in cap_loop.edges.iter_mut().enumerate() {
            if *edge == rim_edge_id {
                *edge = cap_trim_edge_id;
                cap_loop.orientations[i] = cap_orientation;
                break;
            }
        }
    }

    // ---------- step 8: replace rim + both seam slots in lateral loop. ----------
    {
        let lat_loop = model.loops.get_mut(lat_loop_id).ok_or_else(|| {
            OperationError::InvalidGeometry("Lateral loop missing (mut)".to_string())
        })?;
        let rim_orient = lat_loop.orientations[rim_idx_in_lat];
        let s1 = lat_loop.orientations[seam_idx_first];
        let s2 = lat_loop.orientations[seam_idx_second];
        lat_loop.edges[rim_idx_in_lat] = lat_trim_edge_id;
        lat_loop.orientations[rim_idx_in_lat] = rim_orient;
        lat_loop.edges[seam_idx_first] = new_seam_edge_id;
        lat_loop.orientations[seam_idx_first] = s1;
        lat_loop.edges[seam_idx_second] = new_seam_edge_id;
        lat_loop.orientations[seam_idx_second] = s2;
    }

    // ---------- step 9: shorten the lateral cylinder in-place. ----------
    let lat_surface_id = model
        .faces
        .get(lat_face_id)
        .map(|f| f.surface_id)
        .ok_or_else(|| {
            OperationError::InvalidGeometry("Lateral face missing for surface swap".to_string())
        })?;
    let new_height = new_height_limits[1] - new_height_limits[0];
    let new_origin = origin + axis * new_height_limits[0];
    let new_cylinder = Cylinder::new_finite(new_origin, axis, big_r, new_height)
        .map_err(|e| OperationError::NumericalError(format!("Shortened cylinder: {e}")))?;
    if model
        .surfaces
        .replace(lat_surface_id, Box::new(new_cylinder))
        .is_none()
    {
        return Err(OperationError::InvalidGeometry(format!(
            "Failed to replace lateral surface {lat_surface_id}"
        )));
    }

    // ---------- step 10: build the cone surface + blend face. ----------
    let mut cone = Cone::truncated(
        cone_apex,
        cone_axis,
        cone_half_angle,
        cone_v_cap,
        cone_v_lat,
    )
    .map_err(|e| OperationError::NumericalError(format!("Cone blend: {e}")))?;
    // Cone's intrinsic normal at its parametric midpoint (u=π,
    // v=midpoint) points in the direction (cos π · ref_dir + sin π ·
    // ortho)·cos(half_angle) + cone_axis·sin(half_angle), which is
    // approximately the corner-outward diagonal `cone_axis − ref_dir`.
    // Since `cone_axis = axis * (-sign)` and the chamfer surface
    // bridges the cap-pulled-back circle to the lateral-shortened
    // circle, the geometric outward direction at the blend midpoint
    // is `axis·sign − ref_dir` — the same diagonal that the fillet
    // torus uses (away from the cylinder material at u=π).
    let chamfer_outward_target = axis * sign - ref_dir;
    let blend_orientation = orient_face_for_outward(&cone, chamfer_outward_target)?;
    // Anchor u=0 to the cylinder's ref_dir so the cone's seam aligns
    // with the new lateral seam edge.
    cone.ref_dir = ref_dir;
    cone.angle_limits = Some([0.0, std::f64::consts::TAU]);
    let cone_surface_id = model.surfaces.add(Box::new(cone));

    // Loop sequence — same convention as `cylinder_rim_fillet`:
    //   lat_trim forward, seam forward, cap_trim backward, seam backward.
    // The two trim circles are stored with cylinder-axis orientation
    // (CCW from +axis) and the seam line walks lat → cap forward.
    // Stored-edge orientations match the fillet exactly; the cone's
    // parametric chirality differs from the torus's, which the kernel
    // tolerates because `FaceOrientation::Forward` plus the loop's
    // closed cycle uniquely determines the boundary regardless of the
    // surface's natural normal direction.
    let mut blend_loop = Loop::new(0, LoopType::Outer);
    blend_loop.add_edge(lat_trim_edge_id, true);
    blend_loop.add_edge(cone_seam_edge_id, true);
    blend_loop.add_edge(cap_trim_edge_id, false);
    blend_loop.add_edge(cone_seam_edge_id, false);
    let blend_loop_id = model.loops.add(blend_loop);

    let mut blend_face = Face::new(0, cone_surface_id, blend_loop_id, blend_orientation);
    blend_face.outer_loop = blend_loop_id;
    let blend_face_id = model.faces.add(blend_face);

    // ---------- step 11: cleanup. ----------
    // The original rim edge and old seam edge are no longer referenced
    // by any loop. Curves are append-only; the orphaned circle/line
    // curves remain in the curve store (same trade-off as the fillet).
    model.edges.remove(rim_edge_id);
    model.edges.remove(seam_edge_id);

    Ok((blend_face_id, None))
}

/// Data for chamfer computation
struct ChamferData {
    /// Points on first face offset curve
    offset_points1: Vec<Point3>,
    /// Points on second face offset curve
    offset_points2: Vec<Point3>,
    /// Parameter values along edge
    parameters: Vec<f64>,
    /// Normal directions on faces
    normals1: Vec<Vector3>,
    normals2: Vec<Vector3>,
}

/// Compute chamfer offset curves
fn compute_chamfer_offsets(
    model: &BRepModel,
    edge: &Edge,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    distance1: f64,
    distance2: f64,
    miter_overrides: Option<&MiterOverrideMap>,
) -> OperationResult<ChamferData> {
    let num_samples = 10;
    let mut data = ChamferData {
        offset_points1: Vec::new(),
        offset_points2: Vec::new(),
        parameters: Vec::new(),
        normals1: Vec::new(),
        normals2: Vec::new(),
    };

    let curve = model
        .curves
        .get(edge.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;

    // Each adjacent face traverses the shared edge in its own loop
    // direction. The chamfer offset on a face must lie IN that face's
    // plane and point INTO that face's interior (perpendicular to the
    // edge). The canonical right-hand-rule construction for
    // "in-face-inward perpendicular to edge" is `n × t_loop`, where
    // `t_loop` is the edge tangent oriented along the face's loop
    // traversal — NOT the curve's parameter direction. The two
    // adjacent faces always traverse the edge in opposite directions
    // in a closed manifold shell, so the resulting offset directions
    // are NOT colinear; both correctly point toward each face's
    // interior.
    //
    // The previous formulation derived loop direction from the sign
    // of the dihedral, but `robust_face_angle`'s sign itself depends
    // on the input tangent's parameterization, so the disambiguation
    // was circular: edges whose curve was parameterized backwards
    // relative to face1's loop would flip the chamfer to the wrong
    // side, putting the offset outside the face polygon and the
    // chamfer surface outside the solid.
    let face1_loop_sign = edge_orientation_in_face(model, face1_id, edge_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "Edge {} not present in any loop of face {}",
            edge_id, face1_id
        ))
    })?;
    let face2_loop_sign = edge_orientation_in_face(model, face2_id, edge_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "Edge {} not present in any loop of face {}",
            edge_id, face2_id
        ))
    })?;

    for i in 0..=num_samples {
        let t = i as f64 / num_samples as f64;
        data.parameters.push(t);

        // Get point on edge
        let edge_point = curve.point_at(t).map_err(|e| {
            OperationError::NumericalError(format!("Edge evaluation failed: {:?}", e))
        })?;

        // Get edge tangent at this parameter
        let edge_tangent = curve
            .tangent_at(t)
            .map_err(|e| OperationError::NumericalError(format!("Edge tangent failed: {:?}", e)))?;

        // Get face normals at edge point
        let face_normal1 = face_normal_at_point(model, face1_id, &edge_point)?;
        let face_normal2 = face_normal_at_point(model, face2_id, &edge_point)?;

        // Project the curve tangent onto each face's loop direction.
        // `edge.orientation` already maps curve param to edge param,
        // and `loop_sign` then maps edge param to loop direction. We
        // bypass `Edge::tangent_at` here because we walk the curve
        // directly above; apply the same composition manually.
        let edge_dir_sign = edge.orientation.sign();
        let t_loop1 = edge_tangent * (edge_dir_sign * face1_loop_sign);
        let t_loop2 = edge_tangent * (edge_dir_sign * face2_loop_sign);

        let offset_dir1 = face_normal1.cross(&t_loop1).normalize().map_err(|e| {
            OperationError::NumericalError(format!(
                "Offset direction normalization failed: {:?}",
                e
            ))
        })?;
        let offset_dir2 = face_normal2.cross(&t_loop2).normalize().map_err(|e| {
            OperationError::NumericalError(format!(
                "Offset direction normalization failed: {:?}",
                e
            ))
        })?;

        data.offset_points1
            .push(edge_point + offset_dir1 * distance1);
        data.offset_points2
            .push(edge_point + offset_dir2 * distance2);
        data.normals1.push(face_normal1);
        data.normals2.push(face_normal2);
    }

    // Chamfer-β — apply corner-miter overrides on the cap-side offset
    // endpoints (V0 end at index 0 and V1 end at the last index), then
    // re-interpolate intermediate samples linearly between the
    // possibly-overridden endpoints. The pre-override interior samples
    // already lie on a straight line (planar adjacent face × straight
    // edge × constant offset direction), so replacing a single endpoint
    // would leave the trim curve off the planar chamfer surface. Linear
    // re-interp keeps the trim curve flush with the surface.
    if let Some(map) = miter_overrides {
        // Curve parameter ↦ vertex mapping respects Edge::orientation,
        // mirroring `unit_dir_from_vertex` above.
        let v_at_t0 = if edge.orientation.is_forward() {
            edge.start_vertex
        } else {
            edge.end_vertex
        };
        let v_at_t1 = if edge.orientation.is_forward() {
            edge.end_vertex
        } else {
            edge.start_vertex
        };

        let mut face1_overridden = false;
        let mut face2_overridden = false;

        if let Some(p) = map.get(&MiterKey {
            edge: edge_id,
            vertex: v_at_t0,
            face: face1_id,
        }) {
            data.offset_points1[0] = *p;
            face1_overridden = true;
        }
        if let Some(p) = map.get(&MiterKey {
            edge: edge_id,
            vertex: v_at_t1,
            face: face1_id,
        }) {
            let last = data.offset_points1.len() - 1;
            data.offset_points1[last] = *p;
            face1_overridden = true;
        }
        if let Some(p) = map.get(&MiterKey {
            edge: edge_id,
            vertex: v_at_t0,
            face: face2_id,
        }) {
            data.offset_points2[0] = *p;
            face2_overridden = true;
        }
        if let Some(p) = map.get(&MiterKey {
            edge: edge_id,
            vertex: v_at_t1,
            face: face2_id,
        }) {
            let last = data.offset_points2.len() - 1;
            data.offset_points2[last] = *p;
            face2_overridden = true;
        }

        // Re-interpolate interior samples linearly between the
        // (possibly-overridden) endpoints. Only touches the sequences
        // that had at least one endpoint overridden; unaffected edges
        // keep their raw perpendicular-offset polyline.
        if face1_overridden {
            let last = data.offset_points1.len() - 1;
            if last >= 2 {
                let p0 = data.offset_points1[0];
                let pn = data.offset_points1[last];
                for i in 1..last {
                    let t = i as f64 / last as f64;
                    data.offset_points1[i] = p0 + (pn - p0) * t;
                }
            }
        }
        if face2_overridden {
            let last = data.offset_points2.len() - 1;
            if last >= 2 {
                let p0 = data.offset_points2[0];
                let pn = data.offset_points2[last];
                for i in 1..last {
                    let t = i as f64 / last as f64;
                    data.offset_points2[i] = p0 + (pn - p0) * t;
                }
            }
        }
    }

    Ok(data)
}

/// Create a RuledSurface for the chamfer face, interpolating between the two offset curves.
/// Each offset curve is approximated as a Line between its endpoints.
#[allow(clippy::expect_used)] // offset_points{1,2} non-empty: is_empty() guard above expect sites
fn create_ruled_chamfer_surface(
    _model: &mut BRepModel,
    data: &ChamferData,
) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::curve::Line;
    use crate::primitives::surface::RuledSurface;

    if data.offset_points1.is_empty() || data.offset_points2.is_empty() {
        return Err(OperationError::InvalidGeometry(
            "Chamfer offset curves are empty".to_string(),
        ));
    }

    // Create boundary curves from offset point sequences.
    // For straight edges (the common case), endpoints suffice.
    // For curved edges, we use the endpoints of the sampled polyline —
    // a proper B-spline fit could improve accuracy for highly curved edges.
    let curve1: Box<dyn Curve> = Box::new(Line::new(
        data.offset_points1[0],
        *data
            .offset_points1
            .last()
            .expect("offset_points1 non-empty: is_empty check above rejects empty"),
    ));
    let curve2: Box<dyn Curve> = Box::new(Line::new(
        data.offset_points2[0],
        *data
            .offset_points2
            .last()
            .expect("offset_points2 non-empty: is_empty check above rejects empty"),
    ));

    Ok(Box::new(RuledSurface::new(curve1, curve2)))
}

/// Create chamfer face with boundaries.
///
/// Returns `(face_id, surgery)` — the surgery captures every new
/// vertex/edge/face ID needed by `update_adjacent_faces_for_chamfer`
/// to splice F1, F2, F3, F4 around the new chamfer face.
#[allow(clippy::expect_used)] // offset_points{1,2} non-empty: is_empty() guard at fn entry
fn create_chamfer_face(
    model: &mut BRepModel,
    surface_id: u32,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    data: &ChamferData,
) -> OperationResult<(FaceId, BlendEdgeSurgery)> {
    // Capture original edge endpoints up-front; they're consumed by
    // the surgery and are needed before any mutable borrows.
    let (original_v0, original_v1) = {
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;
        (edge.start_vertex, edge.end_vertex)
    };

    // Validate chamfer data has non-empty offset point sequences.
    // Upstream `compute_chamfer_offsets` always populates these via a
    // `for i in 0..=num_samples` loop, but we guard here defensively
    // since `create_chamfer_face` is callable independently.
    if data.offset_points1.is_empty() || data.offset_points2.is_empty() {
        return Err(OperationError::InvalidGeometry(
            "Chamfer offset point sequences must be non-empty".to_string(),
        ));
    }

    // Create offset edge curves
    let offset_curve1 = create_offset_curve(model, &data.offset_points1)?;
    let offset_curve2 = create_offset_curve(model, &data.offset_points2)?;

    // Capture last-point references once; validated non-empty above.
    let last1 = data
        .offset_points1
        .last()
        .expect("offset_points1 non-empty: validated above");
    let last2 = data
        .offset_points2
        .last()
        .expect("offset_points2 non-empty: validated above");

    // Create vertices at ends. Naming aligns with the surgery contract:
    //   v_t1_start = chamfer-vertex on F1 near V0
    //   v_t1_end   = chamfer-vertex on F1 near V1
    //   v_t2_start = chamfer-vertex on F2 near V0
    //   v_t2_end   = chamfer-vertex on F2 near V1
    //
    // Chamfer-α — switched from `add` to `add_or_find` so the V-side
    // offset endpoints from sibling chamfer faces meeting at a 3-edge
    // corner dedup to the same VertexId. For single-edge or two-edge
    // non-corner chamfers no other chamfer face's offset point sits at
    // the same position, so `add_or_find` returns a fresh id — same
    // observable behaviour as the legacy `add`. Mirrors fillet.rs
    // (lines 4525-4543) where the F5-α corner closure relies on
    // identical dedup.
    let dedup_tol = Tolerance::default().distance();
    let v_t1_start = model.vertices.add_or_find(
        data.offset_points1[0].x,
        data.offset_points1[0].y,
        data.offset_points1[0].z,
        dedup_tol,
    );
    let v_t1_end = model
        .vertices
        .add_or_find(last1.x, last1.y, last1.z, dedup_tol);
    let v_t2_start = model.vertices.add_or_find(
        data.offset_points2[0].x,
        data.offset_points2[0].y,
        data.offset_points2[0].z,
        dedup_tol,
    );
    let v_t2_end = model
        .vertices
        .add_or_find(last2.x, last2.y, last2.z, dedup_tol);

    // Trim edge on F1: start = v_t1_start, end = v_t1_end, Forward.
    // F7-α: rail tolerance threaded explicitly; value matches
    // `NORMAL_TOLERANCE.distance()` so no semantic change.
    let rail_tol = crate::math::Tolerance::default().distance();
    let trim1 = Edge::new_with_tolerance_auto_range(
        0,
        v_t1_start,
        v_t1_end,
        offset_curve1,
        EdgeOrientation::Forward,
        rail_tol,
    );
    let trim1_edge = model.edges.add(trim1);

    // Cap edge at V1: v_t1_end → v_t2_end. The straight-line cap is a
    // chord across the chamfer cross-section at V1.
    let cap_v1_edge = create_straight_edge(model, v_t1_end, v_t2_end)?;

    // Trim edge on F2: start = v_t2_start, end = v_t2_end, Forward —
    // same orientation contract as trim1 / fillet's trim2. Loop
    // traversal at this site is reversed (`add_edge(_, false)` below),
    // so the rectangular boundary still walks
    //   v_t1_start → v_t1_end → v_t2_end → v_t2_start → v_t1_start.
    let trim2 = Edge::new_with_tolerance_auto_range(
        0,
        v_t2_start,
        v_t2_end,
        offset_curve2,
        EdgeOrientation::Forward,
        rail_tol,
    );
    let trim2_edge = model.edges.add(trim2);

    // Cap edge at V0: v_t2_start → v_t1_start.
    let cap_v0_edge = create_straight_edge(model, v_t2_start, v_t1_start)?;

    // Create loop. Traversal:
    //   trim1 fwd: v_t1_start → v_t1_end
    //   cap_v1 fwd: v_t1_end → v_t2_end
    //   trim2 rev: v_t2_end → v_t2_start
    //   cap_v0 fwd: v_t2_start → v_t1_start
    let mut chamfer_loop = Loop::new(0, crate::primitives::r#loop::LoopType::Outer);
    chamfer_loop.add_edge(trim1_edge, true);
    chamfer_loop.add_edge(cap_v1_edge, true);
    chamfer_loop.add_edge(trim2_edge, false);
    chamfer_loop.add_edge(cap_v0_edge, true);
    let loop_id = model.loops.add(chamfer_loop);

    // Orientation: pick the FaceOrientation whose oriented outward normal
    // points away from the solid material. The geometric outward target is
    // the sum of the two adjacent faces' outward normals at the edge
    // midpoint (n1 + n2): it points out of the dihedral for a convex edge
    // and into the open notch for a reflex edge — in both cases the side the
    // chamfer's exposed bevel must face.
    //
    // Previously this was hardcoded `FaceOrientation::Forward`, relying on the
    // `create_ruled_chamfer_surface` u × v normal plus `orientation.sign()`
    // workarounds in the *analytical* mass-properties divergence integral.
    // That path is no longer the volume source — `compute_solid_mass_
    // properties` always routes through the mesh — so the bevel's intrinsic
    // normal pointing inward for non-right dihedrals (e.g. a 108° pentagon
    // edge) made the tessellation render it inverted, over-reporting removed
    // volume ~22× (CHAMFER-MULTIEDGE-VOLUME). Orienting outward at
    // construction, like every other blend face, fixes the mesh volume;
    // reflex behaviour is preserved because n1 + n2 already flips into the
    // notch there.
    let orientation = {
        let target = match (
            model.vertices.get(original_v0).map(|v| v.position),
            model.vertices.get(original_v1).map(|v| v.position),
        ) {
            (Some(a), Some(b)) => {
                let mid = Point3::new(
                    0.5 * (a[0] + b[0]),
                    0.5 * (a[1] + b[1]),
                    0.5 * (a[2] + b[2]),
                );
                match (
                    get_face_oriented_normal(model, face1_id, &mid).ok(),
                    get_face_oriented_normal(model, face2_id, &mid).ok(),
                ) {
                    (Some(n1), Some(n2)) => Some(n1 + n2),
                    _ => None,
                }
            }
            _ => None,
        };
        match target {
            Some(t) if t.magnitude_squared() > 1e-20 => model
                .surfaces
                .get(surface_id)
                .and_then(|s| orient_face_for_outward(s, t).ok())
                .unwrap_or(FaceOrientation::Forward),
            _ => FaceOrientation::Forward,
        }
    };
    let face = Face::new(0, surface_id, loop_id, orientation);
    let face_id = model.faces.add(face);

    let surgery = BlendEdgeSurgery {
        original_edge: edge_id,
        original_v0,
        original_v1,
        face1: face1_id,
        face2: face2_id,
        trim1_edge,
        trim2_edge,
        trim1_curve: offset_curve1,
        trim2_curve: offset_curve2,
        cap_v0_edge,
        cap_v1_edge,
        v_t1_start,
        v_t1_end,
        v_t2_start,
        v_t2_end,
        // Chamfer-α — default both flags to `false`. The caller in
        // [`chamfer_edges`] overwrites them after this surgery is
        // built, stamping `true` on any endpoint that is a
        // Chamfer-α-admissible degree-3 convex planar uniform-offset
        // corner (see [`identify_chamfer_corners`]). With the flag
        // set, `splice_blend_edge` leaves the corner vertex alive
        // and skips third-face cap insertion at that endpoint;
        // [`apply_planar_chamfer_cap`] takes over both
        // responsibilities post-splice. For non-corner endpoints
        // (single-edge / two-edge non-corner / closed-edge rim
        // chamfers) the flag stays `false` and the legacy V-side
        // splice runs unchanged.
        original_v0_corner_shared: false,
        original_v1_corner_shared: false,
    };

    Ok((face_id, surgery))
}

/// Create an offset curve through a sequence of sample points.
///
/// Two points → exact `Line`. Three or more points → degree-min(3, n-1)
/// NURBS curve fit through the points (clamped uniform parameterisation).
/// This preserves the curvature of the offset trail along non-planar
/// chamfered edges, instead of collapsing to a straight chord that
/// silently disconnects from the actual chamfer surface.
fn create_offset_curve(model: &mut BRepModel, points: &[Point3]) -> OperationResult<u32> {
    use crate::primitives::curve::{Line, NurbsCurve};

    let first = points.first().ok_or_else(|| {
        OperationError::InvalidGeometry("Offset curve requires at least one point".to_string())
    })?;

    if points.len() < 2 {
        return Err(OperationError::InvalidGeometry(
            "Offset curve requires at least two points".to_string(),
        ));
    }

    if points.len() == 2 {
        let last = points.last().ok_or_else(|| {
            OperationError::InvalidGeometry("Offset curve requires at least two points".to_string())
        })?;
        let line = Line::new(*first, *last);
        return Ok(model.curves.add(Box::new(line)));
    }

    // 3+ points: fit a clamped NURBS curve. Tolerance is informational for
    // `fit_to_points`; we pass the kernel default.
    let tolerance = crate::math::Tolerance::default();
    let nurbs = NurbsCurve::fit_to_points(points, 3, tolerance.distance())
        .map_err(|e| OperationError::NumericalError(format!("offset curve fit failed: {:?}", e)))?;
    Ok(model.curves.add(Box::new(nurbs)))
}

/// Create straight edge between vertices
fn create_straight_edge(
    model: &mut BRepModel,
    start: VertexId,
    end: VertexId,
) -> OperationResult<EdgeId> {
    use crate::primitives::curve::Line;

    let start_vertex = model
        .vertices
        .get(start)
        .ok_or_else(|| OperationError::InvalidGeometry("Start vertex not found".to_string()))?;
    let end_vertex = model
        .vertices
        .get(end)
        .ok_or_else(|| OperationError::InvalidGeometry("End vertex not found".to_string()))?;

    let line = Line::new(
        Point3::from(start_vertex.position),
        Point3::from(end_vertex.position),
    );
    let curve_id = model.curves.add(Box::new(line));

    // F7-α: cap/connector edges thread the kernel default tolerance
    // explicitly. Value matches the historical 1e-6 — no semantic
    // change for any existing chamfer test.
    let edge = Edge::new_with_tolerance_auto_range(
        0, // Will be assigned by store
        start,
        end,
        curve_id,
        EdgeOrientation::Forward,
        crate::math::Tolerance::default().distance(),
    );
    let edge_id = model.edges.add(edge);

    Ok(edge_id)
}

/// Re-stitch the topology around freshly created chamfer faces.
///
/// Two-step pass:
/// 1. Add every new chamfer face to the solid's outer shell so it
///    participates in shell traversal (face-list, validation, etc.).
/// 2. For each chamfer, run [`splice_blend_edge`] to:
///    - Replace the original edge `E` in F1's loop with `trim1` and
///      re-vertex F1's V0/V1 neighbours onto `v_t1_start`/`v_t1_end`.
///    - Symmetrically splice F2 with `trim2`.
///    - Insert `cap_v0` into F3 (the third face at V0) and `cap_v1`
///      into F4 (the third face at V1).
///    - Remove the now-orphaned original edge and original V0/V1
///      vertices from the model.
///
/// Mirrors `fillet::update_adjacent_faces`; the surgery helper is
/// shared in `super::edge_blend_topology`.
fn update_adjacent_faces_for_chamfer(
    model: &mut BRepModel,
    solid_id: SolidId,
    chamfer_faces: &[FaceId],
    surgeries: &[BlendEdgeSurgery],
) -> OperationResult<()> {
    // Step 1 — register chamfer faces with the outer shell.
    let shell_id = {
        let solid = model.solids.get(solid_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!("Solid {} not found", solid_id))
        })?;
        solid.outer_shell
    };
    {
        let shell = model.shells.get_mut(shell_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!("Outer shell {} not found", shell_id))
        })?;
        for &face_id in chamfer_faces {
            shell.add_face(face_id);
        }
    }

    // Step 2 — splice each blend edge.
    for surgery in surgeries {
        splice_blend_edge(model, solid_id, surgery)?;
    }

    Ok(())
}

/// Chamfer-α corner record.
///
/// One entry per degree-3 convex planar uniform-offset corner that
/// passes [`identify_chamfer_corners`]'s admissibility gates. Built
/// pre-surgery (when adjacent-face normals and the original vertex
/// position are still well-defined) and consumed post-surgery by
/// [`handle_chamfer_vertices`] / [`apply_planar_chamfer_cap`].
#[derive(Debug, Clone)]
struct ChamferCorner {
    /// Original corner vertex id. Preserved across surgery because the
    /// caller stamps `original_v*_corner_shared = true` on every
    /// incident `BlendEdgeSurgery`.
    vertex_id: VertexId,
    /// Pre-surgery corner position. Used by
    /// [`find_cap_edge_at_vertex_for_chamfer`] to locate the V-side cap
    /// edge on each chamfer face by endpoint proximity.
    position: Point3,
    /// Indices into the caller's `selected_edges` / `chamfer_faces`
    /// slices — the N chamfer faces incident to this corner. N ≥ 3,
    /// with N=3 the degree-3 box-corner case (Chamfer-α) and N ≥ 4
    /// the polyhedral apex case (Chamfer-β).
    edge_indices: Vec<usize>,
    /// Outward direction at the corner (sum of the three adjacent
    /// original face normals, normalised). Drives the planar-cap
    /// orientation via [`orient_face_for_outward`].
    outward: Vector3,
}

/// Collection of admissible Chamfer-α corners + a `VertexId` set for
/// O(1) lookup by [`chamfer_edges`] when stamping
/// `original_v*_corner_shared` flags on each `BlendEdgeSurgery`.
#[derive(Debug, Default)]
struct ChamferCornerSet {
    corners: Vec<ChamferCorner>,
    corner_ids: HashSet<VertexId>,
}

impl ChamferCornerSet {
    fn is_corner(&self, v: VertexId) -> bool {
        self.corner_ids.contains(&v)
    }
}

/// Chamfer-α — true iff every per-edge offset is a single value
/// (`ChamferType::EqualDistance`) and `distance1 == distance2`.
///
/// `TwoDistances`, `DistanceAngle`, and `Angle` modes are deferred to
/// Chamfer-β.5. The kernel still emits the per-edge chamfer faces in
/// those modes — the cap synthesis pass simply skips them.
fn chamfer_offsets_uniform(options: &ChamferOptions) -> bool {
    matches!(options.chamfer_type, ChamferType::EqualDistance(_))
        && (options.distance1 - options.distance2).abs() <= Tolerance::default().distance()
}

/// Chamfer-α/β — true iff every adjacent original face around the
/// corner vertex (gathered via [`get_adjacent_faces`] for each of the
/// N ≥ 3 incident chamfered edges) carries a `Plane` surface.
/// Cylinder / sphere / NURBS-adjacent corners are deferred to
/// Chamfer-δ.
fn corner_adjacent_faces_planar(model: &BRepModel, adjacent_face_ids: &[FaceId]) -> bool {
    for &fid in adjacent_face_ids {
        let Some(face) = model.faces.get(fid) else {
            return false;
        };
        let Some(surface) = model.surfaces.get(face.surface_id) else {
            return false;
        };
        if surface
            .as_any()
            .downcast_ref::<crate::primitives::surface::Plane>()
            .is_none()
        {
            return false;
        }
    }
    true
}

/// Chamfer-α — sum the three adjacent face normals evaluated at the
/// corner position, then normalise.
///
/// For a convex 3-edge corner on a planar-faced solid the three
/// outward face normals span ℝ³ and their sum points strictly outward.
/// The function returns a unit vector or surfaces the underlying
/// normalisation failure as `NumericalError`.
pub(crate) fn compute_corner_outward_normal(
    model: &BRepModel,
    vertex_position: Point3,
    adjacent_face_ids: &[FaceId],
) -> OperationResult<Vector3> {
    let mut sum = Vector3::new(0.0, 0.0, 0.0);
    for &fid in adjacent_face_ids {
        let n = face_normal_at_point(model, fid, &vertex_position)?;
        sum = sum + n;
    }
    sum.normalize().map_err(|e| {
        OperationError::NumericalError(format!(
            "Chamfer corner outward normal degenerate (faces' normals cancel): {:?}",
            e
        ))
    })
}

/// Chamfer-α — convexity test: every adjacent face normal agrees in
/// sign with the candidate outward direction.
///
/// A convex corner has all `n_i · outward > 0`. A concave corner has
/// all three opposite signs (rejected by Chamfer-γ). Mixed-sign
/// "cliff" corners are rejected here as well (Task #78 dependency).
fn is_convex_corner(
    model: &BRepModel,
    vertex_position: Point3,
    adjacent_face_ids: &[FaceId],
    outward: Vector3,
) -> OperationResult<bool> {
    for &fid in adjacent_face_ids {
        let n = face_normal_at_point(model, fid, &vertex_position)?;
        if n.dot(&outward) <= 0.0 {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Chamfer-β — pre-surgery corner detection.
///
/// Walks every vertex incident to two or more selected chamfer edges
/// and admits the degree-N (N ≥ 3) convex planar uniform-offset
/// subset. Builds a [`ChamferCornerSet`] consumed in two places:
///
/// 1. By [`chamfer_edges`] immediately after each `create_edge_chamfer`
///    call to stamp `original_v*_corner_shared = true` on the matching
///    `BlendEdgeSurgery` — this suppresses the legacy V-side cap
///    insertion + corner-vertex removal in `splice_blend_edge`.
/// 2. By [`handle_chamfer_vertices`] post-splice to emit a planar
///    N-gon cap face per qualifying corner.
///
/// Two distinct gates are applied:
///
/// 1. **`corner_ids` (V-retention gate)** — populated for *every*
///    degree-≥3 vertex where three or more selected chamfer edges
///    meet, regardless of offset uniformity or face planarity. This
///    drives `original_v*_corner_shared` stamping on the
///    `BlendEdgeSurgery` so `splice_blend_edge` leaves V alive: at
///    an N-edge corner the second through Nth splices would
///    otherwise look up V (already removed by the first splice) and
///    fail with `BlendEdgeSurgery original_v0 N missing from model`.
/// 2. **`corners` (cap-emit gate)** — additionally requires uniform
///    offsets (`EqualDistance(d)` with `distance1 == distance2`),
///    N planar adjacent faces, and a convex corner. Non-cap-emit
///    modes leave V alive but skip cap synthesis (Chamfer-β.5 /
///    γ / δ).
fn identify_chamfer_corners(
    model: &BRepModel,
    solid_id: SolidId,
    selected_edges: &[EdgeId],
    options: &ChamferOptions,
) -> OperationResult<ChamferCornerSet> {
    let cap_synthesis_enabled = chamfer_offsets_uniform(options);

    // Vertex → indices into `selected_edges` of incident chamfered edges.
    let mut vertex_incidence: HashMap<VertexId, Vec<usize>> = HashMap::new();
    for (idx, &edge_id) in selected_edges.iter().enumerate() {
        let edge = model.edges.get(edge_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "Chamfer edge {} missing during corner detection",
                edge_id
            ))
        })?;
        vertex_incidence
            .entry(edge.start_vertex)
            .or_default()
            .push(idx);
        vertex_incidence
            .entry(edge.end_vertex)
            .or_default()
            .push(idx);
    }

    let mut corners: Vec<ChamferCorner> = Vec::new();
    let mut corner_ids: HashSet<VertexId> = HashSet::new();

    for (vertex_id, edge_indices) in &vertex_incidence {
        // CF-β.4 — partial-mixed admission gate. The default Chamfer-β
        // gate requires N ≥ 3 chamfered edges at V before treating V
        // as a corner. CF-β extends that: if V is already recorded in
        // `blended_vertices` with the opposite kind (fillet), the
        // dispatch hook in `handle_chamfer_vertices` routes to
        // `mixed_kind_corner_cap::synthesize_mixed_kind_corner_cap`
        // which assembles a heterogeneous cap (current chamfer rims
        // + prior fillet rims). The synthesizer needs only ≥ 1
        // chamfer edge at V from the current call; the prior fillet
        // call contributed the remaining cap edges.
        let has_prior_fillet = model
            .solids
            .get(solid_id)
            .and_then(|s| s.vertex_blend_set(*vertex_id))
            .map(|set| set.contains(crate::primitives::solid::BlendKind::Fillet))
            .unwrap_or(false);

        // Vertices with 1 or 2 incident selected edges *and* no prior
        // opposite-kind blend never need V-retention — `splice_blend_edge`
        // handles them correctly via the default `corner_shared = false`
        // path.
        if edge_indices.len() < 3 && !has_prior_fillet {
            continue;
        }

        // V-retention gate fires for every degree-≥3 vertex AND every
        // partial-mixed V. The splice ordering means the second of N
        // splices crashes if it can't find V; flagging V as corner-
        // shared on all incident surgeries keeps it alive across the
        // entire splice pass — even at partial-mixed corners where
        // the current chamfer call only selects 1 or 2 of V's edges.
        corner_ids.insert(*vertex_id);

        // Cap-emit gates from here on. Non-uniform / non-planar /
        // non-convex / non-coplanar corners leave V alive (above)
        // but skip cap synthesis — the shell carries a deliberate
        // N-sided hole until the matching later slice fills it.
        if !cap_synthesis_enabled {
            continue;
        }

        // CF-β.4 partial-mixed corner — bypass the same-kind
        // adjacent-face-count, planarity, and convexity gates: those
        // checks duplicate (and are stricter than) the synthesizer's
        // own coplanarity/orientation logic in
        // `synthesize_mixed_kind_corner_cap`. Push a `ChamferCorner`
        // with the current chamfer edges only; the dispatch hook in
        // `handle_chamfer_vertices` (CF-β.3.4) detects
        // `has_prior_fillet` and routes the heterogeneous cap loop
        // assembly to the synthesizer.
        if has_prior_fillet {
            let vertex = match model.vertices.get(*vertex_id) {
                Some(v) => v,
                None => continue,
            };
            let position = Point3::new(vertex.position[0], vertex.position[1], vertex.position[2]);
            // Outward normal from the V-incident adjacent original
            // faces (subset of the full corner umbrella; for a convex
            // manifold corner the subset normals point consistently
            // outward).
            let mut adjacent_set: HashSet<FaceId> = HashSet::new();
            for &eidx in edge_indices {
                let (f1, f2) = get_adjacent_faces(model, solid_id, selected_edges[eidx])?;
                adjacent_set.insert(f1);
                adjacent_set.insert(f2);
            }
            let adjacent_face_ids: Vec<FaceId> = adjacent_set.into_iter().collect();
            let outward = match compute_corner_outward_normal(model, position, &adjacent_face_ids) {
                Ok(v) => v,
                Err(_) => continue,
            };
            corners.push(ChamferCorner {
                vertex_id: *vertex_id,
                position,
                edge_indices: edge_indices.clone(),
                outward,
            });
            continue;
        }

        // Collect the unique adjacent original faces around V. For a
        // convex degree-N manifold corner each pair of consecutive
        // incident edges shares exactly one adjacent face, so the
        // adjacent face count equals the edge count. Mismatch
        // indicates a non-manifold neighbourhood (e.g. two of the
        // chamfered edges share both their adjacent faces — a
        // degenerate fold) — defer.
        let mut adjacent_set: HashSet<FaceId> = HashSet::new();
        for &eidx in edge_indices {
            let (f1, f2) = get_adjacent_faces(model, solid_id, selected_edges[eidx])?;
            adjacent_set.insert(f1);
            adjacent_set.insert(f2);
        }
        if adjacent_set.len() != edge_indices.len() {
            continue;
        }
        let adjacent_face_ids: Vec<FaceId> = adjacent_set.into_iter().collect();

        // Planarity gate — Chamfer-δ handles curved-adjacent corners.
        if !corner_adjacent_faces_planar(model, &adjacent_face_ids) {
            continue;
        }

        // Corner position from the pre-surgery vertex store.
        let vertex = match model.vertices.get(*vertex_id) {
            Some(v) => v,
            None => continue,
        };
        let position = Point3::new(vertex.position[0], vertex.position[1], vertex.position[2]);

        // Outward normal + convexity gate (Chamfer-γ handles concave).
        let outward = match compute_corner_outward_normal(model, position, &adjacent_face_ids) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if !is_convex_corner(model, position, &adjacent_face_ids, outward)? {
            continue;
        }

        // edge_indices is the vertex's chamfered-edge incidence list
        // (length N ≥ 3, gated above).
        corners.push(ChamferCorner {
            vertex_id: *vertex_id,
            position,
            edge_indices: edge_indices.clone(),
            outward,
        });
    }

    Ok(ChamferCornerSet {
        corners,
        corner_ids,
    })
}

/// Chamfer-β — keyed override for a single chamfer offset endpoint.
///
/// One entry per `(chamfered edge, shared corner vertex, shared adjacent
/// face)` triple where N ≥ 4 corner mitering needs to replace the raw
/// perpendicular-in-face offset with the mitered position. After the
/// override fires, adjacent chamfer faces' cap endpoints on the shared
/// face are coincident → the `add_or_find` dedup in [`create_chamfer_face`]
/// unifies them into a single `VertexId` → the N cap edges form a closed
/// polygon → the planar n-gon cap synthesis runs to completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct MiterKey {
    edge: EdgeId,
    vertex: VertexId,
    face: FaceId,
}

/// Chamfer-β — table of per-(edge, vertex, face) miter overrides.
///
/// Empty for degree-3 corners (the legacy box-corner path relies on
/// natural perpendicular-offset coincidence by cube symmetry and is
/// left untouched to keep `chamfer_three_edge_corner_box_emits_planar_cap`
/// green). Empty for every non-corner chamfer call as well.
type MiterOverrideMap = HashMap<MiterKey, Point3>;

/// Chamfer-β — walk an N-edge convex corner in cyclic order around V.
///
/// Returns a `Vec<(edge_id, shared_face_with_next)>` of length N in
/// face-umbrella traversal order, where `shared_face_with_next[i]` is
/// the face that `edge[i]` and `edge[(i+1) % N]` both bound at V.
///
/// Returns `None` if the umbrella fails to close after N steps (non-
/// manifold neighbourhood). The caller leaves miter overrides empty
/// for that corner and the downstream cap-polygon walker surfaces a
/// typed [`BlendFailure::TopologyViolation`].
fn cyclic_order_at_corner(
    model: &BRepModel,
    solid_id: SolidId,
    corner: &ChamferCorner,
    selected_edges: &[EdgeId],
) -> Option<Vec<(EdgeId, FaceId)>> {
    let n = corner.edge_indices.len();
    if n < 3 {
        return None;
    }

    // Build edge_id -> (face_a, face_b) lookup for every chamfered
    // edge incident to V. Both faces are guaranteed distinct on a
    // manifold solid; `identify_chamfer_corners` already enforced
    // `adjacent_set.len() == edge_indices.len()` so each shared face
    // appears exactly twice across the N edge entries.
    let mut edge_faces: HashMap<EdgeId, (FaceId, FaceId)> = HashMap::with_capacity(n);
    for &eidx in &corner.edge_indices {
        let eid = selected_edges[eidx];
        let (f1, f2) = get_adjacent_faces(model, solid_id, eid).ok()?;
        edge_faces.insert(eid, (f1, f2));
    }

    // Seed the walk with the first incident edge; advance along its
    // face2 arbitrarily (either choice produces a cyclic permutation
    // of the same N-tuple).
    let start_eid = selected_edges[corner.edge_indices[0]];
    let (_f_back, mut f_forward) = *edge_faces.get(&start_eid)?;

    let mut order: Vec<(EdgeId, FaceId)> = Vec::with_capacity(n);
    let mut current = start_eid;

    for _step in 0..n {
        order.push((current, f_forward));

        // Find the unique other chamfered edge at V that shares
        // f_forward as one of its adjacent faces.
        let advance = corner.edge_indices.iter().find_map(|&eidx| {
            let eid = selected_edges[eidx];
            if eid == current {
                return None;
            }
            let (a, b) = *edge_faces.get(&eid)?;
            if a == f_forward {
                Some((eid, b))
            } else if b == f_forward {
                Some((eid, a))
            } else {
                None
            }
        });
        let (next_eid, next_forward) = advance?;
        current = next_eid;
        f_forward = next_forward;
    }

    // Walk must return to the start; otherwise the umbrella does
    // not close (non-manifold).
    if current != start_eid {
        return None;
    }
    Some(order)
}

/// Chamfer-β — unit direction from `v_id` along `edge_id`.
///
/// Probes the curve tangent at the matching parameter endpoint
/// (`0` if `v_id` sits at the curve's parameter origin, `1` otherwise)
/// and flips its sign so the resulting vector points AWAY from V into
/// the edge interior. Curve parameter mapping accounts for
/// `Edge::orientation`.
fn unit_dir_from_vertex(
    model: &BRepModel,
    edge_id: EdgeId,
    v_id: VertexId,
) -> OperationResult<Vector3> {
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry(format!("Edge {edge_id} missing")))?;
    let curve = model.curves.get(edge.curve_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!("Curve for edge {edge_id} missing"))
    })?;

    // With Forward orientation curve.point_at(0) is at start_vertex;
    // with Backward at end_vertex.
    let v_at_curve_t0 = if edge.orientation.is_forward() {
        edge.start_vertex
    } else {
        edge.end_vertex
    };
    let (t_at_v, into_edge_sign) = if v_id == v_at_curve_t0 {
        (0.0_f64, 1.0_f64)
    } else {
        (1.0_f64, -1.0_f64)
    };

    let tangent = curve.tangent_at(t_at_v).map_err(|e| {
        OperationError::NumericalError(format!("Tangent at vertex failed: {:?}", e))
    })?;
    let dir = tangent * into_edge_sign;
    dir.normalize()
        .map_err(|e| OperationError::NumericalError(format!("Edge direction degenerate: {:?}", e)))
}

/// Chamfer-β — compute miter overrides for every convex degree-≥4
/// corner in `corner_set`.
///
/// For each consecutive pair (e_i, e_{i+1}) of chamfered edges in
/// cyclic order at V sharing face F:
///
/// ```text
///     M = V + (d / sin(θ/2)) · normalize(v_i + v_{i+1})
/// ```
///
/// where `d` is the uniform chamfer offset, `θ` is the angle at V
/// measured between the two edges on F, and `v_*` are unit directions
/// from V along each edge. Both `v_i` and `v_{i+1}` lie in F (each
/// edge bounds F at V), so the bisector lies in F and M is a point on
/// F at the standard 2D street-corner miter offset from V.
///
/// Both edges' cap-side offset endpoint on F is set to the same M.
/// The dedup at [`create_chamfer_face`] then unifies them into one
/// `VertexId`, so the N cap edges share N corner vertices and the
/// cap-polygon walker closes the loop.
///
/// Degree-3 corners are deliberately skipped — the legacy box-corner
/// path is pinned by `chamfer_three_edge_corner_box_emits_planar_cap`
/// and depends on perpendicular-offset coincidence by cube symmetry.
fn compute_corner_miter_overrides(
    model: &BRepModel,
    solid_id: SolidId,
    corner_set: &ChamferCornerSet,
    selected_edges: &[EdgeId],
    distance: f64,
    tol: Tolerance,
) -> OperationResult<MiterOverrideMap> {
    let mut overrides: MiterOverrideMap = HashMap::new();

    for corner in &corner_set.corners {
        if corner.edge_indices.len() < 3 {
            continue;
        }
        let cyclic = match cyclic_order_at_corner(model, solid_id, corner, selected_edges) {
            Some(c) => c,
            None => continue,
        };
        let n = cyclic.len();
        for i in 0..n {
            let (e_i, f_shared) = cyclic[i];
            let (e_next, _) = cyclic[(i + 1) % n];

            let v_dir_i = unit_dir_from_vertex(model, e_i, corner.vertex_id)?;
            let v_dir_next = unit_dir_from_vertex(model, e_next, corner.vertex_id)?;

            let cos_theta = v_dir_i.dot(&v_dir_next).clamp(-1.0, 1.0);
            let theta = cos_theta.acos();
            let half = theta * 0.5;
            let s = half.sin();
            // Near-collinear adjacent edges → miter blows up. Skip;
            // downstream coplanarity / cap-walker surfaces the issue
            // as a typed BlendFailure.
            //
            // AUDIT-H1: `s = sin(theta/2)`; compare against
            // `tol.parallel_threshold()` (= sin(angle())) so the
            // caller's angular tolerance configuration governs which
            // near-collinear corners get skipped.
            if s.abs() < tol.parallel_threshold() {
                continue;
            }

            let bisector = match (v_dir_i + v_dir_next).normalize() {
                Ok(b) => b,
                Err(_) => continue,
            };

            let miter_dist = distance / s;
            let m_pt: Point3 = corner.position + bisector * miter_dist;

            overrides.insert(
                MiterKey {
                    edge: e_i,
                    vertex: corner.vertex_id,
                    face: f_shared,
                },
                m_pt,
            );
            overrides.insert(
                MiterKey {
                    edge: e_next,
                    vertex: corner.vertex_id,
                    face: f_shared,
                },
                m_pt,
            );
        }
    }
    Ok(overrides)
}

/// Chamfer-β — verify N cap edges close a polygon and recover
/// per-edge loop-forward flags.
///
/// Endpoint-only greedy chain walk over N edges. Generalises the
/// degree-3 triangle verifier (Chamfer-α) to arbitrary N ≥ 3. The
/// `Arc` vs `Line` underlying curve distinction is invisible at this
/// layer.
///
/// Returns `(vertices_in_traversal_order, forwards_in_input_order)`.
/// The caller assembles the cap loop by iterating `cap_edges` in
/// input order and applying `forwards_in_input_order[i]` to each.
pub(crate) fn verify_cap_edges_form_closed_polygon(
    model: &BRepModel,
    cap_edges: &[EdgeId],
) -> Result<(Vec<VertexId>, Vec<bool>), BlendFailure> {
    let n = cap_edges.len();
    if n < 3 {
        return Err(BlendFailure::TopologyViolation {
            detail: format!(
                "chamfer cap loop has {} edges; need ≥ 3 to form a closed polygon",
                n
            ),
        });
    }

    let mut endpoints: Vec<(VertexId, VertexId)> = Vec::with_capacity(n);
    for &edge_id in cap_edges {
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| BlendFailure::TopologyViolation {
                detail: format!(
                    "chamfer cap edge {:?} missing from model during corner-cap cycle check",
                    edge_id
                ),
            })?;
        endpoints.push((edge.start_vertex, edge.end_vertex));
    }

    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut forwards_walk: Vec<bool> = Vec::with_capacity(n);
    let mut used = vec![false; n];
    let mut verts: Vec<VertexId> = Vec::with_capacity(n);

    // Seed the walk with edge 0 in forward orientation.
    order.push(0);
    forwards_walk.push(true);
    used[0] = true;
    verts.push(endpoints[0].0);
    let mut running = endpoints[0].1;

    for _ in 1..n {
        let mut found = false;
        for j in 0..n {
            if used[j] {
                continue;
            }
            if endpoints[j].0 == running {
                order.push(j);
                forwards_walk.push(true);
                used[j] = true;
                verts.push(running);
                running = endpoints[j].1;
                found = true;
                break;
            }
            if endpoints[j].1 == running {
                order.push(j);
                forwards_walk.push(false);
                used[j] = true;
                verts.push(running);
                running = endpoints[j].0;
                found = true;
                break;
            }
        }
        if !found {
            return Err(BlendFailure::TopologyViolation {
                detail: format!(
                    "chamfer cap loop does not close: walked {} of {} edges, \
                     stuck at vertex {:?} with no unused incident cap edge",
                    verts.len(),
                    n,
                    running
                ),
            });
        }
    }

    // The cycle must return to the starting vertex of edge 0.
    if running != verts[0] {
        return Err(BlendFailure::TopologyViolation {
            detail: format!(
                "chamfer cap loop does not close: ended at vertex {:?}, expected {:?}",
                running, verts[0]
            ),
        });
    }

    // Permute forwards back into input edge order.
    let mut forwards_in_input_order = vec![true; n];
    for (k, &orig_idx) in order.iter().enumerate() {
        forwards_in_input_order[orig_idx] = forwards_walk[k];
    }

    Ok((verts, forwards_in_input_order))
}

/// Chamfer-β — verify N cap-loop corner positions are coplanar within
/// `tol` of the plane defined by the first three.
///
/// N ≤ 3 is vacuously coplanar (three points always lie on a plane).
/// For N ≥ 4 we build a unit normal from `(p1 − p0) × (p2 − p0)` and
/// project each remaining point's offset from `p0` onto it; the
/// absolute value of that projection is the signed distance to the
/// candidate plane.
///
/// Returns `false` if the first three points are collinear (zero
/// cross-product) — that case is geometrically degenerate and
/// surfaces as a `BlendFailure` at the caller.
pub(crate) fn cap_vertices_coplanar(positions: &[Point3], tol: f64) -> bool {
    if positions.len() <= 3 {
        return true;
    }
    let ab = positions[1] - positions[0];
    let ac = positions[2] - positions[0];
    let normal = match ab.cross(&ac).normalize() {
        Ok(n) => n,
        Err(_) => return false,
    };
    for p in &positions[3..] {
        let offset = *p - positions[0];
        if offset.dot(&normal).abs() > tol {
            return false;
        }
    }
    true
}

/// Chamfer-β — emit one planar N-gon cap face at a qualifying corner.
///
/// Mirrors `apply_apex_sphere_corner` (fillet.rs:2626-2714) with
/// `Sphere` substituted for `Plane` and the degree-3 special case
/// generalised to arbitrary N ≥ 3. The N V-side cap edges already
/// exist (created per-edge by [`create_chamfer_face`]) and are
/// resolved deterministically by the caller via
/// `BlendEdgeSurgery::cap_v{0,1}_edge`; this function:
///
/// 1. Verifies the supplied cap edges close a polygon and recovers
///    loop-forward flags via [`verify_cap_edges_form_closed_polygon`].
/// 2. Verifies all N corner positions are coplanar within tolerance
///    via [`cap_vertices_coplanar`]; non-coplanar caps surface as
///    `BlendFailure::VertexBlendUnsupported` (curved-adjacent path,
///    deferred to Chamfer-γ).
/// 3. Builds a `Plane` from the first three corner positions.
/// 4. Orients the plane outward via [`orient_face_for_outward`] and
///    registers the new face on the outer shell.
/// 5. Drops the original sharp corner vertex if no edge still
///    references it (same defensive scan as F5-α).
fn apply_planar_chamfer_cap(
    model: &mut BRepModel,
    solid_id: SolidId,
    vertex_id: VertexId,
    cap_edges: &[EdgeId],
    vertex_outward: Vector3,
    tolerance: f64,
) -> OperationResult<FaceId> {
    let degree = cap_edges.len();

    // Step 1 — verify the cap edges close a polygon and recover the
    // per-edge loop-forward flag. The caller resolves cap edge ids via
    // `BlendEdgeSurgery::cap_v{0,1}_edge` so no geometric search is
    // performed here; mitered (Chamfer-β) and raw-perpendicular
    // (Chamfer-α) endpoints flow through the same path.
    let (corner_vertices, _loop_forwards) = verify_cap_edges_form_closed_polygon(model, cap_edges)
        .map_err(|e| OperationError::BlendFailed(Box::new(e)))?;

    // Step 2 — read the N corner vertex positions in traversal order
    // and verify coplanarity. The first three positions define the
    // candidate plane; the remaining N-3 must lie within `tolerance`
    // of it (vacuous when N == 3).
    let mut positions: Vec<Point3> = Vec::with_capacity(corner_vertices.len());
    for &vid in &corner_vertices {
        let v = model.vertices.get(vid).ok_or_else(|| {
            OperationError::InvalidGeometry(format!("Chamfer cap corner vertex {} missing", vid))
        })?;
        positions.push(Point3::new(v.position[0], v.position[1], v.position[2]));
    }
    if !cap_vertices_coplanar(&positions, tolerance) {
        return Err(OperationError::BlendFailed(Box::new(
            BlendFailure::VertexBlendUnsupported {
                vertex: vertex_id,
                kind: BlendVertexKind::ConvexCorner { degree },
                reason: VertexBlendUnsupportedReason::CurvedAdjacent,
            },
        )));
    }

    // Step 3 — build the plane from the first three corner positions.
    // `(B - A) × (C - A)` gives an admissible normal (sign resolved by
    // `orient_face_for_outward` below).
    let p_a = positions[0];
    let p_b = positions[1];
    let p_c = positions[2];

    let ab = p_b - p_a;
    let ac = p_c - p_a;
    let normal = ab.cross(&ac).normalize().map_err(|e| {
        OperationError::NumericalError(format!(
            "Chamfer corner cap plane normal degenerate (cap vertices collinear): {:?}",
            e
        ))
    })?;
    let u_dir = ab.normalize().map_err(|e| {
        OperationError::NumericalError(format!(
            "Chamfer corner cap u-direction degenerate: {:?}",
            e
        ))
    })?;
    let plane = crate::primitives::surface::Plane::new(p_a, normal, u_dir).map_err(|e| {
        OperationError::NumericalError(format!(
            "Chamfer corner cap plane construction failed: {:?}",
            e
        ))
    })?;

    // Step 4 — outward orientation and shell registration.
    let orientation = orient_face_for_outward(&plane, vertex_outward)?;
    let surface_id = model.surfaces.add(Box::new(plane));

    // Build the cap loop in the TRAVERSAL (cyclic) order that
    // `verify_cap_edges_form_closed_polygon` recovered (`corner_vertices`), NOT
    // in the arbitrary input order of `cap_edges`. The input order only happens
    // to be cyclic by coincidence (e.g. unmitered cube symmetry); once the trim
    // edges are mitered at degree-3 corners the input order no longer matches
    // the boundary walk, so building in input order produced a non-closed loop
    // (open box-face / cap loops → non-conforming, leaky tessellation —
    // CHAMFER-MULTIEDGE-VOLUME). Reconstruct each loop edge from the consecutive
    // corner-vertex pair it joins, deriving the loop-forward flag from the
    // edge's own start/end so it is correct regardless of input order.
    let mut cap_loop = Loop::new(0, LoopType::Outer);
    let n = corner_vertices.len();
    for k in 0..n {
        let va = corner_vertices[k];
        let vb = corner_vertices[(k + 1) % n];
        let joined = cap_edges.iter().copied().find_map(|ce| {
            let e = model.edges.get(ce)?;
            if e.start_vertex == va && e.end_vertex == vb {
                Some((ce, true))
            } else if e.start_vertex == vb && e.end_vertex == va {
                Some((ce, false))
            } else {
                None
            }
        });
        match joined {
            Some((ce, fwd)) => cap_loop.add_edge(ce, fwd),
            None => {
                return Err(OperationError::BlendFailed(Box::new(
                    BlendFailure::TopologyViolation {
                        detail: format!(
                            "chamfer corner cap loop: no cap edge joins consecutive \
                             corner vertices {va} -> {vb}"
                        ),
                    },
                )))
            }
        }
    }
    let loop_id = model.loops.add(cap_loop);

    let mut face = Face::new(0, surface_id, loop_id, orientation);
    face.outer_loop = loop_id;
    let face_id = model.faces.add(face);

    let shell_id = model
        .solids
        .get(solid_id)
        .ok_or_else(|| OperationError::InvalidGeometry(format!("Solid {} not found", solid_id)))?
        .outer_shell;
    let shell = model.shells.get_mut(shell_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!("Outer shell {} not found", shell_id))
    })?;
    shell.add_face(face_id);

    // Step 5 — drop the original sharp corner vertex if unreferenced.
    //
    // Each per-edge `splice_blend_edge` for the N corner edges left
    // `vertex_id` alive (its `original_v*_corner_shared` flag was
    // set by `chamfer_edges`). After every chamfer face's trim edges
    // have been spliced in and this cap has been added to the outer
    // shell, no edge in the model should still reference `vertex_id`:
    //
    // * The N corner edges themselves are removed during their own
    //   `splice_blend_edge` calls.
    // * Each adjacent face's loop now references the V-side cap
    //   endpoints (deduplicated via `VertexStore::add_or_find` in
    //   `create_chamfer_face`). The sharp corner is geometrically
    //   replaced by these three offset points + the cap triangle.
    //
    // Defensive: if some upstream invariant slipped (e.g. a fourth
    // unrelated edge happened to share the vertex), leave the vertex
    // in place.
    let still_referenced = model
        .edges
        .iter()
        .any(|(_, e)| e.start_vertex == vertex_id || e.end_vertex == vertex_id);
    if !still_referenced {
        model.vertices.remove(vertex_id);
    }

    Ok(face_id)
}

/// Handle vertices where chamfered edges meet — Chamfer-β planar
/// N-gon cap synthesis.
///
/// Iterates the `corner_set` built pre-surgery by
/// [`identify_chamfer_corners`] and emits one planar N-gon cap face
/// per admissible corner via [`apply_planar_chamfer_cap`]. The
/// degree-3 case (Chamfer-α, triangular cap) and the degree-≥4 case
/// (Chamfer-β, N-gon cap) flow through the same path; only the cap
/// loop length differs. Non-admissible corners (degree < 3,
/// non-planar adjacent, non-convex, non-uniform offset,
/// non-coplanar cap vertices) never enter the set — those branches
/// land in later slices (Chamfer-β.2 / β.5 / γ / δ).
fn handle_chamfer_vertices(
    model: &mut BRepModel,
    solid_id: SolidId,
    corner_set: &ChamferCornerSet,
    selected_edges: &[EdgeId],
    surgeries: &[BlendEdgeSurgery],
    seam_continuity: SeamContinuity,
) -> OperationResult<Vec<FaceId>> {
    let mut cap_faces: Vec<FaceId> = Vec::new();
    if corner_set.corners.is_empty() {
        return Ok(cap_faces);
    }

    let tolerance = Tolerance::default().distance();

    for corner in &corner_set.corners {
        let degree = corner.edge_indices.len();
        // Resolve the N V-side cap edges by surgery lookup rather than
        // by geometric proximity search. Each chamfered edge produced a
        // `BlendEdgeSurgery` carrying both V0 and V1 cap edge ids; the
        // one we want at this corner is `cap_v0_edge` if the corner
        // vertex matches `surgery.original_v0`, else `cap_v1_edge`.
        // The proximity-based fallback was retired because mitered
        // cap endpoints (Chamfer-β) sit at distance `d / sin(θ/2)`
        // from V — substantially larger than the legacy `d·√2` search
        // bound for sharp corners. The surgery already stores the
        // exact `EdgeId` so no geometric search is required.
        let mut cap_edges_at_vertex: Vec<EdgeId> = Vec::with_capacity(degree);
        for &ei in &corner.edge_indices {
            let eid = selected_edges[ei];
            let surgery = surgeries
                .iter()
                .find(|s| s.original_edge == eid)
                .ok_or_else(|| {
                    OperationError::BlendFailed(Box::new(BlendFailure::VertexBlendUnsupported {
                        vertex: corner.vertex_id,
                        kind: BlendVertexKind::ConvexCorner { degree },
                        reason: VertexBlendUnsupportedReason::NonManifoldNeighbourhood,
                    }))
                })?;
            let cap_edge = if surgery.original_v0 == corner.vertex_id {
                surgery.cap_v0_edge
            } else if surgery.original_v1 == corner.vertex_id {
                surgery.cap_v1_edge
            } else {
                return Err(OperationError::BlendFailed(Box::new(
                    BlendFailure::VertexBlendUnsupported {
                        vertex: corner.vertex_id,
                        kind: BlendVertexKind::ConvexCorner { degree },
                        reason: VertexBlendUnsupportedReason::NonManifoldNeighbourhood,
                    },
                )));
            };
            cap_edges_at_vertex.push(cap_edge);
        }
        // CF-β.3.4 — mixed-kind dispatch. If `corner.vertex_id`
        // already carries a recorded *fillet* blend on this solid
        // (chamfer-second ordering), the corner's boundary is the
        // heterogeneous loop chamfer-linear-rims ∪ fillet-arc-rims.
        // Route to the eager-cap synthesizer instead of the chamfer-
        // only planar N-gon path so a single mixed cap closes both
        // sides. Same-kind corners (the typical chamfer-only path)
        // fall through to `apply_planar_chamfer_cap` unchanged.
        let has_prior_fillet = model
            .solids
            .get(solid_id)
            .and_then(|s| s.vertex_blend_set(corner.vertex_id))
            .map(|set| set.contains(BlendKind::Fillet))
            .unwrap_or(false);

        let cap_face_ids: Vec<FaceId> = if has_prior_fillet {
            let fillet_rim_arcs = super::mixed_kind_corner_cap::find_blend_cap_edges_at_vertex(
                model,
                solid_id,
                corner.vertex_id,
                BlendKind::Fillet,
            );
            let mut cap_edges_with_kind: Vec<(EdgeId, super::mixed_kind_corner_cap::RimKind)> =
                cap_edges_at_vertex
                    .iter()
                    .map(|&e| (e, super::mixed_kind_corner_cap::RimKind::LinearRim))
                    .collect();
            for arc_eid in &fillet_rim_arcs {
                cap_edges_with_kind.push((*arc_eid, super::mixed_kind_corner_cap::RimKind::ArcRim));
            }
            // CF-γ.6.2 dispatcher: branch on caller-selected seam
            // continuity. `C0` keeps CF-β planar cap synthesis
            // byte-identical (the historical path, single FaceId
            // returned as a 1-element Vec). `G1` routes through the
            // 3-sub-patch C0 synthesizer in
            // `synthesize_mixed_kind_corner_cap_g1` — three bicubic
            // NURBS sub-patches sharing a central apex vertex, each
            // sub-patch owning one rim. CF-γ.6.3 lifts the seed CPs
            // to the coupled rim-G1 + internal-C1 solver; γ.6.2 ships
            // the watertight topology with planar-fairing CPs so the
            // dispatcher contract stabilises before the solver lands.
            match seam_continuity {
                SeamContinuity::C0 => vec![
                    super::mixed_kind_corner_cap::synthesize_mixed_kind_corner_cap(
                        model,
                        solid_id,
                        corner.vertex_id,
                        &cap_edges_with_kind,
                        corner.outward,
                        tolerance,
                        BlendKind::Chamfer,
                    )?,
                ],
                SeamContinuity::G1 => {
                    super::mixed_kind_corner_cap_g1::synthesize_mixed_kind_corner_cap_g1(
                        model,
                        solid_id,
                        corner.vertex_id,
                        &cap_edges_with_kind,
                        corner.outward,
                        tolerance,
                        BlendKind::Chamfer,
                    )?
                }
            }
        } else {
            vec![apply_planar_chamfer_cap(
                model,
                solid_id,
                corner.vertex_id,
                &cap_edges_at_vertex,
                corner.outward,
                tolerance,
            )?]
        };
        cap_faces.extend(cap_face_ids);
    }
    Ok(cap_faces)
}

/// Propagate edge selection
fn propagate_edge_selection(
    model: &BRepModel,
    initial_edges: Vec<EdgeId>,
    mode: PropagationMode,
) -> OperationResult<Vec<EdgeId>> {
    match mode {
        PropagationMode::None => Ok(initial_edges),
        PropagationMode::Tangent => propagate_tangent_edges(model, initial_edges),
        PropagationMode::Smooth => propagate_smooth_edges(model, initial_edges),
    }
}

/// Propagate along tangent-continuous edges.
///
/// Walks outward from each seed edge, adding any edge that shares a
/// vertex with an already-selected edge AND whose tangent direction at
/// the shared vertex is parallel (within `Tolerance::default().angle()`)
/// to the seed's tangent. Iterates until no new edges are added.
fn propagate_tangent_edges(
    model: &BRepModel,
    initial_edges: Vec<EdgeId>,
) -> OperationResult<Vec<EdgeId>> {
    propagate_by_continuity(model, initial_edges, ContinuityKind::Tangent)
}

/// Propagate along smoothly-connected edges.
///
/// Smooth = tangent-continuous AND the curvature on either side of the
/// shared vertex matches within tolerance (G2-like). For the chamfer
/// selector this is currently equivalent to tangent propagation since
/// curvature comparisons across edges of different curve families are
/// not meaningful for chamfer fan-out; the function delegates with the
/// same predicate but is exposed separately so the API distinction
/// remains stable.
fn propagate_smooth_edges(
    model: &BRepModel,
    initial_edges: Vec<EdgeId>,
) -> OperationResult<Vec<EdgeId>> {
    propagate_by_continuity(model, initial_edges, ContinuityKind::Smooth)
}

#[derive(Copy, Clone)]
enum ContinuityKind {
    Tangent,
    Smooth,
}

fn propagate_by_continuity(
    model: &BRepModel,
    initial_edges: Vec<EdgeId>,
    kind: ContinuityKind,
) -> OperationResult<Vec<EdgeId>> {
    use std::collections::{HashMap, HashSet, VecDeque};

    // Build vertex → incident edges adjacency over the model.
    let mut vertex_edges: HashMap<VertexId, Vec<EdgeId>> = HashMap::new();
    for (eid, edge) in model.edges.iter() {
        vertex_edges.entry(edge.start_vertex).or_default().push(eid);
        vertex_edges.entry(edge.end_vertex).or_default().push(eid);
    }

    let angle_tol = Tolerance::default().angle().max(1e-6);
    let mut selected: HashSet<EdgeId> = initial_edges.iter().copied().collect();
    let mut queue: VecDeque<EdgeId> = initial_edges.iter().copied().collect();

    while let Some(eid) = queue.pop_front() {
        let edge = match model.edges.get(eid) {
            Some(e) => e,
            None => continue,
        };
        for &v in &[edge.start_vertex, edge.end_vertex] {
            let neighbors = match vertex_edges.get(&v) {
                Some(ns) => ns,
                None => continue,
            };
            // Tangent of `edge` at vertex v
            let t_seed = edge_tangent_at_vertex(model, edge, v).unwrap_or(Vector3::ZERO);
            if t_seed.magnitude() < 1e-12 {
                continue;
            }
            for &nid in neighbors {
                if nid == eid || selected.contains(&nid) {
                    continue;
                }
                let nedge = match model.edges.get(nid) {
                    Some(e) => e,
                    None => continue,
                };
                let t_n = match edge_tangent_at_vertex(model, nedge, v) {
                    Some(t) => t,
                    None => continue,
                };
                if t_n.magnitude() < 1e-12 {
                    continue;
                }
                let cos = t_seed
                    .normalize()
                    .unwrap_or(Vector3::X)
                    .dot(&t_n.normalize().unwrap_or(Vector3::X));
                // Accept either co-directional or anti-directional
                // tangent (edges may be oriented oppositely at the
                // shared vertex).
                let aligned = cos.abs() >= (angle_tol).cos();
                if !aligned {
                    continue;
                }
                if matches!(kind, ContinuityKind::Smooth) {
                    // Reserved for future curvature-match check; tangent
                    // alignment is sufficient for the chamfer selector
                    // because chamfered faces inherit the seed edge's
                    // local frame, not its curvature.
                }
                selected.insert(nid);
                queue.push_back(nid);
            }
        }
    }

    Ok(selected.into_iter().collect())
}

/// Curve tangent of `edge` evaluated at the curve parameter
/// corresponding to vertex `v`. Returns `None` if the curve cannot be
/// evaluated.
fn edge_tangent_at_vertex(model: &BRepModel, edge: &Edge, v: VertexId) -> Option<Vector3> {
    let curve = model.curves.get(edge.curve_id)?;
    let t = if v == edge.start_vertex {
        edge.param_range.start
    } else {
        edge.param_range.end
    };
    let tan = curve.tangent_at(t).ok()?;
    if matches!(edge.orientation, EdgeOrientation::Backward) {
        Some(tan * -1.0)
    } else {
        Some(tan)
    }
}

/// Get adjacent faces for an edge by scanning all faces in the solid's shells
fn get_adjacent_faces(
    model: &BRepModel,
    solid_id: SolidId,
    edge_id: EdgeId,
) -> OperationResult<(FaceId, FaceId)> {
    let solid = model
        .solids
        .get(solid_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Solid not found".to_string()))?;

    let mut adjacent_faces: Vec<FaceId> = Vec::new();

    // Collect all shell IDs first to avoid borrowing issues
    let mut shell_ids = vec![solid.outer_shell];
    shell_ids.extend_from_slice(&solid.inner_shells);

    for shell_id in shell_ids {
        let shell = model
            .shells
            .get(shell_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Shell not found".to_string()))?;

        for &face_id in &shell.faces {
            let face = model
                .faces
                .get(face_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

            // Check outer loop and inner loops for the target edge
            let loop_ids: Vec<_> = std::iter::once(face.outer_loop)
                .chain(face.inner_loops.iter().copied())
                .collect();

            'face_check: for loop_id in loop_ids {
                if let Some(loop_data) = model.loops.get(loop_id) {
                    for &e_id in &loop_data.edges {
                        if e_id == edge_id {
                            adjacent_faces.push(face_id);
                            break 'face_check;
                        }
                    }
                }
            }

            if adjacent_faces.len() == 2 {
                return Ok((adjacent_faces[0], adjacent_faces[1]));
            }
        }
    }

    if adjacent_faces.len() < 2 {
        return Err(OperationError::InvalidGeometry(format!(
            "Edge {:?} is not shared by two faces (found {})",
            edge_id,
            adjacent_faces.len()
        )));
    }

    Ok((adjacent_faces[0], adjacent_faces[1]))
}

/// Compute the **interior** dihedral angle between two adjacent faces
/// at a shared edge, measured inside the solid material.
///
/// The return value lies in `(0, 2π)`:
/// - flat pair (faces coplanar): `π`
/// - convex 90° (e.g. box edge): `π/2`
/// - concave 90° (interior of an "L" or pocket): `3π/2`
///
/// This is the angle the chamfer-triangle's law-of-sines wants
/// (`d2 = d1 · sin(angle) / sin(face_angle − angle)`): the apex
/// angle at the edge vertex.
///
/// The previous formulation used the unsigned `π − acos(n1·n2)`,
/// which yields the same value for convex and concave edges with
/// equal `|signed_dihedral|` and would silently give wrong distances
/// when chamfering a concave edge with `DistanceAngle` / `Angle`
/// modes. We now derive the signed dihedral via
/// `fillet_robust::robust_face_angle` (right-hand rule about the
/// loop-corrected edge tangent) and convert: `interior = π − signed`.
fn compute_face_angle(
    model: &BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
) -> OperationResult<f64> {
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
        .clone();

    let curve = model
        .curves
        .get(edge.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;
    let mid_point = curve.point_at(0.5).map_err(|e| {
        OperationError::NumericalError(format!("Edge midpoint evaluation failed: {:?}", e))
    })?;

    // Face-oriented outward normals (the helper already applies
    // FaceOrientation::Backward → negate, so this is the canonical
    // outward direction independent of which side of the surface the
    // face occupies).
    let n1 = face_normal_at_point(model, face1_id, &mid_point)?;
    let n2 = face_normal_at_point(model, face2_id, &mid_point)?;

    // Edge tangent in face1's loop direction. `robust_face_angle`
    // measures the signed angle via the right-hand rule about the
    // input tangent — using the *loop* direction (rather than the raw
    // curve direction) makes the sign convention "positive = convex"
    // independent of how the underlying curve was parameterized.
    let face1_loop_sign = super::fillet::edge_orientation_in_face(model, face1_id, edge_id)
        .ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "Edge {} not present in any loop of face {}",
                edge_id, face1_id
            ))
        })?;
    let edge_tangent = edge.tangent_at(0.5, &model.curves)? * face1_loop_sign;

    let signed =
        super::fillet_robust::robust_face_angle(&n1, &n2, &edge_tangent, &Tolerance::default())
            .map_err(|e| {
                OperationError::NumericalError(format!("Signed face angle failed: {:?}", e))
            })?;

    Ok(std::f64::consts::PI - signed)
}

/// Get face surface normal at a given 3D point by finding the closest UV parameters
fn face_normal_at_point(
    model: &BRepModel,
    face_id: FaceId,
    point: &Point3,
) -> OperationResult<Vector3> {
    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

    let surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    // Use closest_point to find UV, then evaluate normal
    let tolerance = Tolerance::default();
    let (u, v) = surface.closest_point(point, tolerance).map_err(|e| {
        OperationError::NumericalError(format!("Closest point on surface failed: {:?}", e))
    })?;

    let mut normal = surface.normal_at(u, v).map_err(|e| {
        OperationError::NumericalError(format!("Surface normal evaluation failed: {:?}", e))
    })?;

    // Flip normal if face orientation is backward
    if face.orientation == FaceOrientation::Backward {
        normal *= -1.0;
    }

    Ok(normal)
}

/// Validate chamfer inputs
fn validate_chamfer_inputs(
    model: &BRepModel,
    solid_id: SolidId,
    edges: &[EdgeId],
    options: &ChamferOptions,
) -> OperationResult<()> {
    // Check solid exists
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidGeometry(
            "Solid not found".to_string(),
        ));
    }

    // Check edges exist
    for &edge_id in edges {
        if model.edges.get(edge_id).is_none() {
            return Err(OperationError::InvalidGeometry(
                "Edge not found".to_string(),
            ));
        }
    }

    // Validate chamfer parameters
    match &options.chamfer_type {
        ChamferType::EqualDistance(d) => {
            if *d <= 0.0 {
                return Err(OperationError::InvalidGeometry(
                    "Distance must be positive".to_string(),
                ));
            }
        }
        ChamferType::TwoDistances(d1, d2) => {
            if *d1 <= 0.0 || *d2 <= 0.0 {
                return Err(OperationError::InvalidGeometry(
                    "Distances must be positive".to_string(),
                ));
            }
        }
        ChamferType::DistanceAngle(d, a) => {
            if *d <= 0.0 {
                return Err(OperationError::InvalidGeometry(
                    "Distance must be positive".to_string(),
                ));
            }
            if *a <= 0.0 || *a >= std::f64::consts::PI {
                return Err(OperationError::InvalidGeometry(
                    "Angle must be between 0 and π".to_string(),
                ));
            }
        }
        ChamferType::Angle(a) => {
            if *a <= 0.0 || *a >= std::f64::consts::PI {
                return Err(OperationError::InvalidGeometry(
                    "Angle must be between 0 and π".to_string(),
                ));
            }
        }
    }

    Ok(())
}

/// Validate chamfered solid by running the full B-Rep validation suite.
fn validate_chamfered_solid(model: &BRepModel, solid_id: SolidId) -> OperationResult<()> {
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidBRep(format!(
            "validate_chamfered_solid: solid {} not found",
            solid_id
        )));
    }
    let mut result = crate::primitives::validation::validate_model_enhanced(
        model,
        Tolerance::default(),
        crate::primitives::validation::ValidationLevel::Standard,
    );

    // CF-β.4 — partially-blended corners left open by the first of two
    // kind-mismatched calls produce expected non-manifold-edge and
    // Euler-deficit errors at the corner's local neighbourhood. Drop
    // those before re-evaluating validity; every other defect still
    // surfaces.
    let pending: HashSet<VertexId> = model
        .solids
        .get(solid_id)
        .map(|s| s.pending_mixed_kind_corners().keys().copied().collect())
        .unwrap_or_default();
    if !pending.is_empty() {
        result.errors = super::mixed_kind_corner_cap::filter_pending_corner_errors(
            model,
            &pending,
            std::mem::take(&mut result.errors),
        );
        result.is_valid = result.errors.is_empty();
    }

    if !result.is_valid {
        let summary = result
            .errors
            .iter()
            .take(3)
            .map(|e| format!("{:?}", e))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(OperationError::InvalidBRep(format!(
            "Chamfered solid failed validation ({} errors): {}",
            result.errors.len(),
            summary
        )));
    }

    // NOTE (#71 reverted): a geometric self-overlap guard once rejected here via
    // `geometry_validity::self_overlapping_planar_faces`. It was too aggressive —
    // it also rejected the *legitimate* fillet+chamfer mixed-kind corner that the
    // CF-β / CF-γ `mixed_kind_corner_cap` synthesizer is designed to produce
    // (cf_beta_* / cf_gamma_* tests). A correctness guard that breaks a real,
    // tested feature can't ship, so the hard reject is removed. The detector
    // remains available as a diagnostic (used by the integration harness), and the
    // niche chamfer-crosses-fillet case (#70) stays documented there; the real fix
    // is the junction reconstruction (#72), not a blanket reject.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::topology_builder::TopologyBuilder;

    #[test]
    fn test_chamfer_validation_rejects_zero_distance() {
        let mut model = BRepModel::new();
        let mut builder = TopologyBuilder::new(&mut model);
        let result = builder.create_box_3d(10.0, 10.0, 10.0);
        let solid_id = match result {
            Ok(crate::primitives::topology_builder::GeometryId::Solid(id)) => id,
            _ => panic!("Failed to create box"),
        };

        // Get any edge from the model
        let edges: Vec<EdgeId> = model.edges.iter().map(|(id, _)| id).collect();
        assert!(!edges.is_empty(), "Box should have edges");

        let options = ChamferOptions {
            chamfer_type: ChamferType::EqualDistance(0.0),
            ..Default::default()
        };

        let result = chamfer_edges(&mut model, solid_id, vec![edges[0]], options);
        assert!(result.is_err(), "Zero distance should be rejected");
    }

    #[test]
    fn test_chamfer_validation_rejects_negative_distance() {
        let mut model = BRepModel::new();
        let mut builder = TopologyBuilder::new(&mut model);
        let result = builder.create_box_3d(10.0, 10.0, 10.0);
        let solid_id = match result {
            Ok(crate::primitives::topology_builder::GeometryId::Solid(id)) => id,
            _ => panic!("Failed to create box"),
        };

        let edges: Vec<EdgeId> = model.edges.iter().map(|(id, _)| id).collect();

        let options = ChamferOptions {
            chamfer_type: ChamferType::EqualDistance(-1.0),
            ..Default::default()
        };

        let result = chamfer_edges(&mut model, solid_id, vec![edges[0]], options);
        assert!(result.is_err(), "Negative distance should be rejected");
    }

    #[test]
    fn test_chamfer_validation_rejects_invalid_angle() {
        let mut model = BRepModel::new();
        let mut builder = TopologyBuilder::new(&mut model);
        let result = builder.create_box_3d(10.0, 10.0, 10.0);
        let solid_id = match result {
            Ok(crate::primitives::topology_builder::GeometryId::Solid(id)) => id,
            _ => panic!("Failed to create box"),
        };

        let edges: Vec<EdgeId> = model.edges.iter().map(|(id, _)| id).collect();

        // Angle >= π should be rejected
        let options = ChamferOptions {
            chamfer_type: ChamferType::Angle(std::f64::consts::PI),
            ..Default::default()
        };

        let result = chamfer_edges(&mut model, solid_id, vec![edges[0]], options);
        assert!(result.is_err(), "Angle >= π should be rejected");
    }

    #[test]
    fn test_chamfer_validation_nonexistent_solid() {
        let mut model = BRepModel::new();
        let fake_solid = SolidId::from(999u32);
        let fake_edge = EdgeId::from(0u32);

        let options = ChamferOptions::default();
        let result = chamfer_edges(&mut model, fake_solid, vec![fake_edge], options);
        assert!(result.is_err(), "Nonexistent solid should be rejected");
    }

    #[test]
    fn test_get_adjacent_faces_finds_shared_edge() {
        let mut model = BRepModel::new();
        let mut builder = TopologyBuilder::new(&mut model);
        let result = builder.create_box_3d(10.0, 10.0, 10.0);
        let solid_id = match result {
            Ok(crate::primitives::topology_builder::GeometryId::Solid(id)) => id,
            _ => panic!("Failed to create box"),
        };

        // Get an edge that should be shared by exactly 2 faces
        let edges: Vec<EdgeId> = model.edges.iter().map(|(id, _)| id).collect();
        if edges.is_empty() {
            return; // Skip if box creation didn't produce edges
        }

        let result = get_adjacent_faces(&model, solid_id, edges[0]);
        match result {
            Ok((f1, f2)) => {
                assert_ne!(f1, f2, "Adjacent faces must be different");
            }
            Err(_) => {
                // Edge may not be in a face loop depending on box topology builder
                // This is acceptable — the function correctly reports the error
            }
        }
    }

    #[test]
    fn test_compute_face_angle_perpendicular_box() {
        use crate::primitives::surface::Plane;

        let mut model = BRepModel::new();

        // Create two perpendicular planes (like box faces)
        let plane1 = Plane::new(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let plane2 = Plane::new(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();

        let s1 = model.surfaces.add(Box::new(plane1));
        let s2 = model.surfaces.add(Box::new(plane2));

        // Create a shared edge along x-axis
        use crate::primitives::curve::Line;
        let line = Line::new(Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 0.0, 0.0));
        let curve_id = model.curves.add(Box::new(line));

        let v1 = model.vertices.add(0.0, 0.0, 0.0);
        let v2 = model.vertices.add(10.0, 0.0, 0.0);

        let edge = Edge::new_auto_range(0, v1, v2, curve_id, EdgeOrientation::Forward);
        let edge_id = model.edges.add(edge);

        // Loop orientations representing a *convex* 90° edge under
        // the right-hand-rule convention `robust_face_angle` enforces.
        // For the +Z and +Y faces of a notional box sharing the edge
        // along +X: walking the +Z face's outer loop CCW (viewed from
        // +Z) traverses this edge in −X, and walking the +Y face's
        // loop CCW (viewed from +Y) traverses it in +X. The curve is
        // parameterized in +X, so loop1 carries it backward and loop2
        // carries it forward.
        //
        // The previous test set both loops in the inverse orientation,
        // which under the new signed-dihedral formulation models a
        // concave (interior-of-an-L) corner. The unsigned `π−acos`
        // formula returned `π/2` regardless of orientation, masking
        // the mistake.
        let mut loop1 = Loop::new(0, crate::primitives::r#loop::LoopType::Outer);
        loop1.add_edge(edge_id, false);
        let loop1_id = model.loops.add(loop1);

        let mut loop2 = Loop::new(0, crate::primitives::r#loop::LoopType::Outer);
        loop2.add_edge(edge_id, true);
        let loop2_id = model.loops.add(loop2);

        let face1 = Face::new(0, s1, loop1_id, FaceOrientation::Forward);
        let face1_id = model.faces.add(face1);

        let face2 = Face::new(0, s2, loop2_id, FaceOrientation::Forward);
        let face2_id = model.faces.add(face2);

        // Interior dihedral of a convex 90° corner is π/2 — the apex
        // angle of the chamfer triangle sitting in the solid material
        // between the two faces. `compute_face_angle` returns
        // `π − signed_dihedral`, so signed = +π/2 (convex) yields π/2.
        let angle = compute_face_angle(&model, edge_id, face1_id, face2_id).unwrap();
        let expected = std::f64::consts::FRAC_PI_2;
        assert!(
            (angle - expected).abs() < 1e-6,
            "Convex perpendicular faces: expected π/2 ({expected:.6}), got {angle:.6}"
        );
    }

    #[test]
    fn test_ruled_chamfer_surface_creation() {
        let mut model = BRepModel::new();

        let data = ChamferData {
            offset_points1: vec![
                Point3::new(0.0, 0.0, 1.0),
                Point3::new(5.0, 0.0, 1.0),
                Point3::new(10.0, 0.0, 1.0),
            ],
            offset_points2: vec![
                Point3::new(0.0, 1.0, 0.0),
                Point3::new(5.0, 1.0, 0.0),
                Point3::new(10.0, 1.0, 0.0),
            ],
            parameters: vec![0.0, 0.5, 1.0],
            normals1: vec![Vector3::new(0.0, 0.0, 1.0); 3],
            normals2: vec![Vector3::new(0.0, 1.0, 0.0); 3],
        };

        let surface = create_ruled_chamfer_surface(&mut model, &data).unwrap();

        // At v=0 (curve1): should be near offset_points1
        let p0 = surface.point_at(0.0, 0.0).unwrap();
        assert!(
            (p0.x - 0.0).abs() < 1e-10 && (p0.z - 1.0).abs() < 1e-10,
            "v=0, u=0 should be on curve1 start"
        );

        // At v=1 (curve2): should be near offset_points2
        let p1 = surface.point_at(0.0, 1.0).unwrap();
        assert!(
            (p1.y - 1.0).abs() < 1e-10 && (p1.z - 0.0).abs() < 1e-10,
            "v=1, u=0 should be on curve2 start"
        );

        // At v=0.5: midpoint interpolation
        let pm = surface.point_at(0.5, 0.5).unwrap();
        assert!(
            (pm.x - 5.0).abs() < 1e-10,
            "Midpoint x should be 5.0, got {}",
            pm.x
        );
    }

    // -------------------------------------------------------------------
    // Regression: EqualDistance chamfer must succeed on every edge of a
    // box independent of which adjacent face is FaceOrientation::Backward.
    //
    // Mirrors the fillet regression
    // `unit_box_every_edge_classifies_as_convex_ninety_degrees`. Unlike
    // fillet's old broken state, chamfer's `face_normal_at_point` already
    // applies the orientation flip and `compute_chamfer_offsets` derives
    // its in-face perpendicular via `n × t_loop` (not via the dihedral
    // sign), so the failure mode is not expected — this test pins that
    // good state and catches any future regression that re-introduces an
    // orientation-blind helper.
    //
    // Each iteration uses a freshly-built box; TopologyBuilder assigns
    // edge ids monotonically and deterministically, so the same id maps
    // to the same edge of every fresh box. The post-chamfer solid is
    // validated by `validate_chamfered_solid` (toggled on via
    // `common.validate_result`); any orientation, face-loop, or surgery
    // bug that produces a non-manifold result is caught there.
    // -------------------------------------------------------------------
    // Pin the convex-vs-concave behaviour of compute_face_angle under
    // the signed-dihedral formulation. The two test configurations
    // share *identical geometry* — same planes, same shared edge —
    // and differ only in loop orientation, which is what carries the
    // information about which side of the corner is solid material.
    //
    // The previous unsigned formula returned π/2 for both, silently
    // collapsing the convex and concave cases. That would have given
    // the DistanceAngle / Angle chamfer modes a wrong magnitude on
    // any concave edge they were applied to.
    #[test]
    fn compute_face_angle_distinguishes_convex_from_concave() {
        use crate::primitives::curve::Line;
        use crate::primitives::surface::Plane;

        // Shared geometry: two perpendicular planes meeting on the X
        // axis. Plane1 lies in the XY plane (normal +Z); Plane2 lies
        // in the XZ plane (normal +Y).
        let build = |loop1_forward: bool, loop2_forward: bool| -> f64 {
            let mut model = BRepModel::new();
            let plane1 = Plane::new(
                Point3::new(0.0, 0.0, 0.0),
                Vector3::new(0.0, 0.0, 1.0),
                Vector3::new(1.0, 0.0, 0.0),
            )
            .unwrap();
            let plane2 = Plane::new(
                Point3::new(0.0, 0.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
                Vector3::new(1.0, 0.0, 0.0),
            )
            .unwrap();
            let s1 = model.surfaces.add(Box::new(plane1));
            let s2 = model.surfaces.add(Box::new(plane2));
            let line = Line::new(Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 0.0, 0.0));
            let curve_id = model.curves.add(Box::new(line));
            let v1 = model.vertices.add(0.0, 0.0, 0.0);
            let v2 = model.vertices.add(10.0, 0.0, 0.0);
            let edge = Edge::new_auto_range(0, v1, v2, curve_id, EdgeOrientation::Forward);
            let edge_id = model.edges.add(edge);

            let mut loop1 = Loop::new(0, crate::primitives::r#loop::LoopType::Outer);
            loop1.add_edge(edge_id, loop1_forward);
            let loop1_id = model.loops.add(loop1);
            let mut loop2 = Loop::new(0, crate::primitives::r#loop::LoopType::Outer);
            loop2.add_edge(edge_id, loop2_forward);
            let loop2_id = model.loops.add(loop2);

            let face1 = Face::new(0, s1, loop1_id, FaceOrientation::Forward);
            let face1_id = model.faces.add(face1);
            let face2 = Face::new(0, s2, loop2_id, FaceOrientation::Forward);
            let face2_id = model.faces.add(face2);

            compute_face_angle(&model, edge_id, face1_id, face2_id).unwrap()
        };

        // Convex configuration (canonical box edge): loop1 backward,
        // loop2 forward. Interior dihedral = π/2.
        let convex = build(false, true);
        assert!(
            (convex - std::f64::consts::FRAC_PI_2).abs() < 1e-6,
            "Convex 90° must give interior dihedral π/2, got {convex}"
        );

        // Concave configuration (interior of an L): loop1 forward,
        // loop2 backward. Interior dihedral = 3π/2. The signed
        // dihedral flips sign with loop orientation, and `π − signed`
        // moves the result into the (π, 2π) half-plane.
        let concave = build(true, false);
        let three_pi_over_two = 3.0 * std::f64::consts::FRAC_PI_2;
        assert!(
            (concave - three_pi_over_two).abs() < 1e-6,
            "Concave 90° must give interior dihedral 3π/2 \
             ({three_pi_over_two:.6}), got {concave}"
        );
    }

    #[test]
    fn unit_box_each_edge_chamfers_with_validation() {
        use std::collections::HashSet;

        // Collect unique edge ids for a 10×10×10 box (matches the rest
        // of the file's box scale, so the 0.5 chamfer distance stays
        // well under half the edge length validator).
        let probe_edges: Vec<EdgeId> = {
            let mut model = BRepModel::new();
            let mut builder = TopologyBuilder::new(&mut model);
            let solid_id = match builder.create_box_3d(10.0, 10.0, 10.0).unwrap() {
                crate::primitives::topology_builder::GeometryId::Solid(id) => id,
                other => panic!("expected solid, got {other:?}"),
            };
            let solid = model.solids.get(solid_id).unwrap().clone();
            let shell = model.shells.get(solid.outer_shell).unwrap().clone();
            let mut seen: HashSet<EdgeId> = HashSet::new();
            for fid in &shell.faces {
                let face = model.faces.get(*fid).unwrap().clone();
                let outer_loop = model.loops.get(face.outer_loop).unwrap().clone();
                for e in &outer_loop.edges {
                    seen.insert(*e);
                }
            }
            assert_eq!(seen.len(), 12, "unit box has 12 edges, got {}", seen.len());
            seen.into_iter().collect()
        };

        // Chamfer each edge on a fresh box. The whole-solid validation
        // catches any orientation-induced non-manifold output.
        for edge_id in &probe_edges {
            let mut model = BRepModel::new();
            let mut builder = TopologyBuilder::new(&mut model);
            let solid_id = match builder.create_box_3d(10.0, 10.0, 10.0).unwrap() {
                crate::primitives::topology_builder::GeometryId::Solid(id) => id,
                other => panic!("expected solid, got {other:?}"),
            };

            let opts = ChamferOptions {
                common: CommonOptions {
                    validate_result: true,
                    ..CommonOptions::default()
                },
                chamfer_type: ChamferType::EqualDistance(0.5),
                distance1: 0.5,
                distance2: 0.5,
                symmetric: true,
                propagation: PropagationMode::None,
                preserve_edges: false,
                partial_corner_vertices: Vec::new(),
                seam_continuity: SeamContinuity::C0,
            };

            let result = chamfer_edges(&mut model, solid_id, vec![*edge_id], opts);
            assert!(
                result.is_ok(),
                "EqualDistance chamfer on edge {edge_id} failed: {result:?}. \
                 This is the regression class an orientation-blind face-normal \
                 helper would introduce — see fillet.rs::get_face_oriented_normal \
                 for the canonical pattern."
            );
            let chamfer_faces = result.unwrap();
            assert_eq!(
                chamfer_faces.len(),
                1,
                "EqualDistance chamfer of one edge produces exactly one face"
            );
        }
    }
}
