//! Fillet Operations for B-Rep Models
//!
//! Creates smooth rounded transitions between faces (edge fillets) and
//! at vertices (vertex fillets/balls).
//!
//! # References
//! - Choi, B.K. & Ju, S.Y. (1989). Constant-radius blending in surface modeling. CAD.
//! - Vida, J. et al. (1994). A survey of blending methods using parametric surfaces. CAD.
//!
//! Indexed access into edge-list and surface-sample arrays is the canonical
//! idiom for fillet construction — all `arr[i]` sites use indices bounded
//! by edge count or sample density. Matches the numerical-kernel pattern
//! used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::blend_graph::{self, BlendGraph, BlendRadius, BlendVertexKind, EdgeFilletProfile};
use super::diagnostics::{BlendFailure, VertexBlendUnsupportedReason};
use super::edge_blend_topology::{splice_blend_edge, BlendEdgeSurgery};
use super::feasibility;
use super::lifecycle::{self, OpSpec};
use super::orientation::orient_face_for_outward;
use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Matrix3, Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::{Arc, Curve, Line, ParameterRange},
    edge::{Edge, EdgeId, EdgeOrientation},
    face::{Face, FaceId},
    fillet_surfaces::{CylindricalFillet, ToroidalFillet, VariableRadiusFillet},
    r#loop::{Loop, LoopType},
    solid::{BlendKind, SolidId},
    surface::{Cylinder, Sphere, Surface},
    topology_builder::BRepModel,
    vertex::VertexId,
};
use std::collections::{HashMap, HashSet};

// Import robust numerical methods
use super::fillet_robust::*;

/// Options for fillet operations
#[derive(Debug, Clone)]
pub struct FilletOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Type of fillet
    pub fillet_type: FilletType,

    /// Convenience radius field for constant fillets
    pub radius: f64,

    /// Propagation mode for edge selection
    pub propagation: PropagationMode,

    /// Whether to preserve sharp edges where fillets meet
    pub preserve_edges: bool,

    /// Quality level (affects tessellation)
    pub quality: FilletQuality,
}

impl Default for FilletOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            fillet_type: FilletType::Constant(5.0),
            radius: 5.0,
            propagation: PropagationMode::Tangent,
            preserve_edges: true,
            quality: FilletQuality::Standard,
        }
    }
}

/// Type of fillet
pub enum FilletType {
    /// Constant radius along edge
    Constant(f64),
    /// Variable radius interpolated linearly between the start and end
    /// of the edge. Equivalent to `VariableStations(vec![(0.0, start),
    /// (1.0, end)])`; kept as a distinct variant because the linear
    /// case is by far the most common and avoids a heap allocation.
    Variable(f64, f64),
    /// Variable radius with explicit control stations along the edge
    /// parameter. Each entry is `(station, radius)` with `station ∈
    /// [0, 1]`. The kernel interpolates linearly between adjacent
    /// stations. F3-ε.1 plumbs this through `spine_solver`; F3-ε.2
    /// exposes it on the kernel surface so timeline / REST / AI can
    /// drive a true per-station variable fillet (e.g. a tear-drop
    /// where the radius peaks mid-edge instead of monotonically
    /// growing).
    ///
    /// Invariants enforced by `validate_fillet_inputs`:
    /// - Non-empty.
    /// - Stations strictly increasing.
    /// - First station ≥ 0.0, last ≤ 1.0.
    /// - Every radius > 0.
    VariableStations(Vec<(f64, f64)>),
    /// Per-edge constant radii. Each entry in the map binds a single
    /// selected edge to its own constant radius. F5-β.5 lifts the
    /// public-API restriction that forced every edge in a single
    /// `fillet_edges` call to share the same radius; this is the
    /// minimal variant — one constant value per edge.
    ///
    /// Invariants enforced by `validate_fillet_inputs`:
    /// - Map contains exactly one entry per edge in the selection.
    /// - No "extra" entries for edges not in the selection.
    /// - Every radius is finite, strictly positive, and exceeds the
    ///   caller-supplied `Tolerance::distance()`.
    ///
    /// Mixed-radii three-edge corners feed `apply_triangular_nurbs_
    /// corner` via the existing F5-β.3 dispatch in
    /// `create_fillet_transitions`; equal-radii corners still route
    /// to `apply_apex_sphere_corner`. Variable / chord per-edge mixes
    /// land in F5-β.5.6+.
    PerEdgeConstant(HashMap<EdgeId, f64>),
    /// Per-edge variable profiles. Each edge can carry its own
    /// [`EdgeFilletProfile`] — wrapping a `BlendRadius`
    /// (`Constant` / `Linear` / `Variable`) or a raw chord length
    /// (`Chord`). Used when a single fillet call mixes different
    /// *kinds* of radii across edges (F5-β.5.6) or mixes chord
    /// profiles with radius profiles (F5-β.5.7). The all-Constant
    /// case stays on [`FilletType::PerEdgeConstant`] to keep the
    /// cheap single-`f64` shape; this variant is reserved for
    /// selections that include at least one non-Constant or chord
    /// profile.
    ///
    /// Invariants enforced by `validate_fillet_inputs`:
    /// - Map contains exactly one entry per edge in the selection.
    /// - No extra entries for edges not in the selection.
    /// - Each `Radius(Constant(r))` is finite, > 0, and >
    ///   `Tolerance::distance`.
    /// - Each `Radius(Linear { start, end })` has both endpoints
    ///   finite, strictly positive, and > tolerance.
    /// - Each `Radius(Variable(samples))` satisfies the same
    ///   invariants as [`FilletType::VariableStations`] (delegated
    ///   to `validate_variable_stations`).
    /// - Each `Chord(c)` is finite, > 0, and > `Tolerance::distance`.
    ///
    /// The dispatcher in `create_fillet_chain` fans each edge out
    /// to the matching legacy creation function:
    /// - `Radius(Constant)` → `create_constant_radius_fillet`
    /// - `Radius(Linear)` → `create_variable_radius_fillet`
    /// - `Radius(Variable)` → `create_function_radius_fillet`
    /// - `Chord` → `create_chord_fillet`
    PerEdgeProfile(HashMap<EdgeId, EdgeFilletProfile>),
    /// Radius function along edge parameter
    Function(Box<dyn Fn(f64) -> f64>),
    /// Chord length fillet
    Chord(f64),
}

impl std::fmt::Debug for FilletType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FilletType::Constant(r) => f.debug_tuple("Constant").field(r).finish(),
            FilletType::Variable(r1, r2) => f.debug_tuple("Variable").field(r1).field(r2).finish(),
            FilletType::VariableStations(samples) => {
                f.debug_tuple("VariableStations").field(samples).finish()
            }
            FilletType::PerEdgeConstant(map) => {
                // Iterate in sorted edge-id order so the Debug output
                // is stable across runs (HashMap iteration order is
                // randomised). The recorded-operation JSON uses this
                // formatter, and timeline replay diffs are easier
                // when the textual form is deterministic.
                let mut entries: Vec<(&EdgeId, &f64)> = map.iter().collect();
                entries.sort_by_key(|(eid, _)| **eid);
                f.debug_tuple("PerEdgeConstant").field(&entries).finish()
            }
            FilletType::PerEdgeProfile(map) => {
                // Same determinism rationale as `PerEdgeConstant`:
                // sort by edge id so recorded-operation JSON and
                // timeline replay diffs are stable regardless of
                // HashMap insertion order.
                let mut entries: Vec<(&EdgeId, &EdgeFilletProfile)> = map.iter().collect();
                entries.sort_by_key(|(eid, _)| **eid);
                f.debug_tuple("PerEdgeProfile").field(&entries).finish()
            }
            FilletType::Function(_) => f.debug_tuple("Function").field(&"<function>").finish(),
            FilletType::Chord(c) => f.debug_tuple("Chord").field(c).finish(),
        }
    }
}

impl Clone for FilletType {
    fn clone(&self) -> Self {
        match self {
            FilletType::Constant(r) => FilletType::Constant(*r),
            FilletType::Variable(r1, r2) => FilletType::Variable(*r1, *r2),
            FilletType::VariableStations(samples) => FilletType::VariableStations(samples.clone()),
            FilletType::PerEdgeConstant(map) => FilletType::PerEdgeConstant(map.clone()),
            FilletType::PerEdgeProfile(map) => FilletType::PerEdgeProfile(map.clone()),
            FilletType::Function(_) => FilletType::Constant(5.0), // Fallback to constant
            FilletType::Chord(c) => FilletType::Chord(*c),
        }
    }
}

/// How to propagate fillet selection
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PropagationMode {
    /// No propagation
    None,
    /// Propagate along tangent edges
    Tangent,
    /// Propagate along smooth (G1) edges
    Smooth,
    /// Propagate all connected edges
    All,
}

/// Fillet quality/tessellation level
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FilletQuality {
    /// Fast computation, lower quality
    Draft,
    /// Standard quality
    Standard,
    /// High quality for final models
    High,
}

/// Apply fillet to edges
pub fn fillet_edges(
    model: &mut BRepModel,
    solid_id: SolidId,
    edges: Vec<EdgeId>,
    options: FilletOptions,
) -> OperationResult<Vec<FaceId>> {
    // F2-δ pre-flight: cheap input validation + setback-aware
    // corner compatibility (replaces the historical
    // `validate_no_shared_corners` blanket reject). Atomic — the
    // model is untouched if pre-flight fails.
    if options.common.validate_before {
        lifecycle::validate_can_apply(
            model,
            OpSpec::FilletEdges {
                solid_id,
                edges: &edges,
            },
        )?;

        // F6-α: radius vs. curvature feasibility gate. Catches the
        // common "rolling ball larger than the cylinder / sphere it
        // sits against" case before the spine solver burns its
        // marching iteration budget diverging on an infeasible
        // request. Conservative — analytic surfaces only; cones,
        // ruled, and NURBS surfaces are pass-through pending F6-β
        // sampling-based curvature evaluation. Failures surface as
        // the typed Diagnostics-α Phase-2
        // `OperationError::BlendFailed(BlendFailure::RadiusExceedsCurvature)`
        // so callers can recover with `r ≤ r_max * 0.95` without
        // string-parsing.
        let max_radius = match &options.fillet_type {
            FilletType::Constant(r) => *r,
            FilletType::Variable(r1, r2) => r1.max(*r2),
            // Per-station upper bound is the largest sample radius;
            // F6-α uses it to gate against the analytic curvature
            // limit. Empty list is rejected later by
            // `validate_fillet_inputs`; here we treat it as 0.0 so
            // F6-α is a no-op and the input validator owns the
            // rejection.
            FilletType::VariableStations(samples) => samples
                .iter()
                .map(|&(_, r)| r)
                .fold(0.0_f64, f64::max),
            // F5-β.5: per-edge map's largest constant radius. Empty
            // / missing-edge cases are rejected by
            // `validate_fillet_inputs`; treating the empty fold
            // identity as 0.0 keeps F6-α a no-op and lets the input
            // validator own the rejection.
            FilletType::PerEdgeConstant(map) => map
                .values()
                .copied()
                .fold(0.0_f64, f64::max),
            // F5-β.5.6/.7: per-edge profile's largest *sample* radius
            // across every profile shape. `EdgeFilletProfile::
            // max_radius_bound` normalises Radius(Constant/Linear/
            // Variable) to its inner `BlendRadius::max_value` and
            // reports 0.0 for `Chord` (chord can produce arbitrarily
            // large radii at small dihedrals, so F6-α has no
            // closed-form bound for it — same skip behaviour as
            // top-level `FilletType::Chord`). Empty fold identity is
            // 0.0 so an empty map degrades F6-α to a no-op and lets
            // `validate_fillet_inputs` own the rejection.
            FilletType::PerEdgeProfile(map) => map
                .values()
                .map(|p| p.max_radius_bound())
                .fold(0.0_f64, f64::max),
            // `Function` and `Chord` paths don't have a closed-form
            // upper bound here; F6-α leaves them to the existing
            // downstream validation. Sampling them is F6-β.
            FilletType::Function(_) | FilletType::Chord(_) => 0.0,
        };
        if max_radius > 0.0 {
            feasibility::validate_radius_against_curvature(model, &edges, max_radius)
                .map_err(|f| OperationError::BlendFailed(Box::new(f)))?;
        }
    }

    // F2-δ transactional wrapper: any Err out of the body restores
    // the pre-call snapshot so the caller sees an unchanged model.
    lifecycle::with_rollback(model, move |model| {
        // Validate inputs
        validate_fillet_inputs(model, solid_id, &edges, &options)?;

        // Capture input edges before `edges` is consumed by the
        // propagation step below — needed for the recorder payload.
        let input_edges_for_record: Vec<u32> = edges.clone();

        // CF-α: snapshot endpoint vertex IDs of every requested edge
        // *before* `splice_blend_edge` destroys the edge (and, when
        // `corner_shared` is not set, the vertex too). After surgery
        // we test each ID against `model.vertices` to record only the
        // vertices that survived, populating
        // `Solid::blended_vertices` so the CF-α pre-flight gate can
        // detect shared-corner cross-kind conflicts in subsequent
        // blend calls.
        let input_edge_endpoints: Vec<VertexId> = edges
            .iter()
            .filter_map(|&eid| model.edges.get(eid))
            .flat_map(|e| [e.start_vertex, e.end_vertex])
            .collect();

        // Additional robust validation. For variable-radius fillets we
        // must check both endpoint radii — the linear interpolant means
        // either end can independently violate the half-edge-length bound,
        // and rejecting only `r1` (as the original loop did) lets a
        // pathological `r2` slip through to surgery time where it would
        // surface a less actionable error.
        // Per-station shape requires structural validation before
        // we walk the radii: empty list, out-of-order stations,
        // stations outside [0, 1], or non-positive radii must surface
        // as `InvalidInput` here (caller-side problem), not deep in
        // the surgery loop where the diagnostic would point at the
        // wrong layer.
        if let FilletType::VariableStations(samples) = &options.fillet_type {
            validate_variable_stations(samples)?;
        }

        for &edge_id in &edges {
            let radii_to_check: Vec<f64> = match &options.fillet_type {
                FilletType::Constant(r) => vec![*r],
                FilletType::Variable(r1, r2) => vec![*r1, *r2],
                FilletType::VariableStations(samples) => {
                    samples.iter().map(|&(_, r)| r).collect()
                }
                // F5-β.5: pick this edge's per-edge radius out of the
                // map for half-edge-length bounds-checking. Missing
                // keys are rejected by `validate_fillet_inputs`
                // upstream; the fallback to the legacy
                // `options.radius` here is purely defensive — if
                // validation passes, the key is present.
                FilletType::PerEdgeConstant(map) => {
                    vec![map.get(&edge_id).copied().unwrap_or(options.radius)]
                }
                // F5-β.5.6/.7: per-edge profile — feed every sample
                // of *this* edge's profile (not the global max) into
                // the half-edge-length bounds check. `Radius(Constant)`
                // → one sample, `Radius(Linear)` → two endpoints,
                // `Radius(Variable)` → every station radius, `Chord`
                // → the raw chord value itself (a chord of length `c`
                // connects two cap points at distance `c` apart along
                // the spine, so `c <= half_edge_length` is the same
                // conservative guard the scalar-radius arms apply).
                // Missing keys are rejected by `validate_fillet_inputs`
                // upstream; the fallback to `options.radius` here is
                // purely defensive — if validation passes, the key is
                // present.
                FilletType::PerEdgeProfile(map) => match map.get(&edge_id) {
                    Some(EdgeFilletProfile::Radius(BlendRadius::Constant(r))) => vec![*r],
                    Some(EdgeFilletProfile::Radius(BlendRadius::Linear { start, end })) => {
                        vec![*start, *end]
                    }
                    Some(EdgeFilletProfile::Radius(BlendRadius::Variable(samples))) => {
                        samples.iter().map(|&(_, r)| r).collect()
                    }
                    Some(EdgeFilletProfile::Chord(c)) => vec![*c],
                    None => vec![options.radius],
                },
                // Function radii are validated per-sample inside the
                // surgery loop; the placeholder of 1.0 only exercises the
                // structural bounds here (edge length non-zero, edge
                // exists).
                FilletType::Function(_) => vec![1.0],
                FilletType::Chord(c) => vec![*c],
            };
            for radius in radii_to_check {
                validate_fillet_parameters(model, edge_id, radius, &options.common.tolerance)?;
            }
        }

        // Get radius value(s)
        let radius = match &options.fillet_type {
            FilletType::Constant(r) => *r,
            FilletType::Variable(r1, _) => *r1, // Use start radius for validation
            // Use the first station's radius as the representative
            // value for the legacy `radius` validation gate. The
            // full curve has already been bounds-checked above; this
            // path only guards against the `radius <= 0` rejection
            // that comes next.
            FilletType::VariableStations(samples) => samples
                .first()
                .map(|&(_, r)| r)
                .unwrap_or(0.0),
            // F5-β.5: pick the smallest per-edge radius as the
            // representative for the legacy `radius > 0.0` gate.
            // Smallest, not largest, so a single zero / negative
            // entry can't be masked by a larger sibling — every
            // per-edge entry has already been individually
            // bounds-checked above against the half-edge-length
            // limit. Empty maps short-circuit to 0.0, which
            // triggers the `radius <= 0.0` rejection two lines
            // below; structurally the empty case is also rejected
            // earlier by `validate_fillet_inputs`.
            FilletType::PerEdgeConstant(map) => {
                if map.is_empty() {
                    0.0
                } else {
                    map.values().copied().fold(f64::INFINITY, f64::min)
                }
            }
            // F5-β.5.6: smallest per-edge profile minimum. Folding
            // `min_value()` (not `max_value()`) ensures a single
            // zero / negative sample anywhere on any edge's profile
            // can't be masked by a larger sibling — each profile
            // has already been individually bounds-checked above.
            // Empty maps short-circuit to 0.0, which trips the
            // `radius <= 0.0` rejection below; the empty case is
            // also rejected structurally by `validate_fillet_inputs`.
            FilletType::PerEdgeProfile(map) => {
                if map.is_empty() {
                    0.0
                } else {
                    map.values()
                        .map(|b| b.min_radius_bound())
                        .fold(f64::INFINITY, f64::min)
                }
            }
            FilletType::Function(_) => 0.0, // Will validate per point
            FilletType::Chord(c) => *c,
        };

        // Check radius validity
        if radius <= 0.0 {
            return Err(OperationError::InvalidRadius(radius));
        }

        // Propagate edge selection if requested
        let selected_edges = propagate_edge_selection(model, edges, options.propagation)?;

        // F3-δ.4: build the F2-β blend graph for the full selection.
        // The graph carries per-edge convexity / dihedral / manifold-
        // kind (F2-α cached classification), plus the per-corner
        // setbacks computed by F2-γ. The constant-radius spine path
        // consults it to retract spine endpoints at shared corners.
        // For today's selections (no shared corners — still rejected
        // by `validate_no_shared_corners` until F5 lands corner
        // patches) every BlendEdge has setbacks = None, so the trim
        // collapses to ParamTrim::FULL and behaviour is identical
        // to the pre-F3-δ.4 path. The wiring is the foundation that
        // F4/F5 corner-blending will activate.
        let blend_selection: Vec<(EdgeId, BlendRadius)> = selected_edges
            .iter()
            .map(|&eid| (eid, fillet_type_to_blend_radius(&options.fillet_type, eid)))
            .collect();
        let mut blend_graph = blend_graph::build(model, &blend_selection)?;
        blend_graph::compute_setbacks(model, &mut blend_graph)?;
        // F5-α.3: refine setbacks at `ConvexCorner { degree: 3 }`
        // apex corners. The Hoffmann smooth-closure value from
        // `compute_setbacks` is correct for a two-edge corner whose
        // adjacent cylinders meet tangentially; for apex-sphere
        // termination each cylinder spine must retract all the way
        // to the rolling-ball centre `C` so the V-side cap arc lands
        // on the apex. For a rectilinear box corner this overrides
        // `r·cos(π/4) ≈ 0.707·r` with `r`; corners outside the
        // F5-α scope (non-planar adjacencies, mixed radii, rank-
        // deficient axes) are silently left at the Hoffmann
        // baseline.
        blend_graph::compute_apex_setbacks(model, &mut blend_graph)?;

        // F5-α: snapshot the original sharp-vertex position for every
        // BlendGraph corner *before* per-edge fillets retract their
        // spines and `update_adjacent_faces` splices the neighbourhood.
        // After the chain pass the original vertex may still exist in
        // the model but with disconnected incident edges; reading the
        // position now keeps the apex-sphere outward direction stable
        // regardless of how the surgery resolves the corner cleanup.
        let corner_positions: HashMap<VertexId, Point3> = blend_graph
            .corners()
            .filter_map(|c| {
                model.vertices.get(c.id).map(|v| {
                    let p = v.position;
                    (c.id, Point3::new(p[0], p[1], p[2]))
                })
            })
            .collect();

        // Group edges into fillet chains
        let edge_chains = group_edges_into_chains(model, &selected_edges)?;

        // Create fillet surfaces for each chain
        let mut fillet_faces: Vec<FaceId> = Vec::new();
        let mut edge_to_face: HashMap<EdgeId, FaceId> = HashMap::new();
        let mut surgeries = Vec::new();
        for chain in edge_chains {
            let (chain_edge_faces, chain_surgeries) =
                create_fillet_chain(model, solid_id, chain, &options, &blend_graph)?;
            for (edge_id, face_id) in &chain_edge_faces {
                edge_to_face.insert(*edge_id, *face_id);
                fillet_faces.push(*face_id);
            }
            surgeries.extend(chain_surgeries);
        }

        // Re-stitch the surrounding topology and add fillet faces to the
        // outer shell so the resulting B-Rep is watertight.
        update_adjacent_faces(model, solid_id, &fillet_faces, &surgeries)?;

        // F5-α: corner-blend dispatch. After per-edge fillets have been
        // spliced in (so their cap arcs are committed to the shell),
        // emit the corner-sphere face for every BlendGraph corner the
        // dispatcher recognises (today: ConvexCorner{degree:3} with
        // equal radius + concurrent axes). Other corner kinds pass
        // through; supported kinds outside today's MVP surface as
        // typed BlendFailure::VertexBlendUnsupported.
        let corner_faces =
            create_fillet_transitions(model, solid_id, &blend_graph, &edge_to_face, &corner_positions)?;
        fillet_faces.extend(corner_faces);

        // Validate result if requested
        if options.common.validate_result {
            validate_filleted_solid(model, solid_id)?;
        }

        // CF-α: populate the per-solid blend registry. Edges are
        // recorded by their pre-surgery IDs (they no longer exist in
        // `model.edges` — `splice_blend_edge` destroyed them — but
        // the registry uses the ID as a stable key for the
        // `validate_blend_conflict` gate, which fires *before*
        // `check_edges_exist`). Endpoint vertices are recorded only
        // when they survived surgery (the F5-α `corner_shared` path);
        // others were also destroyed and would never be re-queried.
        // Pre-collect the surviving vertex set under an immutable
        // borrow so the subsequent `solids.get_mut` doesn't fight
        // the borrow checker.
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
                solid.record_blended_edge(eid, BlendKind::Fillet);
            }
            for vid in surviving_endpoints {
                solid.record_blended_vertex(vid, BlendKind::Fillet);
            }
            // CF-β.3 — tag every emitted fillet face (transition +
            // corner patch) with `BlendKind::Fillet` so the
            // mixed-kind corner cap synthesizer can locate the
            // surviving fillet faces at a shared corner without
            // re-deriving the classification from surface geometry.
            for &fid in &fillet_faces {
                solid.record_blend_face(fid, BlendKind::Fillet);
            }
        }

        // Record for attached recorders. `inputs` lists the user-supplied
        // edges (not the propagated superset — that's a derived detail).
        // `outputs` leads with `solid_id` so the lineage graph treats this
        // fillet as the new "producer" of the modified body — downstream
        // ops (shell-after-fillet, chamfer-after-fillet, …) then parent to
        // this event instead of jumping past it back to the primitive. The
        // generated fillet faces follow so the recorded op still names the
        // topology it introduced.
        model.record_operation(
            crate::operations::recorder::RecordedOperation::new("fillet_edges")
                .with_parameters(serde_json::json!({
                    "solid_id": solid_id,
                    "fillet_type": format!("{:?}", options.fillet_type),
                    "radius": options.radius,
                    "propagation": format!("{:?}", options.propagation),
                    "preserve_edges": options.preserve_edges,
                    "quality": format!("{:?}", options.quality),
                }))
                .with_input_solids([solid_id as u64])
                .with_input_edges(input_edges_for_record.iter().map(|&e| e as u64))
                .with_output_solids([solid_id as u64])
                .with_output_faces(fillet_faces.iter().map(|&f| f as u64)),
        );

        // Drop the solid's cached mass-properties — volume, surface area,
        // COM and inertia tensor all changed when the blend faces were
        // spliced in. Without this, `calculate_solid_volume` returns the
        // pre-fillet figure, and `/api/agent/parts/{id}/mass` reports
        // stale data until the solid is rebuilt.
        if let Some(solid) = model.solids.get_mut(solid_id) {
            solid.invalidate_mass_props_cache();
        }

        Ok(fillet_faces)
    })
}

/// Create a fillet chain along connected edges.
///
/// Returns the new fillet face IDs alongside the per-edge
/// `BlendEdgeSurgery` data (one entry per 4-sided fillet face — the
/// 3-sided degenerate path returns no surgery). The surgery list is
/// passed downstream to `update_adjacent_faces` for topology
/// re-stitching.
/// Translate a [`FilletType`] into the [`BlendRadius`] shape that
/// [`blend_graph::build`] expects. F3-δ.4 wires the BlendGraph into
/// the constant-radius spine path; non-constant types are passed
/// through with their best-effort BlendRadius mapping so the graph
/// classification (convexity / dihedral / manifold-kind) still
/// covers every selected edge, even though the variable / function /
/// chord paths don't yet consult the graph for setback retraction.
fn fillet_type_to_blend_radius(fillet_type: &FilletType, edge_id: EdgeId) -> BlendRadius {
    match fillet_type {
        FilletType::Constant(r) => BlendRadius::Constant(*r),
        FilletType::Variable(r1, r2) => BlendRadius::Linear {
            start: *r1,
            end: *r2,
        },
        // F3-ε.2: per-station variable radius maps directly to the
        // kernel's `BlendRadius::Variable` shape, which `spine_solver`
        // already consumes (F3-ε.1). The samples are passed verbatim;
        // structural invariants (non-empty, monotone, in [0,1])
        // were enforced upstream by `validate_variable_stations`.
        FilletType::VariableStations(samples) => BlendRadius::Variable(samples.clone()),
        // F5-β.5: per-edge constant looked up by edge id. Missing
        // keys are rejected by `validate_fillet_inputs` upstream;
        // the fallback to `Constant(0.0)` here is defensive and
        // would propagate as an `InvalidRadius(0.0)` downstream.
        FilletType::PerEdgeConstant(map) => {
            BlendRadius::Constant(map.get(&edge_id).copied().unwrap_or(0.0))
        }
        // F5-β.5.6/.7: per-edge profile. For `Radius(_)` we forward
        // the inner `BlendRadius` so the graph classifies the edge
        // by its actual schedule. `Chord(c)` has no closed-form
        // radius without the local dihedral (unavailable at graph-
        // build time), so we report `Constant(c)` as a placeholder
        // — matching the existing top-level `FilletType::Chord(c)`
        // behaviour. The actual chord → radius conversion runs
        // later at surgery time inside `create_chord_fillet`.
        // Missing keys are rejected by `validate_fillet_inputs`
        // upstream; the fallback to `Constant(0.0)` here is
        // defensive and would propagate as an `InvalidRadius(0.0)`
        // downstream.
        FilletType::PerEdgeProfile(map) => match map.get(&edge_id) {
            Some(EdgeFilletProfile::Radius(b)) => b.clone(),
            Some(EdgeFilletProfile::Chord(c)) => BlendRadius::Constant(*c),
            None => BlendRadius::Constant(0.0),
        },
        // The Function and Chord paths don't expose a closed-form
        // sampling here; report Constant(1.0) as a placeholder so
        // the edge is still classified into the graph. F4 will
        // refine these when variable-radius fillets land.
        FilletType::Function(_) => BlendRadius::Constant(1.0),
        FilletType::Chord(c) => BlendRadius::Constant(*c),
    }
}

/// Validate the structural invariants of a
/// `FilletType::VariableStations` payload. Failure surfaces as
/// `OperationError::InvalidInput` because the problem is in the
/// caller-supplied selection, not in the topology.
///
/// Rules:
/// - Non-empty.
/// - Stations strictly increasing.
/// - First station ≥ 0.0, last ≤ 1.0.
/// - Every radius > 0 and finite.
fn validate_variable_stations(samples: &[(f64, f64)]) -> OperationResult<()> {
    validate_variable_stations_labelled(samples, "fillet_type.VariableStations")
}

/// Same structural checks as [`validate_variable_stations`] but with
/// a caller-supplied `label` so the `parameter` field on the
/// resulting `InvalidInput` error reads
/// `<label>[<i>].station` instead of
/// `fillet_type.VariableStations[<i>].station`.
///
/// Used by `PerEdgeProfile` validation so the failing parameter
/// names the offending edge (e.g.
/// `fillet_type.PerEdgeProfile[42].Variable[3].station`) rather
/// than the generic VariableStations label.
fn validate_variable_stations_labelled(
    samples: &[(f64, f64)],
    label: &str,
) -> OperationResult<()> {
    if samples.is_empty() {
        return Err(OperationError::InvalidInput {
            parameter: label.into(),
            expected: "non-empty list of (station, radius) samples".into(),
            received: "empty list".into(),
        });
    }
    for (i, &(s, r)) in samples.iter().enumerate() {
        if !s.is_finite() || !r.is_finite() {
            return Err(OperationError::InvalidInput {
                parameter: format!("{label}[{i}]"),
                expected: "finite station and radius".into(),
                received: format!("station={s}, radius={r}"),
            });
        }
        if r <= 0.0 {
            return Err(OperationError::InvalidInput {
                parameter: format!("{label}[{i}].radius"),
                expected: "> 0".into(),
                received: format!("{r}"),
            });
        }
    }
    if samples[0].0 < 0.0 {
        return Err(OperationError::InvalidInput {
            parameter: format!("{label}[0].station"),
            expected: "≥ 0.0".into(),
            received: format!("{}", samples[0].0),
        });
    }
    let last_station = samples[samples.len() - 1].0;
    if last_station > 1.0 {
        return Err(OperationError::InvalidInput {
            parameter: format!("{label}[last].station"),
            expected: "≤ 1.0".into(),
            received: format!("{last_station}"),
        });
    }
    for window in samples.windows(2) {
        if window[1].0 <= window[0].0 {
            return Err(OperationError::InvalidInput {
                parameter: format!("{label}.station"),
                expected: "strictly increasing".into(),
                received: format!("{} → {}", window[0].0, window[1].0),
            });
        }
    }
    Ok(())
}

/// Evaluate a piecewise-linear radius profile at parameter `u`.
///
/// `samples` must satisfy the invariants enforced by
/// `validate_variable_stations`: non-empty, stations strictly
/// increasing, first ≥ 0, last ≤ 1, every radius > 0 and finite.
///
/// Out-of-range `u` is clamped to the nearest endpoint (constant
/// extension). This is the same convention used by `BlendRadius`'s
/// internal sampler in `blend_graph.rs` so the kernel-layer fillet
/// dispatch and the spine-solver sampling agree on the radius
/// profile shape.
fn piecewise_linear_radius(samples: &[(f64, f64)], u: f64) -> f64 {
    // Invariants: samples non-empty, monotone. Endpoints clamp.
    if u <= samples[0].0 {
        return samples[0].1;
    }
    let last = samples.len() - 1;
    if u >= samples[last].0 {
        return samples[last].1;
    }
    // Locate the interval (linear scan is fine — station counts
    // are small, typically 2-8).
    for window in samples.windows(2) {
        let (s0, r0) = window[0];
        let (s1, r1) = window[1];
        if u >= s0 && u <= s1 {
            // Strictly increasing guarantees s1 > s0.
            let t = (u - s0) / (s1 - s0);
            return r0 + t * (r1 - r0);
        }
    }
    // Unreachable given the invariants — kept as a safety net so
    // a future violation surfaces with a defined value rather
    // than panicking.
    samples[last].1
}

/// F5-α.2 — true when `vertex` is classified by `blend_graph` as a
/// 3-edge convex apex (three convex blend edges in this pass share
/// this vertex). When true, `BlendEdgeSurgery` flags this side of the
/// edge as corner-shared so [`splice_blend_edge`] skips the V-side
/// rewires + cap insertion + vertex removal, leaving that work to
/// [`apply_apex_sphere_corner`] after every per-edge splice has run.
///
/// Returns `false` for any other classification (degree ≠ 3, concave,
/// mixed, smooth, cliff, or absent from the graph).
fn is_three_edge_convex_corner(blend_graph: &BlendGraph, vertex: VertexId) -> bool {
    matches!(
        blend_graph.vertex(vertex).map(|v| v.kind),
        Some(BlendVertexKind::ConvexCorner { degree: 3 })
    )
}

fn create_fillet_chain(
    model: &mut BRepModel,
    solid_id: SolidId,
    edges: Vec<EdgeId>,
    options: &FilletOptions,
    blend_graph: &BlendGraph,
) -> OperationResult<(Vec<(EdgeId, FaceId)>, Vec<BlendEdgeSurgery>)> {
    let mut edge_faces: Vec<(EdgeId, FaceId)> = Vec::new();
    let mut surgeries = Vec::new();

    for &edge_id in &edges {
        // Get the two faces adjacent to this edge
        let (face1_id, face2_id) = get_adjacent_faces(model, solid_id, edge_id)?;

        // Create fillet surface between the faces
        let (fillet_face, surgery) = match &options.fillet_type {
            FilletType::Constant(radius) => create_constant_radius_fillet(
                model,
                edge_id,
                face1_id,
                face2_id,
                *radius,
                blend_graph,
            )?,
            FilletType::Variable(r1, r2) => create_variable_radius_fillet(
                model,
                edge_id,
                face1_id,
                face2_id,
                *r1,
                *r2,
                blend_graph,
            )?,
            FilletType::VariableStations(samples) => {
                // Per-station variable radius: build a piecewise-linear
                // evaluator over the stations and route through the
                // existing function-radius surgery path. The structural
                // invariants (non-empty, monotone, in [0,1]) were
                // checked by `validate_variable_stations` at the top
                // of `fillet_edges`, so the evaluator can rely on
                // them.
                let samples = samples.clone();
                let evaluator: Box<dyn Fn(f64) -> f64> =
                    Box::new(move |u: f64| piecewise_linear_radius(&samples, u));
                create_function_radius_fillet(
                    model,
                    edge_id,
                    face1_id,
                    face2_id,
                    &evaluator,
                    blend_graph,
                )?
            }
            FilletType::PerEdgeConstant(map) => {
                // F5-β.5: route each edge through the constant-radius
                // path with its own per-edge value. Coverage is
                // guaranteed by `validate_fillet_inputs` (every
                // selected edge has a map entry); the missing-key
                // path here surfaces as `InternalError` because by
                // this point validation has already passed and any
                // mismatch is a kernel-internal logic violation.
                let r = map.get(&edge_id).copied().ok_or_else(|| {
                    OperationError::InternalError(format!(
                        "PerEdgeConstant: edge {} has no radius entry after validation",
                        edge_id
                    ))
                })?;
                create_constant_radius_fillet(
                    model,
                    edge_id,
                    face1_id,
                    face2_id,
                    r,
                    blend_graph,
                )?
            }
            FilletType::PerEdgeProfile(map) => {
                // F5-β.5.6/.7: per-edge profile fan-out. Each edge
                // picks the surgery route that matches its profile
                // shape — Radius(Constant) → kpart constant path;
                // Radius(Linear) → legacy two-endpoint variable
                // path; Radius(Variable) → piecewise-linear function
                // path (same builder the VariableStations arm above
                // uses); Chord → `create_chord_fillet` (the chord →
                // radius conversion lives there and runs with the
                // local dihedral). Coverage is guaranteed by
                // `validate_fillet_inputs`; the missing-key fallback
                // surfaces as `InternalError` because at this point
                // validation has passed.
                let profile = map.get(&edge_id).ok_or_else(|| {
                    OperationError::InternalError(format!(
                        "PerEdgeProfile: edge {} has no profile entry after validation",
                        edge_id
                    ))
                })?;
                match profile {
                    EdgeFilletProfile::Radius(BlendRadius::Constant(r)) => {
                        create_constant_radius_fillet(
                            model,
                            edge_id,
                            face1_id,
                            face2_id,
                            *r,
                            blend_graph,
                        )?
                    }
                    EdgeFilletProfile::Radius(BlendRadius::Linear { start, end }) => {
                        create_variable_radius_fillet(
                            model,
                            edge_id,
                            face1_id,
                            face2_id,
                            *start,
                            *end,
                            blend_graph,
                        )?
                    }
                    EdgeFilletProfile::Radius(BlendRadius::Variable(samples)) => {
                        let samples = samples.clone();
                        let evaluator: Box<dyn Fn(f64) -> f64> =
                            Box::new(move |u: f64| piecewise_linear_radius(&samples, u));
                        create_function_radius_fillet(
                            model,
                            edge_id,
                            face1_id,
                            face2_id,
                            &evaluator,
                            blend_graph,
                        )?
                    }
                    EdgeFilletProfile::Chord(c) => create_chord_fillet(
                        model,
                        edge_id,
                        face1_id,
                        face2_id,
                        *c,
                        blend_graph,
                    )?,
                }
            }
            FilletType::Function(f) => {
                create_function_radius_fillet(model, edge_id, face1_id, face2_id, f, blend_graph)?
            }
            FilletType::Chord(chord) => {
                create_chord_fillet(model, edge_id, face1_id, face2_id, *chord, blend_graph)?
            }
        };

        edge_faces.push((edge_id, fillet_face));
        if let Some(mut s) = surgery {
            // F5-α.2 — stamp corner-shared flags on the freshly built
            // surgery so `splice_blend_edge` skips the V0/V1-side
            // pred/succ rewires, third-face cap insertion, and vertex
            // removal at any endpoint that the BlendGraph classifies as
            // a 3-edge convex apex shared with two sibling fillets in
            // this same pass. `apply_apex_sphere_corner` takes over the
            // cap insertion (via the sphere face's loop) and the
            // corner-vertex removal once the per-edge splices have all
            // run.
            s.original_v0_corner_shared = is_three_edge_convex_corner(blend_graph, s.original_v0);
            s.original_v1_corner_shared = is_three_edge_convex_corner(blend_graph, s.original_v1);
            surgeries.push(s);
        }
    }

    // F5-α: corner-sphere emission has moved out of the per-chain
    // pipeline into `fillet_edges`. The BlendGraph is global to the
    // selection, and a single corner's three incident edges may
    // straddle multiple chains — the dispatcher must see all chain
    // results before walking corners.

    Ok((edge_faces, surgeries))
}

/// Create a constant radius fillet
fn create_constant_radius_fillet(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    radius: f64,
    blend_graph: &BlendGraph,
) -> OperationResult<(FaceId, Option<BlendEdgeSurgery>)> {
    // Get edge and face data
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
        .clone();

    // Closed circular edges (cylinder/hole rims, torus seams) require a
    // distinct topology — a torus blend with two periodic trim loops and
    // no V0/V1 caps. The default 4-sided template assumes V0 != V1 and
    // would hit `Edge axis normalize failed: DivisionByZero` at the cap
    // construction step. Slice A2 fills in the real implementation; for
    // now this surfaces a clean error so the caller sees an actionable
    // message instead of the cryptic numerical failure.
    if edge.is_loop() {
        return create_closed_edge_fillet(model, edge_id, face1_id, face2_id, radius, radius);
    }

    // F3-δ.5: route every constant-radius chain through the spine
    // solver. With F3-δ.2's `enable_marching: true` default and
    // F3-δ.1's planar-RuledSurface promotion, every surface-pair
    // tuple is claimed by either an analytic arm or the marched
    // arm — `solve_spine_for_chain` returning `Ok(None)` for a
    // single-edge chain is now an internal-consistency failure
    // (the only other `Ok(None)` path is the multi-edge sentinel
    // which we provably don't trigger here). The legacy bisector
    // (`compute_rolling_ball_positions` + `adaptive_rolling_ball_sampling`)
    // is deleted in this slice.
    let spine_options = super::spine_solver::SpineOptions::default();
    let spine_rail =
        super::spine_solver::solve_spine_for_chain(model, &[edge_id], blend_graph, &spine_options)?
            .ok_or_else(|| {
                OperationError::InternalError(format!(
                    "spine solver returned no rail for single-edge chain (edge {})",
                    edge_id
                ))
            })?;
    let rolling_ball_data = rolling_ball_data_from_spine_rail(&spine_rail);

    // F4-α: route through the typed BlendSurfaceCarrier derived from
    // `spine_rail.solver_kind` + per-station radius constancy.
    //
    // * F4-α.1 landed the dispatch enum + table.
    // * F4-α.2 (this slice) routes the analytic carriers through
    //   `*::from_analytic_kpart` constructors that consume the
    //   spine_solver's analytic-exact curves directly — no 20-sample
    //   frame derivation, no 3-point circumscribed-circle estimate.
    //   The dispatcher reads the major radius for the toroidal
    //   arms from the spine's analytic [`Arc`] descriptor, with a
    //   cross-check against the supporting Cylinder/Sphere axis so
    //   FP drift in the spine fit cannot promote a wrong-radius
    //   torus into a watertight model.
    let carrier = super::blend_surface_carrier::BlendSurfaceCarrier::from_spine_rail(
        &spine_rail,
        &spine_options.tolerance,
    );
    let fillet_surface = create_blend_surface_for_carrier(
        carrier,
        &spine_rail,
        &rolling_ball_data,
        model,
        face1_id,
        face2_id,
    )?;
    let surface_id = model.surfaces.add(fillet_surface);

    // Create trimming curves on adjacent faces
    let (trim_curve1, trim_curve2) =
        compute_fillet_trim_curves(model, &rolling_ball_data, face1_id, face2_id)?;

    let cap_v0_center = *rolling_ball_data.centers.first().ok_or_else(|| {
        OperationError::InvalidGeometry("Rolling-ball centers empty".to_string())
    })?;
    let cap_v1_center = *rolling_ball_data.centers.last().ok_or_else(|| {
        OperationError::InvalidGeometry("Rolling-ball centers empty".to_string())
    })?;
    let cap_v0_radius = *rolling_ball_data.radii.first().ok_or_else(|| {
        OperationError::InvalidGeometry("Rolling-ball radii empty".to_string())
    })?;
    let cap_v1_radius = *rolling_ball_data.radii.last().ok_or_else(|| {
        OperationError::InvalidGeometry("Rolling-ball radii empty".to_string())
    })?;

    // Create fillet face with proper trimming. Constant-radius rolling
    // balls trace cylindrical or toroidal surfaces whose natural u=const
    // cross-section is exactly a planar circular arc of the constant
    // radius — so the default `Arc` cap is correct and `None` keeps it.
    create_trimmed_fillet_face(
        model,
        surface_id,
        edge_id,
        face1_id,
        face2_id,
        trim_curve1,
        trim_curve2,
        cap_v0_center,
        cap_v1_center,
        cap_v0_radius,
        cap_v1_radius,
        None,
    )
}

/// Create a variable radius fillet
fn create_variable_radius_fillet(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    start_radius: f64,
    end_radius: f64,
    _blend_graph: &BlendGraph,
) -> OperationResult<(FaceId, Option<BlendEdgeSurgery>)> {
    // Get edge and face data
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
        .clone();

    // Same closed-edge guard as the constant path. A variable-radius
    // closed fillet is geometrically more nuanced (the radius profile
    // must be periodic to avoid a discontinuity at the seam) and is
    // doubly-not-supported until A2 lands the constant case first.
    if edge.is_loop() {
        return create_closed_edge_fillet(
            model,
            edge_id,
            face1_id,
            face2_id,
            start_radius,
            end_radius,
        );
    }

    // Compute rolling ball positions with variable radius
    let rolling_ball_data = compute_variable_rolling_ball_positions(
        model,
        &edge,
        edge_id,
        face1_id,
        face2_id,
        start_radius,
        end_radius,
    )?;

    // Create variable radius fillet surface
    let fillet_surface = create_rolling_ball_surface(&rolling_ball_data)?;

    // Build cap NURBS curves by sampling the surface's parametric
    // boundary at u = u_min (cap V0) and u = u_max (cap V1), v running
    // across the full v-domain. The previous implementation built a
    // circular `Arc` in the plane perpendicular to the edge axis at
    // each cap, which is only on-surface when `dr/du = 0`. For variable
    // radii the surface's cross-section plane tilts in proportion to
    // the radius gradient; sampling the actual iso-curve makes the cap
    // sit on the surface boundary by construction.
    //
    // Orientation: the surface construction places the v=0 control
    // points along contact1 and v=v_max along contact2. The blend-loop
    // traversal needs cap_V0 to go from v_t2_start (contact2[0]) to
    // v_t1_start (contact1[0]) — i.e. v_max → v_min at u_min — and
    // cap_V1 to go from v_t1_end (contact1[N]) to v_t2_end
    // (contact2[N]) — i.e. v_min → v_max at u_max.
    let ((surf_u_min, surf_u_max), (surf_v_min, surf_v_max)) =
        fillet_surface.parameter_bounds();
    let cap_v0_curve =
        sample_cap_iso_curve(&*fillet_surface, surf_u_min, surf_v_max, surf_v_min)?;
    let cap_v1_curve =
        sample_cap_iso_curve(&*fillet_surface, surf_u_max, surf_v_min, surf_v_max)?;

    let surface_id = model.surfaces.add(fillet_surface);

    // Create trimming curves
    let (trim_curve1, trim_curve2) =
        compute_fillet_trim_curves(model, &rolling_ball_data, face1_id, face2_id)?;

    let cap_v0_center = *rolling_ball_data.centers.first().ok_or_else(|| {
        OperationError::InvalidGeometry("Rolling-ball centers empty".to_string())
    })?;
    let cap_v1_center = *rolling_ball_data.centers.last().ok_or_else(|| {
        OperationError::InvalidGeometry("Rolling-ball centers empty".to_string())
    })?;
    let cap_v0_radius = *rolling_ball_data.radii.first().ok_or_else(|| {
        OperationError::InvalidGeometry("Rolling-ball radii empty".to_string())
    })?;
    let cap_v1_radius = *rolling_ball_data.radii.last().ok_or_else(|| {
        OperationError::InvalidGeometry("Rolling-ball radii empty".to_string())
    })?;

    // Create fillet face with surface-sampled cap NURBS
    create_trimmed_fillet_face(
        model,
        surface_id,
        edge_id,
        face1_id,
        face2_id,
        trim_curve1,
        trim_curve2,
        cap_v0_center,
        cap_v1_center,
        cap_v0_radius,
        cap_v1_radius,
        Some((cap_v0_curve, cap_v1_curve)),
    )
}

/// Closed-edge fillet (cylinder/hole rim, torus seam) — entry dispatcher.
///
/// A closed edge has `start_vertex == end_vertex`, so the rolling-ball
/// sweep produces a torus segment that closes on itself with no V0/V1
/// caps. The default 4-sided template assumes V0 != V1 and would hit
/// `Edge axis normalize failed: DivisionByZero` at the cap construction
/// step.
///
/// Slice A2 ships the cylinder-rim case (Plane cap + Cylinder lateral):
///   - The cap circle shrinks from `R` to `R - r`.
///   - The lateral cylinder shortens by `r` along its axis on the rim
///     side; the lateral surface itself is replaced in-place with a new
///     `Cylinder` carrying updated `height_limits`.
///   - A new quarter-torus surface (`Torus` with `param_limits` set to
///     `u ∈ [0, 2π], v ∈ [0, π/2]`) becomes the blend face. Its outer
///     loop is the standard "seamed" pattern shared with cylinder
///     lateral faces:
///         lat_trim_edge  forward
///         seam_arc_edge  forward
///         cap_trim_edge  backward
///         seam_arc_edge  backward
///   - The original rim edge and seam edge are retired; the rim seam
///     vertex is mutated to its new (shortened) position and a fresh
///     vertex is added at the reduced cap radius.
///
/// Cone caps (Cone+Plane), torus seams (Torus+Plane), and revolve seams
/// land in follow-up slices — they need their own surface-type pairs and
/// are surfaced here as `NotImplemented`.
///
/// Returns `(blend_face_id, None)` — closed-edge fillets do their own
/// surgery in-line (no four-sided splice via `BlendEdgeSurgery`), so
/// the surgery slot is left empty for `update_adjacent_faces` to skip.
fn create_closed_edge_fillet(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    start_radius: f64,
    end_radius: f64,
) -> OperationResult<(FaceId, Option<BlendEdgeSurgery>)> {
    use crate::primitives::surface::{Cylinder, Plane};

    // Variable radius on a closed edge requires a *periodic* radius
    // profile (same value at u=0 and u=2π) to avoid a discontinuity at
    // the seam. Slice A2 ships the constant case only.
    if (start_radius - end_radius).abs() > 1e-9 {
        return Err(OperationError::NotImplemented(format!(
            "Variable-radius fillet on closed edges (edge {edge_id}) requires a \
             periodic radius profile and is not yet supported. Use a constant \
             radius (start == end). Tracked as a follow-up under task #89."
        )));
    }
    let radius = start_radius;
    if !radius.is_finite() || radius <= 0.0 {
        return Err(OperationError::InvalidGeometry(format!(
            "Closed-edge fillet (edge {edge_id}) radius must be a positive finite \
             number; got {radius}"
        )));
    }

    // Identify cap (Plane) vs lateral (Cylinder). The cylinder-rim case
    // is the only Plane–Cylinder pair we ship in slice A2.
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
            "Closed-edge fillet (edge {edge_id}) currently supports only \
             Plane–Cylinder rims (cylinder caps). Cone, torus, and revolve \
             seams are tracked as follow-up slices under task #89."
        )));
    };

    cylinder_rim_fillet(
        model,
        edge_id,
        cap_face_id,
        lat_face_id,
        &plane,
        &cylinder,
        radius,
    )
}

/// Build the quarter-torus blend that replaces a cylinder/cap rim.
///
/// See [`create_closed_edge_fillet`] for the high-level recipe. This
/// helper does the topology surgery directly because the new face's
/// loop pattern is the same "seamed" rectangle a cylinder lateral uses,
/// which `splice_blend_edge` (designed for the open V0–V1 case) cannot
/// produce.
fn cylinder_rim_fillet(
    model: &mut BRepModel,
    rim_edge_id: EdgeId,
    cap_face_id: FaceId,
    lat_face_id: FaceId,
    plane: &crate::primitives::surface::Plane,
    cylinder: &crate::primitives::surface::Cylinder,
    radius: f64,
) -> OperationResult<(FaceId, Option<BlendEdgeSurgery>)> {
    use crate::primitives::curve::Arc;
    use crate::primitives::edge::EdgeOrientation;
    use crate::primitives::face::Face;
    use crate::primitives::r#loop::{Loop, LoopType};
    use crate::primitives::surface::{Cylinder, Torus};
    use std::f64::consts::{FRAC_PI_2, PI, TAU};

    let axis = cylinder.axis;
    let ref_dir = cylinder.ref_dir;
    let big_r = cylinder.radius;
    let origin = cylinder.origin;

    let height_limits = cylinder.height_limits.ok_or_else(|| {
        OperationError::InvalidGeometry(
            "Closed-edge fillet requires a finite cylinder (height_limits set)".to_string(),
        )
    })?;
    let h_low = height_limits[0];
    let h_high = height_limits[1];
    let height = h_high - h_low;

    // Geometric preconditions:
    //   - r < R/2 keeps the resulting torus's minor radius strictly
    //     below its major radius, so the surface doesn't self-pinch
    //     (Torus::new rejects minor >= major outright).
    //   - r < height keeps the lateral cylinder non-degenerate after
    //     it's shortened on the rim side.
    if radius >= big_r * 0.5 - 1e-9 {
        return Err(OperationError::InvalidGeometry(format!(
            "Fillet radius {radius} too large for cylinder rim: must be strictly \
             less than half the cylinder radius ({big_r}); the resulting torus \
             would self-pinch (minor >= major)."
        )));
    }
    if radius >= height - 1e-9 {
        return Err(OperationError::InvalidGeometry(format!(
            "Fillet radius {radius} too large for cylinder rim: exceeds available \
             cylinder height ({height}); the lateral surface would collapse."
        )));
    }

    // sign = +1 for top rim (cap normal aligned with cylinder axis),
    //      = -1 for bottom rim (cap normal opposite the cylinder axis).
    let sign: f64 = if plane.normal.dot(&axis) > 0.0 {
        1.0
    } else {
        -1.0
    };

    // After the fillet:
    //   - cap stays at its original height (cap_h) but its outer
    //     boundary shrinks from R to (R - r);
    //   - the lateral cylinder is shortened by r on the cap side, so
    //     its boundary on that side moves to lat_seam_h.
    let cap_h = if sign > 0.0 { h_high } else { h_low };
    let lat_seam_h = cap_h - sign * radius;

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
    let cap_seam_pos = origin + axis * cap_h + ref_dir * (big_r - radius);

    // Torus blend center on the cylinder axis at the lateral shrink
    // height; its axis is the cylinder axis flipped for the rim's
    // outward direction so v=0 of the parametrisation lies on the
    // lateral side and v=π/2 on the cap side, regardless of which rim.
    let torus_center = origin + axis * lat_seam_h;
    let torus_axis = axis * sign;
    // Center of the meridional quarter-arc (in the seam plane).
    let torus_arc_center = torus_center + ref_dir * (big_r - radius);

    // Snapshot the rim edge before mutating anything else.
    let rim_edge = model
        .edges
        .get(rim_edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Rim edge not found".to_string()))?
        .clone();
    if rim_edge.start_vertex != rim_edge.end_vertex {
        return Err(OperationError::InvalidGeometry(
            "Closed-edge fillet invariant violated: rim edge is not a loop".to_string(),
        ));
    }
    let v_lat = rim_edge.start_vertex;

    // Locate the two loops we need to rewrite.
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
    // forward, once backward) — that's the canonical "seamed face"
    // pattern. We need both index positions so we can replace them
    // together with a fresh seam edge whose endpoints reflect v_lat's
    // new position.
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
                OperationError::InvalidGeometry(
                    "Rim edge not found in lateral loop".to_string(),
                )
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

    // Snapshot the seam edge so we preserve its (start, end) vertex
    // ordering on the replacement.
    let seam_edge = model
        .edges
        .get(seam_edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Seam edge not found".to_string()))?
        .clone();

    // ---- step 1: mutate v_lat to its new shortened-rim position. ----
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

    // ---- step 2: create v_cap at the reduced cap radius. ----
    let tol = Tolerance::default().distance();
    let v_cap = model
        .vertices
        .add_or_find(cap_seam_pos.x, cap_seam_pos.y, cap_seam_pos.z, tol);

    // ---- step 3: build the new curves. ----
    // Cap and lateral trim circles share the cylinder's parametric
    // direction so loop orientation flags carry over from the original
    // top/bottom edges unchanged (see step 5).
    let cap_trim_circle = Arc::circle(origin + axis * cap_h, axis, big_r - radius)
        .map_err(|e| OperationError::NumericalError(format!("cap trim circle: {e}")))?;
    let lat_trim_circle = Arc::circle(torus_center, axis, big_r)
        .map_err(|e| OperationError::NumericalError(format!("lat trim circle: {e}")))?;

    // The seam arc connects v_lat (radial direction at the start) to
    // v_cap (axial direction at the end) along a quarter-circle in the
    // meridional plane spanned by (ref_dir, axis*sign). Arc picks its
    // own canonical x_axis based on the normal it's given, so we
    // compute start/sweep in the (x_axis, y_axis) frame Arc actually
    // uses rather than assuming any particular alignment.
    let normal_arc = ref_dir.cross(&axis) * sign;
    let seam_arc = {
        let probe = Arc::new(torus_arc_center, normal_arc, radius, 0.0, 0.0)
            .map_err(|e| OperationError::NumericalError(format!("seam arc probe: {e}")))?;
        let x_axis = probe.x_axis;
        // probe.normal is the normalised version of normal_arc.
        let y_axis = probe.normal.cross(&x_axis);
        let start_a = ref_dir.dot(&y_axis).atan2(ref_dir.dot(&x_axis));
        let end_dir = axis * sign;
        let end_a = end_dir.dot(&y_axis).atan2(end_dir.dot(&x_axis));
        let mut sweep = end_a - start_a;
        if sweep > PI {
            sweep -= TAU;
        } else if sweep < -PI {
            sweep += TAU;
        }
        Arc::new(torus_arc_center, normal_arc, radius, start_a, sweep)
            .map_err(|e| OperationError::NumericalError(format!("seam arc: {e}")))?
    };

    // The new lateral seam line keeps the original edge's (start, end)
    // vertex IDs — the IDs are unchanged, only their positions have
    // moved (or v_cap, which didn't exist before).
    let start_pos = {
        let v = model.vertices.get(seam_edge.start_vertex).ok_or_else(|| {
            OperationError::InvalidGeometry("Seam start vertex missing".to_string())
        })?;
        Point3::new(v.position[0], v.position[1], v.position[2])
    };
    let end_pos = {
        let v = model.vertices.get(seam_edge.end_vertex).ok_or_else(|| {
            OperationError::InvalidGeometry("Seam end vertex missing".to_string())
        })?;
        Point3::new(v.position[0], v.position[1], v.position[2])
    };
    let new_seam_line = Line::new(start_pos, end_pos);

    let cap_trim_curve_id = model.curves.add(Box::new(cap_trim_circle));
    let lat_trim_curve_id = model.curves.add(Box::new(lat_trim_circle));
    let seam_arc_curve_id = model.curves.add(Box::new(seam_arc));
    let new_seam_line_id = model.curves.add(Box::new(new_seam_line));

    // ---- step 4: build the new edges. ----
    // ParameterRange::new(0.0, 1.0) matches `Arc`'s unit
    // parameterisation (see `Arc::new` → `range = ParameterRange::unit()`)
    // — using [0, 2π] would clamp every t > 1 to 1 and pile every
    // sample after the first onto the seam vertex (same trap fixed
    // in `create_cylinder_topology`).
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
    let torus_seam_edge_id = model.edges.add(Edge::new_with_tolerance(
        0,
        v_lat,
        v_cap,
        seam_arc_curve_id,
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

    // ---- step 5: replace the rim slot in the cap loop. ----
    // The cap loop is a single closed circle; preserve its original
    // orientation flag (forward for top, backward for bottom — see
    // `create_cylinder_topology` for why).
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
        let cap_loop = model.loops.get_mut(cap_loop_id).ok_or_else(|| {
            OperationError::InvalidGeometry("Cap loop missing (mut)".to_string())
        })?;
        for (i, edge) in cap_loop.edges.iter_mut().enumerate() {
            if *edge == rim_edge_id {
                *edge = cap_trim_edge_id;
                cap_loop.orientations[i] = cap_orientation;
                break;
            }
        }
    }

    // ---- step 6: replace rim + both seam slots in the lateral loop. ----
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

    // ---- step 7: shorten the lateral cylinder surface in-place. ----
    let lat_surface_id = model
        .faces
        .get(lat_face_id)
        .map(|f| f.surface_id)
        .ok_or_else(|| {
            OperationError::InvalidGeometry(
                "Lateral face missing for surface swap".to_string(),
            )
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

    // ---- step 8: build the torus surface + blend face. ----
    let mut torus = Torus::new(torus_center, torus_axis, big_r - radius, radius)
        .map_err(|e| OperationError::NumericalError(format!("Torus blend: {e}")))?;
    // Restrict to the quarter-toroidal sector u ∈ [0, 2π], v ∈ [0, π/2]
    // so the surface domain matches the loop the four edges define.
    torus.param_limits = Some([0.0, TAU, 0.0, FRAC_PI_2]);
    // Anchor u=0 to the same ref_dir as the cylinder so the torus seam
    // aligns with the new lateral seam edge.
    torus.ref_dir = ref_dir;
    // Outward target at the torus's parametric midpoint (u=π, v=π/4):
    // the surface normal there is in the direction
    // -ref_dir·cos(π/4) + (axis·sign)·sin(π/4) ≡ (axis·sign − ref_dir)/√2.
    // This is the geometric "diagonal" between the lateral-outward and
    // cap-outward directions at the corner — the fillet blend face must
    // have its oriented outward normal align with this diagonal.
    let blend_outward_target = torus_axis - ref_dir;
    let blend_orientation = orient_face_for_outward(&torus, blend_outward_target)?;
    let torus_surface_id = model.surfaces.add(Box::new(torus));

    // Loop sequence (parameter-space CCW for outward-pointing torus):
    //   (u=0, v=0)    → (u=2π, v=0)   : lat_trim_edge   forward
    //   (u=2π, v=0)   → (u=2π, v=π/2) : torus_seam_edge forward
    //   (u=2π, v=π/2) → (u=0,  v=π/2) : cap_trim_edge   backward
    //   (u=0,  v=π/2) → (u=0,  v=0)   : torus_seam_edge backward
    let mut blend_loop = Loop::new(0, LoopType::Outer);
    blend_loop.add_edge(lat_trim_edge_id, true);
    blend_loop.add_edge(torus_seam_edge_id, true);
    blend_loop.add_edge(cap_trim_edge_id, false);
    blend_loop.add_edge(torus_seam_edge_id, false);
    let blend_loop_id = model.loops.add(blend_loop);

    let mut blend_face = Face::new(0, torus_surface_id, blend_loop_id, blend_orientation);
    blend_face.outer_loop = blend_loop_id;
    let blend_face_id = model.faces.add(blend_face);

    // ---- step 9: cleanup. ----
    // The original rim edge and seam edge are no longer referenced by
    // any loop. Curves are append-only in the kernel, so the old
    // circle/line curves become orphaned but cannot be safely removed
    // here without a curve-store reference count (out of scope).
    model.edges.remove(rim_edge_id);
    model.edges.remove(seam_edge_id);

    Ok((blend_face_id, None))
}

/// Compute rolling ball positions for variable radius.
///
/// Convexity classification is shared with `compute_rolling_ball_positions`
/// — a single signed-dihedral measurement at the edge midpoint
/// (`robust_face_angle` on outward-oriented normals + the loop-aligned
/// tangent) determines whether the ball sits inside the solid (convex
/// edge) or in the cavity (concave edge), and that classification is
/// then committed to every sample along the edge. The previous
/// `normal1·normal2 < 0` heuristic was orientation-dependent and
/// flipped on perpendicular box edges (dot = 0 → wrong branch),
/// producing concave fillets on edges that should have been convex.
fn compute_variable_rolling_ball_positions(
    model: &BRepModel,
    edge: &Edge,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    start_radius: f64,
    end_radius: f64,
) -> OperationResult<RollingBallData> {
    let num_samples = 20;
    let mut data = RollingBallData {
        centers: Vec::with_capacity(num_samples + 1),
        contacts1: Vec::with_capacity(num_samples + 1),
        contacts2: Vec::with_capacity(num_samples + 1),
        parameters: Vec::with_capacity(num_samples + 1),
        radii: Vec::with_capacity(num_samples + 1),
    };

    // Signed-dihedral classification at midpoint — see the matching
    // block in `compute_rolling_ball_positions` for the full
    // derivation. Briefly: convex ⇒ ball inside solid along -bisector
    // and contact = center + r·n; concave ⇒ ball in cavity along
    // +bisector and contact = center - r·n.
    let edge_midpoint = edge.evaluate(0.5, &model.curves)?;
    let face1_mid_normal = get_face_oriented_normal(model, face1_id, &edge_midpoint)?;
    let face2_mid_normal = get_face_oriented_normal(model, face2_id, &edge_midpoint)?;
    let face1_loop_sign = edge_orientation_in_face(model, face1_id, edge_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "Edge {} not present in any loop of face {}",
            edge_id, face1_id
        ))
    })?;
    let edge_tangent_in_loop = edge.tangent_at(0.5, &model.curves)? * face1_loop_sign;
    let dihedral_angle = robust_face_angle(
        &face1_mid_normal,
        &face2_mid_normal,
        &edge_tangent_in_loop,
        &Tolerance::default(),
    )
    .map_err(|e| OperationError::NumericalError(format!("Dihedral angle failed: {:?}", e)))?;
    let (offset_sign, contact_sign) = if dihedral_angle > 0.0 {
        (-1.0, 1.0)
    } else {
        (1.0, -1.0)
    };

    for i in 0..=num_samples {
        let t = i as f64 / num_samples as f64;
        data.parameters.push(t);

        let radius = start_radius + t * (end_radius - start_radius);
        data.radii.push(radius);

        // Validate edge differentiability at this sample (tangent value
        // is unused — we only fail if the edge is non-differentiable).
        let edge_point = edge.evaluate(t, &model.curves)?;
        edge.tangent_at(t, &model.curves)?;

        let normal1 = get_face_oriented_normal(model, face1_id, &edge_point)?;
        let normal2 = get_face_oriented_normal(model, face2_id, &edge_point)?;

        let bisector = (normal1 + normal2).normalize().map_err(|e| {
            OperationError::NumericalError(format!("Bisector normalization failed: {:?}", e))
        })?;
        let bisector_dot_n1 = bisector.dot(&normal1);
        if bisector_dot_n1.abs() < 1e-9 {
            return Err(OperationError::NumericalError(
                "Bisector orthogonal to face normal — degenerate dihedral".to_string(),
            ));
        }
        let offset_distance = radius / bisector_dot_n1;

        let fillet_center = edge_point + bisector * (offset_sign * offset_distance);
        data.centers.push(fillet_center);

        let contact1 = fillet_center + normal1 * (contact_sign * radius);
        let contact2 = fillet_center + normal2 * (contact_sign * radius);

        data.contacts1.push(contact1);
        data.contacts2.push(contact2);
    }

    Ok(data)
}

/// Create a function-based radius fillet.
///
/// The radius function `r(t)` is sampled along the edge parameter; the
/// resulting profile drives the rolling-ball construction directly so
/// arbitrary radius variation (linear, quadratic, sigmoid, polyline,
/// any user-supplied closure) is honored exactly. If the function is
/// near-constant within 1% relative tolerance, this collapses to the
/// constant-radius fast path for the cylindrical/toroidal patches.
fn create_function_radius_fillet(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    radius_fn: &Box<dyn Fn(f64) -> f64>,
    blend_graph: &BlendGraph,
) -> OperationResult<(FaceId, Option<BlendEdgeSurgery>)> {
    // Validate the function over the full edge parameter at the same
    // density used by the rolling-ball construction — we don't want
    // to discover a non-finite radius midway through surface assembly.
    const NUM_SAMPLES: usize = 20;
    let mut radii = Vec::with_capacity(NUM_SAMPLES + 1);
    for i in 0..=NUM_SAMPLES {
        let t = i as f64 / NUM_SAMPLES as f64;
        let r = radius_fn(t);
        if !r.is_finite() || r <= 0.0 {
            return Err(OperationError::InvalidGeometry(format!(
                "Radius function returned invalid value {} at t={:.3}",
                r, t
            )));
        }
        radii.push(r);
    }

    // Constant-radius shortcut: if every sample lies within 1% of the
    // mean, the cylindrical/toroidal exact path is faster and produces
    // a smaller (and analytically simpler) surface than the NURBS path.
    let r_mean = radii.iter().copied().sum::<f64>() / radii.len() as f64;
    let r_max_dev = radii
        .iter()
        .map(|r| (r - r_mean).abs())
        .fold(0.0_f64, f64::max);
    if r_mean > 0.0 && r_max_dev / r_mean < 0.01 {
        return create_constant_radius_fillet(
            model,
            edge_id,
            face1_id,
            face2_id,
            r_mean,
            blend_graph,
        );
    }

    // Variable path: build rolling-ball data using the function-derived
    // radii directly, then route through the same NURBS surface
    // construction the start/end variant uses.
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
        .clone();
    let rolling_ball_data =
        compute_function_rolling_ball_positions(model, &edge, edge_id, face1_id, face2_id, &radii)?;
    let fillet_surface = create_rolling_ball_surface(&rolling_ball_data)?;

    // Same surface-iso-curve cap-sampling as the linear variable-radius
    // path — see the docstring there for the orientation rationale.
    // A non-constant radius profile (function radius) makes the
    // surface's cross-section plane tilt with `dr/du`, so the
    // perpendicular-plane Arc cap drifts off the surface.
    let ((surf_u_min, surf_u_max), (surf_v_min, surf_v_max)) =
        fillet_surface.parameter_bounds();
    let cap_v0_curve =
        sample_cap_iso_curve(&*fillet_surface, surf_u_min, surf_v_max, surf_v_min)?;
    let cap_v1_curve =
        sample_cap_iso_curve(&*fillet_surface, surf_u_max, surf_v_min, surf_v_max)?;

    let surface_id = model.surfaces.add(fillet_surface);

    let (trim_curve1, trim_curve2) =
        compute_fillet_trim_curves(model, &rolling_ball_data, face1_id, face2_id)?;

    let cap_v0_center = *rolling_ball_data.centers.first().ok_or_else(|| {
        OperationError::InvalidGeometry("Rolling-ball centers empty".to_string())
    })?;
    let cap_v1_center = *rolling_ball_data.centers.last().ok_or_else(|| {
        OperationError::InvalidGeometry("Rolling-ball centers empty".to_string())
    })?;
    let cap_v0_radius = *rolling_ball_data.radii.first().ok_or_else(|| {
        OperationError::InvalidGeometry("Rolling-ball radii empty".to_string())
    })?;
    let cap_v1_radius = *rolling_ball_data.radii.last().ok_or_else(|| {
        OperationError::InvalidGeometry("Rolling-ball radii empty".to_string())
    })?;

    create_trimmed_fillet_face(
        model,
        surface_id,
        edge_id,
        face1_id,
        face2_id,
        trim_curve1,
        trim_curve2,
        cap_v0_center,
        cap_v1_center,
        cap_v0_radius,
        cap_v1_radius,
        Some((cap_v0_curve, cap_v1_curve)),
    )
}

/// Build rolling-ball data using a caller-supplied per-sample radius
/// profile. Identical geometry as `compute_variable_rolling_ball_positions`
/// but with arbitrary radii instead of linear interpolation between
/// start/end.
fn compute_function_rolling_ball_positions(
    model: &BRepModel,
    edge: &Edge,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    radii: &[f64],
) -> OperationResult<RollingBallData> {
    let num_samples = radii.len() - 1;
    let mut data = RollingBallData {
        centers: Vec::with_capacity(radii.len()),
        contacts1: Vec::with_capacity(radii.len()),
        contacts2: Vec::with_capacity(radii.len()),
        parameters: Vec::with_capacity(radii.len()),
        radii: Vec::with_capacity(radii.len()),
    };

    // Same midpoint signed-dihedral classification as the variable /
    // constant paths — see `compute_rolling_ball_positions` for the
    // geometric derivation.
    let edge_midpoint = edge.evaluate(0.5, &model.curves)?;
    let face1_mid_normal = get_face_oriented_normal(model, face1_id, &edge_midpoint)?;
    let face2_mid_normal = get_face_oriented_normal(model, face2_id, &edge_midpoint)?;
    let face1_loop_sign = edge_orientation_in_face(model, face1_id, edge_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "Edge {} not present in any loop of face {}",
            edge_id, face1_id
        ))
    })?;
    let edge_tangent_in_loop = edge.tangent_at(0.5, &model.curves)? * face1_loop_sign;
    let dihedral_angle = robust_face_angle(
        &face1_mid_normal,
        &face2_mid_normal,
        &edge_tangent_in_loop,
        &Tolerance::default(),
    )
    .map_err(|e| OperationError::NumericalError(format!("Dihedral angle failed: {:?}", e)))?;
    let (offset_sign, contact_sign) = if dihedral_angle > 0.0 {
        (-1.0, 1.0)
    } else {
        (1.0, -1.0)
    };

    for (i, &radius) in radii.iter().enumerate() {
        let t = i as f64 / num_samples as f64;
        data.parameters.push(t);
        data.radii.push(radius);

        let edge_point = edge.evaluate(t, &model.curves)?;
        edge.tangent_at(t, &model.curves)?;

        let normal1 = get_face_oriented_normal(model, face1_id, &edge_point)?;
        let normal2 = get_face_oriented_normal(model, face2_id, &edge_point)?;

        let bisector = (normal1 + normal2).normalize().map_err(|e| {
            OperationError::NumericalError(format!("Bisector normalization failed: {:?}", e))
        })?;
        let bisector_dot_n1 = bisector.dot(&normal1);
        if bisector_dot_n1.abs() < 1e-9 {
            return Err(OperationError::NumericalError(
                "Bisector orthogonal to face normal — degenerate dihedral".to_string(),
            ));
        }
        let offset_distance = radius / bisector_dot_n1;

        let fillet_center = edge_point + bisector * (offset_sign * offset_distance);
        data.centers.push(fillet_center);
        data.contacts1
            .push(fillet_center + normal1 * (contact_sign * radius));
        data.contacts2
            .push(fillet_center + normal2 * (contact_sign * radius));
    }

    Ok(data)
}

/// Resample an arbitrary-length radius array onto a fixed `target_len`
/// uniform grid via linear interpolation. Required because the
/// rolling-ball pipeline samples 21 points (`0..=20`) but the
/// `VariableRadiusFillet` surface uses 20 control points in u —
/// without resampling the surface and the trimming data drift apart.
fn resample_radii_uniform(radii: &[f64], target_len: usize) -> Vec<f64> {
    if radii.is_empty() || target_len == 0 {
        return Vec::new();
    }
    if radii.len() == target_len {
        return radii.to_vec();
    }
    let mut out = Vec::with_capacity(target_len);
    let last = radii.len() - 1;
    for j in 0..target_len {
        // Map j ∈ [0, target_len-1] → s ∈ [0, last]
        let s = if target_len == 1 {
            0.0
        } else {
            j as f64 * (last as f64) / (target_len - 1) as f64
        };
        let i = (s.floor() as usize).min(last);
        let i_next = (i + 1).min(last);
        let frac = s - i as f64;
        out.push(radii[i] * (1.0 - frac) + radii[i_next] * frac);
    }
    out
}

/// Create a chord length fillet
fn create_chord_fillet(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    chord_length: f64,
    blend_graph: &BlendGraph,
) -> OperationResult<(FaceId, Option<BlendEdgeSurgery>)> {
    // Compute radius from chord length and face angle
    let angle = compute_face_angle(model, edge_id, face1_id, face2_id)?;
    let half_sin = (angle / 2.0).sin();
    if half_sin.abs() < 1e-10 {
        return Err(OperationError::InvalidGeometry(
            "Cannot fillet flat or reflex edge".into(),
        ));
    }
    let radius = chord_length / (2.0 * half_sin);

    create_constant_radius_fillet(model, edge_id, face1_id, face2_id, radius, blend_graph)
}

/// Describes one filleted edge-blend surface adjacent to a vertex.
///
/// A constant-radius edge fillet between two faces is realized in the
/// kernel as a cylindrical face whose axis is the rolling-ball spine
/// (an offset of the original edge curve). For vertex-blend purposes
/// we only need the axis line and radius: the sphere center lies on
/// the axis line of every incident edge-blend, and the sphere radius
/// must equal the edge-blend radius (a constant-radius vertex blend
/// across N edge fillets is well-posed only when all radii agree).
///
/// Variable-radius (toroidal) and offset-NURBS edge fillets are not
/// classified by slice 1 — those surface kinds report `None` from
/// `classify_blend_for_edge` so the caller raises NotImplemented.
#[derive(Debug, Clone)]
struct EdgeBlendDescriptor {
    /// The cylindrical fillet face adjacent to the vertex. Consumed by
    /// slice 2 (Task #82) to re-trim the fillet face against the sphere.
    #[allow(dead_code)]
    face_id: FaceId,
    /// Unit axis direction of the fillet cylinder.
    axis: Vector3,
    /// A point on the fillet-cylinder axis (the cylinder's `origin`).
    axis_origin: Point3,
    /// Radius of the fillet cylinder.
    radius: f64,
}

/// One incident edge at the vertex, optionally classified as a blend.
///
/// `adjacent_faces` is the pair of original B-Rep faces meeting at this
/// edge. After `fillet_edges` has run, exactly one of these face pairs
/// per filleted incident is replaced by a cylindrical fillet face whose
/// descriptor is captured in `blend`. Non-filleted incident edges
/// (which the caller has not asked to be rounded) carry
/// `blend == None`; they bound the spherical patch only via the
/// sphere/face intersection on the original face, not through a fillet
/// surface.
#[derive(Debug, Clone)]
struct IncidentEdgeClassification {
    edge_id: EdgeId,
    /// The original face pair across this edge. Consumed by slice 2 to
    /// stitch the sphere patch into the adjacent shell faces.
    #[allow(dead_code)]
    adjacent_faces: (FaceId, FaceId),
    /// `Some` if at least one of the adjacent faces is a cylindrical
    /// edge-fillet that we recognize as a blend; otherwise `None`.
    blend: Option<EdgeBlendDescriptor>,
}

/// Classify the surface pair `(face1, face2)` adjacent to one edge.
///
/// Returns `Some(EdgeBlendDescriptor)` when exactly one of the two
/// faces is a recognised cylindrical edge-blend — that face is the
/// fillet just produced by `fillet_edges`, and its axis carries the
/// information needed to place the corner sphere. Two surface kinds
/// qualify:
///
///   * Raw [`Cylinder`] — produced by the pre-F4-α path and by
///     a handful of specialised closed-edge specialisations
///     (`cylinder_rim_fillet`).
///   * [`CylindricalFillet`] — the F4-α analytic dispatch output
///     for plane/plane and coaxial-cylinder/cylinder edges. Even
///     though it reports `SurfaceType::Cylinder`, it is a distinct
///     wrapper type carrying spine + per-station frame fields; the
///     vertex-blend classifier reads the cylinder axis/origin/radius
///     out of those fields.
///
/// Returns `None` when neither face is a recognised blend (the
/// incident edge was not filleted) and when both faces are cylindrical
/// blends (ambiguous — a fillet-of-fillet scenario, deferred to Task
/// #102).
fn classify_blend_for_edge(
    model: &BRepModel,
    face1: FaceId,
    face2: FaceId,
) -> Option<EdgeBlendDescriptor> {
    let f1 = model.faces.get(face1)?;
    let f2 = model.faces.get(face2)?;
    let s1 = model.surfaces.get(f1.surface_id)?;
    let s2 = model.surfaces.get(f2.surface_id)?;

    // Extract (axis, axis_origin, radius) from either a raw Cylinder
    // or a CylindricalFillet. Returns None when the surface is neither.
    let extract = |surf: &dyn Surface| -> Option<(Vector3, Point3, f64)> {
        if let Some(c) = surf.as_any().downcast_ref::<Cylinder>() {
            return Some((c.axis, c.origin, c.radius));
        }
        if let Some(f) = surf.as_any().downcast_ref::<CylindricalFillet>() {
            // For the analytic plane/plane and coaxial-cylinder kparts
            // the axis is constant along the spine, so axis_field[0]
            // gives the cylinder direction. A point on the axis is
            // any spine sample; the start of the spine curve is the
            // simplest exact choice. Both bits are debug-asserted as
            // unit-norm / on-spine in `from_analytic_kpart`.
            let axis = *f.axis_field.first()?;
            let axis_origin = f.spine.evaluate(0.0).ok()?.position;
            return Some((axis, axis_origin, f.radius));
        }
        None
    };

    let blend1 = extract(s1);
    let blend2 = extract(s2);
    match (blend1, blend2) {
        (Some((axis, axis_origin, radius)), None) => Some(EdgeBlendDescriptor {
            face_id: face1,
            axis,
            axis_origin,
            radius,
        }),
        (None, Some((axis, axis_origin, radius))) => Some(EdgeBlendDescriptor {
            face_id: face2,
            axis,
            axis_origin,
            radius,
        }),
        // Two cylindrical blends or two non-cylindrical surfaces both
        // opt out of F5-α classification; F5-β / Task #102 will
        // broaden this to cover fillet-of-fillet.
        _ => None,
    }
}

/// Walk `fillet_face_id`'s outer loop and return the `EdgeId` of the
/// cap arc whose underlying [`Arc`] is centred on `sphere_center`
/// within `1e-6` (plane units).
///
/// A cylindrical edge fillet's outer loop has four edges: two `trim`
/// edges on the adjacent original faces (the rolling-ball contact
/// rails) plus two `cap` edges (the rolling-ball cross-section arcs
/// at each end of the edge). Both caps are [`Arc`] primitives;
/// their centres sit at the two rolling-ball centres along the edge.
/// For the equal-radius F5-α corner case, the V-side cap's centre
/// coincides exactly with the sphere centre C (geometric uniqueness
/// of the rolling ball tangent to both adjacent original faces at
/// radius r), so a centre-coincidence test identifies the right cap.
///
/// Returns `None` if the face is missing, its outer loop is missing,
/// no edge of the loop carries an `Arc`, or no Arc centre is within
/// tolerance of `sphere_center`.
pub(crate) fn find_cap_arc_edge_at_vertex(
    model: &BRepModel,
    fillet_face_id: FaceId,
    sphere_center: Point3,
) -> Option<EdgeId> {
    const CENTER_TOL_SQ: f64 = 1.0e-12; // (1e-6)^2 in squared distance
    let face = model.faces.get(fillet_face_id)?;
    let outer_loop = model.loops.get(face.outer_loop)?;
    for &edge_id in &outer_loop.edges {
        let edge = model.edges.get(edge_id)?;
        let curve = model.curves.get(edge.curve_id)?;
        if let Some(arc) = curve.as_any().downcast_ref::<Arc>() {
            let d = arc.center - sphere_center;
            if d.x * d.x + d.y * d.y + d.z * d.z <= CENTER_TOL_SQ {
                return Some(edge_id);
            }
        }
    }
    None
}

/// F5-β cap-arc lookup keyed by the cylinder's axis line.
///
/// Mixed-radii corners break the F5-α
/// [`find_cap_arc_edge_at_vertex`] invariant that every cap arc on
/// every incident fillet face shares the same centre point (the apex
/// sphere centre). For a 3-edge corner with `r_0 ≠ r_1 ≠ r_2` the
/// three V-side cap centres `C_i` are pairwise distinct: each lies
/// on its own cylinder axis at the foot of the perpendicular from
/// the apex point `A`. The legacy centre-coincidence test rejects
/// every cap.
///
/// This helper matches a cap arc by the *cylinder axis line* it
/// belongs to instead:
///
/// 1. `arc.normal` must be parallel (or anti-parallel) to
///    `cylinder_axis`. Tolerance: `|arc.normal × cylinder_axis| <
///    1.0e-9` (both are unit vectors).
/// 2. `arc.center` must lie on the cylinder axis line through
///    `cylinder_origin` with direction `cylinder_axis`. Tolerance:
///    point-to-line distance `≤ 1.0e-7`.
///
/// A cylindrical fillet face has two cap arcs (V-side and far-side
/// caps). Both pass conditions 1 and 2, so the helper picks the cap
/// whose centre is closer to `corner_apex` — the V-side cap is by
/// construction the one nearest the corner.
///
/// Returns `None` when the face is missing, its outer loop is
/// missing, or no qualifying arc is found.
fn find_cap_arc_edge_by_cylinder_axis(
    model: &BRepModel,
    fillet_face_id: FaceId,
    cylinder_origin: Point3,
    cylinder_axis: Vector3,
    corner_apex: Point3,
) -> Option<EdgeId> {
    const NORMAL_PARALLEL_TOL_SQ: f64 = 1.0e-18; // (1e-9)^2
    const AXIS_DISTANCE_TOL_SQ: f64 = 1.0e-14;   // (1e-7)^2

    let face = model.faces.get(fillet_face_id)?;
    let outer_loop = model.loops.get(face.outer_loop)?;

    let mut best: Option<(EdgeId, f64)> = None;
    for &edge_id in &outer_loop.edges {
        let edge = model.edges.get(edge_id)?;
        let curve = model.curves.get(edge.curve_id)?;
        let arc = match curve.as_any().downcast_ref::<Arc>() {
            Some(a) => a,
            None => continue,
        };

        // Parallel-normal check: |arc.normal × cylinder_axis|² below tol.
        let cross = arc.normal.cross(&cylinder_axis);
        let cross_sq = cross.x * cross.x + cross.y * cross.y + cross.z * cross.z;
        if cross_sq > NORMAL_PARALLEL_TOL_SQ {
            continue;
        }

        // Axis-line distance check: dist² = |v|² − (v·u)² (u is unit).
        let v = arc.center - cylinder_origin;
        let dot = v.x * cylinder_axis.x + v.y * cylinder_axis.y + v.z * cylinder_axis.z;
        let v_sq = v.x * v.x + v.y * v.y + v.z * v.z;
        let dist_sq = (v_sq - dot * dot).max(0.0);
        if dist_sq > AXIS_DISTANCE_TOL_SQ {
            continue;
        }

        // Prefer the cap nearest the corner apex (the V-side cap).
        let d = arc.center - corner_apex;
        let d_apex_sq = d.x * d.x + d.y * d.y + d.z * d.z;
        match best {
            None => best = Some((edge_id, d_apex_sq)),
            Some((_, prev)) if d_apex_sq < prev => best = Some((edge_id, d_apex_sq)),
            _ => {}
        }
    }
    best.map(|(eid, _)| eid)
}

/// Reason a pair of V-side cap circles failed to intersect.
///
/// Returned by [`intersect_two_caps`]; the caller maps each variant
/// onto a typed `BlendFailure` with full vertex / kind context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntersectCapsError {
    /// `u_i × u_j ≈ 0`: cylinder axes parallel — the two cap planes
    /// are parallel (or identical) so no transverse intersection.
    AxesParallel,
    /// Quadratic discriminant negative — the two circles lie in
    /// transverse planes but do not meet (gap exceeds `r_i + r_j` or
    /// circles are too far apart laterally).
    NoIntersection,
    /// Linear-system solve for the plane-plane line anchor was
    /// singular (should not happen if `AxesParallel` is rejected
    /// first; defensive).
    LinearSystemSingular,
}

/// Intersect two V-side cap circles `cap_i` and `cap_j` of a mixed-
/// radii 3-edge corner.
///
/// `cap_i` lies in the plane through `c_i` perpendicular to `u_i`,
/// with radius `r_i`; same for `cap_j`. The two planes are distinct
/// when `u_i ∦ u_j` (the rectilinear-corner precondition), so their
/// intersection is a line `L_ij`. Each cap is a circle on its own
/// plane; the corner-patch vertex `P_{ij}` is one of the at-most-
/// two points where the two cap circles cross.
///
/// # Algorithm
///
/// 1. Set `d_ij = (u_i × u_j).normalize()`. This is the direction of
///    `L_ij`.
/// 2. Anchor `P_0` on `L_ij` by solving the 3×3 system
///    ```text
///    u_i  · P_0 = u_i  · c_i
///    u_j  · P_0 = u_j  · c_j
///    d_ij · P_0 = d_ij · ((c_i + c_j) / 2)
///    ```
///    The third row picks the unique anchor on `L_ij` closest in
///    `d_ij` parameter to the midpoint of `c_i, c_j` — purely a
///    parameterisation choice, the resulting `x(s) = P_0 + s · d_ij`
///    covers `L_ij` for all `s ∈ ℝ`. The matrix has rows
///    `[u_i; u_j; d_ij]`; it is invertible iff `{u_i, u_j, u_i × u_j}`
///    spans ℝ³, i.e. iff `u_i ∦ u_j`.
/// 3. Substitute into `|x − c_i|² = r_i²`. Since `d_ij ⊥ u_i` by
///    construction, the quadratic is
///    ```text
///    s² + 2 s ((P_0 − c_i) · d_ij) + (|P_0 − c_i|² − r_i²) = 0
///    ```
/// 4. `Δ = ((P_0 − c_i) · d_ij)² − (|P_0 − c_i|² − r_i²)`. If
///    `Δ < 0` the cap circles do not intersect — return
///    [`IntersectCapsError::NoIntersection`].
/// 5. Numerical sanity: each candidate must also lie on the second
///    circle (`|x − c_j|² ≈ r_j²` within `1.0e-9`). Mathematically
///    automatic; the check catches gross floating-point drift.
/// 6. Pick the root maximising `(x − vertex) · vertex_outward` — the
///    V-side of the corner. No positivity gate: for typical
///    perpendicular-cube corners both candidates sit *inside* the
///    cube (negative score), and the relevant criterion is "which is
///    closer to V" — i.e. the *larger* (least-negative) score wins.
///    A no-intersection geometry has already been caught by the
///    discriminant gate at step 4.
#[allow(clippy::too_many_arguments)]
fn intersect_two_caps(
    c_i: Point3,
    u_i: Vector3,
    r_i: f64,
    c_j: Point3,
    u_j: Vector3,
    r_j: f64,
    vertex: Point3,
    vertex_outward: Vector3,
) -> Result<Point3, IntersectCapsError> {
    const AXES_PARALLEL_TOL_SQ: f64 = 1.0e-18; // (1e-9)^2
    const CIRCLE_SANITY_TOL: f64 = 1.0e-9;

    // Step 1 — direction of the plane-plane intersection line.
    let d_raw = u_i.cross(&u_j);
    let d_norm_sq = d_raw.x * d_raw.x + d_raw.y * d_raw.y + d_raw.z * d_raw.z;
    if d_norm_sq <= AXES_PARALLEL_TOL_SQ {
        return Err(IntersectCapsError::AxesParallel);
    }
    let d_norm = d_norm_sq.sqrt();
    let d_ij = Vector3::new(d_raw.x / d_norm, d_raw.y / d_norm, d_raw.z / d_norm);

    // Step 2 — anchor P_0 on L_ij via 3×3 solve.
    let mat = Matrix3::from_rows(&u_i, &u_j, &d_ij);
    let m = Vector3::new(
        (c_i.x + c_j.x) * 0.5,
        (c_i.y + c_j.y) * 0.5,
        (c_i.z + c_j.z) * 0.5,
    );
    let rhs = Vector3::new(
        u_i.x * c_i.x + u_i.y * c_i.y + u_i.z * c_i.z,
        u_j.x * c_j.x + u_j.y * c_j.y + u_j.z * c_j.z,
        d_ij.x * m.x + d_ij.y * m.y + d_ij.z * m.z,
    );
    let inv = mat
        .inverse()
        .map_err(|_| IntersectCapsError::LinearSystemSingular)?;
    let p0_vec = inv.transform_vector(&rhs);
    let p0 = Point3::new(p0_vec.x, p0_vec.y, p0_vec.z);

    // Step 3 — quadratic coefficients (a = 1 since d_ij is unit).
    let w = p0 - c_i;
    let b_half = w.x * d_ij.x + w.y * d_ij.y + w.z * d_ij.z;
    let w_sq = w.x * w.x + w.y * w.y + w.z * w.z;
    let c_coeff = w_sq - r_i * r_i;

    // Step 4 — discriminant.
    let disc = b_half * b_half - c_coeff;
    if disc < 0.0 {
        return Err(IntersectCapsError::NoIntersection);
    }
    let sqrt_disc = disc.sqrt();
    let s_plus = -b_half + sqrt_disc;
    let s_minus = -b_half - sqrt_disc;

    let make_candidate = |s: f64| -> Point3 {
        Point3::new(
            p0.x + s * d_ij.x,
            p0.y + s * d_ij.y,
            p0.z + s * d_ij.z,
        )
    };
    let on_second_circle = |x: Point3| -> bool {
        let dx = x.x - c_j.x;
        let dy = x.y - c_j.y;
        let dz = x.z - c_j.z;
        ((dx * dx + dy * dy + dz * dz).sqrt() - r_j).abs() <= CIRCLE_SANITY_TOL
    };
    let v_side_score = |x: Point3| -> f64 {
        (x.x - vertex.x) * vertex_outward.x
            + (x.y - vertex.y) * vertex_outward.y
            + (x.z - vertex.z) * vertex_outward.z
    };

    let cand_plus = make_candidate(s_plus);
    let cand_minus = make_candidate(s_minus);

    // Step 5 — numerical sanity on the second circle.
    let plus_ok = on_second_circle(cand_plus);
    let minus_ok = on_second_circle(cand_minus);
    if !plus_ok && !minus_ok {
        return Err(IntersectCapsError::NoIntersection);
    }

    // Step 6 — pick V-side candidate maximising the outward score.
    let plus_score = v_side_score(cand_plus);
    let minus_score = v_side_score(cand_minus);

    let best_point = if plus_ok && (!minus_ok || plus_score >= minus_score) {
        cand_plus
    } else {
        cand_minus
    };
    Ok(best_point)
}

/// Verify that the three cap-arc edges `cap_arc_edges` form a closed
/// triangular cycle in the BRep, and recover both the corner-vertex
/// sequence (A, B, C) and the per-edge "follow start→end?" flag for
/// the new sphere-face loop.
///
/// The cycle is found by:
///   1. Anchor `A = edges[0].start_vertex`, `B = edges[0].end_vertex`.
///   2. Find which of `edges[1]`, `edges[2]` is incident to `B`; the
///      matching edge's other endpoint is `C`, and its forward flag
///      is `true` iff its `start_vertex == B`.
///   3. The remaining edge must connect `C → A`; its forward flag is
///      `true` iff its `start_vertex == C`.
///
/// `forwards[i]` is the flag for `cap_arc_edges[i]` in the input
/// order — the caller adds edges to the new sphere-face loop in that
/// same order.
pub(crate) fn verify_cap_arcs_form_closed_triangle(
    model: &BRepModel,
    cap_arc_edges: &[EdgeId; 3],
) -> Result<([VertexId; 3], [bool; 3]), BlendFailure> {
    let mut endpoints = [(0u32, 0u32); 3];
    for (i, &edge_id) in cap_arc_edges.iter().enumerate() {
        let edge = model.edges.get(edge_id).ok_or_else(|| {
            BlendFailure::TopologyViolation {
                detail: format!(
                    "cap-arc edge {:?} missing from model during corner-blend cycle check",
                    edge_id
                ),
            }
        })?;
        endpoints[i] = (edge.start_vertex, edge.end_vertex);
    }

    let a = endpoints[0].0;
    let b = endpoints[0].1;

    // Which of edges 1, 2 carries B as an endpoint?
    let pick_middle = |i: usize| -> Option<(bool, VertexId)> {
        if endpoints[i].0 == b {
            Some((true, endpoints[i].1))
        } else if endpoints[i].1 == b {
            Some((false, endpoints[i].0))
        } else {
            None
        }
    };
    let (next_idx, last_idx, next_forward, c) = if let Some((fwd, c)) = pick_middle(1) {
        (1usize, 2usize, fwd, c)
    } else if let Some((fwd, c)) = pick_middle(2) {
        (2usize, 1usize, fwd, c)
    } else {
        return Err(BlendFailure::TopologyViolation {
            detail: format!(
                "corner cap-arc edges do not form a closed triangle: edge 0 ends at \
                 vertex {:?} but neither of the other two cap arcs is incident to it",
                b
            ),
        });
    };

    let last_forward = if endpoints[last_idx].0 == c && endpoints[last_idx].1 == a {
        true
    } else if endpoints[last_idx].1 == c && endpoints[last_idx].0 == a {
        false
    } else {
        return Err(BlendFailure::TopologyViolation {
            detail: format!(
                "corner cap-arc edges do not form a closed triangle: a={:?}, b={:?}, \
                 c={:?}, but the remaining cap arc has endpoints {:?} — does not \
                 close the cycle",
                a, b, c, endpoints[last_idx]
            ),
        });
    };

    let mut forwards = [true; 3];
    forwards[0] = true;
    forwards[next_idx] = next_forward;
    forwards[last_idx] = last_forward;

    Ok(([a, b, c], forwards))
}

/// Solve for the point closest to every axis line `(q_i, u_i)`.
///
/// Each filleted incident contributes an axis line `L_i = { q_i + t·u_i }`.
/// A point C lies on `L_i` iff its projection onto the plane through
/// `q_i` perpendicular to `u_i` coincides with `q_i`. The orthogonal
/// projector onto that plane is `M_i = I − u_i u_iᵀ`, so the
/// condition is `M_i (C − q_i) = 0`. Stacking that condition across
/// every filleted incident gives an over-determined system whose
/// normal equations are
///
/// ```text
///     ( Σ M_i ) C = Σ M_i q_i
/// ```
///
/// The 3×3 matrix `A = Σ M_i` is positive-semidefinite. It is
/// invertible iff the axis directions `{u_i}` span ℝ³ — i.e. the
/// filleted edges are not all parallel and not all coplanar. The
/// rank-deficient cases are exactly the geometrically degenerate
/// vertex-blend inputs (one or two filleted edges, or three parallel
/// fillets); they are caught here by the `inverse()` error and turned
/// into a precise `InvalidGeometry` upstream.
fn compute_concurrent_axes_center(
    filleted: &[IncidentEdgeClassification],
    vertex_id: VertexId,
) -> OperationResult<Point3> {
    // Σ M_i and Σ M_i q_i.
    let mut a = [0.0_f64; 9]; // column-major 3x3 accumulator
    let mut b = Vector3::new(0.0, 0.0, 0.0);

    for incident in filleted {
        let blend = incident
            .blend
            .as_ref()
            .ok_or_else(|| OperationError::InvalidGeometry(
                "compute_concurrent_axes_center received an unclassified incident edge"
                    .to_string(),
            ))?;
        let u = blend.axis;
        let q = blend.axis_origin;

        // M = I - u uᵀ. Column-major: m[col*3 + row].
        let m00 = 1.0 - u.x * u.x;
        let m11 = 1.0 - u.y * u.y;
        let m22 = 1.0 - u.z * u.z;
        let m01 = -u.x * u.y;
        let m02 = -u.x * u.z;
        let m12 = -u.y * u.z;

        // Accumulate A += M. Symmetric, six unique entries.
        a[0] += m00;
        a[4] += m11;
        a[8] += m22;
        a[3] += m01; // col=1,row=0
        a[1] += m01; // col=0,row=1
        a[6] += m02; // col=2,row=0
        a[2] += m02; // col=0,row=2
        a[7] += m12; // col=2,row=1
        a[5] += m12; // col=1,row=2

        // M·q = q − (u·q) u.
        let dot = u.x * q.x + u.y * q.y + u.z * q.z;
        let mq = Vector3::new(q.x - dot * u.x, q.y - dot * u.y, q.z - dot * u.z);
        b.x += mq.x;
        b.y += mq.y;
        b.z += mq.z;
    }

    let mat = Matrix3::from_cols(a);

    // Rank-deficiency check tailored to the projector-sum problem.
    //
    // `Matrix3::inverse()` uses `f64::EPSILON` (~2.22e-16) as its
    // singularity cutoff, which is too permissive here: a rank-2
    // projector sum can have a floating-point-noise determinant of
    // ~1e-17, sneak past that gate, and yield a nonsense pseudo-
    // inverse. For our well-conditioned input scale (each M_i has
    // trace 2, so A has trace 2·n_filleted ≤ ~20 for any realistic
    // vertex) a rank-3 A has |det| on the order of 1.0; values
    // below 1e-9 unambiguously signal linear dependence among the
    // axis directions.
    let det = mat.determinant();
    if det.abs() < 1.0e-9 {
        return Err(OperationError::InvalidGeometry(format!(
            "vertex {:?}: filleted-edge axes span at most 2 dimensions (|det| = \
             {:.3e} below the 1e-9 rank-deficiency threshold) — corner sphere \
             placement requires axes that span ℝ³",
            vertex_id, det
        )));
    }

    let inv = mat.inverse().map_err(|_| {
        OperationError::InvalidGeometry(format!(
            "vertex {:?}: filleted-edge normal-equations matrix is singular \
             — corner sphere placement is undefined",
            vertex_id
        ))
    })?;

    let c = inv.transform_vector(&b);
    Ok(Point3::new(c.x, c.y, c.z))
}

/// Apex-sphere corner emission for the F5-α three-edge convex
/// equal-radius case.
///
/// Called from [`create_fillet_transitions`] once per
/// `BlendVertexKind::ConvexCorner { degree: 3 }` after the per-edge
/// cylinder fillets have been spliced in. The caller supplies the
/// pre-computed sphere geometry (centre / radius derived from
/// `compute_concurrent_axes_center` over the three cylindrical-
/// fillet axes) and the three fillet `FaceId`s; this helper performs
/// the surgery that closes the triangular hole:
///
/// 1. Locate the three V-side cap arcs on the incident fillet
///    faces. Each cap arc's centre coincides exactly with the
///    sphere centre by the F5-α invariant (rolling-ball tangent to
///    two original faces at radius `r` is geometrically unique on
///    a flat dihedral), so [`find_cap_arc_edge_at_vertex`] picks
///    out the right cap by centre-coincidence. Failure surfaces as
///    `BlendFailure::TopologyViolation` — that means the upstream
///    concurrent-axes promise was violated by a numerically off-
///    circle cap.
/// 2. Verify the three cap arcs form a closed triangular cycle
///    `P_A → P_B → P_C → P_A` and recover the per-arc forward /
///    backward orientation flag for the new sphere-face loop.
/// 3. Build a [`Sphere`] surface at `(sphere_center, sphere_radius)`,
///    pick an outward-pointing orientation, add a new face backed
///    by that surface and the three-edge loop, and register the
///    face on the solid's outer shell.
///
/// Per-corner Euler delta: ΔV = 0, ΔE = 0, ΔF = +1. The three cap
/// arcs were boundary edges (each used by exactly one fillet face's
/// loop); the new sphere face turns them into interior edges shared
/// between the sphere face and one fillet face each. V − E + F goes
/// from `2 − 1 = 1` (open triangular hole) to `2`, restoring
/// watertightness.
fn apply_apex_sphere_corner(
    model: &mut BRepModel,
    solid_id: SolidId,
    vertex_id: VertexId,
    fillet_face_ids: &[FaceId; 3],
    sphere_center: Point3,
    sphere_radius: f64,
    vertex_outward: Vector3,
) -> OperationResult<FaceId> {
    // Step 1 — locate the three V-side cap arcs.
    let mut cap_arc_edges: [EdgeId; 3] = [0; 3];
    for (i, &face_id) in fillet_face_ids.iter().enumerate() {
        cap_arc_edges[i] = find_cap_arc_edge_at_vertex(model, face_id, sphere_center)
            .ok_or_else(|| {
                OperationError::BlendFailed(Box::new(BlendFailure::VertexBlendUnsupported {
                    vertex: vertex_id,
                    kind: BlendVertexKind::ConvexCorner { degree: 3 },
                    reason: VertexBlendUnsupportedReason::NonManifoldNeighbourhood,
                }))
            })?;
    }

    // Step 2 — verify the cap arcs close a triangle and recover the
    // per-arc forward/backward flag for the sphere-face loop.
    let (_corner_vertices, loop_forwards) =
        verify_cap_arcs_form_closed_triangle(model, &cap_arc_edges)
            .map_err(|e| OperationError::BlendFailed(Box::new(e)))?;

    // Step 3 — build the sphere surface, pick an outward face
    // orientation, add the face and register it on the outer shell.
    let sphere = Sphere::new(sphere_center, sphere_radius).map_err(|e| {
        OperationError::NumericalError(format!("corner sphere construction failed: {:?}", e))
    })?;

    let orientation = orient_face_for_outward(&sphere, vertex_outward)?;
    let surface_id = model.surfaces.add(Box::new(sphere));

    let mut blend_loop = Loop::new(0, LoopType::Outer);
    for i in 0..3 {
        blend_loop.add_edge(cap_arc_edges[i], loop_forwards[i]);
    }
    let loop_id = model.loops.add(blend_loop);

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

    // F5-α.2 — drop the original sharp corner vertex.
    //
    // Each per-edge splice for the three corner edges kept `vertex_id`
    // alive because its `original_v*_corner_shared` flag was set (see
    // [`splice_blend_edge`]). After every cylindrical fillet's trim
    // arc has been spliced in and the apex sphere face has been added
    // to the outer shell, no edge in the model references `vertex_id`:
    //
    // * The three corner edges themselves are removed during their
    //   own [`splice_blend_edge`] calls.
    // * Each adjacent face's loop now references the apex-side
    //   P-vertex (P_a / P_b / P_c) where the two corresponding trim
    //   arcs meet, courtesy of `VertexStore::add_or_find`'s
    //   tolerance dedup at trim-arc construction time. The sharp
    //   corner is geometrically replaced by these three tangent
    //   points.
    //
    // We defensively scan the live edge store and only drop the
    // vertex if no edge still references it. If something upstream
    // diverged from the invariant — e.g. a fourth, non-corner blend
    // edge happened to share the vertex — leaving the vertex in the
    // store is safe; dropping it would invalidate that fourth edge.
    let still_referenced = model
        .edges
        .iter()
        .any(|(_, e)| e.start_vertex == vertex_id || e.end_vertex == vertex_id);
    if !still_referenced {
        model.vertices.remove(vertex_id);
    }

    Ok(face_id)
}

/// F5-β triangular-NURBS corner emission — mixed-radii path.
///
/// Called from [`create_fillet_transitions`] for a degree-3 convex
/// corner whose three incident fillet radii are not all equal. The
/// equal-radius case still routes through [`apply_apex_sphere_corner`].
///
/// # Steps
///
/// 1. Locate the V-side cap-arc edge on each incident fillet face by
///    cylinder-axis match ([`find_cap_arc_edge_by_cylinder_axis`]).
///    Failure → `NonManifoldNeighbourhood`.
/// 2. Compute per-cylinder cap centre
///    `C_i = q_i + ((A − q_i) · u_i) · u_i` for each i. Pure
///    arithmetic — cannot fail.
/// 3. Compute pairwise intersection points `P_{ij}` for `(i, j) ∈
///    {(0, 1), (1, 2), (2, 0)}` via [`intersect_two_caps`]. Failure
///    (axes parallel, no intersection, or singular linear solve) →
///    `NonManifoldNeighbourhood`.
/// 4. Trim each cap-arc edge in place so its endpoints sit at the
///    two `P_{ij}` it owns. [`trim_cap_arc_in_place`] inserts a new
///    `Arc` curve, rebinds the edge's `curve_id`, and replaces its
///    start/end vertices with deduped `VertexStore::add_or_find`
///    calls — so cap arcs `i` and `j` that share `P_{ij}` end up
///    referencing the same `VertexId`.
/// 5. Re-anchor every straight seam edge on each cylindrical fillet
///    face whose endpoint was the cap arc's old apex-side vertex —
///    rebuild that seam as a fresh `Line` from its unchanged far
///    end to the freshly-deduped `P_{ij}` vertex.
/// 6. Build a rational bi-quadratic NURBS surface with a degenerate
///    `u = 1` column collapsed to `P_{12}`. Each of the three
///    boundary cap arcs reproduces *exactly* on an isoparametric
///    line via Piegl-Tiller §7.5 eq. 7.31; the interior control
///    point and weight are the symmetric average of the three
///    off-arc tangent-intersection controls.
/// 7. Orient the new face via [`orient_face_for_outward`] sampled at
///    a non-degenerate `(u, v)` (avoid the `u = 1` collapsed
///    boundary). Build a 3-edge outer loop in the cycle order
///    recovered by [`verify_cap_arcs_form_closed_triangle`]. Add
///    the face to `model.faces` and register it on the solid's
///    outer shell.
/// 8. Drop the original sharp corner vertex if no edge in the live
///    model still references it (defensive — the F5-α surgery
///    pattern is preserved).
///
/// Per-corner Euler delta: ΔV = 0 (three new `P_{ij}` vertices
/// added during trim, three old apex-side vertices dropped when
/// their last seam reference disappears), ΔE = 0 (three cap arcs
/// re-parameterised, six seam edges re-anchored — no edges
/// added/removed), ΔF = +1 (the corner patch). V − E + F goes
/// from `2 − 1 = 1` (three-sided open hole) to `2`.
#[allow(clippy::too_many_arguments)]
fn apply_triangular_nurbs_corner(
    model: &mut BRepModel,
    solid_id: SolidId,
    vertex_id: VertexId,
    vertex_pos: Point3,
    fillet_face_ids: &[FaceId; 3],
    cylinder_axes: &[(Point3, Vector3, f64); 3],
    corner_apex: Point3,
    vertex_outward: Vector3,
) -> OperationResult<FaceId> {
    use crate::math::nurbs::NurbsSurface;
    use crate::primitives::face::FaceOrientation;
    use crate::primitives::surface::GeneralNurbsSurface;

    // Step 1 — locate the three V-side cap-arc edges by cylinder axis.
    let mut cap_arc_edges: [EdgeId; 3] = [0; 3];
    for i in 0..3 {
        let (q, u, _r) = cylinder_axes[i];
        cap_arc_edges[i] =
            find_cap_arc_edge_by_cylinder_axis(model, fillet_face_ids[i], q, u, corner_apex)
                .ok_or_else(|| {
                    OperationError::BlendFailed(Box::new(BlendFailure::VertexBlendUnsupported {
                        vertex: vertex_id,
                        kind: BlendVertexKind::ConvexCorner { degree: 3 },
                        reason: VertexBlendUnsupportedReason::NonManifoldNeighbourhood,
                    }))
                })?;
    }

    // Step 2 — per-cylinder cap centres C_i = q_i + ((A − q_i) · u_i) u_i.
    let mut cap_centres = [Point3::new(0.0, 0.0, 0.0); 3];
    for i in 0..3 {
        let (q, u, _) = cylinder_axes[i];
        let v = corner_apex - q;
        let t = v.x * u.x + v.y * u.y + v.z * u.z;
        cap_centres[i] = Point3::new(q.x + t * u.x, q.y + t * u.y, q.z + t * u.z);
    }

    // Step 3 — pairwise cap-circle intersection points.
    //
    // Convention: `p_ij[k]` is the intersection of caps `pairs[k].0`
    // and `pairs[k].1`. After trim, cap-arc `i`'s two endpoints are
    // the two `p_ij[k]` whose pair contains `i`:
    //
    //   * cap 0 owns P_{01} = p_ij[0] (pair 0-1) and P_{20} = p_ij[2] (pair 2-0)
    //   * cap 1 owns P_{12} = p_ij[1] (pair 1-2) and P_{01} = p_ij[0] (pair 0-1)
    //   * cap 2 owns P_{20} = p_ij[2] (pair 2-0) and P_{12} = p_ij[1] (pair 1-2)
    let pairs: [(usize, usize); 3] = [(0, 1), (1, 2), (2, 0)];
    let mut p_ij = [Point3::new(0.0, 0.0, 0.0); 3];
    for (k, &(i, j)) in pairs.iter().enumerate() {
        let (_, u_i, r_i) = cylinder_axes[i];
        let (_, u_j, r_j) = cylinder_axes[j];
        p_ij[k] = intersect_two_caps(
            cap_centres[i],
            u_i,
            r_i,
            cap_centres[j],
            u_j,
            r_j,
            vertex_pos,
            vertex_outward,
        )
        .map_err(|_| {
            OperationError::BlendFailed(Box::new(BlendFailure::VertexBlendUnsupported {
                vertex: vertex_id,
                kind: BlendVertexKind::ConvexCorner { degree: 3 },
                reason: VertexBlendUnsupportedReason::NonManifoldNeighbourhood,
            }))
        })?;
    }

    // Step 4 — trim cap arcs in place. Each cap arc keeps its centre
    // / axis / radius / x_axis frame; only `start_angle` /
    // `sweep_angle` and the two endpoint vertex ids change.
    //
    // Convention: cap-arc-i is trimmed from `p_prev_pair` (the
    // `P_{ki}` it shares with cap-arc-k where k = (i + 2) mod 3) to
    // `p_next_pair` (the `P_{ij}` it shares with cap-arc-j where
    // j = (i + 1) mod 3). This gives a consistent cycle:
    //
    //   cap 0: P_{20} → P_{01}
    //   cap 1: P_{01} → P_{12}
    //   cap 2: P_{12} → P_{20}    (intentionally reversed —
    //                              keeps the cycle direction
    //                              consistent so the new loop
    //                              traversal is non-twisted)
    //
    // The actual orientation of each cap arc within the patch's
    // loop is recovered by `verify_cap_arcs_form_closed_triangle`
    // below, so the trim direction here only needs to keep both
    // endpoints on the same circle — `trim_cap_arc_in_place` then
    // picks the V-side short/long sweep by midpoint-outward score.
    let endpoint_tolerance = Tolerance::default().distance();
    let mut old_vertex_pairs: [(VertexId, VertexId); 3] = [(0, 0); 3];
    let mut new_vertex_pairs: [(VertexId, VertexId); 3] = [(0, 0); 3];
    for i in 0..3 {
        let prev_pair_index = (i + 2) % 3; // P_{ki} pair
        let next_pair_index = i; // P_{ij} pair: pairs[i] = (i, i+1)
        let p_start_new = p_ij[prev_pair_index];
        let p_end_new = p_ij[next_pair_index];

        // Snapshot the cap arc's current endpoint vertex ids — the
        // seam edges in the same fillet face still reference these
        // and must be re-anchored to the new ids that
        // `trim_cap_arc_in_place` is about to install.
        let pre_trim_edge = model.edges.get(cap_arc_edges[i]).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "apply_triangular_nurbs_corner: cap arc edge {} missing before trim",
                cap_arc_edges[i]
            ))
        })?;
        old_vertex_pairs[i] = (pre_trim_edge.start_vertex, pre_trim_edge.end_vertex);

        trim_cap_arc_in_place(
            model,
            cap_arc_edges[i],
            p_start_new,
            p_end_new,
            vertex_pos,
            vertex_outward,
            endpoint_tolerance,
        )?;

        let post_trim_edge = model.edges.get(cap_arc_edges[i]).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "apply_triangular_nurbs_corner: cap arc edge {} missing after trim",
                cap_arc_edges[i]
            ))
        })?;
        new_vertex_pairs[i] = (post_trim_edge.start_vertex, post_trim_edge.end_vertex);
    }

    // Step 5 — re-anchor seam edges on each cylindrical fillet face.
    //
    // Cap arc i's old `(α_i, β_i)` are replaced by new `(γ_i, δ_i)`
    // (via `add_or_find` inside trim). The seam edges on the same
    // fillet face that referenced α_i or β_i must now reference
    // the matching new vertex; otherwise the fillet face's outer
    // loop is broken at the cap arc joints.
    for i in 0..3 {
        let (old_alpha, old_beta) = old_vertex_pairs[i];
        let (new_alpha, new_beta) = new_vertex_pairs[i];
        if old_alpha != new_alpha {
            reanchor_seam_edges_at_cap_arc_endpoint(
                model,
                fillet_face_ids[i],
                old_alpha,
                new_alpha,
            )?;
        }
        if old_beta != new_beta {
            reanchor_seam_edges_at_cap_arc_endpoint(
                model,
                fillet_face_ids[i],
                old_beta,
                new_beta,
            )?;
        }
    }

    // Step 6 — build the rational bi-quadratic NURBS patch.
    //
    // Each trimmed cap arc gives a rational-quadratic Bezier triple
    // (P_start, T_mid, P_end) with weights [1, cos(θ/2), 1]. The
    // 3×3 control net layout:
    //
    //     P[0][0] = cap_0.start  P[0][1] = cap_0.T_mid  P[0][2] = cap_0.end
    //     P[1][0] = cap_2.T_mid  P[1][1] = M_center     P[1][2] = cap_1.T_mid
    //     P[2][0] = cap_2.end    P[2][1] = degen P_12   P[2][2] = degen P_12
    //
    // where the v=0 / v=1 / u=0 boundaries reproduce cap-arc-2 /
    // cap-arc-1 / cap-arc-0 exactly, and the u=1 column collapses
    // to a single point (the shared `P_{12}`). M_center is the
    // simple average of the three T_mids, weighted by the simple
    // average of the three rational weights — this gives a
    // symmetric G0 corner patch with each cap arc on its assigned
    // isoparametric boundary.
    let mut cap_controls: [([Point3; 3], [f64; 3]); 3] =
        [([Point3::new(0.0, 0.0, 0.0); 3], [1.0; 3]); 3];
    for i in 0..3 {
        let edge = model.edges.get(cap_arc_edges[i]).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "apply_triangular_nurbs_corner: cap arc edge {} missing for control extraction",
                cap_arc_edges[i]
            ))
        })?;
        let curve_box = model.curves.get(edge.curve_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "apply_triangular_nurbs_corner: trimmed cap arc {}'s curve {} missing",
                cap_arc_edges[i], edge.curve_id
            ))
        })?;
        let arc = curve_box.as_any().downcast_ref::<Arc>().ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "apply_triangular_nurbs_corner: trimmed cap edge {} is no longer an Arc",
                cap_arc_edges[i]
            ))
        })?;
        cap_controls[i] = arc_to_rational_quadratic_controls(arc)?;
    }

    // cap-arc-0 was trimmed P_{20} → P_{01}; its rational-quad
    // controls are (P_{20}, T_0, P_{01}). cap-arc-1 was P_{01} →
    // P_{12}: (P_{01}, T_1, P_{12}). cap-arc-2 was P_{12} → P_{20}:
    // (P_{12}, T_2, P_{20}). The patch wants (P_{20}, T_2, P_{12})
    // along v=0, so we read cap-2's controls reversed.
    let (cap0_ctrl, cap0_w) = cap_controls[0];
    let (cap1_ctrl, cap1_w) = cap_controls[1];
    let (cap2_ctrl, cap2_w) = cap_controls[2];

    // p_20 / p_01 / p_12 are reproducible from the trimmed cap arcs
    // for assertion purposes; the actual patch uses the cap-arc
    // controls so any sub-tolerance drift between `p_ij[k]` and
    // the trimmed arc endpoint stays on the arc.
    let p_20 = cap0_ctrl[0];
    let p_01 = cap0_ctrl[2];
    let p_12 = cap1_ctrl[2];

    let t_0 = cap0_ctrl[1];
    let t_1 = cap1_ctrl[1];
    let t_2 = cap2_ctrl[1];
    let w_0 = cap0_w[1];
    let w_1 = cap1_w[1];
    let w_2 = cap2_w[1];

    let m_center = Point3::new(
        (t_0.x + t_1.x + t_2.x) / 3.0,
        (t_0.y + t_1.y + t_2.y) / 3.0,
        (t_0.z + t_1.z + t_2.z) / 3.0,
    );
    let w_center = (w_0 + w_1 + w_2) / 3.0;

    let control_points: Vec<Vec<Point3>> = vec![
        vec![p_20, t_0, p_01],
        vec![t_2, m_center, t_1],
        vec![p_12, p_12, p_12],
    ];
    let weights: Vec<Vec<f64>> = vec![
        vec![1.0, w_0, 1.0],
        vec![w_2, w_center, w_1],
        vec![1.0, 1.0, 1.0],
    ];
    let knots_open = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    let nurbs = NurbsSurface::new(
        control_points,
        weights,
        knots_open.clone(),
        knots_open,
        2,
        2,
    )
    .map_err(|e| {
        OperationError::NumericalError(format!(
            "F5-β triangular NURBS construction failed: {}",
            e
        ))
    })?;
    let corner_surface = GeneralNurbsSurface { nurbs };

    // Step 7 — orient the patch and stitch it into the topology.
    //
    // The parametric midpoint (u=0.5, v=0.5) is well-defined for
    // this patch (away from the u=1 degenerate column) and gives
    // a reliable interior normal sample.
    let orientation =
        crate::operations::orientation::orient_face_for_outward_at(
            &corner_surface,
            vertex_outward,
            0.5,
            0.5,
        )
        .unwrap_or(FaceOrientation::Forward);

    let (_corner_vertices, loop_forwards) =
        verify_cap_arcs_form_closed_triangle(model, &cap_arc_edges)
            .map_err(|e| OperationError::BlendFailed(Box::new(e)))?;

    let surface_id = model.surfaces.add(Box::new(corner_surface));
    let mut blend_loop = Loop::new(0, LoopType::Outer);
    for i in 0..3 {
        blend_loop.add_edge(cap_arc_edges[i], loop_forwards[i]);
    }
    let loop_id = model.loops.add(blend_loop);

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

    // Step 8 — drop the original sharp corner vertex if no edge
    // still references it. Same defensive pattern as
    // `apply_apex_sphere_corner` (Step 8 there).
    let still_referenced = model
        .edges
        .iter()
        .any(|(_, e)| e.start_vertex == vertex_id || e.end_vertex == vertex_id);
    if !still_referenced {
        model.vertices.remove(vertex_id);
    }

    // Defensive: also drop any of the *old* cap-arc apex-side
    // vertices that are now orphaned by the trim+reanchor pass.
    // Each `old_vertex_pairs[i]` slot may have been replaced via
    // `add_or_find` returning a new id; the originals are then
    // orphaned unless a non-corner edge happens to share them.
    let mut to_drop: HashSet<VertexId> = HashSet::new();
    for (old_alpha, old_beta) in &old_vertex_pairs {
        to_drop.insert(*old_alpha);
        to_drop.insert(*old_beta);
    }
    // Don't drop the same vertex we just kept above.
    to_drop.remove(&vertex_id);
    for vid in to_drop {
        // Only drop if no edge references it AND it wasn't reused
        // by add_or_find for one of the new endpoints.
        let reused = new_vertex_pairs
            .iter()
            .any(|(a, b)| *a == vid || *b == vid);
        if reused {
            continue;
        }
        let referenced = model
            .edges
            .iter()
            .any(|(_, e)| e.start_vertex == vid || e.end_vertex == vid);
        if !referenced {
            model.vertices.remove(vid);
        }
    }

    Ok(face_id)
}

/// Trim `arc_edge_id` so its 3D extent runs from `p_start_new` to
/// `p_end_new`, preserving the arc's center / normal / radius / x_axis
/// frame.
///
/// F5-β.2 surgery primitive. The cap arc on a cylindrical fillet face
/// is a sub-arc of the V-side cap circle (radius `r_i`, centred at
/// `C_i`, in the plane perpendicular to the cylinder axis `u_i`).
/// F5-α leaves the cap as a quarter-arc anchored at the apex; F5-β
/// re-anchors it to the pairwise intersection points `P_{ij}`,
/// `P_{ki}` computed by [`intersect_two_caps`].
///
/// Algorithm:
///
/// 1. Read the existing arc through `model.curves.get`. Use the stored
///    frame `(center, normal, x_axis)` directly. The `y_axis` is
///    `normal × x_axis` (matches `Arc::y_axis()` internally).
/// 2. Project each endpoint onto the frame:
///    `θ_P = atan2((P − C) · y_axis, (P − C) · x_axis)`.
/// 3. Sweep direction disambiguation. Take the *short-way* sweep
///    `Δθ = wrap_pi(θ_end − θ_start)`. Sample the midpoint of the
///    candidate arc at parameter `t = 0.5`. If
///    `(midpoint − vertex) · vertex_outward ≥ 0` the V-side test
///    passes — keep the short sweep. Otherwise flip to the long way
///    by adding `2π · sign(Δθ)` (so the midpoint lies on the opposite
///    side of the chord, which by construction is the V-side).
/// 4. Construct a fresh `Arc` *by direct struct literal* (not via
///    `Arc::new`, which would canonicalise `x_axis` and invalidate
///    the angle computations from step 2).
/// 5. Insert the new arc into `model.curves`, rebind `edge.curve_id`,
///    and replace `edge.start_vertex` / `edge.end_vertex` via
///    `VertexStore::add_or_find(x, y, z, tolerance)` so any other
///    consumer of the same intersection point shares the vertex id.
///
/// Returns `Err(OperationError::InvalidGeometry)` if either endpoint
/// is more than `tolerance` off the existing cap circle — that
/// indicates the upstream [`intersect_two_caps`] result was applied
/// to the wrong cap arc.
fn trim_cap_arc_in_place(
    model: &mut BRepModel,
    arc_edge_id: EdgeId,
    p_start_new: Point3,
    p_end_new: Point3,
    vertex: Point3,
    vertex_outward: Vector3,
    tolerance: f64,
) -> OperationResult<()> {
    // 1 — read the existing arc.
    let edge = model.edges.get(arc_edge_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "trim_cap_arc_in_place: edge {} not found",
            arc_edge_id
        ))
    })?;
    let curve_id = edge.curve_id;
    let edge_tolerance = edge.tolerance;

    let curve_box = model.curves.get(curve_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "trim_cap_arc_in_place: curve {} not found for edge {}",
            curve_id, arc_edge_id
        ))
    })?;
    let arc = curve_box.as_any().downcast_ref::<Arc>().ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "trim_cap_arc_in_place: edge {} does not back an Arc curve",
            arc_edge_id
        ))
    })?;
    let center = arc.center;
    let normal = arc.normal;
    let x_axis = arc.x_axis;
    let radius = arc.radius;
    let range = arc.range.clone();

    // y_axis = normal × x_axis (matches Arc::y_axis()).
    let y_axis = normal.cross(&x_axis);

    // 2 — project both new endpoints onto the frame, verify on-circle.
    let project = |p: Point3| -> OperationResult<(f64, f64, f64)> {
        let d = Vector3::new(p.x - center.x, p.y - center.y, p.z - center.z);
        let cx = d.x * x_axis.x + d.y * x_axis.y + d.z * x_axis.z;
        let cy = d.x * y_axis.x + d.y * y_axis.y + d.z * y_axis.z;
        let cz = d.x * normal.x + d.y * normal.y + d.z * normal.z;
        // Tolerance gate: in-plane radial deviation + out-of-plane
        // deviation. Both must fit within the caller-supplied
        // `tolerance` (typed kernel tolerance, not a hard literal).
        let radial = (cx * cx + cy * cy).sqrt();
        if (radial - radius).abs() > tolerance || cz.abs() > tolerance {
            return Err(OperationError::InvalidGeometry(format!(
                "trim_cap_arc_in_place: endpoint deviates from cap circle by \
                 radial = {:.3e}, axial = {:.3e} (tolerance = {:.3e})",
                (radial - radius).abs(),
                cz.abs(),
                tolerance
            )));
        }
        Ok((cx, cy, cz))
    };
    let (sx, sy, _) = project(p_start_new)?;
    let (ex, ey, _) = project(p_end_new)?;

    let theta_start = sy.atan2(sx);
    let theta_end = ey.atan2(ex);

    // 3 — sweep-direction disambiguation. Compute the two candidate
    // sweeps (short way + long way) and pick whichever midpoint scores
    // higher against `vertex_outward`. We do NOT gate on
    // `score >= 0` — F5-α's cube-corner geometry yields both midpoints
    // with negative outward scores (the cap circle radius is smaller
    // than the corner-to-centre distance), but the *less negative*
    // score still correctly identifies the V-side sub-arc. This
    // matches the convention in [`intersect_two_caps`].
    let mut sweep_short = theta_end - theta_start;
    while sweep_short > std::f64::consts::PI {
        sweep_short -= std::f64::consts::TAU;
    }
    while sweep_short <= -std::f64::consts::PI {
        sweep_short += std::f64::consts::TAU;
    }
    let sweep_long = if sweep_short >= 0.0 {
        sweep_short - std::f64::consts::TAU
    } else {
        sweep_short + std::f64::consts::TAU
    };

    let outward_score_for = |sweep: f64| -> f64 {
        let mid_angle = theta_start + 0.5 * sweep;
        let (sin_m, cos_m) = mid_angle.sin_cos();
        let mid_offset_x = radius * cos_m;
        let mid_offset_y = radius * sin_m;
        let midpoint = Point3::new(
            center.x + mid_offset_x * x_axis.x + mid_offset_y * y_axis.x,
            center.y + mid_offset_x * x_axis.y + mid_offset_y * y_axis.y,
            center.z + mid_offset_x * x_axis.z + mid_offset_y * y_axis.z,
        );
        (midpoint.x - vertex.x) * vertex_outward.x
            + (midpoint.y - vertex.y) * vertex_outward.y
            + (midpoint.z - vertex.z) * vertex_outward.z
    };
    let sweep = if outward_score_for(sweep_short) >= outward_score_for(sweep_long) {
        sweep_short
    } else {
        sweep_long
    };

    // 4 — construct the trimmed arc by direct struct literal to
    // preserve the existing x_axis frame (Arc::new canonicalises it).
    let trimmed = Arc {
        center,
        normal,
        x_axis,
        radius,
        start_angle: theta_start,
        sweep_angle: sweep,
        range,
    };

    // 5 — commit: insert curve, rebind, retarget vertices.
    let new_curve_id = model.curves.add(Box::new(trimmed));
    let new_start_vertex =
        model
            .vertices
            .add_or_find(p_start_new.x, p_start_new.y, p_start_new.z, tolerance);
    let new_end_vertex =
        model
            .vertices
            .add_or_find(p_end_new.x, p_end_new.y, p_end_new.z, tolerance);

    let edge_mut = model.edges.get_mut(arc_edge_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "trim_cap_arc_in_place: edge {} disappeared between read and write",
            arc_edge_id
        ))
    })?;
    edge_mut.curve_id = new_curve_id;
    edge_mut.start_vertex = new_start_vertex;
    edge_mut.end_vertex = new_end_vertex;
    if edge_mut.tolerance < edge_tolerance {
        edge_mut.tolerance = edge_tolerance;
    }
    Ok(())
}

/// Re-anchor every non-arc edge in `fillet_face_id`'s outer loop that
/// terminated at `old_p_vertex` to terminate at `new_p_vertex` instead.
///
/// F5-β.2 sibling of [`trim_cap_arc_in_place`]. After a cap arc on a
/// cylindrical fillet face is trimmed so its endpoint moves from the
/// F5-α apex P-vertex to the F5-β intersection point `P_{ij}`, the
/// two seam edges on the same fillet face that shared the old apex
/// vertex must be retargeted so the face's outer loop stays closed.
///
/// Cylindrical fillet faces in the kernel use straight-line seam
/// edges (cylinder generators along the axis direction). The
/// re-anchor therefore preserves the far end of each seam and
/// replaces the V-side end with the new `P_{ij}` position — a fresh
/// `Line` curve with the same `(far, near)` orientation as the
/// original.
///
/// No-op for edges in the loop whose endpoints don't reference
/// `old_p_vertex`; safe to call once per cap-arc endpoint.
fn reanchor_seam_edges_at_cap_arc_endpoint(
    model: &mut BRepModel,
    fillet_face_id: FaceId,
    old_p_vertex: VertexId,
    new_p_vertex: VertexId,
) -> OperationResult<()> {
    let face = model.faces.get(fillet_face_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "reanchor_seam_edges_at_cap_arc_endpoint: face {} not found",
            fillet_face_id
        ))
    })?;
    let outer_loop_id = face.outer_loop;
    let loop_ref = model.loops.get(outer_loop_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "reanchor_seam_edges_at_cap_arc_endpoint: loop {} not found",
            outer_loop_id
        ))
    })?;
    let edge_ids: Vec<EdgeId> = loop_ref.edges.clone();

    let new_pos = model.vertices.get_position(new_p_vertex).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "reanchor_seam_edges_at_cap_arc_endpoint: new vertex {} not found",
            new_p_vertex
        ))
    })?;
    let new_point = Point3::new(new_pos[0], new_pos[1], new_pos[2]);

    for edge_id in edge_ids {
        let edge = match model.edges.get(edge_id) {
            Some(e) => e,
            None => continue,
        };
        let curve = match model.curves.get(edge.curve_id) {
            Some(c) => c,
            None => continue,
        };
        // Skip arc edges — those are the cap arcs themselves and are
        // handled by `trim_cap_arc_in_place`. Only refresh straight
        // seam edges (cylinder generators).
        if curve.as_any().downcast_ref::<Line>().is_none() {
            continue;
        }
        let replaces_start = edge.start_vertex == old_p_vertex;
        let replaces_end = edge.end_vertex == old_p_vertex;
        if !replaces_start && !replaces_end {
            continue;
        }
        // Read the far-end position before we touch anything else.
        let far_vertex = if replaces_start {
            edge.end_vertex
        } else {
            edge.start_vertex
        };
        let far_pos = model.vertices.get_position(far_vertex).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "reanchor_seam_edges_at_cap_arc_endpoint: far vertex {} of edge \
                 {} not found",
                far_vertex, edge_id
            ))
        })?;
        let far_point = Point3::new(far_pos[0], far_pos[1], far_pos[2]);

        // Preserve start→end direction. If the old apex vertex was the
        // edge's start, the new line goes new_point → far_point; if it
        // was the end, far_point → new_point.
        let (line_start, line_end) = if replaces_start {
            (new_point, far_point)
        } else {
            (far_point, new_point)
        };
        let new_curve_id = model.curves.add(Box::new(Line::new(line_start, line_end)));

        let edge_mut = model.edges.get_mut(edge_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "reanchor_seam_edges_at_cap_arc_endpoint: edge {} disappeared",
                edge_id
            ))
        })?;
        edge_mut.curve_id = new_curve_id;
        if replaces_start {
            edge_mut.start_vertex = new_p_vertex;
        } else {
            edge_mut.end_vertex = new_p_vertex;
        }
    }
    Ok(())
}

/// Rational-quadratic Bezier control net + weights for the arc
/// `arc`. Piegl-Tiller §7.5 eq. 7.31.
///
/// Returns `([P_start, T_mid, P_end], [1.0, cos(half_sweep), 1.0])`.
/// `T_mid` is the intersection of the two end-tangents — placed at
/// `C + (r / cos(half_sweep)) · (cos(α+θ/2) · x_axis + sin(α+θ/2) ·
/// y_axis)`. The resulting rational quadratic reproduces the arc
/// *exactly* on its parameter domain (not an approximation).
///
/// **Precondition**: `|arc.sweep_angle| < π`. For sweeps that meet or
/// exceed a half-circle the weight `cos(θ/2)` vanishes and `T_mid`
/// diverges; the arc must be split into multiple rational-quadratic
/// segments first. F5-β cap arcs always satisfy this — they start as
/// quarter-arcs (sweep ≤ π/2) and trim can only shrink them — so the
/// helper guards instead of splitting. Callers see
/// `Err(OperationError::InvalidGeometry)` when the precondition
/// fails.
fn arc_to_rational_quadratic_controls(
    arc: &Arc,
) -> OperationResult<([Point3; 3], [f64; 3])> {
    let sweep = arc.sweep_angle;
    if !sweep.is_finite() || sweep.abs() >= std::f64::consts::PI {
        return Err(OperationError::InvalidGeometry(format!(
            "arc_to_rational_quadratic_controls: sweep_angle = {:.6} rad must \
             satisfy 0 < |sweep| < π for a single rational-quadratic segment",
            sweep
        )));
    }
    let half = 0.5 * sweep;
    let w_mid = half.cos();
    if w_mid <= 0.0 {
        return Err(OperationError::InvalidGeometry(format!(
            "arc_to_rational_quadratic_controls: weight cos(sweep/2) = {:.3e} \
             is non-positive — arc sweep crosses ±π singularity",
            w_mid
        )));
    }

    let center = arc.center;
    let x_axis = arc.x_axis;
    let y_axis = arc.normal.cross(&arc.x_axis);
    let radius = arc.radius;

    let alpha = arc.start_angle;
    let beta = alpha + sweep;
    let theta_mid = alpha + half;

    let (sin_a, cos_a) = alpha.sin_cos();
    let (sin_b, cos_b) = beta.sin_cos();
    let (sin_m, cos_m) = theta_mid.sin_cos();

    // P_start, P_end exactly on the circle.
    let p_start = Point3::new(
        center.x + radius * (cos_a * x_axis.x + sin_a * y_axis.x),
        center.y + radius * (cos_a * x_axis.y + sin_a * y_axis.y),
        center.z + radius * (cos_a * x_axis.z + sin_a * y_axis.z),
    );
    let p_end = Point3::new(
        center.x + radius * (cos_b * x_axis.x + sin_b * y_axis.x),
        center.y + radius * (cos_b * x_axis.y + sin_b * y_axis.y),
        center.z + radius * (cos_b * x_axis.z + sin_b * y_axis.z),
    );
    // T_mid = C + (r / w_mid) · (cos(mid)·x + sin(mid)·y).
    let r_over_w = radius / w_mid;
    let t_mid = Point3::new(
        center.x + r_over_w * (cos_m * x_axis.x + sin_m * y_axis.x),
        center.y + r_over_w * (cos_m * x_axis.y + sin_m * y_axis.y),
        center.z + r_over_w * (cos_m * x_axis.z + sin_m * y_axis.z),
    );
    Ok(([p_start, t_mid, p_end], [1.0, w_mid, 1.0]))
}

// `gather_vertex_blend_context` and `create_vertex_blend` were the
// pre-F5-α scaffolding for the standalone `fillet_vertices` public
// API. F5-α moves corner emission inline into `fillet_edges` via
// `create_fillet_transitions` walking the BlendGraph, so the
// standalone API and both helpers are gone. The surgery they
// performed survives, factored as `apply_apex_sphere_corner` above
// and the unchanged `find_cap_arc_edge_at_vertex` /
// `verify_cap_arcs_form_closed_triangle` /
// `compute_concurrent_axes_center` helpers above.

/// Look up `edge_id` in `face_id`'s outer + inner loops and return the
/// orientation sign (+1.0 if the edge appears forward in the loop,
/// -1.0 if backward). `None` if the edge is not present in any of the
/// face's loops — that indicates a topology bug upstream.
///
/// Used to project the edge curve's parameter-direction tangent into
/// the loop-traversal direction of `face_id`. The signed dihedral
/// returned by `robust_face_angle` is only a geometric invariant
/// (positive ⇒ convex, negative ⇒ concave) when the tangent it sees
/// is the loop tangent of one of the two adjacent faces. The raw
/// curve tangent flips sign across edges that happen to be
/// parameterized against face1's CCW loop, so a classifier built on
/// `angle.signum()` with the curve tangent is wrong for ~half of
/// edges in any given solid.
pub(crate) fn edge_orientation_in_face(
    model: &BRepModel,
    face_id: FaceId,
    edge_id: EdgeId,
) -> Option<f64> {
    let face = model.faces.get(face_id)?;
    for loop_id in face.all_loops() {
        let lp = model.loops.get(loop_id)?;
        for (i, &eid) in lp.edges.iter().enumerate() {
            if eid == edge_id {
                return Some(if lp.orientations[i] { 1.0 } else { -1.0 });
            }
        }
    }
    None
}

/// Pure sign-counting kernel for inflection detection.
///
/// Returns `true` iff `signs` contains at least one entry strictly
/// greater than `+threshold` AND at least one entry strictly less
/// than `-threshold`. Entries with `|s| <= threshold` are treated as
/// indeterminate (numerical noise near zero) and never contribute to
/// classification.
///
/// Factored out so the sign-flip logic can be unit-tested independent
/// of the BRep geometry machinery that feeds it.
fn signs_indicate_inflection(signs: &[f64], threshold: f64) -> bool {
    let has_pos = signs.iter().any(|&s| s > threshold);
    let has_neg = signs.iter().any(|&s| s < -threshold);
    has_pos && has_neg
}

/// Detect a dihedral-sign inflection along `edge_id` between
/// `face1_id` and `face2_id` (Task #98 slice 1).
///
/// Samples the signed dihedral at `sample_count` parameter values
/// uniformly spaced over `[0, 1]` and asks `signs_indicate_inflection`
/// whether the resulting sign vector flips. The angle at each sample
/// is computed the same way `compute_rolling_ball_positions` computes
/// its midpoint angle — surface normals projected at the edge point,
/// edge tangent rotated into `face1`'s loop direction, fed through
/// `robust_face_angle`. The sign-flip threshold is `0.05 rad ≈ 2.86°`,
/// well below the existing near-tangent gate's `0.1 rad ≈ 5.73°`,
/// so an edge whose dihedral straddles zero by more than the noise
/// floor is classified as inflection.
///
/// The current rolling-ball pipeline classifies an edge as convex or
/// concave based on a *single* midpoint sample of the dihedral and
/// commits the ball offset to that side for the whole edge. An
/// inflection edge — convex at one end, concave at the other — fed
/// to this pipeline produces a blend surface that caves outward on
/// the convex segment and inward on the concave segment, meeting in
/// a singular self-intersecting fold at the inflection parameter.
/// Slice 1 detects this case and rejects it with a clear diagnostic;
/// slice 2 will split the edge at the inflection parameter and
/// fillet each sub-segment with its sign-appropriate orientation.
fn detect_dihedral_inflection(
    model: &BRepModel,
    edge: &Edge,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    sample_count: usize,
) -> OperationResult<bool> {
    // Need ≥ 2 distinct parameter samples to spot a sign change.
    // Caller passes 11 by default; clamp defensively here so an
    // accidental 0 or 1 just no-ops to "no inflection found".
    if sample_count < 2 {
        return Ok(false);
    }

    // The face/surface stores are still consulted here only for the
    // existence check — actual normal evaluation goes through
    // `get_face_oriented_normal` which re-fetches both per call so
    // it can apply the face's orientation sign.
    let _ = model
        .faces
        .get(face1_id)
        .ok_or_else(|| OperationError::InvalidGeometry(format!("Face {} missing", face1_id)))?;
    let _ = model
        .faces
        .get(face2_id)
        .ok_or_else(|| OperationError::InvalidGeometry(format!("Face {} missing", face2_id)))?;
    let face1_loop_sign = edge_orientation_in_face(model, face1_id, edge_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "Edge {} not present in any loop of face {}",
            edge_id, face1_id
        ))
    })?;

    let tol = Tolerance::default();
    let mut signs = Vec::with_capacity(sample_count);
    for i in 0..sample_count {
        // Uniform sweep across the parameter interval: t = i / (n-1).
        let t = i as f64 / (sample_count - 1) as f64;
        let p = edge.evaluate(t, &model.curves)?;
        let n1 = get_face_oriented_normal(model, face1_id, &p)?;
        let n2 = get_face_oriented_normal(model, face2_id, &p)?;
        let tangent = edge.tangent_at(t, &model.curves)? * face1_loop_sign;
        let angle = robust_face_angle(&n1, &n2, &tangent, &tol)?;
        signs.push(angle);
    }

    // 0.05 rad (~2.86°) noise floor — below the near-tangent gate's
    // 0.1 rad but well above floating-point noise on the angle
    // computation. Edges that straddle zero by less than this are
    // treated as "effectively tangent at that sample" and skipped
    // rather than being miscounted as a sign flip.
    Ok(signs_indicate_inflection(&signs, 0.05))
}

/// Data for rolling ball fillet computation
struct RollingBallData {
    /// Center positions of rolling ball along edge
    centers: Vec<Point3>,
    /// Contact points on first face
    contacts1: Vec<Point3>,
    /// Contact points on second face
    contacts2: Vec<Point3>,
    /// Parameter values along edge
    parameters: Vec<f64>,
    /// Radius at each position
    radii: Vec<f64>,
}

/// Convert a F3-α [`SpineRail`](super::spine_solver::SpineRail) into the
/// legacy [`RollingBallData`] shape so the existing downstream pipeline
/// (`create_rolling_ball_surface`, `compute_fillet_trim_curves`, cap
/// construction) consumes analytic-arm output without modification.
///
/// This is the parallel-deployment bridge: F3-α produces a richer
/// `SpineRail` (with explicit `solver_kind` + spine/rail curves), but
/// the downstream code still reads sample arrays. F3-δ will replace
/// the downstream readers with `SpineRail` consumers directly and
/// delete this converter.
fn rolling_ball_data_from_spine_rail(
    spine_rail: &super::spine_solver::SpineRail,
) -> RollingBallData {
    let n = spine_rail.samples.len();
    let mut data = RollingBallData {
        centers: Vec::with_capacity(n),
        contacts1: Vec::with_capacity(n),
        contacts2: Vec::with_capacity(n),
        parameters: Vec::with_capacity(n),
        radii: Vec::with_capacity(n),
    };
    for s in &spine_rail.samples {
        data.parameters.push(s.edge_parameter);
        data.radii.push(s.radius);
        data.centers.push(s.center);
        data.contacts1.push(s.contact_a);
        data.contacts2.push(s.contact_b);
    }
    data
}

/// Get the face's outward-oriented surface normal at a 3D point.
///
/// Half the faces of any solid have `FaceOrientation::Backward` —
/// the kernel records the surface's parametric normal (from `du × dv`)
/// and a per-face sign that says "this face flips it" so the *outward*
/// normal is consistent across the solid. `Face::normal_at(u, v, …)`
/// applies that flip; this helper does the same for code that has a
/// `Point3` rather than `(u, v)`.
///
/// Every fillet path that computes a signed dihedral or a rolling-ball
/// bisector relies on **outward** normals — calling
/// `robust_surface_normal` directly on the raw surface was a latent
/// bug that silently produced correct fillets for the half of faces
/// whose orientation happens to be `Forward` and concave-flipped
/// fillets for the other half. The user-visible symptom was "one edge
/// fillets convex, the next is concave" on a freshly extruded box.
pub(crate) fn get_face_oriented_normal(
    model: &BRepModel,
    face_id: FaceId,
    point: &Point3,
) -> OperationResult<Vector3> {
    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry(format!("Face {} not found", face_id)))?;
    let surface = model.surfaces.get(face.surface_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!("Surface {} not found", face.surface_id))
    })?;
    let tolerance = &Tolerance::default();
    let (u, v) = project_point_to_surface(point, surface, (0.5, 0.5), tolerance, 100)?;
    let normal = robust_surface_normal(surface, u, v, tolerance).map_err(|e| {
        OperationError::NumericalError(format!("Surface normal evaluation failed: {:?}", e))
    })?;
    Ok(normal * face.orientation.sign())
}

/// Create surface from rolling ball data.
///
/// **Legacy heuristic dispatch** retained for callers that do not
/// produce a [`SpineRail`] — today that is exclusively the variable-
/// radius path (`create_variable_radius_fillet`) which still routes
/// through `compute_variable_rolling_ball_positions`'s legacy
/// bisector. The constant-radius path now goes through
/// [`create_blend_surface_for_carrier`] which dispatches on the
/// solver's `solver_kind` rather than re-deriving the geometry from
/// sample arrays.
fn create_rolling_ball_surface(data: &RollingBallData) -> OperationResult<Box<dyn Surface>> {
    // Analyze the rolling ball data to determine surface type
    let is_straight_edge = is_edge_straight(data);
    let is_constant_radius = is_radius_constant(data);

    if is_straight_edge && is_constant_radius {
        // Create cylindrical fillet
        create_cylindrical_fillet_surface(data)
    } else if !is_straight_edge && is_constant_radius {
        // Create toroidal fillet
        create_toroidal_fillet_surface(data)
    } else {
        // Create general NURBS fillet for variable radius
        create_nurbs_fillet_surface(data)
    }
}

/// F4-α — carrier-driven blend surface construction.
///
/// Route the [`BlendSurfaceCarrier`] derived from the
/// [`super::spine_solver::SpineRail`] to the appropriate surface
/// constructor.
///
/// * **F4-α.1** landed the carrier enum + dispatch table.
/// * **F4-α.2** (this version) splits the dispatch by both the
///   carrier *and* the underlying [`SolverKind`]: analytic arms route
///   through `*::from_analytic_kpart` constructors that consume the
///   spine solver's analytic-exact curves directly (no 20-sample
///   frame derivation, no 3-point circumscribed-circle estimate),
///   mirroring OCCT's `ChFiKPart_ComputeData` shape. The marching arm
///   and any analytic case whose face surfaces fail to downcast fall
///   back to the legacy sample-based constructors.
///
/// The dispatcher reads supporting-face analytic data
/// (`Plane.normal`, `Cylinder.axis/radius`, `Sphere.center/radius`)
/// directly from `model.surfaces`, then computes:
///
/// * Toroidal `major_radius` — taken from the spine [`Arc`]'s exact
///   `radius` field when the spine downcasts, cross-checked against
///   `r_cyl ± r_fillet` (plane/cylinder) or the closed-form
///   `√(R² − d² + 2r(R + d))` (plane/sphere). A discrepancy beyond
///   `2·tolerance.distance()` is a spine-solver bug; we fall back to
///   the legacy sample-based constructor and let downstream
///   verification flag the inconsistency rather than silently
///   building a wrong-radius torus.
/// * Toroidal `angle_bounds` — `(0, π − |dihedral|)` for convex,
///   `(0, |dihedral|)` for concave, with the dihedral re-derived from
///   the supporting face normals at the edge midpoint.
fn create_blend_surface_for_carrier(
    carrier: super::blend_surface_carrier::BlendSurfaceCarrier,
    spine_rail: &super::spine_solver::SpineRail,
    data: &RollingBallData,
    model: &BRepModel,
    face_a_id: FaceId,
    face_b_id: FaceId,
) -> OperationResult<Box<dyn Surface>> {
    use super::blend_surface_carrier::BlendSurfaceCarrier;
    use super::spine_solver::SolverKind;

    match carrier {
        BlendSurfaceCarrier::Cylindrical => match spine_rail.solver_kind {
            SolverKind::AnalyticPlanePlane | SolverKind::AnalyticCylCylCoaxial => {
                build_cylindrical_kpart_from_spine(spine_rail, data)
            }
            _ => create_cylindrical_fillet_surface(data),
        },
        BlendSurfaceCarrier::Toroidal => match spine_rail.solver_kind {
            SolverKind::AnalyticPlaneCylinder => {
                build_toroidal_kpart_plane_cylinder(spine_rail, data, model, face_a_id, face_b_id)
            }
            SolverKind::AnalyticPlaneSphere => {
                build_toroidal_kpart_plane_sphere(spine_rail, data, model, face_a_id, face_b_id)
            }
            _ => create_toroidal_fillet_surface(data),
        },
        BlendSurfaceCarrier::GeneralNurbs => create_nurbs_fillet_surface(data),
    }
}

/// F4-α.2 — cylindrical kpart construction for the analytic plane/
/// plane and coaxial cyl/cyl arms.
///
/// Clones the spine_rail's analytic-exact curves (a [`Line`] for both
/// arms in the constant-radius case) and feeds them to
/// [`CylindricalFillet::from_analytic_kpart`]. That constructor reads
/// the (z, x, y) frame once at the spine midpoint rather than 20×.
///
/// If any of the curve clones or the radius is malformed the
/// dispatcher falls back to the legacy sample-based path. The fall-
/// back path is observably distinguishable in tests via the
/// `axis_field`/`frame_x_field` length (2 for kpart, 20 for legacy);
/// the integration suite asserts the kpart path is taken for the
/// analytic test models.
fn build_cylindrical_kpart_from_spine(
    spine_rail: &super::spine_solver::SpineRail,
    data: &RollingBallData,
) -> OperationResult<Box<dyn Surface>> {
    let radius = match data.radii.first() {
        Some(&r) if r > 0.0 => r,
        _ => return create_cylindrical_fillet_surface(data),
    };
    let spine = spine_rail.spine.clone_box();
    let contact1 = spine_rail.rail_a.clone_box();
    let contact2 = spine_rail.rail_b.clone_box();
    match CylindricalFillet::from_analytic_kpart(spine, radius, contact1, contact2) {
        Ok(fillet) => Ok(Box::new(fillet)),
        Err(_) => create_cylindrical_fillet_surface(data),
    }
}

/// F4-α.2 — toroidal kpart construction for the analytic plane/
/// cylinder arm.
///
/// Reads the supporting [`Cylinder`] surface from the model, extracts
/// the spine [`crate::primitives::curve::Arc`] descriptor, and
/// validates that the spine arc's radius matches the closed-form
/// `r_cyl ± r_fillet` value to within `2·tolerance`. The closed-form
/// value (computed from the spine midpoint's signed offset from the
/// cylinder axis) is used as the torus major radius — this is exact
/// to f64 precision and decoupled from the spine fit's sample density.
///
/// `angle_bounds` is recovered from the spine arc's `sweep_angle`
/// (always (0, π/2) for a 90° dihedral, generalised here so that
/// non-perpendicular plane/cylinder cases route through the same
/// kpart entry point). Falls back to the legacy 3-point sampling
/// constructor on any downcast / inconsistency.
fn build_toroidal_kpart_plane_cylinder(
    spine_rail: &super::spine_solver::SpineRail,
    data: &RollingBallData,
    model: &BRepModel,
    face_a_id: FaceId,
    face_b_id: FaceId,
) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::curve::Arc;
    use crate::primitives::surface::Cylinder;

    let fillet_radius = match data.radii.first() {
        Some(&r) if r > 0.0 => r,
        _ => return create_toroidal_fillet_surface(data),
    };

    // Locate the cylinder face. Either of face_a/face_b can carry it.
    let cylinder = match read_cylinder_face(model, face_a_id, face_b_id) {
        Some(cyl) => cyl,
        None => return create_toroidal_fillet_surface(data),
    };

    // Major radius read from the spine arc's analytic descriptor when
    // available, cross-checked against `|r_cyl ± r_fillet|` via the
    // signed distance from spine midpoint to cylinder axis.
    let spine_arc = spine_rail.spine.as_any().downcast_ref::<Arc>();
    let spine_mid = match spine_rail.spine.evaluate(0.5) {
        Ok(p) => p.position,
        Err(_) => return create_toroidal_fillet_surface(data),
    };
    let offset = spine_mid - cylinder.origin;
    let axial = offset.dot(&cylinder.axis);
    let radial_vec = offset - cylinder.axis * axial;
    let major_radius_from_axis = radial_vec.magnitude();
    let major_radius = match spine_arc {
        Some(arc) => {
            // Cross-check: spine arc radius and the cylinder-axis
            // distance must agree to ~tolerance. They are the same
            // geometric quantity computed two ways.
            let drift = (arc.radius - major_radius_from_axis).abs();
            let tol = 1e-6_f64.max(arc.radius * 1e-9);
            if drift > tol {
                // Spine solver and cylinder geometry disagree — bail
                // to legacy path so the inconsistency surfaces in
                // sew/verify rather than as a wrong-radius torus.
                return create_toroidal_fillet_surface(data);
            }
            arc.radius
        }
        None => major_radius_from_axis,
    };
    if major_radius <= 0.0 || !major_radius.is_finite() {
        return create_toroidal_fillet_surface(data);
    }

    // Angle bounds. For the standard 90° dihedral the rolling-ball
    // sweep is exactly π/2; non-perpendicular plane/cylinder cases
    // still route through the same analytic arm, so honour the spine
    // arc's `sweep_angle` when readable, otherwise default to π/2.
    let angle_bounds = match spine_arc {
        Some(arc) if arc.sweep_angle > 0.0 && arc.sweep_angle.is_finite() => {
            (0.0, std::f64::consts::FRAC_PI_2.min(arc.sweep_angle.abs()))
        }
        _ => (0.0, std::f64::consts::FRAC_PI_2),
    };

    let center_curve = spine_rail.spine.clone_box();
    let contact1 = spine_rail.rail_a.clone_box();
    let contact2 = spine_rail.rail_b.clone_box();
    match ToroidalFillet::from_analytic_kpart(
        center_curve,
        fillet_radius,
        contact1,
        contact2,
        major_radius,
        angle_bounds,
    ) {
        Ok(fillet) => Ok(Box::new(fillet)),
        Err(_) => create_toroidal_fillet_surface(data),
    }
}

/// F4-α.2 — toroidal kpart construction for the analytic plane/
/// sphere arm.
///
/// Reads the supporting [`Sphere`] surface from the model and uses
/// the closed-form spine-circle radius
/// `√(R² − d² + 2·r·(R + d))` for the convex blend / `√(R² − d² − 2·r·(R − d))`
/// for the concave blend, where `R` is the sphere radius, `r` the
/// fillet radius, and `d` the signed distance from the sphere centre
/// to the plane. The convex/concave sign is recovered from the
/// spine arc midpoint's offset against the sphere centre + plane
/// normal. Cross-checks against the spine arc's `radius` field; any
/// drift beyond `2·tolerance` routes back to the legacy constructor.
fn build_toroidal_kpart_plane_sphere(
    spine_rail: &super::spine_solver::SpineRail,
    data: &RollingBallData,
    model: &BRepModel,
    face_a_id: FaceId,
    face_b_id: FaceId,
) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::curve::Arc;
    use crate::primitives::surface::Sphere;

    let fillet_radius = match data.radii.first() {
        Some(&r) if r > 0.0 => r,
        _ => return create_toroidal_fillet_surface(data),
    };

    let sphere = match read_sphere_face(model, face_a_id, face_b_id) {
        Some(s) => s,
        None => return create_toroidal_fillet_surface(data),
    };

    // Spine arc midpoint — exact for the analytic arm where the spine
    // is an `Arc` in the (plane offset by ±r_fillet) plane.
    let spine_mid = match spine_rail.spine.evaluate(0.5) {
        Ok(p) => p.position,
        Err(_) => return create_toroidal_fillet_surface(data),
    };

    // Distance from spine midpoint to sphere centre. The rolling ball
    // centre sits at distance `R + r_fillet` (convex) or `|R − r_fillet|`
    // (concave) from the sphere centre by definition.
    let to_sphere = spine_mid - sphere.center;
    let centre_dist = to_sphere.magnitude();
    if centre_dist < 1e-12 {
        return create_toroidal_fillet_surface(data);
    }
    let is_convex = centre_dist > sphere.radius;
    let expected_centre_dist = if is_convex {
        sphere.radius + fillet_radius
    } else {
        (sphere.radius - fillet_radius).abs()
    };
    // Consistency: ball-centre distance must equal R ± r_fillet up to
    // tolerance. A mismatch means the spine arm produced a spine in
    // the wrong locus — defer to legacy.
    let consistency_drift = (centre_dist - expected_centre_dist).abs();
    let consistency_tol = 1e-6_f64.max(sphere.radius * 1e-9 + fillet_radius * 1e-9);
    if consistency_drift > consistency_tol {
        return create_toroidal_fillet_surface(data);
    }

    let spine_arc = spine_rail.spine.as_any().downcast_ref::<Arc>();
    let major_radius = match spine_arc {
        Some(arc) if arc.radius > 0.0 && arc.radius.is_finite() => arc.radius,
        _ => return create_toroidal_fillet_surface(data),
    };
    if major_radius <= 0.0 || !major_radius.is_finite() {
        return create_toroidal_fillet_surface(data);
    }

    let angle_bounds = match spine_arc {
        Some(arc) if arc.sweep_angle > 0.0 && arc.sweep_angle.is_finite() => {
            (0.0, std::f64::consts::FRAC_PI_2.min(arc.sweep_angle.abs()))
        }
        _ => (0.0, std::f64::consts::FRAC_PI_2),
    };

    let center_curve = spine_rail.spine.clone_box();
    let contact1 = spine_rail.rail_a.clone_box();
    let contact2 = spine_rail.rail_b.clone_box();
    match ToroidalFillet::from_analytic_kpart(
        center_curve,
        fillet_radius,
        contact1,
        contact2,
        major_radius,
        angle_bounds,
    ) {
        Ok(fillet) => Ok(Box::new(fillet)),
        Err(_) => create_toroidal_fillet_surface(data),
    }
}

/// Helper: look up a [`Cylinder`] surface on either of the two given
/// faces. Returns `None` if neither face's underlying surface is a
/// `Cylinder`, which signals "fall back to legacy sample-based
/// construction" at the call site.
fn read_cylinder_face(
    model: &BRepModel,
    face_a_id: FaceId,
    face_b_id: FaceId,
) -> Option<crate::primitives::surface::Cylinder> {
    use crate::primitives::surface::Cylinder;
    for &fid in &[face_a_id, face_b_id] {
        let face = model.faces.get(fid)?;
        let surface = model.surfaces.get(face.surface_id)?;
        if let Some(cyl) = surface.as_any().downcast_ref::<Cylinder>() {
            return Some(*cyl);
        }
    }
    None
}

/// Helper: look up a [`crate::primitives::surface::Sphere`] surface
/// on either of the two given faces. Returns `None` if neither face
/// is sphere-backed.
fn read_sphere_face(
    model: &BRepModel,
    face_a_id: FaceId,
    face_b_id: FaceId,
) -> Option<crate::primitives::surface::Sphere> {
    use crate::primitives::surface::Sphere;
    for &fid in &[face_a_id, face_b_id] {
        let face = model.faces.get(fid)?;
        let surface = model.surfaces.get(face.surface_id)?;
        if let Some(sph) = surface.as_any().downcast_ref::<Sphere>() {
            return Some(*sph);
        }
    }
    None
}

/// Check if edge is straight within tolerance
fn is_edge_straight(data: &RollingBallData) -> bool {
    if data.centers.len() < 3 {
        return true;
    }

    // Check if all centers are collinear
    let v1 = data.centers[1] - data.centers[0];
    let v1_norm = match v1.normalize() {
        Ok(n) => n,
        Err(_) => return true,
    };

    for i in 2..data.centers.len() {
        let v2 = data.centers[i] - data.centers[0];
        let v2_norm = match v2.normalize() {
            Ok(n) => n,
            Err(_) => continue,
        };

        let cross = v1_norm.cross(&v2_norm);
        if cross.magnitude_squared() > 1e-6 {
            return false;
        }
    }

    true
}

/// Check if radius is constant
fn is_radius_constant(data: &RollingBallData) -> bool {
    if data.radii.is_empty() {
        return true;
    }

    let first_radius = data.radii[0];
    for &radius in &data.radii[1..] {
        if (radius - first_radius).abs() > 1e-6 {
            return false;
        }
    }

    true
}

/// Create cylindrical fillet surface
fn create_cylindrical_fillet_surface(data: &RollingBallData) -> OperationResult<Box<dyn Surface>> {
    // Create spine curve from edge centers
    let spine = create_spine_curve_from_points(&data.centers)?;

    // Create contact curves
    let contact1 = create_curve_from_points(&data.contacts1)?;
    let contact2 = create_curve_from_points(&data.contacts2)?;

    let fillet = CylindricalFillet::new(spine, data.radii[0], contact1, contact2).map_err(|e| {
        OperationError::NumericalError(format!("Failed to create cylindrical fillet: {:?}", e))
    })?;

    Ok(Box::new(fillet))
}

/// Create toroidal fillet surface
fn create_toroidal_fillet_surface(data: &RollingBallData) -> OperationResult<Box<dyn Surface>> {
    // Create center curve
    let center_curve = create_spine_curve_from_points(&data.centers)?;

    // Create contact curves
    let contact1 = create_curve_from_points(&data.contacts1)?;
    let contact2 = create_curve_from_points(&data.contacts2)?;

    let fillet =
        ToroidalFillet::new(center_curve, data.radii[0], contact1, contact2).map_err(|e| {
            OperationError::NumericalError(format!("Failed to create toroidal fillet: {:?}", e))
        })?;

    Ok(Box::new(fillet))
}

/// Create NURBS fillet surface for variable radius
fn create_nurbs_fillet_surface(data: &RollingBallData) -> OperationResult<Box<dyn Surface>> {
    // Create spine curve
    let spine = create_spine_curve_from_points(&data.centers)?;

    // Create contact curves
    let contact1 = create_curve_from_points(&data.contacts1)?;
    let contact2 = create_curve_from_points(&data.contacts2)?;

    // Resample the rolling-ball radii onto the surface's u-sampling
    // density (20). The previous implementation collapsed the entire
    // radius profile to its endpoints — `VariableRadiusFillet::new`
    // would then linearly interpolate, discarding any non-linear
    // variation the caller had provided. `with_radius_profile` honors
    // every sample independently.
    let resampled_radii = resample_radii_uniform(&data.radii, 20);

    let fillet = VariableRadiusFillet::with_radius_profile(spine, resampled_radii, contact1, contact2)
        .map_err(|e| {
            OperationError::NumericalError(format!(
                "Failed to create variable radius fillet: {:?}",
                e
            ))
        })?;

    Ok(Box::new(fillet))
}

/// Fit a curve through the given sample points.
///
/// Two points → exact `Line`. Three or more points → degree-min(3, n-1)
/// clamped NURBS curve fit through the points, preserving curvature along
/// the rolling-ball spine and contact trails for non-circular fillets.
/// A straight line through the endpoints would discard intermediate
/// curvature and silently misplace the fillet surface for any non-planar
/// edge.
fn create_curve_from_points(points: &[Point3]) -> OperationResult<Box<dyn Curve>> {
    use crate::primitives::curve::NurbsCurve;

    if points.len() < 2 {
        return Err(OperationError::InvalidGeometry(
            "Need at least 2 points for curve".to_string(),
        ));
    }

    if points.len() == 2 {
        return Ok(Box::new(Line::new(points[0], points[points.len() - 1])));
    }

    let tolerance = Tolerance::default();
    let nurbs = NurbsCurve::fit_to_points(points, 3, tolerance.distance())
        .map_err(|e| OperationError::NumericalError(format!("fillet curve fit failed: {:?}", e)))?;
    Ok(Box::new(nurbs))
}

/// Create spine curve from edge center points
fn create_spine_curve_from_points(points: &[Point3]) -> OperationResult<Box<dyn Curve>> {
    create_curve_from_points(points)
}

/// Compute trim curves for fillet on adjacent faces
fn compute_fillet_trim_curves(
    model: &BRepModel,
    data: &RollingBallData,
    face1_id: FaceId,
    face2_id: FaceId,
) -> OperationResult<(Vec<Point3>, Vec<Point3>)> {
    // For trim curve computation, we use the contact curves from the rolling ball data
    // The actual fillet surface is created separately

    // Get adjacent surfaces
    let face1 = model
        .faces
        .get(face1_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face1 not found".to_string()))?;
    let face2 = model
        .faces
        .get(face2_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face2 not found".to_string()))?;

    // Validate both surfaces exist; the trim curves come straight from
    // the rolling-ball data so the surface bodies themselves are not
    // needed here.
    model
        .surfaces
        .get(face1.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface1 not found".to_string()))?;
    model
        .surfaces
        .get(face2.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface2 not found".to_string()))?;

    // Use the contact curves from the rolling ball data directly
    // These represent where the fillet will meet the adjacent faces
    let trim_points1 = data.contacts1.clone();
    let trim_points2 = data.contacts2.clone();

    Ok((trim_points1, trim_points2))
}

/// Build a circular-arc cap edge that closes the fillet face's outer
/// loop at one end of the original edge.
///
/// The arc lies in the plane perpendicular to `axis_dir` through
/// `arc_center`, has radius `radius`, and is oriented so that
/// `arc.point_at(0) ≈ trim_a` and `arc.point_at(1) ≈ trim_b`. The arc
/// is the cylinder/torus cross-section the rolling ball traces at the
/// V0 (or V1) end of its sweep — using it (rather than the chord
/// between the two trim endpoints) keeps the loop boundary on the
/// fillet surface, eliminating the triangular gap a chord would leave.
fn build_cap_arc(
    arc_center: Point3,
    axis_dir: Vector3,
    radius: f64,
    trim_a: Point3,
    trim_b: Point3,
) -> OperationResult<crate::primitives::curve::Arc> {
    use crate::primitives::curve::Arc;
    use std::f64::consts::PI;

    // Construct a seed arc so we can read the canonical `x_axis` the
    // Arc primitive uses for angle measurement; the angle convention
    // depends on `axis_dir` orientation, so we match it exactly.
    let seed = Arc::new(arc_center, axis_dir, radius, 0.0, 1.0)
        .map_err(|e| OperationError::NumericalError(format!("Cap arc seed: {:?}", e)))?;
    let x_axis = seed.x_axis;
    let normal = axis_dir.normalize().map_err(|e| {
        OperationError::NumericalError(format!("Cap axis normalize failed: {:?}", e))
    })?;
    let y_axis = normal.cross(&x_axis);

    let project = |p: Point3| -> (f64, f64) {
        let v = p - arc_center;
        (v.dot(&x_axis), v.dot(&y_axis))
    };
    let (ax, ay) = project(trim_a);
    let (bx, by) = project(trim_b);
    let alpha_a = ay.atan2(ax);
    let alpha_b = by.atan2(bx);

    // Pick the short arc (|sweep| ≤ π). For the canonical convex 90°
    // fillet this gives the +π/2 arc through the corner side, exactly
    // matching the natural cylindrical cross-section the rolling ball
    // traces. Concave dihedrals likewise produce the supplement angle.
    let two_pi = 2.0 * PI;
    let mut sweep = alpha_b - alpha_a;
    while sweep > PI {
        sweep -= two_pi;
    }
    while sweep <= -PI {
        sweep += two_pi;
    }

    Arc::new(arc_center, axis_dir, radius, alpha_a, sweep)
        .map_err(|e| OperationError::NumericalError(format!("Cap arc construction: {:?}", e)))
}

/// Sample an iso-parametric curve on a fillet surface as a `Box<dyn Curve>`,
/// used to build cap edges on variable-radius / function-radius fillet
/// blends where the swept-surface boundary at `u = u_min` / `u = u_max`
/// is no longer a planar circular arc. When `dr/du ≠ 0` the rolling-ball
/// cross-section plane tilts in proportion to the radius gradient and a
/// perpendicular-plane `Arc` cap drifts off the surface. Sampling the
/// actual surface iso-curve keeps the cap on the surface boundary by
/// construction.
///
/// `v_start` / `v_end` may be supplied in either order so the caller can
/// match the cap-edge orientation expected by the blend-loop traversal:
/// the v-sweep walks linearly from `v_start` to `v_end`.
fn sample_cap_iso_curve(
    surface: &dyn Surface,
    u_fixed: f64,
    v_start: f64,
    v_end: f64,
) -> OperationResult<Box<dyn Curve>> {
    // 31 samples along the v-iso-curve at fixed u. The cap is a short
    // arc (rolling-ball cross-section, length ≈ r · π/2 ≈ 0.6-1.5 plane
    // units for typical sub-mm-to-mm radii), so 31 samples ≈ 1 sample
    // per 0.03 plane units. This gives the degree-3 NURBS least-squares
    // fit enough constraints to hug the iso-curve to well within the
    // 1e-3 surface-residence tolerance pinned by the cap-validation tests
    // — `fit_to_points` is least-squares (not interpolation), so under-
    // sampling lets the fit drift mid-cap even though both endpoints
    // are pinned.
    const NUM_SAMPLES: usize = 31;
    let mut points = Vec::with_capacity(NUM_SAMPLES);
    for i in 0..NUM_SAMPLES {
        let s = i as f64 / (NUM_SAMPLES - 1) as f64;
        let v = v_start + s * (v_end - v_start);
        let p = surface.point_at(u_fixed, v).map_err(|e| {
            OperationError::NumericalError(format!(
                "Cap iso-curve sample at (u={u_fixed}, v={v}) failed: {:?}",
                e
            ))
        })?;
        points.push(p);
    }
    create_curve_from_points(&points)
}

/// Create trimmed fillet face.
///
/// Returns `(face_id, surgery)` where `surgery` is `Some(BlendEdgeSurgery)`
/// in the 4-sided case (the production path on convex/concave dihedrals)
/// and `None` in the 3-sided degenerate case (zero-radius limit, where
/// trim curves collapse onto the original edge endpoints and no F3/F4
/// cap insertion is needed).
///
/// `cap_v0_center` / `cap_v1_center` are the rolling-ball centers at the
/// V0 and V1 ends of the original edge (== fillet-surface axis at the
/// caps). `cap_v0_radius` / `cap_v1_radius` are the rolling-ball radii at
/// those ends. Together they define the circular-arc cross-section the
/// blend face's natural boundary takes at each cap; the cap edges are
/// constructed as those arcs so the loop boundary tracks the fillet
/// surface instead of cutting a chord across it.
///
/// `cap_curve_overrides`: optional caller-supplied cap curves
/// `(cap_v0_curve, cap_v1_curve)`. When `None`, the function builds a
/// circular `Arc` cap in the plane perpendicular to the edge axis — the
/// natural cross-section for a constant-radius rolling-ball sweep
/// (cylindrical or toroidal fillet surface). When `Some`, the supplied
/// curves are used verbatim. This is required for variable-radius and
/// function-radius fillets where the rolling-ball cross-section plane
/// tilts in proportion to `dr/du` and the swept-surface boundary at
/// `u = u_min` / `u = u_max` is no longer a planar circular arc — the
/// caller samples the surface's actual iso-curve and passes a NURBS so
/// the cap stays on the surface boundary.
fn create_trimmed_fillet_face(
    model: &mut BRepModel,
    surface_id: u32,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    trim_curve1: Vec<Point3>,
    trim_curve2: Vec<Point3>,
    cap_v0_center: Point3,
    cap_v1_center: Point3,
    cap_v0_radius: f64,
    cap_v1_radius: f64,
    cap_curve_overrides: Option<(Box<dyn Curve>, Box<dyn Curve>)>,
) -> OperationResult<(FaceId, Option<BlendEdgeSurgery>)> {
    use crate::math::surface_intersection::intersection_curve_to_nurbs;
    use crate::primitives::r#loop::Loop;

    // Get the original edge for start/end vertices
    let original_edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

    // Extract values from edge before mutable borrows
    let start_vertex = original_edge.start_vertex;
    let end_vertex = original_edge.end_vertex;

    // Create curves for trim boundaries
    let trim_curve1_math = intersection_curve_to_nurbs(
        &crate::math::surface_intersection::IntersectionCurve {
            points: trim_curve1.clone(),
            params1: vec![(0.0, 0.0); trim_curve1.len()],
            params2: vec![(0.0, 0.0); trim_curve1.len()],
            tangents: vec![Vector3::X; trim_curve1.len()],
            is_closed: false,
        },
        3,
    )
    .map_err(|e| {
        OperationError::NumericalError(format!("Failed to create trim curve 1: {:?}", e))
    })?;

    let trim_curve2_math = intersection_curve_to_nurbs(
        &crate::math::surface_intersection::IntersectionCurve {
            points: trim_curve2.clone(),
            params1: vec![(0.0, 0.0); trim_curve2.len()],
            params2: vec![(0.0, 0.0); trim_curve2.len()],
            tangents: vec![Vector3::X; trim_curve2.len()],
            is_closed: false,
        },
        3,
    )
    .map_err(|e| {
        OperationError::NumericalError(format!("Failed to create trim curve 2: {:?}", e))
    })?;

    // Convert to primitives NurbsCurve
    use crate::primitives::curve::NurbsCurve as PrimNurbsCurve;
    let trim_curve1_nurbs = PrimNurbsCurve::new(
        trim_curve1_math.degree,
        trim_curve1_math.control_points,
        trim_curve1_math.weights,
        trim_curve1_math.knots.values().to_vec(),
    )
    .map_err(|e| {
        OperationError::NumericalError(format!("Failed to convert trim curve 1: {:?}", e))
    })?;

    let trim_curve2_nurbs = PrimNurbsCurve::new(
        trim_curve2_math.degree,
        trim_curve2_math.control_points,
        trim_curve2_math.weights,
        trim_curve2_math.knots.values().to_vec(),
    )
    .map_err(|e| {
        OperationError::NumericalError(format!("Failed to convert trim curve 2: {:?}", e))
    })?;

    // Add trim curves to model up-front; reused by both 3-sided and
    // 4-sided face construction below.
    let curve1_id = model.curves.add(Box::new(trim_curve1_nurbs));
    let curve2_id = model.curves.add(Box::new(trim_curve2_nurbs));

    // Trim-curve endpoints + original-edge vertex positions drive the
    // 3-vs-4-sided decision. Both branches need the same data, so
    // compute once.
    let endpoint_tol = Tolerance::default().distance().max(1e-6);
    let start_pos = model
        .vertices
        .get(start_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("Start vertex not found".to_string()))?
        .position;
    let end_pos = model
        .vertices
        .get(end_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("End vertex not found".to_string()))?
        .position;
    let dist = |a: [f64; 3], b: Point3| -> f64 {
        ((a[0] - b.x).powi(2) + (a[1] - b.y).powi(2) + (a[2] - b.z).powi(2)).sqrt()
    };
    let trim1_first = trim_curve1
        .first()
        .copied()
        .ok_or_else(|| OperationError::InvalidGeometry("Trim curve 1 is empty".to_string()))?;
    let trim1_last = trim_curve1
        .last()
        .copied()
        .ok_or_else(|| OperationError::InvalidGeometry("Trim curve 1 is empty".to_string()))?;
    let trim2_first = trim_curve2
        .first()
        .copied()
        .ok_or_else(|| OperationError::InvalidGeometry("Trim curve 2 is empty".to_string()))?;
    let trim2_last = trim_curve2
        .last()
        .copied()
        .ok_or_else(|| OperationError::InvalidGeometry("Trim curve 2 is empty".to_string()))?;

    let three_sided_ok = dist(start_pos, trim1_first) < endpoint_tol
        && dist(end_pos, trim1_last) < endpoint_tol
        && dist(start_pos, trim2_first) < endpoint_tol
        && dist(end_pos, trim2_last) < endpoint_tol;

    let (loop_id, surgery) = if three_sided_ok {
        // 3-sided fillet topology: the rolling-ball radius vanishes at
        // both edge endpoints so the trim curves on F1, F2 meet exactly
        // at V0 and V1. This is the canonical sharp-edge limiting case
        // (effectively zero-radius). Loop has 3 edges:
        //   trim1 (V0→V1) → original_edge (V1→V0 via Backward) → trim2 (V0→V1 reversed)
        let edge1 = Edge::new(
            0,
            start_vertex,
            end_vertex,
            curve1_id,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        );
        let edge1_id = model.edges.add(edge1);
        let edge2 = Edge::new(
            0,
            end_vertex,
            start_vertex,
            curve2_id,
            EdgeOrientation::Backward,
            ParameterRange::new(0.0, 1.0),
        );
        let edge2_id = model.edges.add(edge2);
        let mut fillet_loop = Loop::new(0, crate::primitives::r#loop::LoopType::Outer);
        fillet_loop.add_edge(edge1_id, true);
        fillet_loop.add_edge(edge_id, true);
        fillet_loop.add_edge(edge2_id, true);
        // 3-sided is the zero-radius degenerate: trim curves coincide
        // with the original edge, the new fillet face shares the
        // original edge with F1/F2, and no cap insertion at V0/V1 is
        // required. Surgery is therefore None — the caller skips it.
        (model.loops.add(fillet_loop), None)
    } else {
        // 4-sided fillet topology: the trim curves on F1 and F2 do not
        // reach V0/V1 — the rolling-ball envelope lifts off each face
        // at a positive distance from the original edge endpoints. The
        // fillet face boundary is therefore a quadrilateral:
        //
        //   v_t1_start ──[trim1 fwd]──▶ v_t1_end
        //        ▲                           │
        //  [cap_V0 fwd]                 [cap_V1 fwd]
        //        │                           ▼
        //   v_t2_start ◀──[trim2 rev]── v_t2_end
        //
        // The two cap edges close the loop at the rolling-ball cross-
        // section at V0 and V1. The cross-section is a circular arc on
        // the fillet surface centered at the rolling-ball center
        // (`cap_v0_center` / `cap_v1_center`), of radius equal to the
        // rolling-ball radius at that end (`cap_v0_radius` /
        // `cap_v1_radius`), in the plane perpendicular to the edge axis
        // direction. Building the cap edges as arcs (not chords) makes
        // the loop boundary coincide with the fillet face's natural
        // boundary so tessellation does not leak triangles into the
        // chord-arc gap.
        //
        // The returned `BlendEdgeSurgery` carries every new ID so
        // `update_adjacent_faces` can re-stitch F1, F2, F3, F4 around
        // the freshly-built blend face.
        let v_t1_start = model.vertices.add_or_find(
            trim1_first.x,
            trim1_first.y,
            trim1_first.z,
            endpoint_tol,
        );
        let v_t1_end = model.vertices.add_or_find(
            trim1_last.x,
            trim1_last.y,
            trim1_last.z,
            endpoint_tol,
        );
        let v_t2_start = model.vertices.add_or_find(
            trim2_first.x,
            trim2_first.y,
            trim2_first.z,
            endpoint_tol,
        );
        let v_t2_end = model.vertices.add_or_find(
            trim2_last.x,
            trim2_last.y,
            trim2_last.z,
            endpoint_tol,
        );

        let edge_trim1 = Edge::new(
            0,
            v_t1_start,
            v_t1_end,
            curve1_id,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        );
        let edge_trim1_id = model.edges.add(edge_trim1);

        let edge_trim2 = Edge::new(
            0,
            v_t2_start,
            v_t2_end,
            curve2_id,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        );
        let edge_trim2_id = model.edges.add(edge_trim2);

        // Edge axis direction (V0 → V1). The cap arcs lie in planes
        // perpendicular to this direction — the cylinder/torus
        // cross-section at each end of the rolling-ball sweep. Only
        // used when `cap_curve_overrides` is None (constant-radius
        // fillet). For variable / function radius, the caller has
        // sampled the actual fillet surface boundary and passed
        // explicit NURBS cap curves; the perpendicular-plane arc is
        // wrong there because the surface's cross-section plane
        // tilts in proportion to `dr/du`.
        let (cap_v0_curve, cap_v1_curve): (Box<dyn Curve>, Box<dyn Curve>) =
            if let Some((c0, c1)) = cap_curve_overrides {
                (c0, c1)
            } else {
                let axis_dir = (Point3::new(end_pos[0], end_pos[1], end_pos[2])
                    - Point3::new(start_pos[0], start_pos[1], start_pos[2]))
                .normalize()
                .map_err(|e| {
                    OperationError::NumericalError(format!(
                        "Edge axis normalize failed: {:?}",
                        e
                    ))
                })?;
                let v1_arc =
                    build_cap_arc(cap_v1_center, axis_dir, cap_v1_radius, trim1_last, trim2_last)?;
                let v0_arc = build_cap_arc(
                    cap_v0_center,
                    axis_dir,
                    cap_v0_radius,
                    trim2_first,
                    trim1_first,
                )?;
                (Box::new(v0_arc), Box::new(v1_arc))
            };

        let cap_v1_curve_id = model.curves.add(cap_v1_curve);
        let edge_cap_v1 = Edge::new(
            0,
            v_t1_end,
            v_t2_end,
            cap_v1_curve_id,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        );
        let edge_cap_v1_id = model.edges.add(edge_cap_v1);

        let cap_v0_curve_id = model.curves.add(cap_v0_curve);
        let edge_cap_v0 = Edge::new(
            0,
            v_t2_start,
            v_t1_start,
            cap_v0_curve_id,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        );
        let edge_cap_v0_id = model.edges.add(edge_cap_v0);

        // Loop traversal:
        //   v_t1_start →[trim1 fwd]→ v_t1_end →[cap_V1 fwd]→ v_t2_end
        //              →[trim2 rev]→ v_t2_start →[cap_V0 fwd]→ v_t1_start
        let mut fillet_loop = Loop::new(0, crate::primitives::r#loop::LoopType::Outer);
        fillet_loop.add_edge(edge_trim1_id, true);
        fillet_loop.add_edge(edge_cap_v1_id, true);
        fillet_loop.add_edge(edge_trim2_id, false);
        fillet_loop.add_edge(edge_cap_v0_id, true);
        let lid = model.loops.add(fillet_loop);

        let surgery = BlendEdgeSurgery {
            original_edge: edge_id,
            original_v0: start_vertex,
            original_v1: end_vertex,
            face1: face1_id,
            face2: face2_id,
            trim1_edge: edge_trim1_id,
            trim2_edge: edge_trim2_id,
            trim1_curve: curve1_id,
            trim2_curve: curve2_id,
            cap_v0_edge: edge_cap_v0_id,
            cap_v1_edge: edge_cap_v1_id,
            v_t1_start,
            v_t1_end,
            v_t2_start,
            v_t2_end,
            // F5-α.2 — defaults; the caller (`create_fillet_chain`) sets
            // these to `true` after construction when the BlendGraph
            // classifies the corresponding endpoint as a
            // `ConvexCorner { degree: 3 }` apex shared with two other
            // blend edges in this pass. `create_trimmed_fillet_face` does
            // not consult the BlendGraph directly.
            original_v0_corner_shared: false,
            original_v1_corner_shared: false,
        };
        (lid, Some(surgery))
    };

    // Pick the fillet face's orientation so its oriented outward normal
    // points away from the rolling-ball trajectory. The rolling-ball
    // center at the surface's parametric midpoint is approximately the
    // midpoint of the two cap centers (which are the rolling-ball
    // positions at the V0 and V1 ends). The surface midpoint sample
    // gives us the geometric point on the blend face there; their
    // difference is the geometric outward target.
    let surface_ref = model
        .surfaces
        .get(surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Fillet surface not found".to_string()))?;
    let ((u_min, u_max), (v_min, v_max)) = surface_ref.parameter_bounds();
    let u_mid = 0.5 * (u_min + u_max);
    let v_mid = 0.5 * (v_min + v_max);
    let surface_mid_pt = surface_ref.point_at(u_mid, v_mid).map_err(|e| {
        OperationError::NumericalError(format!(
            "Fillet surface midpoint evaluation failed: {:?}",
            e
        ))
    })?;
    let rolling_ball_mid = Point3::new(
        0.5 * (cap_v0_center.x + cap_v1_center.x),
        0.5 * (cap_v0_center.y + cap_v1_center.y),
        0.5 * (cap_v0_center.z + cap_v1_center.z),
    );
    let outward_target = surface_mid_pt - rolling_ball_mid;
    // If the surface midpoint coincides with the rolling-ball midpoint
    // (radius collapses to 0, degenerate three-sided limit), fall back
    // to the bisector of the two face1 / face2 normals at the edge
    // midpoint — that direction points away from the dihedral.
    let outward_target = if outward_target.magnitude_squared() > 1e-20 {
        outward_target
    } else {
        let edge_mid = Point3::new(
            0.5 * (start_pos[0] + end_pos[0]),
            0.5 * (start_pos[1] + end_pos[1]),
            0.5 * (start_pos[2] + end_pos[2]),
        );
        let n1 = get_face_oriented_normal(model, face1_id, &edge_mid)
            .unwrap_or(Vector3::Z);
        let n2 = get_face_oriented_normal(model, face2_id, &edge_mid)
            .unwrap_or(Vector3::Z);
        let bisector = n1 + n2;
        if bisector.magnitude_squared() > 1e-20 {
            bisector
        } else {
            Vector3::Z
        }
    };
    let surface_ref = model
        .surfaces
        .get(surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Fillet surface not found".to_string()))?;
    let orientation = orient_face_for_outward(surface_ref, outward_target)?;
    let face = Face::new(0, surface_id, loop_id, orientation);
    Ok((model.faces.add(face), surgery))
}

/// Extract `(axis, axis_origin, radius)` from a freshly created
/// cylindrical-fillet face. Mirrors the inner closure of
/// [`classify_blend_for_edge`] but applies to a single known face
/// rather than searching a pair. Returns `None` if the face's
/// surface is neither a raw [`Cylinder`] nor a [`CylindricalFillet`]
/// — F5-α only supports the cylindrical edge-blend case.
fn extract_fillet_cylinder_descriptor(
    model: &BRepModel,
    face_id: FaceId,
) -> Option<EdgeBlendDescriptor> {
    let face = model.faces.get(face_id)?;
    let surf = model.surfaces.get(face.surface_id)?;
    if let Some(c) = surf.as_any().downcast_ref::<Cylinder>() {
        return Some(EdgeBlendDescriptor {
            face_id,
            axis: c.axis,
            axis_origin: c.origin,
            radius: c.radius,
        });
    }
    if let Some(f) = surf.as_any().downcast_ref::<CylindricalFillet>() {
        let axis = *f.axis_field.first()?;
        let axis_origin = f.spine.evaluate(0.0).ok()?.position;
        return Some(EdgeBlendDescriptor {
            face_id,
            axis,
            axis_origin,
            radius: f.radius,
        });
    }
    None
}

/// Emit corner-sphere transition faces at every BlendGraph corner
/// the F5-α dispatcher recognises.
///
/// MVP scope (F5-α / Task #10):
/// - `BlendVertexKind::ConvexCorner { degree: 3 }`
/// - all three incident blend edges produced a cylindrical fillet
///   face (raw `Cylinder` or `CylindricalFillet`)
/// - radii agree within `1e-9`
/// - the three cylinder axes are concurrent (least-squares residual
///   from [`compute_concurrent_axes_center`] ≤ `1e-6`)
///
/// When all four conditions hold this calls
/// [`apply_apex_sphere_corner`] which:
///   1. locates the V-side cap arcs on each incident fillet face by
///      centre-coincidence with the apex sphere centre
///   2. verifies the three caps close a triangle
///   3. emits a [`Sphere`] face with the cap-arc loop as its outer
///      boundary, registers it on the outer shell
///
/// Out-of-MVP variants surface as typed
/// `BlendFailure::VertexBlendUnsupported` so callers can branch on
/// the structured reason:
/// - degree ≠ 3 → `DegreeTooHigh`
/// - mismatched radii → `MixedRadii`
/// - non-concurrent axes → `NonManifoldNeighbourhood`
/// - one of the incident edges had no fillet face produced (e.g.
///   propagation dropped it) → corner is silently skipped — its
///   neighbourhood is not three-cylinder anyway and the shell stays
///   watertight without a sphere
///
/// `ConcaveCorner` / `Mixed` / `Cliff` are filtered out by the
/// `BlendGraph::corners()` iterator's classification — F5-δ widens
/// coverage to those. `Smooth` vertices never reach
/// `corners()` (their dihedral is continuous so no patch is needed).
fn create_fillet_transitions(
    model: &mut BRepModel,
    solid_id: SolidId,
    blend_graph: &BlendGraph,
    edge_to_face: &HashMap<EdgeId, FaceId>,
    corner_positions: &HashMap<VertexId, Point3>,
) -> OperationResult<Vec<FaceId>> {
    const RADIUS_TOL: f64 = 1.0e-9;
    const AXES_RESIDUAL_TOL: f64 = 1.0e-6;

    let mut new_faces: Vec<FaceId> = Vec::new();

    for corner in blend_graph.corners() {
        let degree = match corner.kind {
            BlendVertexKind::ConvexCorner { degree } => degree,
            // Concave / Mixed corners are F5-δ territory. The
            // lifecycle gate already rejects them with the right
            // typed surface; nothing for the transitions pass to
            // do.
            _ => continue,
        };

        if degree != 3 {
            // Non-degree-3 ConvexCorner vertices reach this iterator
            // legitimately during single-edge fillets (degree 1, the
            // edge's endpoint) and two-edge non-shared-vertex
            // selections (degree 1 at every endpoint). Those cases
            // need no corner patch — the per-edge cylinder fillet
            // already closes the topology. Multi-edge shared corners
            // with degree ≠ 3 (e.g. a four-edge prism apex, F5-γ
            // territory) are pre-rejected by
            // `lifecycle::validate_corner_compatibility`; if a
            // degree-2/4/… corner somehow surfaced here it should
            // skip silently rather than poison an otherwise valid
            // multi-edge pass.
            continue;
        }

        if corner.incident_blend_edges.len() != 3 {
            // The classifier promised degree 3 but the incidence
            // list disagrees — should not happen given BlendGraph's
            // own consistency invariant; treat as non-manifold and
            // skip this corner so the rest of the dispatch can
            // proceed.
            continue;
        }

        // Resolve the three fillet face ids. If any incident edge
        // is missing a face (shouldn't happen for a degree-3 corner
        // unless propagation dropped a member), skip the corner —
        // there is nothing to seal because the cylindrical fillets
        // simply do not all exist.
        let mut face_ids = [0u32; 3];
        let mut all_present = true;
        for (i, eid) in corner.incident_blend_edges.iter().enumerate() {
            match edge_to_face.get(eid) {
                Some(f) => face_ids[i] = *f,
                None => {
                    all_present = false;
                    break;
                }
            }
        }
        if !all_present {
            continue;
        }

        // Pull cylinder descriptors from each fillet face surface.
        let mut descriptors: Vec<EdgeBlendDescriptor> = Vec::with_capacity(3);
        for fid in &face_ids {
            match extract_fillet_cylinder_descriptor(model, *fid) {
                Some(d) => descriptors.push(d),
                None => {
                    // A fillet face whose surface isn't a cylinder
                    // means the F5-α MVP doesn't apply here — bail
                    // with a typed unsupported reason rather than
                    // silently corrupting the shell.
                    return Err(OperationError::BlendFailed(Box::new(
                        BlendFailure::VertexBlendUnsupported {
                            vertex: corner.id,
                            kind: BlendVertexKind::ConvexCorner { degree: 3 },
                            reason: VertexBlendUnsupportedReason::NonManifoldNeighbourhood,
                        },
                    )));
                }
            }
        }

        // F5-β dispatcher — branch on per-edge radius equality.
        // Equal radii (`F5-α MVP`) route through
        // `apply_apex_sphere_corner` and emit a spherical apex face.
        // Mixed radii route through the F5-β triangular-NURBS skeleton
        // (still emits the structured `MixedRadii` failure at the end
        // of F5-β.1; F5-β.3 lifts that).
        let r0 = descriptors[0].radius;
        let radii_equal = descriptors
            .iter()
            .all(|d| (d.radius - r0).abs() <= RADIUS_TOL);

        // Assemble classifications and solve for the corner apex
        // point. `compute_concurrent_axes_center` is radius-
        // independent — it returns the least-squares closest point
        // to all three cylinder axis lines. For equal radii this is
        // the apex sphere centre; for mixed radii this is the F5-β
        // corner anchor `A` used to derive per-cylinder cap centres.
        let classifications: Vec<IncidentEdgeClassification> = corner
            .incident_blend_edges
            .iter()
            .zip(face_ids.iter())
            .zip(descriptors.iter())
            .map(|((eid, fid), descriptor)| IncidentEdgeClassification {
                edge_id: *eid,
                adjacent_faces: (*fid, *fid),
                blend: Some(descriptor.clone()),
            })
            .collect();

        let corner_apex = compute_concurrent_axes_center(&classifications, corner.id)?;

        // Residual check: distance from the solved apex to each axis
        // line. For mixed radii the residual can legitimately exceed
        // zero even at a perfectly rectilinear corner (the axes are
        // not concurrent when `r_i` differ), so the residual gate
        // only applies to the equal-radius branch where the apex IS
        // the sphere centre and must coincide with every axis. Mixed
        // radii fall to the F5-β LS interpretation: `A` is the
        // closest point, not a point ON every axis.
        if radii_equal {
            let max_residual = descriptors
                .iter()
                .map(|d| {
                    let q = d.axis_origin;
                    let u = d.axis;
                    let v = corner_apex - q;
                    let dot = v.x * u.x + v.y * u.y + v.z * u.z;
                    let perp =
                        Vector3::new(v.x - dot * u.x, v.y - dot * u.y, v.z - dot * u.z);
                    (perp.x * perp.x + perp.y * perp.y + perp.z * perp.z).sqrt()
                })
                .fold(0.0_f64, f64::max);
            if max_residual > AXES_RESIDUAL_TOL {
                return Err(OperationError::BlendFailed(Box::new(
                    BlendFailure::VertexBlendUnsupported {
                        vertex: corner.id,
                        kind: BlendVertexKind::ConvexCorner { degree: 3 },
                        reason: VertexBlendUnsupportedReason::NonManifoldNeighbourhood,
                    },
                )));
            }
        }

        // Outward direction at the corner: the original sharp vertex
        // sits *outside* the corner cavity — the segment from corner
        // apex to vertex points away from the solid material.
        // `corner_positions` snapshotted the vertex position before
        // per-edge surgery so the read is well-defined regardless of
        // whether the splice has since orphaned the vertex.
        let vertex_pos = corner_positions.get(&corner.id).copied().ok_or_else(|| {
            OperationError::InternalError(format!(
                "corner vertex {} missing from snapshot taken before fillet chains ran",
                corner.id
            ))
        })?;
        let outward_raw = vertex_pos - corner_apex;
        let vertex_outward = outward_raw.normalize().map_err(|_| {
            OperationError::BlendFailed(Box::new(BlendFailure::VertexBlendUnsupported {
                vertex: corner.id,
                kind: BlendVertexKind::ConvexCorner { degree: 3 },
                reason: VertexBlendUnsupportedReason::NonManifoldNeighbourhood,
            }))
        })?;

        if radii_equal {
            let face_id = apply_apex_sphere_corner(
                model,
                solid_id,
                corner.id,
                &face_ids,
                corner_apex,
                r0,
                vertex_outward,
            )?;
            new_faces.push(face_id);
        } else {
            let cylinder_axes: [(Point3, Vector3, f64); 3] = [
                (
                    descriptors[0].axis_origin,
                    descriptors[0].axis,
                    descriptors[0].radius,
                ),
                (
                    descriptors[1].axis_origin,
                    descriptors[1].axis,
                    descriptors[1].radius,
                ),
                (
                    descriptors[2].axis_origin,
                    descriptors[2].axis,
                    descriptors[2].radius,
                ),
            ];
            let face_id = apply_triangular_nurbs_corner(
                model,
                solid_id,
                corner.id,
                vertex_pos,
                &face_ids,
                &cylinder_axes,
                corner_apex,
                vertex_outward,
            )?;
            new_faces.push(face_id);
        }
    }

    Ok(new_faces)
}

/// Re-stitch the topology around freshly created fillet faces.
///
/// Two-step pass:
/// 1. Add every new fillet face to the solid's outer shell so it
///    participates in shell traversal (face-list, validation, etc.).
/// 2. For each 4-sided fillet, run [`splice_blend_edge`] to:
///    - Replace the original edge `E` in F1's loop with `trim1` and
///      re-vertex F1's V0/V1 neighbours onto `v_t1_start`/`v_t1_end`.
///    - Symmetrically splice F2 with `trim2`.
///    - Insert `cap_v0` into F3 (the third face at V0) and `cap_v1`
///      into F4 (the third face at V1).
///    - Remove the now-orphaned original edge and original V0/V1
///      vertices from the model.
///
/// The 3-sided (zero-radius degenerate) path produces no surgery and is
/// correct as built — `create_trimmed_fillet_face` returns `None` in
/// that case and the loop here simply skips it.
fn update_adjacent_faces(
    model: &mut BRepModel,
    solid_id: SolidId,
    fillet_faces: &[FaceId],
    surgeries: &[BlendEdgeSurgery],
) -> OperationResult<()> {
    // Step 1 — register fillet faces with the outer shell.
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
        for &face_id in fillet_faces {
            shell.add_face(face_id);
        }
    }

    // Step 2 — splice each blend edge.
    for surgery in surgeries {
        splice_blend_edge(model, solid_id, surgery)?;
    }

    Ok(())
}

/// Propagate edge selection based on mode
fn propagate_edge_selection(
    model: &BRepModel,
    initial_edges: Vec<EdgeId>,
    mode: PropagationMode,
) -> OperationResult<Vec<EdgeId>> {
    match mode {
        PropagationMode::None => Ok(initial_edges),
        PropagationMode::Tangent => propagate_tangent_edges(model, initial_edges),
        PropagationMode::Smooth => propagate_smooth_edges(model, initial_edges),
        PropagationMode::All => propagate_all_edges(model, initial_edges),
    }
}

/// Propagate along tangent edges
fn propagate_tangent_edges(
    model: &BRepModel,
    initial_edges: Vec<EdgeId>,
) -> OperationResult<Vec<EdgeId>> {
    let mut result = HashSet::new();
    let mut to_process: Vec<EdgeId> = initial_edges.clone();

    // Add initial edges
    for &edge in &initial_edges {
        result.insert(edge);
    }

    while let Some(current_edge_id) = to_process.pop() {
        let current_edge = model
            .edges
            .get(current_edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

        // Get vertices of current edge
        let vertices = [current_edge.start_vertex, current_edge.end_vertex];

        for vertex_id in vertices {
            // Find all edges connected to this vertex
            let connected_edges = find_edges_at_vertex(model, vertex_id)?;

            for &connected_edge_id in &connected_edges {
                if !result.contains(&connected_edge_id) {
                    // Check if edges are tangent
                    if are_edges_tangent(model, current_edge_id, connected_edge_id)? {
                        result.insert(connected_edge_id);
                        to_process.push(connected_edge_id);
                    }
                }
            }
        }
    }

    Ok(result.into_iter().collect())
}

/// Check if two edges are tangent at their common vertex
fn are_edges_tangent(
    model: &BRepModel,
    edge1_id: EdgeId,
    edge2_id: EdgeId,
) -> OperationResult<bool> {
    let edge1 = model
        .edges
        .get(edge1_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge1 not found".to_string()))?;
    let edge2 = model
        .edges
        .get(edge2_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge2 not found".to_string()))?;

    // Find common vertex
    let common_vertex =
        if edge1.start_vertex == edge2.start_vertex || edge1.start_vertex == edge2.end_vertex {
            Some(edge1.start_vertex)
        } else if edge1.end_vertex == edge2.start_vertex || edge1.end_vertex == edge2.end_vertex {
            Some(edge1.end_vertex)
        } else {
            None
        };

    if let Some(vertex_id) = common_vertex {
        // Get tangents at the common vertex
        let t1 = if edge1.start_vertex == vertex_id {
            0.0
        } else {
            1.0
        };
        let t2 = if edge2.start_vertex == vertex_id {
            0.0
        } else {
            1.0
        };

        let tangent1 = edge1.tangent_at(t1, &model.curves)?;
        let tangent2 = edge2.tangent_at(t2, &model.curves)?;

        // Check if tangents are parallel (within tolerance)
        let angle = tangent1
            .normalize()?
            .angle(&tangent2.normalize()?)
            .unwrap_or(0.0);
        Ok(angle < 0.1 || (std::f64::consts::PI - angle) < 0.1) // ~5.7 degrees
    } else {
        Ok(false)
    }
}

/// Find all edges connected to a vertex
fn find_edges_at_vertex(model: &BRepModel, vertex_id: VertexId) -> OperationResult<Vec<EdgeId>> {
    // Use the efficient edges_at_vertex method
    let edges = model.edges.edges_at_vertex(vertex_id).to_vec();

    Ok(edges)
}

/// Propagate along smooth edges
fn propagate_smooth_edges(
    model: &BRepModel,
    initial_edges: Vec<EdgeId>,
) -> OperationResult<Vec<EdgeId>> {
    let mut result = HashSet::new();
    let mut to_process: Vec<EdgeId> = initial_edges.clone();

    // Add initial edges
    for &edge in &initial_edges {
        result.insert(edge);
    }

    while let Some(current_edge_id) = to_process.pop() {
        // Get faces adjacent to current edge
        let (face1_id, face2_id) = match get_adjacent_faces_safe(model, current_edge_id) {
            Ok(faces) => faces,
            Err(_) => continue, // Skip boundary edges
        };

        // Find edges that share faces with current edge
        let connected_edges =
            find_smooth_connected_edges(model, current_edge_id, face1_id, face2_id)?;

        for connected_edge_id in connected_edges {
            if !result.contains(&connected_edge_id) {
                // Check G1 continuity
                if check_g1_continuity(model, current_edge_id, connected_edge_id)? {
                    result.insert(connected_edge_id);
                    to_process.push(connected_edge_id);
                }
            }
        }
    }

    Ok(result.into_iter().collect())
}

/// Get adjacent faces (safe version that doesn't error on boundary edges)
fn get_adjacent_faces_safe(
    model: &BRepModel,
    edge_id: EdgeId,
) -> OperationResult<(FaceId, FaceId)> {
    // Linear face scan: O(F) per call, but the caller invokes this once
    // per filleted edge and the model rarely exceeds a few thousand
    // faces. A face-edge incidence index would shave this to O(1) but
    // adds maintenance overhead on every topology mutation; the trade
    // is not worthwhile until profiling proves otherwise.
    let mut adjacent_faces = Vec::new();

    // Iterate through all faces by index
    for face_id in 0..model.faces.len() as u32 {
        if let Some(face) = model.faces.get(face_id) {
            if face_contains_edge(model, face, edge_id)? {
                adjacent_faces.push(face_id);
            }
        }
    }

    match adjacent_faces.len() {
        2 => Ok((adjacent_faces[0], adjacent_faces[1])),
        _ => Err(OperationError::InvalidGeometry(
            "Not an interior edge".to_string(),
        )),
    }
}

/// Find edges connected through smooth faces
fn find_smooth_connected_edges(
    model: &BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
) -> OperationResult<Vec<EdgeId>> {
    let mut connected_edges = Vec::new();

    // Get all edges of both faces
    let face1 = model
        .faces
        .get(face1_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face1 not found".to_string()))?;
    let face2 = model
        .faces
        .get(face2_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face2 not found".to_string()))?;

    // Get edges from face loops
    let mut face_edges = HashSet::new();

    // Add edges from face1
    if let Some(outer_loop) = model.loops.get(face1.outer_loop) {
        for &e in &outer_loop.edges {
            if e != edge_id {
                face_edges.insert(e);
            }
        }
    }

    // Add edges from face2
    if let Some(outer_loop) = model.loops.get(face2.outer_loop) {
        for &e in &outer_loop.edges {
            if e != edge_id {
                face_edges.insert(e);
            }
        }
    }

    connected_edges.extend(face_edges);
    Ok(connected_edges)
}

/// Check G1 continuity between edges
fn check_g1_continuity(
    model: &BRepModel,
    edge1_id: EdgeId,
    edge2_id: EdgeId,
) -> OperationResult<bool> {
    // Get faces adjacent to each edge
    let (face1a, face1b) = match get_adjacent_faces_safe(model, edge1_id) {
        Ok(faces) => faces,
        Err(_) => return Ok(false),
    };

    let (face2a, face2b) = match get_adjacent_faces_safe(model, edge2_id) {
        Ok(faces) => faces,
        Err(_) => return Ok(false),
    };

    // Check if they share a face
    let shared_face = if face1a == face2a || face1a == face2b {
        Some(face1a)
    } else if face1b == face2a || face1b == face2b {
        Some(face1b)
    } else {
        None
    };

    if shared_face.is_some() {
        // If edges share a face and are connected, check tangent continuity
        are_edges_tangent(model, edge1_id, edge2_id)
    } else {
        Ok(false)
    }
}

/// Propagate to all connected edges
fn propagate_all_edges(
    model: &BRepModel,
    initial_edges: Vec<EdgeId>,
) -> OperationResult<Vec<EdgeId>> {
    let mut result = HashSet::new();
    let mut to_process: Vec<EdgeId> = initial_edges.clone();

    // Add initial edges
    for &edge in &initial_edges {
        result.insert(edge);
    }

    while let Some(current_edge_id) = to_process.pop() {
        let current_edge = model
            .edges
            .get(current_edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

        // Get vertices of current edge
        let vertices = [current_edge.start_vertex, current_edge.end_vertex];

        for vertex_id in vertices {
            // Find all edges connected to this vertex
            let connected_edges = find_edges_at_vertex(model, vertex_id)?;

            for &connected_edge_id in &connected_edges {
                if !result.contains(&connected_edge_id) {
                    result.insert(connected_edge_id);
                    to_process.push(connected_edge_id);
                }
            }
        }
    }

    Ok(result.into_iter().collect())
}

/// Group edges into continuous chains by shared vertices.
///
/// Two edges in `edges` are in the same chain when they share a start or end
/// vertex (transitive closure). Implemented via union-find on the input
/// edges, indexed by vertex incidence. Edges that do not connect to any
/// other input edge are returned as singleton chains.
///
/// Chains preserve no particular order within themselves — `create_fillet_chain`
/// re-resolves edge adjacency per-edge anyway. This grouping ensures multi-edge
/// fillet runs (e.g., all 12 edges of a box selected together) emit a single
/// fillet patch family rather than 12 disjoint cylinders that don't share
/// transition surfaces.
fn group_edges_into_chains(
    model: &BRepModel,
    edges: &[EdgeId],
) -> OperationResult<Vec<Vec<EdgeId>>> {
    if edges.is_empty() {
        return Ok(Vec::new());
    }

    // Build vertex -> list of edge indices incidence map.
    let mut vertex_to_edges: HashMap<VertexId, Vec<usize>> = HashMap::new();
    for (idx, &edge_id) in edges.iter().enumerate() {
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;
        vertex_to_edges
            .entry(edge.start_vertex)
            .or_default()
            .push(idx);
        vertex_to_edges
            .entry(edge.end_vertex)
            .or_default()
            .push(idx);
    }

    // Union-find over edge indices.
    let mut parent: Vec<usize> = (0..edges.len()).collect();
    fn find(parent: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while parent[r] != r {
            r = parent[r];
        }
        // Path compression.
        let mut cur = x;
        while parent[cur] != r {
            let next = parent[cur];
            parent[cur] = r;
            cur = next;
        }
        r
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent[ra] = rb;
        }
    }
    for incident in vertex_to_edges.values() {
        if let Some(&first) = incident.first() {
            for &other in &incident[1..] {
                union(&mut parent, first, other);
            }
        }
    }

    // Bucket edge indices by their root, then materialize.
    let mut buckets: HashMap<usize, Vec<EdgeId>> = HashMap::new();
    for (idx, &edge_id) in edges.iter().enumerate() {
        let root = find(&mut parent, idx);
        buckets.entry(root).or_default().push(edge_id);
    }
    Ok(buckets.into_values().collect())
}

/// Get faces adjacent to an edge in a solid
fn get_adjacent_faces(
    model: &BRepModel,
    solid_id: SolidId,
    edge_id: EdgeId,
) -> OperationResult<(FaceId, FaceId)> {
    // Get the solid and its shell
    let solid = model
        .solids
        .get(solid_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Solid not found".to_string()))?;

    let shell = model
        .shells
        .get(solid.outer_shell)
        .ok_or_else(|| OperationError::InvalidGeometry("Shell not found".to_string()))?;

    // Search through all faces in the shell to find which ones use this edge
    let mut adjacent_faces = Vec::new();

    for &face_id in &shell.faces {
        let face = model
            .faces
            .get(face_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

        // Check if this face's outer loop contains the edge
        if face_contains_edge(model, face, edge_id)? {
            adjacent_faces.push(face_id);
        }
    }

    // An edge should be shared by exactly two faces in a manifold solid
    match adjacent_faces.len() {
        2 => Ok((adjacent_faces[0], adjacent_faces[1])),
        0 => Err(OperationError::InvalidGeometry(
            "Edge not found in any face".to_string(),
        )),
        1 => Err(OperationError::InvalidGeometry(
            "Edge is boundary - only one adjacent face".to_string(),
        )),
        n => Err(OperationError::InvalidGeometry(format!(
            "Non-manifold edge with {} adjacent faces",
            n
        ))),
    }
}

/// Check if a face contains a specific edge
fn face_contains_edge(
    model: &BRepModel,
    face: &Face,
    target_edge_id: EdgeId,
) -> OperationResult<bool> {
    // Check outer loop
    let outer_loop = model
        .loops
        .get(face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Outer loop not found".to_string()))?;

    for &edge_id in &outer_loop.edges {
        if edge_id == target_edge_id {
            return Ok(true);
        }
    }

    // Check inner loops (holes)
    for &inner_loop_id in &face.inner_loops {
        let inner_loop = model
            .loops
            .get(inner_loop_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Inner loop not found".to_string()))?;

        for &edge_id in &inner_loop.edges {
            if edge_id == target_edge_id {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

/// Get edges connected to a vertex in a solid.
///
/// Walks the solid's outer shell, scans each face's outer and inner loops,
/// and collects every edge that has the given vertex as either its start or
/// end vertex. Result is deduplicated.
fn get_edges_at_vertex(
    model: &BRepModel,
    solid_id: SolidId,
    vertex_id: VertexId,
) -> OperationResult<Vec<EdgeId>> {
    let solid = model
        .solids
        .get(solid_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Solid not found".to_string()))?;
    let shell = model
        .shells
        .get(solid.outer_shell)
        .ok_or_else(|| OperationError::InvalidGeometry("Shell not found".to_string()))?;

    let mut seen: HashSet<EdgeId> = HashSet::new();
    let mut visit_loop = |loop_id: crate::primitives::r#loop::LoopId| -> OperationResult<()> {
        let l = model
            .loops
            .get(loop_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?;
        for &edge_id in &l.edges {
            let edge = model
                .edges
                .get(edge_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;
            if edge.start_vertex == vertex_id || edge.end_vertex == vertex_id {
                seen.insert(edge_id);
            }
        }
        Ok(())
    };

    for &face_id in &shell.faces {
        let face = model
            .faces
            .get(face_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;
        visit_loop(face.outer_loop)?;
        for &inner in &face.inner_loops {
            visit_loop(inner)?;
        }
    }

    Ok(seen.into_iter().collect())
}

/// Compute the signed dihedral angle between two faces at an edge.
///
/// The sign of the returned angle is a geometric invariant of the
/// edge — positive ⇒ convex (solid sticks out), negative ⇒ concave
/// (interior corner) — **iff** the inputs to `robust_face_angle` are
/// (a) the outward face normals (not the raw surface normals) and
/// (b) the edge tangent rotated into one face's CCW loop direction.
/// Both corrections are applied here: normals come from
/// `get_face_oriented_normal` which multiplies by
/// `face.orientation.sign()`, and the tangent is multiplied by
/// `face1_loop_sign` so its direction matches `face1`'s loop
/// traversal regardless of which way the underlying curve happens to
/// be parameterized.
fn compute_face_angle(
    model: &BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
) -> OperationResult<f64> {
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

    let edge_midpoint = edge.evaluate(0.5, &model.curves)?;
    let face1_normal = get_face_oriented_normal(model, face1_id, &edge_midpoint)?;
    let face2_normal = get_face_oriented_normal(model, face2_id, &edge_midpoint)?;

    let face1_loop_sign = edge_orientation_in_face(model, face1_id, edge_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "Edge {} not present in any loop of face {}",
            edge_id, face1_id
        ))
    })?;
    let edge_tangent = edge.tangent_at(0.5, &model.curves)? * face1_loop_sign;

    robust_face_angle(
        &face1_normal,
        &face2_normal,
        &edge_tangent,
        &Tolerance::default(),
    )
    .map_err(|e| OperationError::NumericalError(format!("Failed to compute face angle: {:?}", e)))
}

/// Validate fillet inputs
fn validate_fillet_inputs(
    model: &BRepModel,
    solid_id: SolidId,
    edges: &[EdgeId],
    options: &FilletOptions,
) -> OperationResult<()> {
    // Check solid exists
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidGeometry(format!(
            "validate_fillet_inputs: solid {} not found",
            solid_id
        )));
    }

    // Reject empty edge lists up front — every fillet operation requires at
    // least one edge to round.
    if edges.is_empty() {
        return Err(OperationError::InvalidGeometry(
            "validate_fillet_inputs: edges list is empty".to_string(),
        ));
    }

    // Check edges exist
    for &edge_id in edges {
        if model.edges.get(edge_id).is_none() {
            return Err(OperationError::InvalidGeometry(format!(
                "validate_fillet_inputs: edge {} not found",
                edge_id
            )));
        }
    }

    // Validate option-driven parameters: radius must exceed tolerance to be
    // geometrically meaningful, otherwise the resulting blend collapses to a
    // numerical artifact rather than a real round.
    let tol = options.common.tolerance.distance();
    if !options.radius.is_finite() || options.radius <= tol {
        return Err(OperationError::InvalidGeometry(format!(
            "validate_fillet_inputs: radius {:.6} is not greater than tolerance {:.3e}",
            options.radius, tol
        )));
    }

    // For variable-radius / chord-based / setback fillets, defer per-segment
    // radius validation to the variant-specific code paths; constant fillets
    // are already covered above.
    match &options.fillet_type {
        FilletType::Constant(r) => {
            if !r.is_finite() || *r <= tol {
                return Err(OperationError::InvalidGeometry(format!(
                    "validate_fillet_inputs: Constant fillet radius {:.6} \
                     is not greater than tolerance {:.3e}",
                    r, tol
                )));
            }
        }
        FilletType::PerEdgeConstant(map) => {
            // F5-β.5: structural validation for the per-edge variant.
            // Surfaces as `InvalidInput` because every failure mode
            // here is caller-side (wrong DTO shape, missing or extra
            // edges, non-finite or non-positive radius).
            if map.is_empty() {
                return Err(OperationError::InvalidInput {
                    parameter: "fillet_type.PerEdgeConstant".into(),
                    expected: "non-empty per-edge radius map".into(),
                    received: "empty map".into(),
                });
            }
            // Coverage: every selected edge must have an entry.
            let selection: HashSet<EdgeId> = edges.iter().copied().collect();
            for &edge_id in edges {
                if !map.contains_key(&edge_id) {
                    return Err(OperationError::InvalidInput {
                        parameter: "fillet_type.PerEdgeConstant".into(),
                        expected: "radius entry for every selected edge".into(),
                        received: format!("edge {} has no radius entry", edge_id),
                    });
                }
            }
            // No extras: every map key must correspond to a selected
            // edge. Catches typos in caller-supplied edge ids and
            // stale entries from interactive UIs that mutate the
            // selection after the radius dictionary was built.
            for &mapped_id in map.keys() {
                if !selection.contains(&mapped_id) {
                    return Err(OperationError::InvalidInput {
                        parameter: "fillet_type.PerEdgeConstant".into(),
                        expected: "no extra map entries beyond the selection".into(),
                        received: format!(
                            "edge {} has a radius entry but is not in the selection",
                            mapped_id
                        ),
                    });
                }
            }
            // Per-entry radius validity. Tolerance-aware so a
            // sub-tolerance "round" can't slip past as 1e-15 and
            // collapse into a numerical artifact downstream.
            for (&edge_id, &r) in map.iter() {
                if !r.is_finite() || r <= tol {
                    return Err(OperationError::InvalidInput {
                        parameter: format!("fillet_type.PerEdgeConstant[{}]", edge_id),
                        expected: format!("finite radius > tolerance ({:.3e})", tol),
                        received: format!("{r}"),
                    });
                }
            }
        }
        FilletType::PerEdgeProfile(map) => {
            // F5-β.5.6: structural validation for the per-edge
            // mixed-kind variant. Same coverage / no-extras
            // contract as `PerEdgeConstant`, but each entry is
            // a `BlendRadius` profile so the per-entry validity
            // check needs to descend into the variant.
            if map.is_empty() {
                return Err(OperationError::InvalidInput {
                    parameter: "fillet_type.PerEdgeProfile".into(),
                    expected: "non-empty per-edge profile map".into(),
                    received: "empty map".into(),
                });
            }
            // Coverage: every selected edge must have an entry.
            let selection: HashSet<EdgeId> = edges.iter().copied().collect();
            for &edge_id in edges {
                if !map.contains_key(&edge_id) {
                    return Err(OperationError::InvalidInput {
                        parameter: "fillet_type.PerEdgeProfile".into(),
                        expected: "profile entry for every selected edge".into(),
                        received: format!("edge {} has no profile entry", edge_id),
                    });
                }
            }
            // No extras: every map key must correspond to a
            // selected edge.
            for &mapped_id in map.keys() {
                if !selection.contains(&mapped_id) {
                    return Err(OperationError::InvalidInput {
                        parameter: "fillet_type.PerEdgeProfile".into(),
                        expected: "no extra map entries beyond the selection".into(),
                        received: format!(
                            "edge {} has a profile entry but is not in the selection",
                            mapped_id
                        ),
                    });
                }
            }
            // Per-entry profile validity. Radius(Constant) +
            // Radius(Linear) share a finite/positive/tolerance
            // check; Radius(Variable) delegates to the per-station
            // validator with a per-edge label so the failing
            // parameter names the offending edge. F5-β.5.7: Chord
            // gets its own finite/positive/tolerance gate using
            // the raw chord length (the radius derivation happens
            // later at surgery time inside `create_chord_fillet`,
            // but a non-positive chord is meaningless on its face).
            for (&edge_id, profile) in map.iter() {
                match profile {
                    EdgeFilletProfile::Radius(BlendRadius::Constant(r)) => {
                        if !r.is_finite() || *r <= tol {
                            return Err(OperationError::InvalidInput {
                                parameter: format!(
                                    "fillet_type.PerEdgeProfile[{}].Constant",
                                    edge_id
                                ),
                                expected: format!(
                                    "finite radius > tolerance ({:.3e})",
                                    tol
                                ),
                                received: format!("{r}"),
                            });
                        }
                    }
                    EdgeFilletProfile::Radius(BlendRadius::Linear { start, end }) => {
                        if !start.is_finite() || *start <= tol {
                            return Err(OperationError::InvalidInput {
                                parameter: format!(
                                    "fillet_type.PerEdgeProfile[{}].Linear.start",
                                    edge_id
                                ),
                                expected: format!(
                                    "finite radius > tolerance ({:.3e})",
                                    tol
                                ),
                                received: format!("{start}"),
                            });
                        }
                        if !end.is_finite() || *end <= tol {
                            return Err(OperationError::InvalidInput {
                                parameter: format!(
                                    "fillet_type.PerEdgeProfile[{}].Linear.end",
                                    edge_id
                                ),
                                expected: format!(
                                    "finite radius > tolerance ({:.3e})",
                                    tol
                                ),
                                received: format!("{end}"),
                            });
                        }
                    }
                    EdgeFilletProfile::Radius(BlendRadius::Variable(samples)) => {
                        let label = format!(
                            "fillet_type.PerEdgeProfile[{}].Variable",
                            edge_id
                        );
                        validate_variable_stations_labelled(samples, &label)?;
                    }
                    EdgeFilletProfile::Chord(c) => {
                        if !c.is_finite() || *c <= tol {
                            return Err(OperationError::InvalidInput {
                                parameter: format!(
                                    "fillet_type.PerEdgeProfile[{}].Chord",
                                    edge_id
                                ),
                                expected: format!(
                                    "finite chord length > tolerance ({:.3e})",
                                    tol
                                ),
                                received: format!("{c}"),
                            });
                        }
                    }
                }
            }
        }
        _ => {
            // Variant-specific validators handle their own radius checks.
        }
    }

    Ok(())
}

/// Validate filleted solid via the kernel's parallel B-Rep validator.
///
/// Runs `Standard`-level validation (topology connectivity + basic geometry
/// checks) on the solid that just received fillet faces. Returns
/// `OperationError::TopologyError` when validation fails so callers can
/// surface the issue rather than silently produce a malformed solid.
fn validate_filleted_solid(model: &BRepModel, solid_id: SolidId) -> OperationResult<()> {
    // Solid existence is a precondition here; if the caller passed an
    // unknown id, treat that as an internal logic error.
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidGeometry(format!(
            "validate_filleted_solid: solid {} not found",
            solid_id
        )));
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
        return Err(OperationError::TopologyError(format!(
            "filleted solid failed validation ({} error(s)): {}",
            result.errors.len(),
            summary
        )));
    }
    Ok(())
}

/// Validate fillet parameters
fn validate_fillet_parameters(
    model: &BRepModel,
    edge_id: EdgeId,
    radius: f64,
    tolerance: &Tolerance,
) -> OperationResult<()> {
    if radius <= 0.0 {
        return Err(OperationError::InvalidRadius(radius));
    }

    // Get edge
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

    // Check that radius is not too large for the edge length. Use the
    // caller-supplied tolerance so arc-length integration matches the
    // precision the fillet operation will run at downstream — running
    // it at a different (default) tolerance can let a borderline-too-
    // large radius slip past validation.
    let edge_length = edge.compute_arc_length(&model.curves, *tolerance)?;
    if radius > edge_length * 0.5 {
        return Err(OperationError::InvalidRadius(radius));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::curve::Line;
    use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

    /// Build a 1m × 1m × 1m box and return its solid id.
    fn build_unit_box(model: &mut BRepModel) -> SolidId {
        let mut builder = TopologyBuilder::new(model);
        match builder.create_box_3d(1.0, 1.0, 1.0).expect("box") {
            GeometryId::Solid(id) => id,
            other => panic!("expected solid, got {other:?}"),
        }
    }

    /// Add a line edge between two new vertices and return its id along
    /// with both vertex ids.
    fn add_simple_edge(
        model: &mut BRepModel,
        from: Point3,
        to: Point3,
    ) -> (EdgeId, VertexId, VertexId) {
        let v0 = model.vertices.add(from.x, from.y, from.z);
        let v1 = model.vertices.add(to.x, to.y, to.z);
        let line = Line::new(from, to);
        let curve_id = model.curves.add(Box::new(line));
        let edge = Edge::new_auto_range(0, v0, v1, curve_id, EdgeOrientation::Forward);
        let edge_id = model.edges.add(edge);
        (edge_id, v0, v1)
    }

    // -------------------------------------------------------------------
    // FilletOptions / FilletType / PropagationMode / FilletQuality
    // -------------------------------------------------------------------

    #[test]
    fn fillet_options_default_radius_is_five() {
        let opts = FilletOptions::default();
        assert!((opts.radius - 5.0).abs() < 1e-12);
        assert!(matches!(opts.fillet_type, FilletType::Constant(r) if (r - 5.0).abs() < 1e-12));
        assert_eq!(opts.propagation, PropagationMode::Tangent);
        assert!(opts.preserve_edges);
        assert_eq!(opts.quality, FilletQuality::Standard);
    }

    #[test]
    fn fillet_type_function_clone_falls_back_to_constant_five() {
        let f = FilletType::Function(Box::new(|t: f64| 1.0 + t));
        let cloned = f.clone();
        match cloned {
            FilletType::Constant(r) => assert!((r - 5.0).abs() < 1e-12),
            other => panic!("expected Constant fallback, got {other:?}"),
        }
    }

    #[test]
    fn fillet_type_constant_clone_round_trips() {
        let f = FilletType::Constant(2.5);
        if let FilletType::Constant(r) = f.clone() {
            assert!((r - 2.5).abs() < 1e-12);
        } else {
            panic!("clone changed variant");
        }
    }

    #[test]
    fn fillet_type_variable_clone_round_trips() {
        let f = FilletType::Variable(1.0, 3.0);
        if let FilletType::Variable(a, b) = f.clone() {
            assert!((a - 1.0).abs() < 1e-12);
            assert!((b - 3.0).abs() < 1e-12);
        } else {
            panic!("clone changed variant");
        }
    }

    #[test]
    fn fillet_type_chord_clone_round_trips() {
        let f = FilletType::Chord(0.7);
        if let FilletType::Chord(c) = f.clone() {
            assert!((c - 0.7).abs() < 1e-12);
        } else {
            panic!("clone changed variant");
        }
    }

    #[test]
    fn fillet_type_debug_format_includes_value() {
        let s = format!("{:?}", FilletType::Constant(2.0));
        assert!(s.contains("Constant"));
        let s = format!("{:?}", FilletType::Function(Box::new(|_| 0.0)));
        assert!(s.contains("Function"));
    }

    #[test]
    fn propagation_mode_variants_distinct() {
        assert_ne!(PropagationMode::None, PropagationMode::Tangent);
        assert_ne!(PropagationMode::Tangent, PropagationMode::Smooth);
        assert_ne!(PropagationMode::Smooth, PropagationMode::All);
    }

    // =================================================================
    // F3-ε.2 kernel-layer harness: FilletType::VariableStations
    // =================================================================
    //
    // These tests pin (a) the structural invariants the kernel
    // enforces on a per-station payload, (b) the piecewise-linear
    // evaluator's correctness against hand-computed reference
    // values, and (c) the `FilletType → BlendRadius` bridge so the
    // shape passed downstream to spine_solver / BlendGraph is
    // exactly what the spec says.

    #[test]
    fn variable_stations_validator_accepts_minimal_two_point() {
        let samples = vec![(0.0, 1.0), (1.0, 2.0)];
        assert!(validate_variable_stations(&samples).is_ok());
    }

    #[test]
    fn variable_stations_validator_accepts_full_unit_range_with_interior() {
        let samples = vec![(0.0, 1.0), (0.25, 1.5), (0.5, 2.0), (0.75, 1.5), (1.0, 1.0)];
        assert!(validate_variable_stations(&samples).is_ok());
    }

    #[test]
    fn variable_stations_validator_rejects_empty() {
        let err = validate_variable_stations(&[]).unwrap_err();
        match err {
            OperationError::InvalidInput { parameter, .. } => {
                assert!(
                    parameter.contains("VariableStations"),
                    "parameter must name the offending field; got {parameter}"
                );
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn variable_stations_validator_rejects_non_increasing_stations() {
        let samples = vec![(0.5, 1.0), (0.5, 2.0)]; // equal stations
        let err = validate_variable_stations(&samples).unwrap_err();
        assert!(matches!(err, OperationError::InvalidInput { .. }));

        let samples = vec![(0.6, 1.0), (0.4, 2.0)]; // decreasing
        let err = validate_variable_stations(&samples).unwrap_err();
        assert!(matches!(err, OperationError::InvalidInput { .. }));
    }

    #[test]
    fn variable_stations_validator_rejects_station_below_zero() {
        let samples = vec![(-0.1, 1.0), (1.0, 2.0)];
        let err = validate_variable_stations(&samples).unwrap_err();
        match err {
            OperationError::InvalidInput { parameter, .. } => {
                assert!(parameter.contains("station"));
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn variable_stations_validator_rejects_station_above_one() {
        let samples = vec![(0.0, 1.0), (1.5, 2.0)];
        let err = validate_variable_stations(&samples).unwrap_err();
        assert!(matches!(err, OperationError::InvalidInput { .. }));
    }

    #[test]
    fn variable_stations_validator_rejects_non_positive_radius() {
        let samples = vec![(0.0, 0.0), (1.0, 1.0)];
        let err = validate_variable_stations(&samples).unwrap_err();
        match err {
            OperationError::InvalidInput { parameter, expected, .. } => {
                assert!(parameter.contains("radius"));
                assert_eq!(expected, "> 0");
            }
            other => panic!("expected InvalidInput on radius, got {other:?}"),
        }

        let samples = vec![(0.0, 1.0), (1.0, -1.0)];
        let err = validate_variable_stations(&samples).unwrap_err();
        assert!(matches!(err, OperationError::InvalidInput { .. }));
    }

    #[test]
    fn variable_stations_validator_rejects_non_finite() {
        let samples = vec![(0.0, f64::NAN), (1.0, 2.0)];
        assert!(validate_variable_stations(&samples).is_err());
        let samples = vec![(f64::INFINITY, 1.0), (1.0, 2.0)];
        assert!(validate_variable_stations(&samples).is_err());
    }

    #[test]
    fn piecewise_linear_radius_endpoint_clamps() {
        let samples = vec![(0.0, 1.0), (1.0, 3.0)];
        // u below first station clamps to first radius.
        assert!((piecewise_linear_radius(&samples, -0.1) - 1.0).abs() < 1e-12);
        assert!((piecewise_linear_radius(&samples, 0.0) - 1.0).abs() < 1e-12);
        // u above last clamps to last radius.
        assert!((piecewise_linear_radius(&samples, 1.5) - 3.0).abs() < 1e-12);
        assert!((piecewise_linear_radius(&samples, 1.0) - 3.0).abs() < 1e-12);
    }

    #[test]
    fn piecewise_linear_radius_midpoint_is_linear_mean() {
        let samples = vec![(0.0, 1.0), (1.0, 3.0)];
        assert!((piecewise_linear_radius(&samples, 0.5) - 2.0).abs() < 1e-12);
        assert!((piecewise_linear_radius(&samples, 0.25) - 1.5).abs() < 1e-12);
        assert!((piecewise_linear_radius(&samples, 0.75) - 2.5).abs() < 1e-12);
    }

    #[test]
    fn piecewise_linear_radius_non_monotone_profile() {
        // Tear-drop profile: peak at u=0.5.
        let samples = vec![(0.0, 1.0), (0.5, 3.0), (1.0, 1.0)];
        assert!((piecewise_linear_radius(&samples, 0.0) - 1.0).abs() < 1e-12);
        assert!((piecewise_linear_radius(&samples, 0.5) - 3.0).abs() < 1e-12);
        assert!((piecewise_linear_radius(&samples, 1.0) - 1.0).abs() < 1e-12);
        // Midpoint of first segment.
        assert!((piecewise_linear_radius(&samples, 0.25) - 2.0).abs() < 1e-12);
        // Midpoint of second segment.
        assert!((piecewise_linear_radius(&samples, 0.75) - 2.0).abs() < 1e-12);
    }

    #[test]
    fn piecewise_linear_radius_single_sample_returns_constant() {
        let samples = vec![(0.5, 2.5)];
        // All u values clamp to the single sample's radius.
        assert!((piecewise_linear_radius(&samples, 0.0) - 2.5).abs() < 1e-12);
        assert!((piecewise_linear_radius(&samples, 0.5) - 2.5).abs() < 1e-12);
        assert!((piecewise_linear_radius(&samples, 1.0) - 2.5).abs() < 1e-12);
    }

    #[test]
    fn fillet_type_to_blend_radius_maps_variable_stations_to_variable() {
        let samples = vec![(0.0, 1.0), (0.5, 2.0), (1.0, 1.5)];
        let ft = FilletType::VariableStations(samples.clone());
        match fillet_type_to_blend_radius(&ft, 0) {
            BlendRadius::Variable(out) => assert_eq!(out, samples),
            other => panic!("expected BlendRadius::Variable, got {other:?}"),
        }
    }

    #[test]
    fn fillet_type_to_blend_radius_constant_still_maps_to_constant() {
        // Regression pin: adding VariableStations / PerEdgeConstant
        // must not alter the existing Constant → Constant / Variable(2)
        // → Linear mappings used by every legacy fillet path.
        match fillet_type_to_blend_radius(&FilletType::Constant(2.0), 0) {
            BlendRadius::Constant(r) => assert!((r - 2.0).abs() < 1e-12),
            other => panic!("expected BlendRadius::Constant, got {other:?}"),
        }
        match fillet_type_to_blend_radius(&FilletType::Variable(1.0, 3.0), 0) {
            BlendRadius::Linear { start, end } => {
                assert!((start - 1.0).abs() < 1e-12);
                assert!((end - 3.0).abs() < 1e-12);
            }
            other => panic!("expected BlendRadius::Linear, got {other:?}"),
        }
    }

    #[test]
    fn fillet_type_variable_stations_clones_independently() {
        let original = FilletType::VariableStations(vec![(0.0, 1.0), (1.0, 2.0)]);
        let copy = original.clone();
        // Both Debug to the same string; the underlying samples
        // are owned by independent Vecs (Clone == deep on Vec).
        let s_orig = format!("{:?}", original);
        let s_copy = format!("{:?}", copy);
        assert_eq!(s_orig, s_copy);
        assert!(s_orig.contains("VariableStations"));
    }

    #[test]
    fn fillet_type_variable_stations_debug_includes_samples() {
        let ft = FilletType::VariableStations(vec![(0.25, 1.5), (0.75, 2.5)]);
        let s = format!("{:?}", ft);
        assert!(s.contains("VariableStations"));
        assert!(s.contains("0.25"));
        assert!(s.contains("1.5"));
        assert!(s.contains("0.75"));
        assert!(s.contains("2.5"));
    }

    #[test]
    fn fillet_quality_variants_distinct() {
        assert_ne!(FilletQuality::Draft, FilletQuality::Standard);
        assert_ne!(FilletQuality::Standard, FilletQuality::High);
    }

    // -------------------------------------------------------------------
    // validate_fillet_inputs
    // -------------------------------------------------------------------

    #[test]
    fn validate_fillet_inputs_rejects_unknown_solid() {
        let model = BRepModel::new();
        let opts = FilletOptions::default();
        let result = validate_fillet_inputs(&model, 99_999, &[0], &opts);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn validate_fillet_inputs_rejects_empty_edge_list() {
        let mut model = BRepModel::new();
        let solid_id = build_unit_box(&mut model);
        let opts = FilletOptions::default();
        let result = validate_fillet_inputs(&model, solid_id, &[], &opts);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn validate_fillet_inputs_rejects_unknown_edge() {
        let mut model = BRepModel::new();
        let solid_id = build_unit_box(&mut model);
        let opts = FilletOptions::default();
        let result = validate_fillet_inputs(&model, solid_id, &[99_999], &opts);
        match result {
            Err(OperationError::InvalidGeometry(msg)) => {
                assert!(msg.contains("edge"), "msg = {msg}");
            }
            other => panic!("expected InvalidGeometry, got {other:?}"),
        }
    }

    #[test]
    fn validate_fillet_inputs_rejects_radius_below_tolerance() {
        let mut model = BRepModel::new();
        let solid_id = build_unit_box(&mut model);
        let (edge_id, _, _) =
            add_simple_edge(&mut model, Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0));
        let opts = FilletOptions {
            radius: 1e-15, // below default tolerance
            fillet_type: FilletType::Constant(1e-15),
            ..Default::default()
        };
        let result = validate_fillet_inputs(&model, solid_id, &[edge_id], &opts);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn validate_fillet_inputs_rejects_non_finite_radius() {
        let mut model = BRepModel::new();
        let solid_id = build_unit_box(&mut model);
        let (edge_id, _, _) =
            add_simple_edge(&mut model, Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0));
        let opts = FilletOptions {
            radius: f64::NAN,
            fillet_type: FilletType::Constant(0.5),
            ..Default::default()
        };
        let result = validate_fillet_inputs(&model, solid_id, &[edge_id], &opts);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn validate_fillet_inputs_accepts_valid_constant_radius() {
        let mut model = BRepModel::new();
        let solid_id = build_unit_box(&mut model);
        let (edge_id, _, _) =
            add_simple_edge(&mut model, Point3::ORIGIN, Point3::new(2.0, 0.0, 0.0));
        let opts = FilletOptions {
            radius: 0.1,
            fillet_type: FilletType::Constant(0.1),
            ..Default::default()
        };
        assert!(validate_fillet_inputs(&model, solid_id, &[edge_id], &opts).is_ok());
    }

    #[test]
    fn validate_fillet_inputs_accepts_chord_variant_with_valid_options_radius() {
        // Variant-specific (non-Constant) types skip the inner radius
        // check; only the outer options.radius must clear tolerance.
        let mut model = BRepModel::new();
        let solid_id = build_unit_box(&mut model);
        let (edge_id, _, _) =
            add_simple_edge(&mut model, Point3::ORIGIN, Point3::new(2.0, 0.0, 0.0));
        let opts = FilletOptions {
            radius: 0.5,
            fillet_type: FilletType::Chord(0.3),
            ..Default::default()
        };
        assert!(validate_fillet_inputs(&model, solid_id, &[edge_id], &opts).is_ok());
    }

    // -------------------------------------------------------------------
    // validate_fillet_parameters
    // -------------------------------------------------------------------

    #[test]
    fn validate_fillet_parameters_rejects_negative_radius() {
        let mut model = BRepModel::new();
        let (edge_id, _, _) =
            add_simple_edge(&mut model, Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0));
        let tol = Tolerance::default();
        let result = validate_fillet_parameters(&model, edge_id, -1.0, &tol);
        assert!(matches!(result, Err(OperationError::InvalidRadius(_))));
    }

    #[test]
    fn validate_fillet_parameters_rejects_zero_radius() {
        let mut model = BRepModel::new();
        let (edge_id, _, _) =
            add_simple_edge(&mut model, Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0));
        let tol = Tolerance::default();
        let result = validate_fillet_parameters(&model, edge_id, 0.0, &tol);
        assert!(matches!(result, Err(OperationError::InvalidRadius(_))));
    }

    #[test]
    fn validate_fillet_parameters_rejects_unknown_edge() {
        let model = BRepModel::new();
        let tol = Tolerance::default();
        let result = validate_fillet_parameters(&model, 99_999, 1.0, &tol);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn validate_fillet_parameters_rejects_radius_above_half_edge_length() {
        let mut model = BRepModel::new();
        let (edge_id, _, _) =
            add_simple_edge(&mut model, Point3::ORIGIN, Point3::new(2.0, 0.0, 0.0));
        let tol = Tolerance::default();
        // edge length = 2, half = 1. radius = 1.5 > 1 must fail.
        let result = validate_fillet_parameters(&model, edge_id, 1.5, &tol);
        assert!(matches!(result, Err(OperationError::InvalidRadius(_))));
    }

    #[test]
    fn validate_fillet_parameters_accepts_radius_below_half_edge_length() {
        let mut model = BRepModel::new();
        let (edge_id, _, _) =
            add_simple_edge(&mut model, Point3::ORIGIN, Point3::new(10.0, 0.0, 0.0));
        let tol = Tolerance::default();
        assert!(validate_fillet_parameters(&model, edge_id, 1.0, &tol).is_ok());
    }

    // -------------------------------------------------------------------
    // group_edges_into_chains
    // -------------------------------------------------------------------

    #[test]
    fn group_edges_into_chains_empty_input_yields_empty_output() {
        let model = BRepModel::new();
        let chains = group_edges_into_chains(&model, &[]).expect("group");
        assert!(chains.is_empty());
    }

    #[test]
    fn group_edges_into_chains_single_edge_yields_single_chain() {
        let mut model = BRepModel::new();
        let (e, _, _) = add_simple_edge(&mut model, Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0));
        let chains = group_edges_into_chains(&model, &[e]).expect("group");
        assert_eq!(chains.len(), 1);
        assert_eq!(chains[0], vec![e]);
    }

    #[test]
    fn group_edges_into_chains_disconnected_pair_yields_two_chains() {
        let mut model = BRepModel::new();
        let (e1, _, _) = add_simple_edge(&mut model, Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0));
        let (e2, _, _) = add_simple_edge(
            &mut model,
            Point3::new(10.0, 10.0, 0.0),
            Point3::new(11.0, 10.0, 0.0),
        );
        let chains = group_edges_into_chains(&model, &[e1, e2]).expect("group");
        assert_eq!(chains.len(), 2);
    }

    #[test]
    fn group_edges_into_chains_three_connected_edges_yields_one_chain() {
        // Build a 3-edge connected chain v0-v1-v2-v3 sharing vertices.
        let mut model = BRepModel::new();
        let v0 = model.vertices.add(0.0, 0.0, 0.0);
        let v1 = model.vertices.add(1.0, 0.0, 0.0);
        let v2 = model.vertices.add(2.0, 0.0, 0.0);
        let v3 = model.vertices.add(3.0, 0.0, 0.0);
        let l1 = Line::new(Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0));
        let l2 = Line::new(Point3::new(1.0, 0.0, 0.0), Point3::new(2.0, 0.0, 0.0));
        let l3 = Line::new(Point3::new(2.0, 0.0, 0.0), Point3::new(3.0, 0.0, 0.0));
        let c1 = model.curves.add(Box::new(l1));
        let c2 = model.curves.add(Box::new(l2));
        let c3 = model.curves.add(Box::new(l3));
        let e1 = model
            .edges
            .add(Edge::new_auto_range(0, v0, v1, c1, EdgeOrientation::Forward));
        let e2 = model
            .edges
            .add(Edge::new_auto_range(0, v1, v2, c2, EdgeOrientation::Forward));
        let e3 = model
            .edges
            .add(Edge::new_auto_range(0, v2, v3, c3, EdgeOrientation::Forward));
        let chains = group_edges_into_chains(&model, &[e1, e2, e3]).expect("group");
        assert_eq!(chains.len(), 1);
        assert_eq!(chains[0].len(), 3);
    }

    #[test]
    fn group_edges_into_chains_rejects_unknown_edge() {
        let model = BRepModel::new();
        let result = group_edges_into_chains(&model, &[99_999]);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    // -------------------------------------------------------------------
    // are_edges_tangent
    // -------------------------------------------------------------------

    #[test]
    fn are_edges_tangent_collinear_edges_are_tangent() {
        let mut model = BRepModel::new();
        let v0 = model.vertices.add(0.0, 0.0, 0.0);
        let v1 = model.vertices.add(1.0, 0.0, 0.0);
        let v2 = model.vertices.add(2.0, 0.0, 0.0);
        let l1 = Line::new(Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0));
        let l2 = Line::new(Point3::new(1.0, 0.0, 0.0), Point3::new(2.0, 0.0, 0.0));
        let c1 = model.curves.add(Box::new(l1));
        let c2 = model.curves.add(Box::new(l2));
        let e1 = model
            .edges
            .add(Edge::new_auto_range(0, v0, v1, c1, EdgeOrientation::Forward));
        let e2 = model
            .edges
            .add(Edge::new_auto_range(0, v1, v2, c2, EdgeOrientation::Forward));
        assert!(are_edges_tangent(&model, e1, e2).expect("tangent"));
    }

    #[test]
    fn are_edges_tangent_orthogonal_edges_are_not_tangent() {
        let mut model = BRepModel::new();
        let v0 = model.vertices.add(0.0, 0.0, 0.0);
        let v1 = model.vertices.add(1.0, 0.0, 0.0);
        let v2 = model.vertices.add(1.0, 1.0, 0.0);
        let l1 = Line::new(Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0));
        let l2 = Line::new(Point3::new(1.0, 0.0, 0.0), Point3::new(1.0, 1.0, 0.0));
        let c1 = model.curves.add(Box::new(l1));
        let c2 = model.curves.add(Box::new(l2));
        let e1 = model
            .edges
            .add(Edge::new_auto_range(0, v0, v1, c1, EdgeOrientation::Forward));
        let e2 = model
            .edges
            .add(Edge::new_auto_range(0, v1, v2, c2, EdgeOrientation::Forward));
        assert!(!are_edges_tangent(&model, e1, e2).expect("tangent"));
    }

    #[test]
    fn are_edges_tangent_disconnected_edges_are_not_tangent() {
        let mut model = BRepModel::new();
        let (e1, _, _) = add_simple_edge(&mut model, Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0));
        let (e2, _, _) = add_simple_edge(
            &mut model,
            Point3::new(10.0, 10.0, 0.0),
            Point3::new(11.0, 10.0, 0.0),
        );
        assert!(!are_edges_tangent(&model, e1, e2).expect("tangent"));
    }

    #[test]
    fn are_edges_tangent_rejects_unknown_first_edge() {
        let mut model = BRepModel::new();
        let (e2, _, _) = add_simple_edge(&mut model, Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0));
        let result = are_edges_tangent(&model, 99_999, e2);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    // -------------------------------------------------------------------
    // find_edges_at_vertex
    // -------------------------------------------------------------------

    #[test]
    fn find_edges_at_vertex_returns_incident_edges() {
        let mut model = BRepModel::new();
        let v0 = model.vertices.add(0.0, 0.0, 0.0);
        let v1 = model.vertices.add(1.0, 0.0, 0.0);
        let v2 = model.vertices.add(0.0, 1.0, 0.0);
        let l1 = Line::new(Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0));
        let l2 = Line::new(Point3::ORIGIN, Point3::new(0.0, 1.0, 0.0));
        let c1 = model.curves.add(Box::new(l1));
        let c2 = model.curves.add(Box::new(l2));
        let e1 = model
            .edges
            .add(Edge::new_auto_range(0, v0, v1, c1, EdgeOrientation::Forward));
        let e2 = model
            .edges
            .add(Edge::new_auto_range(0, v0, v2, c2, EdgeOrientation::Forward));
        let edges = find_edges_at_vertex(&model, v0).expect("edges");
        assert!(edges.contains(&e1));
        assert!(edges.contains(&e2));
    }

    #[test]
    fn find_edges_at_vertex_returns_empty_for_isolated_vertex() {
        let mut model = BRepModel::new();
        let v = model.vertices.add(0.0, 0.0, 0.0);
        let edges = find_edges_at_vertex(&model, v).expect("edges");
        assert!(edges.is_empty());
    }

    // -------------------------------------------------------------------
    // face_contains_edge
    // -------------------------------------------------------------------

    #[test]
    fn face_contains_edge_returns_true_for_edge_in_outer_loop() {
        // Use a unit-box face — pick one of its edges.
        let mut model = BRepModel::new();
        let solid_id = build_unit_box(&mut model);
        let solid = model.solids.get(solid_id).expect("solid");
        let shell = model.shells.get(solid.outer_shell).expect("shell");
        let face_id = *shell.faces.first().expect("face");
        let face = model.faces.get(face_id).expect("face data").clone();
        let outer_loop = model.loops.get(face.outer_loop).expect("loop");
        let edge_in_face = *outer_loop.edges.first().expect("edge");
        assert!(face_contains_edge(&model, &face, edge_in_face).expect("contains"));
    }

    #[test]
    fn face_contains_edge_returns_false_for_unrelated_edge() {
        let mut model = BRepModel::new();
        let solid_id = build_unit_box(&mut model);
        let solid = model.solids.get(solid_id).expect("solid");
        let shell = model.shells.get(solid.outer_shell).expect("shell");
        let face_id = *shell.faces.first().expect("face");
        let face = model.faces.get(face_id).expect("face data").clone();
        // Synthesise a fresh edge unrelated to the box.
        let (foreign_edge, _, _) = add_simple_edge(
            &mut model,
            Point3::new(50.0, 50.0, 50.0),
            Point3::new(51.0, 50.0, 50.0),
        );
        assert!(!face_contains_edge(&model, &face, foreign_edge).expect("contains"));
    }

    // -------------------------------------------------------------------
    // get_adjacent_faces / get_adjacent_faces_safe
    // -------------------------------------------------------------------

    #[test]
    fn get_adjacent_faces_returns_two_faces_for_box_edge() {
        let mut model = BRepModel::new();
        let solid_id = build_unit_box(&mut model);
        let solid = model.solids.get(solid_id).expect("solid").clone();
        let shell = model.shells.get(solid.outer_shell).expect("shell").clone();
        let face = model.faces.get(shell.faces[0]).expect("face").clone();
        let outer_loop = model.loops.get(face.outer_loop).expect("loop");
        let edge = outer_loop.edges[0];
        let (a, b) = get_adjacent_faces(&model, solid_id, edge).expect("adjacent");
        assert_ne!(a, b);
    }

    #[test]
    fn get_adjacent_faces_safe_returns_two_for_interior_box_edge() {
        let mut model = BRepModel::new();
        let solid_id = build_unit_box(&mut model);
        let solid = model.solids.get(solid_id).expect("solid").clone();
        let shell = model.shells.get(solid.outer_shell).expect("shell").clone();
        let face = model.faces.get(shell.faces[0]).expect("face").clone();
        let outer_loop = model.loops.get(face.outer_loop).expect("loop");
        let edge = outer_loop.edges[0];
        let result = get_adjacent_faces_safe(&model, edge);
        assert!(result.is_ok());
    }

    #[test]
    fn get_adjacent_faces_safe_errors_for_orphan_edge() {
        let mut model = BRepModel::new();
        let _ = build_unit_box(&mut model);
        let (foreign, _, _) = add_simple_edge(
            &mut model,
            Point3::new(99.0, 99.0, 99.0),
            Point3::new(100.0, 99.0, 99.0),
        );
        let result = get_adjacent_faces_safe(&model, foreign);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    // -------------------------------------------------------------------
    // propagate_edge_selection - PropagationMode::None passthrough
    // -------------------------------------------------------------------

    #[test]
    fn propagate_edge_selection_none_returns_input_unchanged() {
        let mut model = BRepModel::new();
        let (e1, _, _) = add_simple_edge(&mut model, Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0));
        let result =
            propagate_edge_selection(&model, vec![e1], PropagationMode::None).expect("propagate");
        assert_eq!(result, vec![e1]);
    }

    // -------------------------------------------------------------------
    // resample_radii_uniform — variable-radius profile resampling
    // -------------------------------------------------------------------

    #[test]
    fn resample_radii_uniform_preserves_endpoints() {
        let radii = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let resampled = resample_radii_uniform(&radii, 20);
        assert_eq!(resampled.len(), 20);
        assert!((resampled[0] - 1.0).abs() < 1e-12);
        assert!((resampled[19] - 5.0).abs() < 1e-12);
    }

    #[test]
    fn resample_radii_uniform_linear_profile_stays_linear() {
        // r(t) = 1 + 9t over t ∈ [0, 1] sampled at 21 points
        let radii: Vec<f64> = (0..=20).map(|i| 1.0 + 9.0 * (i as f64 / 20.0)).collect();
        let resampled = resample_radii_uniform(&radii, 20);
        for (j, r) in resampled.iter().enumerate() {
            // Map j ∈ [0, 19] → t ∈ [0, 1]
            let t = j as f64 / 19.0;
            let expected = 1.0 + 9.0 * t;
            assert!(
                (r - expected).abs() < 1e-9,
                "resample[{}]={} expected {}",
                j,
                r,
                expected
            );
        }
    }

    #[test]
    fn resample_radii_uniform_identity_when_lengths_match() {
        let radii = vec![1.0, 2.5, 4.0, 0.5, 3.0];
        let resampled = resample_radii_uniform(&radii, 5);
        assert_eq!(resampled, radii);
    }

    #[test]
    fn resample_radii_uniform_handles_empty_input() {
        let resampled = resample_radii_uniform(&[], 20);
        assert!(resampled.is_empty());
    }

    // ------------------------------------------------------------------
    // Vertex-blend slice-1 (Task #104) math: concurrent-axes center.
    //
    // These tests target the pure-math helper `compute_concurrent_axes_center`
    // directly, without any topology setup. The helper is the load-bearing
    // numerical primitive for slice 2 (sphere/cylinder SSI) and any future
    // multi-axis corner-blend variants.
    // ------------------------------------------------------------------

    fn make_incident(axis_origin: Point3, axis: Vector3) -> IncidentEdgeClassification {
        IncidentEdgeClassification {
            edge_id: 0,
            adjacent_faces: (0, 0),
            blend: Some(EdgeBlendDescriptor {
                face_id: 0,
                axis,
                axis_origin,
                radius: 0.5,
            }),
        }
    }

    /// Three mutually-perpendicular axes that all pass through (3.5, 3.5, 3.5)
    /// — the canonical "convex corner of a box at (4,4,4) with edge fillets
    /// of r=0.5" scenario. The corner sphere center must land exactly on
    /// that point.
    #[test]
    fn concurrent_axes_center_box_corner() {
        // Axis 1: parallel to X, offset from corner by (0, -0.5, -0.5)
        //   → passes through (anything, 3.5, 3.5).
        // Axis 2: parallel to Y, passes through (3.5, anything, 3.5).
        // Axis 3: parallel to Z, passes through (3.5, 3.5, anything).
        let incidents = vec![
            make_incident(Point3::new(0.0, 3.5, 3.5), Vector3::new(1.0, 0.0, 0.0)),
            make_incident(Point3::new(3.5, 0.0, 3.5), Vector3::new(0.0, 1.0, 0.0)),
            make_incident(Point3::new(3.5, 3.5, 0.0), Vector3::new(0.0, 0.0, 1.0)),
        ];
        let center = compute_concurrent_axes_center(&incidents, 0)
            .expect("three orthogonal axes through a common point must yield a center");
        assert!((center.x - 3.5).abs() < 1e-9, "x = {}", center.x);
        assert!((center.y - 3.5).abs() < 1e-9, "y = {}", center.y);
        assert!((center.z - 3.5).abs() < 1e-9, "z = {}", center.z);
    }

    /// Three non-orthogonal but linearly-independent axes through (1, 2, 3).
    /// The projector-matrix least-squares solver must recover the exact
    /// concurrent point regardless of axis orientation, as long as the
    /// directions span ℝ³.
    #[test]
    fn concurrent_axes_center_skew_axes() {
        let p = Point3::new(1.0, 2.0, 3.0);
        let dirs = [
            Vector3::new(1.0, 1.0, 0.0),
            Vector3::new(0.0, 1.0, 1.0),
            Vector3::new(1.0, 0.0, 1.0),
        ];
        let mut incidents = Vec::new();
        for d in &dirs {
            let n = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt();
            let u = Vector3::new(d.x / n, d.y / n, d.z / n);
            // Pick an axis_origin = p + (offset along u), so the line
            // passes through p but axis_origin is not p itself.
            let q = Point3::new(p.x + 0.3 * u.x, p.y + 0.3 * u.y, p.z + 0.3 * u.z);
            incidents.push(make_incident(q, u));
        }
        let center = compute_concurrent_axes_center(&incidents, 0)
            .expect("three skew non-coplanar axes through a common point must solve");
        assert!((center.x - 1.0).abs() < 1e-9, "x = {}", center.x);
        assert!((center.y - 2.0).abs() < 1e-9, "y = {}", center.y);
        assert!((center.z - 3.0).abs() < 1e-9, "z = {}", center.z);
    }

    /// Three parallel axes (all in the Z direction) leave the projector
    /// sum rank-deficient (A has a Z-direction null space). The 3×3
    /// inverse must fail; the helper turns that into InvalidGeometry.
    #[test]
    fn concurrent_axes_center_rejects_parallel_axes() {
        let incidents = vec![
            make_incident(Point3::new(0.0, 0.0, 0.0), Vector3::new(0.0, 0.0, 1.0)),
            make_incident(Point3::new(1.0, 0.0, 5.0), Vector3::new(0.0, 0.0, 1.0)),
            make_incident(Point3::new(0.0, 1.0, -2.0), Vector3::new(0.0, 0.0, 1.0)),
        ];
        let err = compute_concurrent_axes_center(&incidents, 42)
            .expect_err("parallel axes must be rejected as rank-deficient");
        match err {
            OperationError::InvalidGeometry(msg) => {
                assert!(
                    msg.contains("rank-deficiency") || msg.contains("span ℝ³"),
                    "expected rank-deficiency / span-ℝ³ diagnostic; got: {msg}"
                );
            }
            other => panic!("expected InvalidGeometry; got {other:?}"),
        }
    }

    /// Two non-parallel axes (X and Y through origin) form a rank-3
    /// projector sum: A = M_X + M_Y = diag(1, 1, 2), invertible. The
    /// helper therefore *succeeds* and returns the least-squares-best
    /// center — which, for axes that genuinely meet, is the exact
    /// intersection. Slice 1's "≥3 incidents" rule is enforced by the
    /// caller (`gather_vertex_blend_context`), not by the math helper.
    #[test]
    fn concurrent_axes_center_two_axes_solves_when_intersecting() {
        let incidents = vec![
            make_incident(Point3::new(0.0, 0.0, 0.0), Vector3::new(1.0, 0.0, 0.0)),
            make_incident(Point3::new(0.0, 0.0, 0.0), Vector3::new(0.0, 1.0, 0.0)),
        ];
        let center = compute_concurrent_axes_center(&incidents, 0)
            .expect("two intersecting axes form an invertible normal-equations matrix");
        assert!(center.x.abs() < 1e-12);
        assert!(center.y.abs() < 1e-12);
        assert!(center.z.abs() < 1e-12);
    }

    // ------------------------------------------------------------------
    // Vertex-blend slice-1 property tests (Task #105).
    //
    // These exercise `compute_concurrent_axes_center` across the full
    // input envelope rather than a fixed scenario. Each property
    // encodes an *invariant* that must hold for every input drawn from
    // the strategy:
    //
    //   1. **Round-trip recovery**: for any common point P and any
    //      three linearly-independent unit axes through P, the solver
    //      must recover P to within 1e-9. This catches sign errors,
    //      transposed accumulators, and column/row major confusion in
    //      the projector-matrix code.
    //
    //   2. **Rank-deficiency rejection**: for any set of axes all
    //      parallel to a common direction, the solver must fail with
    //      InvalidGeometry. Catches accidental regularization that
    //      would silently produce a fake answer in the degenerate case.
    //
    //   3. **Translation invariance**: shifting every (axis_origin, P)
    //      by the same vector must shift the recovered center by the
    //      same vector. Catches absolute-position bias in the
    //      least-squares formulation.
    //
    //   4. **Residual identity**: when the input axes really are
    //      concurrent, the per-axis residual (distance from recovered
    //      center to each axis line) must be ≤ 1e-9 — the same bound
    //      the slice-1 wrapper's gate uses, with 3 orders of magnitude
    //      of headroom over the 1e-6 production gate.
    // ------------------------------------------------------------------

    use proptest::prelude::*;

    /// Strategy for a finite, well-conditioned 3D point in [-50, 50]³.
    fn arb_point() -> impl Strategy<Value = Point3> {
        (-50.0_f64..50.0, -50.0_f64..50.0, -50.0_f64..50.0)
            .prop_map(|(x, y, z)| Point3::new(x, y, z))
    }

    /// Strategy for a unit vector whose components are not so small
    /// that normalization loses precision. We reject candidates with
    /// magnitude below 0.25 to keep the unit direction stable.
    fn arb_unit_vector() -> impl Strategy<Value = Vector3> {
        (-1.0_f64..1.0, -1.0_f64..1.0, -1.0_f64..1.0)
            .prop_filter("reject near-zero vectors", |(x, y, z)| {
                let m = (x * x + y * y + z * z).sqrt();
                m > 0.25
            })
            .prop_map(|(x, y, z)| {
                let m = (x * x + y * y + z * z).sqrt();
                Vector3::new(x / m, y / m, z / m)
            })
    }

    /// Strategy for three unit vectors whose Gram determinant exceeds
    /// 0.1 — i.e. they span ℝ³ with a healthy margin. Filtering on
    /// |det| > 0.1 prevents the least-squares system from being ill-
    /// conditioned (which would still solve, but with a larger
    /// numerical-rounding residual).
    fn arb_three_independent_axes() -> impl Strategy<Value = (Vector3, Vector3, Vector3)> {
        (arb_unit_vector(), arb_unit_vector(), arb_unit_vector()).prop_filter(
            "axes must span ℝ³ with |det| > 0.1",
            |(u, v, w)| {
                // Determinant of the matrix [u | v | w] (column-stacked).
                let det = u.x * (v.y * w.z - v.z * w.y)
                    - u.y * (v.x * w.z - v.z * w.x)
                    + u.z * (v.x * w.y - v.y * w.x);
                det.abs() > 0.1
            },
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 256,
            max_global_rejects: 4096,
            ..ProptestConfig::default()
        })]

        /// Property 1: round-trip recovery. For any common point P and
        /// three linearly-independent unit directions, build three
        /// incidents whose lines all pass through P. The solver must
        /// return P to within 1e-9.
        #[test]
        fn prop_axes_through_common_point_recover_it(
            p in arb_point(),
            (u1, u2, u3) in arb_three_independent_axes(),
            t1 in -10.0_f64..10.0,
            t2 in -10.0_f64..10.0,
            t3 in -10.0_f64..10.0,
        ) {
            // Each axis_origin = P + t·u so the line through it in
            // direction u definitionally passes through P.
            let q1 = Point3::new(p.x + t1 * u1.x, p.y + t1 * u1.y, p.z + t1 * u1.z);
            let q2 = Point3::new(p.x + t2 * u2.x, p.y + t2 * u2.y, p.z + t2 * u2.z);
            let q3 = Point3::new(p.x + t3 * u3.x, p.y + t3 * u3.y, p.z + t3 * u3.z);
            let incidents = vec![
                make_incident(q1, u1),
                make_incident(q2, u2),
                make_incident(q3, u3),
            ];
            let center = compute_concurrent_axes_center(&incidents, 0)
                .expect("axes through a common point are always rank-3");
            prop_assert!(
                (center.x - p.x).abs() < 1e-9,
                "x recovered={} expected={}", center.x, p.x,
            );
            prop_assert!(
                (center.y - p.y).abs() < 1e-9,
                "y recovered={} expected={}", center.y, p.y,
            );
            prop_assert!(
                (center.z - p.z).abs() < 1e-9,
                "z recovered={} expected={}", center.z, p.z,
            );
        }

        /// Property 2: rank-deficiency. Any three axes that all share
        /// the same direction (regardless of axis_origin) must be
        /// rejected with InvalidGeometry. Catches accidental
        /// regularization or silent fall-through paths.
        #[test]
        fn prop_parallel_axes_always_rejected(
            u in arb_unit_vector(),
            q1 in arb_point(),
            q2 in arb_point(),
            q3 in arb_point(),
        ) {
            let incidents = vec![
                make_incident(q1, u),
                make_incident(q2, u),
                make_incident(q3, u),
            ];
            let result = compute_concurrent_axes_center(&incidents, 0);
            match result {
                Err(OperationError::InvalidGeometry(_)) => {}
                Err(other) => prop_assert!(
                    false,
                    "expected InvalidGeometry for parallel axes; got {other:?}"
                ),
                Ok(c) => prop_assert!(
                    false,
                    "expected rejection for parallel axes; got center {:?}", c
                ),
            }
        }

        /// Property 3: translation invariance. Translating every
        /// (axis_origin) by a constant vector δ must translate the
        /// recovered center by the same δ. Catches absolute-position
        /// bias in the least-squares formulation — the normal
        /// equations are affine-invariant, so a translation of the
        /// inputs must come out as a translation of the output, with
        /// no residual error introduced.
        #[test]
        fn prop_translation_invariance(
            p in arb_point(),
            (u1, u2, u3) in arb_three_independent_axes(),
            delta in arb_point(),
        ) {
            let q1 = p;
            let q2 = p;
            let q3 = p;
            let incidents_base = vec![
                make_incident(q1, u1),
                make_incident(q2, u2),
                make_incident(q3, u3),
            ];
            let center_base = compute_concurrent_axes_center(&incidents_base, 0)
                .expect("concurrent axes always solve");

            let q1p = Point3::new(q1.x + delta.x, q1.y + delta.y, q1.z + delta.z);
            let q2p = Point3::new(q2.x + delta.x, q2.y + delta.y, q2.z + delta.z);
            let q3p = Point3::new(q3.x + delta.x, q3.y + delta.y, q3.z + delta.z);
            let incidents_shifted = vec![
                make_incident(q1p, u1),
                make_incident(q2p, u2),
                make_incident(q3p, u3),
            ];
            let center_shifted = compute_concurrent_axes_center(&incidents_shifted, 0)
                .expect("translated concurrent axes still solve");

            prop_assert!(
                (center_shifted.x - center_base.x - delta.x).abs() < 1e-7,
                "Δx invariance violated: {} vs {}",
                center_shifted.x - center_base.x,
                delta.x,
            );
            prop_assert!(
                (center_shifted.y - center_base.y - delta.y).abs() < 1e-7,
                "Δy invariance violated: {} vs {}",
                center_shifted.y - center_base.y,
                delta.y,
            );
            prop_assert!(
                (center_shifted.z - center_base.z - delta.z).abs() < 1e-7,
                "Δz invariance violated: {} vs {}",
                center_shifted.z - center_base.z,
                delta.z,
            );
        }

        /// Property 4: residual identity. When the input axes really
        /// are concurrent through P, the recovered center must lie on
        /// every axis line. We measure this as the perpendicular
        /// distance from `center` to each axis line; all three must
        /// be ≤ 1e-9.
        #[test]
        fn prop_residual_zero_for_concurrent_inputs(
            p in arb_point(),
            (u1, u2, u3) in arb_three_independent_axes(),
        ) {
            let incidents = vec![
                make_incident(p, u1),
                make_incident(p, u2),
                make_incident(p, u3),
            ];
            let center = compute_concurrent_axes_center(&incidents, 0)
                .expect("concurrent axes always solve");

            for (u, q) in [(u1, p), (u2, p), (u3, p)] {
                let dx = center.x - q.x;
                let dy = center.y - q.y;
                let dz = center.z - q.z;
                let proj = u.x * dx + u.y * dy + u.z * dz;
                let perpx = dx - proj * u.x;
                let perpy = dy - proj * u.y;
                let perpz = dz - proj * u.z;
                let dist = (perpx * perpx + perpy * perpy + perpz * perpz).sqrt();
                prop_assert!(
                    dist < 1e-9,
                    "residual {:.3e} exceeds 1e-9 for axis through {:?} in direction {:?}",
                    dist, q, u,
                );
            }
        }
    }

    /// Two skew (non-intersecting, non-parallel) axes still solve, but
    /// the result is the least-squares best point — the midpoint of
    /// the common perpendicular. The residual check in
    /// `gather_vertex_blend_context` rejects this case downstream; here
    /// we only verify the helper does not panic and returns a finite
    /// answer.
    #[test]
    fn concurrent_axes_center_two_skew_axes_returns_midpoint() {
        // L1: x-axis through z = -1.
        // L2: y-axis through z = +1.
        // Common perpendicular is the z-axis; midpoint is the origin.
        let incidents = vec![
            make_incident(Point3::new(0.0, 0.0, -1.0), Vector3::new(1.0, 0.0, 0.0)),
            make_incident(Point3::new(0.0, 0.0, 1.0), Vector3::new(0.0, 1.0, 0.0)),
        ];
        let center = compute_concurrent_axes_center(&incidents, 0)
            .expect("two skew non-parallel axes give an invertible system");
        assert!(center.x.abs() < 1e-12, "x = {}", center.x);
        assert!(center.y.abs() < 1e-12, "y = {}", center.y);
        assert!(center.z.abs() < 1e-12, "z = {}", center.z);
        // Residual: distance from (0,0,0) to L1 is 1.0; the slice-1
        // wrapper's 1e-6 residual gate would reject this case.
    }

    // ------------------------------------------------------------------
    // Task #98 slice 1: dihedral inflection sign-counting kernel
    // ------------------------------------------------------------------

    #[test]
    fn signs_indicate_inflection_all_positive_returns_false() {
        // A consistently convex edge: every sample is > +threshold.
        assert!(!signs_indicate_inflection(
            &[0.5, 1.0, 1.2, 0.8, 0.3],
            0.05
        ));
    }

    #[test]
    fn signs_indicate_inflection_all_negative_returns_false() {
        // A consistently concave edge: every sample is < -threshold.
        assert!(!signs_indicate_inflection(
            &[-0.5, -1.0, -1.2, -0.8, -0.3],
            0.05
        ));
    }

    #[test]
    fn signs_indicate_inflection_mixed_signs_returns_true() {
        // The defining case: at least one sample is convex, at least
        // one is concave, both clear the threshold.
        assert!(signs_indicate_inflection(
            &[0.5, 0.3, 0.1, -0.2, -0.4],
            0.05
        ));
    }

    #[test]
    fn signs_indicate_inflection_endpoint_flip_returns_true() {
        // Inflection at the edge endpoints — the easy detection case.
        assert!(signs_indicate_inflection(&[0.4, 0.2, -0.3], 0.05));
    }

    #[test]
    fn signs_indicate_inflection_below_threshold_treated_as_indeterminate() {
        // Mixed magnitudes but only one side clears the noise floor.
        // The other side is below threshold → indeterminate, not a
        // sign flip. Without this gate, floating-point noise around
        // a tangent-coincident region would generate false positives.
        assert!(!signs_indicate_inflection(
            &[0.4, 0.3, -0.01, -0.02],
            0.05
        ));
        assert!(!signs_indicate_inflection(&[0.02, 0.01, -0.4, -0.3], 0.05));
    }

    #[test]
    fn signs_indicate_inflection_pure_zeros_returns_false() {
        // Degenerate: all-zero (or all-near-zero) sign vector. Neither
        // pos nor neg side has a sample above threshold → no
        // inflection claim. The near-tangent gate that runs alongside
        // catches this case via a different diagnostic.
        assert!(!signs_indicate_inflection(&[0.0, 0.0, 0.0], 0.05));
        assert!(!signs_indicate_inflection(&[0.04, -0.04, 0.03], 0.05));
    }

    #[test]
    fn signs_indicate_inflection_threshold_is_strict() {
        // Threshold comparison is strict (`> threshold`, not `>=`),
        // so a sample exactly at +/- threshold does NOT count. This
        // matches the comparator semantics; the helper's behaviour
        // at the boundary should be deterministic, not flicker.
        assert!(!signs_indicate_inflection(&[0.05, -0.05], 0.05));
        // A hair past the threshold on both sides DOES count.
        assert!(signs_indicate_inflection(
            &[0.051, -0.051],
            0.05
        ));
    }

    #[test]
    fn detect_dihedral_inflection_clamps_below_two_samples() {
        // Sample counts of 0 and 1 cannot represent a sign change;
        // the helper short-circuits to false without touching the
        // model. We use a bogus EdgeId/FaceId because the function
        // must return before any lookups.
        let model = BRepModel::new();
        let edge = Edge::new_auto_range(
            0,
            0,
            1,
            0,
            EdgeOrientation::Forward,
        );
        assert!(!detect_dihedral_inflection(
            &model,
            &edge,
            999_u32,
            999_u32,
            999_u32,
            0
        )
        .expect("sample_count = 0 must short-circuit to Ok(false)"));
        assert!(!detect_dihedral_inflection(
            &model,
            &edge,
            999_u32,
            999_u32,
            999_u32,
            1
        )
        .expect("sample_count = 1 must short-circuit to Ok(false)"));
    }

    // -------------------------------------------------------------------
    // Regression: face-oriented normals across all 12 edges of a box
    //
    // Pins the fix for the "fillet rounds one direction wrong" bug:
    // `get_face_oriented_normal` must apply `face.orientation.sign()` so
    // that faces with `FaceOrientation::Backward` produce outward
    // normals — otherwise half of the edges of an extruded box yield
    // negative signed dihedrals and classify as concave, inverting the
    // rolling-ball offset.
    //
    // Invariant exercised:
    //   For a convex polyhedron (cube), every edge must have a positive
    //   signed dihedral angle (`compute_face_angle > 0`) with magnitude
    //   π/2 (perpendicular adjacent faces).
    // -------------------------------------------------------------------
    #[test]
    fn unit_box_every_edge_classifies_as_convex_ninety_degrees() {
        use std::collections::HashSet;

        let mut model = BRepModel::new();
        let solid_id = build_unit_box(&mut model);

        // Collect the unique edge ids of the box by walking every face's
        // outer loop. Each of the 12 edges appears twice (once per
        // adjacent face) — the HashSet deduplicates.
        let solid = model.solids.get(solid_id).expect("solid").clone();
        let shell = model.shells.get(solid.outer_shell).expect("shell").clone();

        let mut edge_ids: HashSet<EdgeId> = HashSet::new();
        for face_id in &shell.faces {
            let face = model.faces.get(*face_id).expect("face").clone();
            let outer_loop = model.loops.get(face.outer_loop).expect("loop").clone();
            for e in &outer_loop.edges {
                edge_ids.insert(*e);
            }
        }
        assert_eq!(
            edge_ids.len(),
            12,
            "unit box must have exactly 12 unique edges, got {}",
            edge_ids.len()
        );

        // Every box edge: adjacent faces meet at a 90° convex angle. The
        // pre-fix code returned the wrong sign on roughly half of the
        // edges because backward-oriented faces fed their parametric
        // (non-outward) normal into `robust_face_angle`.
        for edge_id in &edge_ids {
            let (face1_id, face2_id) = get_adjacent_faces(&model, solid_id, *edge_id)
                .unwrap_or_else(|e| panic!("adjacency for edge {edge_id}: {e:?}"));

            let angle = compute_face_angle(&model, *edge_id, face1_id, face2_id)
                .unwrap_or_else(|e| panic!("face angle for edge {edge_id}: {e:?}"));

            assert!(
                angle > 0.0,
                "edge {edge_id} (faces {face1_id} & {face2_id}): expected positive \
                 (convex) signed dihedral, got {angle}. This is the regression the \
                 face-oriented-normal fix prevents."
            );

            let pi_over_two = std::f64::consts::FRAC_PI_2;
            assert!(
                (angle - pi_over_two).abs() < 1e-6,
                "edge {edge_id} (faces {face1_id} & {face2_id}): expected π/2 = \
                 {pi_over_two}, got {angle} (delta = {})",
                (angle - pi_over_two).abs()
            );
        }
    }

    // ------------------------------------------------------------------
    // F5-β.1 (Task #88) — circle-circle cap intersection +
    // per-cylinder cap-arc lookup helpers.
    //
    // `intersect_two_caps` is a pure-math primitive — these tests pin
    // its discriminant, axis-parallel rejection, V-side
    // disambiguation, and mixed-radii correctness on hand-crafted
    // inputs whose intersection points are derivable in closed form.
    //
    // `find_cap_arc_edge_by_cylinder_axis` is exercised against a
    // hand-built BRep with one fillet face carrying one or two Arc
    // edges; the test confirms parallel-normal + axis-distance
    // gating and the "pick closer to apex" tie-break.
    // ------------------------------------------------------------------

    fn vec3(x: f64, y: f64, z: f64) -> Vector3 {
        Vector3::new(x, y, z)
    }

    fn pt(x: f64, y: f64, z: f64) -> Point3 {
        Point3::new(x, y, z)
    }

    fn approx_eq_pt(a: Point3, b: Point3, tol: f64) -> bool {
        (a.x - b.x).abs() <= tol && (a.y - b.y).abs() <= tol && (a.z - b.z).abs() <= tol
    }

    #[test]
    fn intersect_two_caps_equal_radii_perpendicular_picks_v_side() {
        // Two unit circles centred at origin, one in plane y=0 with
        // normal +y, one in plane x=0 with normal +x. They cross at
        // (0, 0, +1) and (0, 0, -1). vertex_outward = +z picks the
        // +z candidate.
        let p = intersect_two_caps(
            pt(0.0, 0.0, 0.0),
            vec3(0.0, 1.0, 0.0),
            1.0,
            pt(0.0, 0.0, 0.0),
            vec3(1.0, 0.0, 0.0),
            1.0,
            pt(0.0, 0.0, 2.0),     // vertex along +z
            vec3(0.0, 0.0, 1.0),   // outward +z
        )
        .expect("two unit caps must intersect");
        assert!(
            approx_eq_pt(p, pt(0.0, 0.0, 1.0), 1.0e-9),
            "expected (0,0,1) got {:?}",
            p
        );
    }

    #[test]
    fn intersect_two_caps_v_side_flip_picks_opposite_candidate() {
        // Same geometry as above, but vertex_outward = -z must pick
        // (0, 0, -1) instead of (0, 0, +1).
        let p = intersect_two_caps(
            pt(0.0, 0.0, 0.0),
            vec3(0.0, 1.0, 0.0),
            1.0,
            pt(0.0, 0.0, 0.0),
            vec3(1.0, 0.0, 0.0),
            1.0,
            pt(0.0, 0.0, -2.0),
            vec3(0.0, 0.0, -1.0),
        )
        .expect("two unit caps must intersect");
        assert!(
            approx_eq_pt(p, pt(0.0, 0.0, -1.0), 1.0e-9),
            "outward flip must select -z candidate; got {:?}",
            p
        );
    }

    #[test]
    fn intersect_two_caps_mixed_radii_known_intersection() {
        // Cap 0: centre (3, 0, 4), normal +y, radius 5 — passes
        //        through (0, 0, 0) and (0, 0, 8) on line L = {x=0, y=0}.
        // Cap 1: centre (0, 1, 4), normal +x, radius sqrt(17) — also
        //        passes through (0, 0, 0) and (0, 0, 8).
        // Radii are unmistakably mixed (5 ≠ sqrt(17) ≈ 4.123).
        // vertex at (0, 0, 10), outward = +z: max (P-V)·outward
        // selects (0, 0, 8) (score = -2) over (0, 0, 0) (score = -10).
        let r1 = (17.0_f64).sqrt();
        let p = intersect_two_caps(
            pt(3.0, 0.0, 4.0),
            vec3(0.0, 1.0, 0.0),
            5.0,
            pt(0.0, 1.0, 4.0),
            vec3(1.0, 0.0, 0.0),
            r1,
            pt(0.0, 0.0, 10.0),
            vec3(0.0, 0.0, 1.0),
        )
        .expect("mixed-radii caps intersect at (0,0,0) and (0,0,8)");
        assert!(
            approx_eq_pt(p, pt(0.0, 0.0, 8.0), 1.0e-9),
            "expected (0,0,8) (the +z candidate); got {:?}",
            p
        );
    }

    #[test]
    fn intersect_two_caps_axes_parallel_rejected() {
        // Same-direction axes ⇒ d_ij = u_i × u_j = 0 ⇒ AxesParallel.
        let err = intersect_two_caps(
            pt(0.0, 0.0, 0.0),
            vec3(0.0, 1.0, 0.0),
            1.0,
            pt(0.0, 0.0, 5.0),
            vec3(0.0, 1.0, 0.0),
            1.0,
            pt(0.0, 0.0, 10.0),
            vec3(0.0, 0.0, 1.0),
        )
        .expect_err("parallel cylinder axes must reject");
        assert_eq!(err, IntersectCapsError::AxesParallel);
    }

    #[test]
    fn intersect_two_caps_anti_parallel_axes_rejected() {
        // u_i and u_j antiparallel ⇒ cross product still zero ⇒
        // AxesParallel.
        let err = intersect_two_caps(
            pt(0.0, 0.0, 0.0),
            vec3(0.0, 1.0, 0.0),
            1.0,
            pt(0.0, 0.0, 5.0),
            vec3(0.0, -1.0, 0.0),
            1.0,
            pt(0.0, 0.0, 10.0),
            vec3(0.0, 0.0, 1.0),
        )
        .expect_err("antiparallel cylinder axes must reject");
        assert_eq!(err, IntersectCapsError::AxesParallel);
    }

    #[test]
    fn intersect_two_caps_no_intersection_when_caps_too_far() {
        // Two unit caps in perpendicular planes, but C_0 and C_1
        // separated along L_ij by ~10 — distance from P_0 to C_i on
        // the line is large, |P_0 − C_i|² − r_i² > b_half² so
        // discriminant negative.
        let err = intersect_two_caps(
            pt(0.0, 0.0, 0.0),
            vec3(0.0, 1.0, 0.0),
            1.0,
            pt(0.0, 0.0, 10.0),
            vec3(1.0, 0.0, 0.0),
            1.0,
            pt(0.0, 0.0, 5.0),
            vec3(0.0, 0.0, 1.0),
        )
        .expect_err("caps separated by ~10 with unit radii cannot meet");
        assert_eq!(err, IntersectCapsError::NoIntersection);
    }

    #[test]
    fn intersect_two_caps_sanity_filter_rejects_off_circle_candidate() {
        // Cap 0: centre (0,0,0) radius 1 in plane y=0 → meets line
        //        L_ij = {x=0, y=0} at z = ±1.
        // Cap 1: centre (0,0,2) radius 1 in plane x=0 → meets L_ij at
        //        z = 1 and z = 3.
        // The discriminant solve produces two candidates on the
        // y=0/x=0 line; only (0, 0, 1) lies on both circles. The
        // sanity check at step 5 must drop the (0, 0, -1) candidate
        // (off cap 1) so the V-side selection picks (0, 0, 1).
        let p = intersect_two_caps(
            pt(0.0, 0.0, 0.0),
            vec3(0.0, 1.0, 0.0),
            1.0,
            pt(0.0, 0.0, 2.0),
            vec3(1.0, 0.0, 0.0),
            1.0,
            pt(0.0, 0.0, 5.0),
            vec3(0.0, 0.0, 1.0),
        )
        .expect("caps share exactly one common point at (0,0,1)");
        assert!(
            approx_eq_pt(p, pt(0.0, 0.0, 1.0), 1.0e-7),
            "expected (0,0,1) after off-circle candidate filtered; got {:?}",
            p
        );
    }

    // -------- find_cap_arc_edge_by_cylinder_axis tests --------

    /// Build a minimal BRep face holding `arcs` as outer-loop edges.
    /// Surface is a placeholder Sphere — the lookup helper only reads
    /// the loop's curves, not the surface.
    fn build_face_with_arcs(model: &mut BRepModel, arcs: Vec<Arc>) -> FaceId {
        use crate::primitives::face::FaceOrientation;
        let surface_id = model
            .surfaces
            .add(Box::new(Sphere::new(pt(0.0, 0.0, 0.0), 1.0).expect("sphere")));
        let mut lp = Loop::new(0, LoopType::Outer);
        for arc in arcs {
            let v0 = model.vertices.add(0.0, 0.0, 0.0);
            let v1 = model.vertices.add(0.0, 0.0, 0.0);
            let curve_id = model.curves.add(Box::new(arc));
            let edge = Edge::new_auto_range(0, v0, v1, curve_id, EdgeOrientation::Forward);
            let edge_id = model.edges.add(edge);
            lp.add_edge(edge_id, true);
        }
        let loop_id = model.loops.add(lp);
        let mut face = Face::new(0, surface_id, loop_id, FaceOrientation::Forward);
        face.outer_loop = loop_id;
        model.faces.add(face)
    }

    #[test]
    fn find_cap_arc_edge_by_cylinder_axis_picks_unique_match() {
        let mut model = BRepModel::new();
        // Arc centred on cylinder axis (line x=0, z=0, varies y),
        // normal aligned with axis +y, radius 1.
        let arc = Arc::new(
            pt(0.0, 2.0, 0.0),
            vec3(0.0, 1.0, 0.0),
            1.0,
            0.0,
            std::f64::consts::FRAC_PI_2,
        )
        .expect("arc");
        let face_id = build_face_with_arcs(&mut model, vec![arc]);
        let found = find_cap_arc_edge_by_cylinder_axis(
            &model,
            face_id,
            pt(0.0, 0.0, 0.0),
            vec3(0.0, 1.0, 0.0),
            pt(0.0, 0.0, 0.0),
        );
        assert!(found.is_some(), "matching arc must be located");
    }

    #[test]
    fn find_cap_arc_edge_by_cylinder_axis_prefers_nearest_to_apex() {
        let mut model = BRepModel::new();
        // Two arcs both on cylinder axis (line x=0, z=0): one at
        // y=1 (V-side cap, nearer apex at origin), one at y=10
        // (far-side cap). The helper must return the y=1 one.
        let v_side = Arc::new(
            pt(0.0, 1.0, 0.0),
            vec3(0.0, 1.0, 0.0),
            1.0,
            0.0,
            std::f64::consts::FRAC_PI_2,
        )
        .expect("v-side arc");
        let far_side = Arc::new(
            pt(0.0, 10.0, 0.0),
            vec3(0.0, 1.0, 0.0),
            1.0,
            0.0,
            std::f64::consts::FRAC_PI_2,
        )
        .expect("far-side arc");
        let face_id = build_face_with_arcs(&mut model, vec![v_side, far_side]);

        let found_edge_id = find_cap_arc_edge_by_cylinder_axis(
            &model,
            face_id,
            pt(0.0, 0.0, 0.0),
            vec3(0.0, 1.0, 0.0),
            pt(0.0, 0.0, 0.0),
        )
        .expect("at least one arc must match");

        // Resolve the arc behind the returned edge and verify its
        // centre is the V-side one.
        let edge = model.edges.get(found_edge_id).expect("edge");
        let curve = model.curves.get(edge.curve_id).expect("curve");
        let arc = curve
            .as_any()
            .downcast_ref::<Arc>()
            .expect("matched edge must carry an Arc");
        assert!(
            (arc.center.y - 1.0).abs() < 1.0e-9,
            "expected V-side arc at y=1, got centre {:?}",
            arc.center
        );
    }

    #[test]
    fn find_cap_arc_edge_by_cylinder_axis_rejects_off_axis_centre() {
        let mut model = BRepModel::new();
        // Arc centred at (5, 0, 0) with normal +y — normal is OK
        // (parallel to axis) but centre is 5 units off the line
        // x=0, z=0. Helper must reject.
        let arc = Arc::new(
            pt(5.0, 0.0, 0.0),
            vec3(0.0, 1.0, 0.0),
            1.0,
            0.0,
            std::f64::consts::FRAC_PI_2,
        )
        .expect("arc");
        let face_id = build_face_with_arcs(&mut model, vec![arc]);
        let found = find_cap_arc_edge_by_cylinder_axis(
            &model,
            face_id,
            pt(0.0, 0.0, 0.0),
            vec3(0.0, 1.0, 0.0),
            pt(0.0, 0.0, 0.0),
        );
        assert!(
            found.is_none(),
            "arc centred 5 units off axis must not match"
        );
    }

    #[test]
    fn find_cap_arc_edge_by_cylinder_axis_rejects_perpendicular_normal() {
        let mut model = BRepModel::new();
        // Arc on the cylinder axis line but with normal perpendicular
        // to the cylinder axis — wrong cap orientation.
        let arc = Arc::new(
            pt(0.0, 0.0, 0.0),
            vec3(1.0, 0.0, 0.0),
            1.0,
            0.0,
            std::f64::consts::FRAC_PI_2,
        )
        .expect("arc");
        let face_id = build_face_with_arcs(&mut model, vec![arc]);
        let found = find_cap_arc_edge_by_cylinder_axis(
            &model,
            face_id,
            pt(0.0, 0.0, 0.0),
            vec3(0.0, 1.0, 0.0),
            pt(0.0, 0.0, 0.0),
        );
        assert!(
            found.is_none(),
            "arc normal perpendicular to axis must not match"
        );
    }

    // ------------------------------------------------------------------
    // F5-β.2 (Task #89) — cap-arc trimming + arc → rational-quadratic
    // Bezier control-net helpers.
    //
    // `trim_cap_arc_in_place` is exercised on a single-edge BRep that
    // carries one Arc curve. Each test trims the arc to a new sub-
    // range, then reads the result back from `model.edges` /
    // `model.curves` and asserts: new endpoints land at the requested
    // points, sweep direction obeys the V-side disambiguation rule,
    // and the arc's stored `x_axis` frame is preserved (Arc::new
    // would canonicalise it; direct struct construction must not).
    //
    // `arc_to_rational_quadratic_controls` is a pure-math primitive.
    // Tests pin the closed-form Piegl-Tiller §7.5 eq. 7.31 control
    // net for the quarter-arc, endpoint exactness, and the precondition
    // gate at the ±π singularity.
    // ------------------------------------------------------------------

    /// Build a minimal BRep edge carrying `arc` as its curve. Returns
    /// the new edge id. Vertices are placed at the arc's evaluated
    /// start / end positions so the edge is geometrically consistent
    /// from the outset.
    fn build_single_arc_edge(model: &mut BRepModel, arc: Arc) -> EdgeId {
        let start_pt = arc
            .evaluate(0.0)
            .expect("arc start evaluable")
            .position;
        let end_pt = arc
            .evaluate(1.0)
            .expect("arc end evaluable")
            .position;
        let v0 = model.vertices.add(start_pt.x, start_pt.y, start_pt.z);
        let v1 = model.vertices.add(end_pt.x, end_pt.y, end_pt.z);
        let curve_id = model.curves.add(Box::new(arc));
        let edge = Edge::new_auto_range(0, v0, v1, curve_id, EdgeOrientation::Forward);
        model.edges.add(edge)
    }

    #[test]
    fn trim_cap_arc_in_place_quarter_to_eighth_short_way() {
        // Quarter arc on the +z normal plane centred at origin,
        // sweep 0 → π/2 (i.e. (1,0,0) → (0,1,0)). Trim to (1,0,0) →
        // (cos π/4, sin π/4, 0). vertex at (1, 1, 0) outward (1,1,0)/√2
        // — midpoint of the short-way sub-arc sits at angle π/8, well
        // inside the +x/+y quadrant, so the V-side score is positive
        // and the short sweep is kept.
        let arc = Arc::new(
            pt(0.0, 0.0, 0.0),
            vec3(0.0, 0.0, 1.0),
            1.0,
            0.0,
            std::f64::consts::FRAC_PI_2,
        )
        .expect("arc");
        let mut model = BRepModel::new();
        let edge_id = build_single_arc_edge(&mut model, arc);

        let new_start = pt(1.0, 0.0, 0.0);
        let new_end = pt(
            std::f64::consts::FRAC_1_SQRT_2,
            std::f64::consts::FRAC_1_SQRT_2,
            0.0,
        );
        trim_cap_arc_in_place(
            &mut model,
            edge_id,
            new_start,
            new_end,
            pt(1.0, 1.0, 0.0),
            vec3(std::f64::consts::FRAC_1_SQRT_2, std::f64::consts::FRAC_1_SQRT_2, 0.0),
            1.0e-9,
        )
        .expect("trim succeeds");

        let edge = model.edges.get(edge_id).expect("edge");
        let trimmed = model
            .curves
            .get(edge.curve_id)
            .expect("curve")
            .as_any()
            .downcast_ref::<Arc>()
            .expect("Arc")
            .clone();
        assert!(
            (trimmed.start_angle - 0.0).abs() < 1.0e-12,
            "start_angle expected 0, got {}",
            trimmed.start_angle
        );
        assert!(
            (trimmed.sweep_angle - std::f64::consts::FRAC_PI_4).abs() < 1.0e-12,
            "sweep_angle expected π/4 (short way), got {}",
            trimmed.sweep_angle
        );
    }

    #[test]
    fn trim_cap_arc_in_place_v_side_flip_picks_long_way() {
        // Same quarter-arc as above, requested same endpoints, but
        // vertex_outward points into −x − y so the +x/+y quadrant
        // midpoint scores negative — algorithm must flip to the
        // 2π − π/4 = 7π/4 sweep (long way around, signed negative).
        let arc = Arc::new(
            pt(0.0, 0.0, 0.0),
            vec3(0.0, 0.0, 1.0),
            1.0,
            0.0,
            std::f64::consts::FRAC_PI_2,
        )
        .expect("arc");
        let mut model = BRepModel::new();
        let edge_id = build_single_arc_edge(&mut model, arc);

        let new_start = pt(1.0, 0.0, 0.0);
        let new_end = pt(
            std::f64::consts::FRAC_1_SQRT_2,
            std::f64::consts::FRAC_1_SQRT_2,
            0.0,
        );
        trim_cap_arc_in_place(
            &mut model,
            edge_id,
            new_start,
            new_end,
            pt(-1.0, -1.0, 0.0),
            vec3(
                -std::f64::consts::FRAC_1_SQRT_2,
                -std::f64::consts::FRAC_1_SQRT_2,
                0.0,
            ),
            1.0e-9,
        )
        .expect("trim succeeds");

        let edge = model.edges.get(edge_id).expect("edge");
        let trimmed = model
            .curves
            .get(edge.curve_id)
            .expect("curve")
            .as_any()
            .downcast_ref::<Arc>()
            .expect("Arc")
            .clone();
        // Long way: sweep should be π/4 − 2π = −7π/4.
        let expected = std::f64::consts::FRAC_PI_4 - std::f64::consts::TAU;
        assert!(
            (trimmed.sweep_angle - expected).abs() < 1.0e-12,
            "expected long-way sweep {}, got {}",
            expected,
            trimmed.sweep_angle
        );
    }

    #[test]
    fn trim_cap_arc_in_place_preserves_frame_after_replace() {
        // Build an arc with a deliberately non-canonical x_axis (rotate
        // the default +X by π/8). Arc::new canonicalises x_axis to +X
        // for a +Z normal, so we hand-build the Arc via struct literal
        // here to make the test meaningful — the helper must NOT call
        // Arc::new during replace.
        let half = std::f64::consts::FRAC_PI_4 / 2.0; // π/8
        let (sin_h, cos_h) = half.sin_cos();
        let custom_x = vec3(cos_h, sin_h, 0.0);
        let original = Arc {
            center: pt(0.0, 0.0, 0.0),
            normal: vec3(0.0, 0.0, 1.0),
            x_axis: custom_x,
            radius: 1.0,
            start_angle: 0.0,
            sweep_angle: std::f64::consts::FRAC_PI_2,
            range: crate::primitives::curve::ParameterRange::unit(),
        };

        let mut model = BRepModel::new();
        // We cannot use build_single_arc_edge because that path
        // evaluates the arc directly and uses VertexStore::add — which
        // is fine, but we hand-build here for explicit control over
        // the start / end positions.
        let start = original
            .evaluate(0.0)
            .expect("start eval")
            .position;
        let end = original
            .evaluate(1.0)
            .expect("end eval")
            .position;
        let v0 = model.vertices.add(start.x, start.y, start.z);
        let v1 = model.vertices.add(end.x, end.y, end.z);
        let curve_id = model.curves.add(Box::new(original));
        let edge = Edge::new_auto_range(0, v0, v1, curve_id, EdgeOrientation::Forward);
        let edge_id = model.edges.add(edge);

        trim_cap_arc_in_place(
            &mut model,
            edge_id,
            start,
            end,
            pt(1.0, 1.0, 0.0),
            vec3(std::f64::consts::FRAC_1_SQRT_2, std::f64::consts::FRAC_1_SQRT_2, 0.0),
            1.0e-9,
        )
        .expect("trim succeeds (same endpoints)");

        let edge = model.edges.get(edge_id).expect("edge");
        let trimmed = model
            .curves
            .get(edge.curve_id)
            .expect("curve")
            .as_any()
            .downcast_ref::<Arc>()
            .expect("Arc")
            .clone();
        // The custom x_axis must survive — Arc::new would have
        // canonicalised it to +X for a +Z normal.
        assert!(
            (trimmed.x_axis.x - custom_x.x).abs() < 1.0e-12
                && (trimmed.x_axis.y - custom_x.y).abs() < 1.0e-12,
            "x_axis frame mutated: was {:?}, got {:?}",
            custom_x,
            trimmed.x_axis
        );
    }

    #[test]
    fn trim_cap_arc_in_place_rejects_off_circle_endpoint() {
        // Endpoint half a unit off the unit circle → must trip the
        // tolerance gate inside trim_cap_arc_in_place.
        let arc = Arc::new(
            pt(0.0, 0.0, 0.0),
            vec3(0.0, 0.0, 1.0),
            1.0,
            0.0,
            std::f64::consts::FRAC_PI_2,
        )
        .expect("arc");
        let mut model = BRepModel::new();
        let edge_id = build_single_arc_edge(&mut model, arc);

        let result = trim_cap_arc_in_place(
            &mut model,
            edge_id,
            pt(1.0, 0.0, 0.0),
            pt(0.5, 0.5, 0.0), // |·| = √0.5 ≠ 1, off the circle
            pt(1.0, 1.0, 0.0),
            vec3(std::f64::consts::FRAC_1_SQRT_2, std::f64::consts::FRAC_1_SQRT_2, 0.0),
            1.0e-9,
        );
        match result {
            Err(OperationError::InvalidGeometry(msg)) => {
                assert!(
                    msg.contains("deviates from cap circle"),
                    "expected diagnostic about off-circle endpoint, got: {}",
                    msg
                );
            }
            other => panic!(
                "expected InvalidGeometry for off-circle endpoint, got {:?}",
                other
            ),
        }
    }

    #[test]
    fn arc_to_rational_quadratic_controls_endpoints_lie_on_arc() {
        // Quarter arc (0 → π/2) on unit circle: control net P_0, P_1, P_2
        // — endpoints must coincide with the arc's geometric endpoints.
        let arc = Arc::new(
            pt(0.0, 0.0, 0.0),
            vec3(0.0, 0.0, 1.0),
            1.0,
            0.0,
            std::f64::consts::FRAC_PI_2,
        )
        .expect("arc");
        let (pts, ws) = arc_to_rational_quadratic_controls(&arc).expect("controls");
        assert!(approx_eq_pt(pts[0], pt(1.0, 0.0, 0.0), 1.0e-12));
        assert!(approx_eq_pt(pts[2], pt(0.0, 1.0, 0.0), 1.0e-12));
        // Weights for quarter-arc: cos(π/4) = √2/2.
        assert!((ws[0] - 1.0).abs() < 1.0e-12);
        assert!((ws[1] - std::f64::consts::FRAC_1_SQRT_2).abs() < 1.0e-12);
        assert!((ws[2] - 1.0).abs() < 1.0e-12);
    }

    #[test]
    fn arc_to_rational_quadratic_controls_quarter_arc_midpoint_reproduces_circle() {
        // The rational-quadratic Bezier evaluated at u = 0.5 must land
        // on the geometric mid-arc point (cos π/4, sin π/4, 0). Eval
        // formula: R(u) = Σ w_i B_i^2(u) P_i / Σ w_i B_i^2(u), with
        // Bernstein basis B_0^2 = (1-u)², B_1^2 = 2u(1-u), B_2^2 = u².
        let arc = Arc::new(
            pt(0.0, 0.0, 0.0),
            vec3(0.0, 0.0, 1.0),
            1.0,
            0.0,
            std::f64::consts::FRAC_PI_2,
        )
        .expect("arc");
        let (pts, ws) = arc_to_rational_quadratic_controls(&arc).expect("controls");

        let u = 0.5;
        let b = [(1.0 - u) * (1.0 - u), 2.0 * u * (1.0 - u), u * u];
        let denom = ws[0] * b[0] + ws[1] * b[1] + ws[2] * b[2];
        let num_x =
            ws[0] * b[0] * pts[0].x + ws[1] * b[1] * pts[1].x + ws[2] * b[2] * pts[2].x;
        let num_y =
            ws[0] * b[0] * pts[0].y + ws[1] * b[1] * pts[1].y + ws[2] * b[2] * pts[2].y;
        let num_z =
            ws[0] * b[0] * pts[0].z + ws[1] * b[1] * pts[1].z + ws[2] * b[2] * pts[2].z;
        let mid = pt(num_x / denom, num_y / denom, num_z / denom);
        let expected = pt(
            std::f64::consts::FRAC_1_SQRT_2,
            std::f64::consts::FRAC_1_SQRT_2,
            0.0,
        );
        assert!(
            approx_eq_pt(mid, expected, 1.0e-12),
            "rational-quadratic midpoint {:?} ≠ analytical {:?}",
            mid,
            expected
        );
    }

    #[test]
    fn arc_to_rational_quadratic_controls_rejects_half_circle_or_more() {
        // Sweep = π (half-circle) hits the weight-zero singularity.
        // A single rational-quadratic segment cannot represent it
        // (T_mid → ∞); helper must return InvalidGeometry, not silently
        // emit a control net with a degenerate weight or NaN point.
        let arc = Arc::new(
            pt(0.0, 0.0, 0.0),
            vec3(0.0, 0.0, 1.0),
            1.0,
            0.0,
            std::f64::consts::PI,
        )
        .expect("arc");
        let result = arc_to_rational_quadratic_controls(&arc);
        match result {
            Err(OperationError::InvalidGeometry(msg)) => {
                assert!(
                    msg.contains("rational-quadratic")
                        || msg.contains("π")
                        || msg.contains("PI")
                        || msg.contains("sweep"),
                    "diagnostic should reference sweep / π / rational-quadratic, got: {}",
                    msg
                );
            }
            other => panic!(
                "expected InvalidGeometry for half-circle sweep, got {:?}",
                other
            ),
        }
    }

    // -------------------------------------------------------------------
    // F5-β.5.1 — FilletType::PerEdgeConstant
    //
    // Coverage:
    //   - Clone / Debug invariants on the new variant.
    //   - `validate_fillet_inputs` accepts a fully-covered, finite-and-
    //     positive radius map and rejects every structural / numeric
    //     mismatch (empty, missing entry, extra entry, non-finite,
    //     zero, sub-tolerance).
    //   - `fillet_type_to_blend_radius` looks up by edge id.
    //   - Eight proptests stress the validator over randomised maps,
    //     mutations, and permutations.
    // -------------------------------------------------------------------

    /// Helper: build a model with `n` distinct line edges and return
    /// the solid id plus the list of edge ids. Each edge sits on its
    /// own pair of unit-displaced vertices so half-edge-length bounds
    /// always permit a radius of 0.1.
    fn build_n_edge_model(n: usize) -> (BRepModel, SolidId, Vec<EdgeId>) {
        let mut model = BRepModel::new();
        let solid_id = build_unit_box(&mut model);
        let mut edges = Vec::with_capacity(n);
        for i in 0..n {
            let from = Point3::new(i as f64 * 10.0, 0.0, 0.0);
            let to = Point3::new(i as f64 * 10.0 + 4.0, 0.0, 0.0);
            let (eid, _, _) = add_simple_edge(&mut model, from, to);
            edges.push(eid);
        }
        (model, solid_id, edges)
    }

    fn per_edge_map(entries: &[(EdgeId, f64)]) -> HashMap<EdgeId, f64> {
        entries.iter().copied().collect()
    }

    fn opts_with_per_edge(map: HashMap<EdgeId, f64>) -> FilletOptions {
        FilletOptions {
            radius: 1.0,
            fillet_type: FilletType::PerEdgeConstant(map),
            ..Default::default()
        }
    }

    #[test]
    fn per_edge_constant_clone_preserves_map() {
        let mut m = HashMap::new();
        m.insert(7_u32 as EdgeId, 1.25);
        m.insert(11_u32 as EdgeId, 2.5);
        let ft = FilletType::PerEdgeConstant(m.clone());
        let cloned = ft.clone();
        match cloned {
            FilletType::PerEdgeConstant(c) => {
                assert_eq!(c.len(), m.len());
                for (k, v) in m.iter() {
                    assert!((c.get(k).copied().unwrap_or(0.0) - v).abs() < 1e-12);
                }
            }
            other => panic!("expected PerEdgeConstant, got {other:?}"),
        }
    }

    #[test]
    fn per_edge_constant_debug_includes_entries_and_label() {
        let mut m = HashMap::new();
        m.insert(3_u32 as EdgeId, 0.75);
        let ft = FilletType::PerEdgeConstant(m);
        let s = format!("{:?}", ft);
        assert!(s.contains("PerEdgeConstant"), "missing label: {s}");
        assert!(s.contains("3"), "missing edge id: {s}");
        assert!(s.contains("0.75"), "missing radius: {s}");
    }

    #[test]
    fn per_edge_constant_debug_is_deterministic_across_insertion_order() {
        // HashMap iteration order is randomised; the Debug impl
        // must sort by edge id so timeline replay diffs are
        // stable. We build two maps with the same logical entries
        // inserted in opposite order and assert their Debug strings
        // match exactly.
        let mut a = HashMap::new();
        a.insert(2_u32 as EdgeId, 1.0);
        a.insert(5_u32 as EdgeId, 2.0);
        a.insert(11_u32 as EdgeId, 3.0);

        let mut b = HashMap::new();
        b.insert(11_u32 as EdgeId, 3.0);
        b.insert(2_u32 as EdgeId, 1.0);
        b.insert(5_u32 as EdgeId, 2.0);

        assert_eq!(
            format!("{:?}", FilletType::PerEdgeConstant(a)),
            format!("{:?}", FilletType::PerEdgeConstant(b)),
        );
    }

    #[test]
    fn validate_fillet_inputs_per_edge_constant_accepts_full_coverage() {
        let (model, solid_id, edges) = build_n_edge_model(3);
        let map = per_edge_map(&[(edges[0], 0.5), (edges[1], 0.75), (edges[2], 1.0)]);
        let opts = opts_with_per_edge(map);
        assert!(validate_fillet_inputs(&model, solid_id, &edges, &opts).is_ok());
    }

    #[test]
    fn validate_fillet_inputs_per_edge_constant_rejects_empty_map() {
        let (model, solid_id, edges) = build_n_edge_model(1);
        let opts = opts_with_per_edge(HashMap::new());
        match validate_fillet_inputs(&model, solid_id, &edges, &opts) {
            Err(OperationError::InvalidInput { parameter, .. }) => {
                assert!(parameter.contains("PerEdgeConstant"), "{parameter}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn validate_fillet_inputs_per_edge_constant_rejects_missing_edge() {
        let (model, solid_id, edges) = build_n_edge_model(3);
        // Two edges in the selection, only one in the map.
        let map = per_edge_map(&[(edges[0], 1.0)]);
        let opts = opts_with_per_edge(map);
        let result = validate_fillet_inputs(&model, solid_id, &edges[..2], &opts);
        match result {
            Err(OperationError::InvalidInput { received, .. }) => {
                assert!(received.contains("no radius entry"), "{received}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn validate_fillet_inputs_per_edge_constant_rejects_extra_map_entry() {
        let (model, solid_id, edges) = build_n_edge_model(3);
        // Selection has 1 edge; map has 2 — one extra beyond the
        // selection. Catches stale UI dictionary entries.
        let map = per_edge_map(&[(edges[0], 1.0), (edges[1], 0.5)]);
        let opts = opts_with_per_edge(map);
        let result = validate_fillet_inputs(&model, solid_id, &edges[..1], &opts);
        match result {
            Err(OperationError::InvalidInput { received, .. }) => {
                assert!(
                    received.contains("not in the selection"),
                    "{received}"
                );
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn validate_fillet_inputs_per_edge_constant_rejects_non_finite_radius() {
        let (model, solid_id, edges) = build_n_edge_model(2);
        let map = per_edge_map(&[(edges[0], 1.0), (edges[1], f64::NAN)]);
        let opts = opts_with_per_edge(map);
        match validate_fillet_inputs(&model, solid_id, &edges, &opts) {
            Err(OperationError::InvalidInput { parameter, .. }) => {
                assert!(parameter.contains("PerEdgeConstant"), "{parameter}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }

        let map = per_edge_map(&[(edges[0], 1.0), (edges[1], f64::INFINITY)]);
        let opts = opts_with_per_edge(map);
        match validate_fillet_inputs(&model, solid_id, &edges, &opts) {
            Err(OperationError::InvalidInput { .. }) => {}
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn validate_fillet_inputs_per_edge_constant_rejects_zero_or_negative_radius() {
        let (model, solid_id, edges) = build_n_edge_model(2);

        let map = per_edge_map(&[(edges[0], 1.0), (edges[1], 0.0)]);
        let opts = opts_with_per_edge(map);
        assert!(matches!(
            validate_fillet_inputs(&model, solid_id, &edges, &opts),
            Err(OperationError::InvalidInput { .. })
        ));

        let map = per_edge_map(&[(edges[0], 1.0), (edges[1], -0.5)]);
        let opts = opts_with_per_edge(map);
        assert!(matches!(
            validate_fillet_inputs(&model, solid_id, &edges, &opts),
            Err(OperationError::InvalidInput { .. })
        ));
    }

    #[test]
    fn validate_fillet_inputs_per_edge_constant_rejects_sub_tolerance_radius() {
        let (model, solid_id, edges) = build_n_edge_model(1);
        let tol = Tolerance::default().distance();
        // Half of the default tolerance is definitely below the gate
        // and well above the 0.0 short-circuit, exercising the
        // `r <= tol` arm specifically.
        let r_sub = tol * 0.5;
        let map = per_edge_map(&[(edges[0], r_sub)]);
        let opts = opts_with_per_edge(map);
        match validate_fillet_inputs(&model, solid_id, &edges, &opts) {
            Err(OperationError::InvalidInput { parameter, .. }) => {
                assert!(parameter.contains("PerEdgeConstant"), "{parameter}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn fillet_type_to_blend_radius_per_edge_constant_returns_edge_value() {
        let mut m = HashMap::new();
        m.insert(7_u32 as EdgeId, 2.0);
        m.insert(13_u32 as EdgeId, 3.5);
        let ft = FilletType::PerEdgeConstant(m);
        match fillet_type_to_blend_radius(&ft, 13) {
            BlendRadius::Constant(r) => assert!((r - 3.5).abs() < 1e-12),
            other => panic!("expected BlendRadius::Constant(3.5), got {other:?}"),
        }
        match fillet_type_to_blend_radius(&ft, 7) {
            BlendRadius::Constant(r) => assert!((r - 2.0).abs() < 1e-12),
            other => panic!("expected BlendRadius::Constant(2.0), got {other:?}"),
        }
    }

    #[test]
    fn fillet_type_to_blend_radius_per_edge_constant_missing_key_falls_back_to_zero() {
        // Defensive fallback only — by the time this is called,
        // `validate_fillet_inputs` has already enforced coverage.
        // The fallback exists so the kernel never panics on a logic
        // violation and the downstream `InvalidRadius(0.0)` gate
        // surfaces a typed error instead.
        let mut m = HashMap::new();
        m.insert(7_u32 as EdgeId, 2.0);
        let ft = FilletType::PerEdgeConstant(m);
        match fillet_type_to_blend_radius(&ft, 99) {
            BlendRadius::Constant(r) => assert!(r == 0.0),
            other => panic!("expected BlendRadius::Constant(0.0), got {other:?}"),
        }
    }

    // -------------------------------------------------------------------
    // F5-β.5.6 — FilletType::PerEdgeProfile (mixed-kind per-edge radii)
    //
    // Coverage:
    //   - Clone / Debug invariants on the mixed-kind variant
    //     (sorted-by-edge-id Debug for replay determinism).
    //   - `validate_fillet_inputs` accepts mixed-kind maps and
    //     rejects every structural / per-profile failure.
    //   - `fillet_type_to_blend_radius` returns the exact profile
    //     shape for each edge.
    // -------------------------------------------------------------------

    fn per_edge_profile_map(
        entries: &[(EdgeId, EdgeFilletProfile)],
    ) -> HashMap<EdgeId, EdgeFilletProfile> {
        entries.iter().cloned().collect()
    }

    fn opts_with_per_edge_profile(map: HashMap<EdgeId, EdgeFilletProfile>) -> FilletOptions {
        FilletOptions {
            radius: 1.0,
            fillet_type: FilletType::PerEdgeProfile(map),
            ..Default::default()
        }
    }

    #[test]
    fn per_edge_profile_debug_is_sorted() {
        // Insert in shuffled order; the Debug output must list
        // entries in ascending edge-id order so replay diffs of
        // recorded-operation JSON are stable.
        let mut a = HashMap::new();
        a.insert(11_u32 as EdgeId, EdgeFilletProfile::Radius(BlendRadius::Constant(3.0)));
        a.insert(2_u32 as EdgeId, EdgeFilletProfile::Radius(BlendRadius::Constant(1.0)));
        a.insert(
            5_u32 as EdgeId,
            EdgeFilletProfile::Radius(BlendRadius::Linear {
                start: 0.5,
                end: 2.0,
            }),
        );

        let mut b = HashMap::new();
        b.insert(
            5_u32 as EdgeId,
            EdgeFilletProfile::Radius(BlendRadius::Linear { start: 0.5, end: 2.0 }),
        );
        b.insert(2_u32 as EdgeId, EdgeFilletProfile::Radius(BlendRadius::Constant(1.0)));
        b.insert(11_u32 as EdgeId, EdgeFilletProfile::Radius(BlendRadius::Constant(3.0)));

        assert_eq!(
            format!("{:?}", FilletType::PerEdgeProfile(a)),
            format!("{:?}", FilletType::PerEdgeProfile(b)),
        );
    }

    #[test]
    fn per_edge_profile_clone_preserves_map() {
        let mut m = HashMap::new();
        m.insert(
            7_u32 as EdgeId,
            EdgeFilletProfile::Radius(BlendRadius::Constant(1.25)),
        );
        m.insert(
            11_u32 as EdgeId,
            EdgeFilletProfile::Radius(BlendRadius::Variable(vec![(0.0, 0.5), (1.0, 1.5)])),
        );
        let ft = FilletType::PerEdgeProfile(m.clone());
        let cloned = ft.clone();
        match cloned {
            FilletType::PerEdgeProfile(c) => {
                assert_eq!(c.len(), m.len());
                assert_eq!(
                    c.get(&7),
                    Some(&EdgeFilletProfile::Radius(BlendRadius::Constant(1.25)))
                );
                assert_eq!(
                    c.get(&11),
                    Some(&EdgeFilletProfile::Radius(BlendRadius::Variable(vec![
                        (0.0, 0.5),
                        (1.0, 1.5)
                    ])))
                );
            }
            other => panic!("expected PerEdgeProfile, got {other:?}"),
        }
    }

    #[test]
    fn validate_per_edge_profile_accepts_mixed_kinds() {
        let (model, solid_id, edges) = build_n_edge_model(3);
        let map = per_edge_profile_map(&[
            (edges[0], EdgeFilletProfile::Radius(BlendRadius::Constant(0.5))),
            (
                edges[1],
                EdgeFilletProfile::Radius(BlendRadius::Linear {
                    start: 0.3,
                    end: 0.8,
                }),
            ),
            (
                edges[2],
                EdgeFilletProfile::Radius(BlendRadius::Variable(vec![
                    (0.0, 0.4),
                    (0.5, 0.6),
                    (1.0, 0.4),
                ])),
            ),
        ]);
        let opts = opts_with_per_edge_profile(map);
        assert!(validate_fillet_inputs(&model, solid_id, &edges, &opts).is_ok());
    }

    #[test]
    fn validate_per_edge_profile_rejects_empty_map() {
        let (model, solid_id, edges) = build_n_edge_model(1);
        let opts = opts_with_per_edge_profile(HashMap::new());
        match validate_fillet_inputs(&model, solid_id, &edges, &opts) {
            Err(OperationError::InvalidInput { parameter, .. }) => {
                assert!(parameter.contains("PerEdgeProfile"), "{parameter}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn validate_per_edge_profile_rejects_missing_edge() {
        let (model, solid_id, edges) = build_n_edge_model(3);
        // Three edges selected, only two in the map.
        let map = per_edge_profile_map(&[
            (edges[0], EdgeFilletProfile::Radius(BlendRadius::Constant(0.5))),
            (edges[1], EdgeFilletProfile::Radius(BlendRadius::Constant(0.75))),
        ]);
        let opts = opts_with_per_edge_profile(map);
        match validate_fillet_inputs(&model, solid_id, &edges, &opts) {
            Err(OperationError::InvalidInput { received, .. }) => {
                assert!(received.contains("no profile entry"), "{received}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn validate_per_edge_profile_rejects_extra_entry() {
        let (model, solid_id, edges) = build_n_edge_model(3);
        let map = per_edge_profile_map(&[
            (edges[0], EdgeFilletProfile::Radius(BlendRadius::Constant(0.5))),
            (edges[1], EdgeFilletProfile::Radius(BlendRadius::Constant(0.75))),
        ]);
        let opts = opts_with_per_edge_profile(map);
        match validate_fillet_inputs(&model, solid_id, &edges[..1], &opts) {
            Err(OperationError::InvalidInput { received, .. }) => {
                assert!(received.contains("not in the selection"), "{received}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn validate_per_edge_profile_rejects_negative_constant() {
        let (model, solid_id, edges) = build_n_edge_model(2);
        let map = per_edge_profile_map(&[
            (edges[0], EdgeFilletProfile::Radius(BlendRadius::Constant(1.0))),
            (edges[1], EdgeFilletProfile::Radius(BlendRadius::Constant(-0.5))),
        ]);
        let opts = opts_with_per_edge_profile(map);
        match validate_fillet_inputs(&model, solid_id, &edges, &opts) {
            Err(OperationError::InvalidInput { parameter, .. }) => {
                assert!(parameter.contains("PerEdgeProfile"), "{parameter}");
                assert!(parameter.contains("Constant"), "{parameter}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn validate_per_edge_profile_rejects_zero_linear_endpoint() {
        let (model, solid_id, edges) = build_n_edge_model(2);
        let map = per_edge_profile_map(&[
            (edges[0], EdgeFilletProfile::Radius(BlendRadius::Constant(1.0))),
            (
                edges[1],
                EdgeFilletProfile::Radius(BlendRadius::Linear {
                    start: 0.0,
                    end: 1.0,
                }),
            ),
        ]);
        let opts = opts_with_per_edge_profile(map);
        match validate_fillet_inputs(&model, solid_id, &edges, &opts) {
            Err(OperationError::InvalidInput { parameter, .. }) => {
                assert!(parameter.contains("PerEdgeProfile"), "{parameter}");
                assert!(parameter.contains("Linear"), "{parameter}");
                assert!(parameter.contains("start"), "{parameter}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn validate_per_edge_profile_rejects_non_monotone_variable_stations() {
        let (model, solid_id, edges) = build_n_edge_model(1);
        // Stations [0.0, 0.5, 0.3] — third station regresses.
        let map = per_edge_profile_map(&[(
            edges[0],
            EdgeFilletProfile::Radius(BlendRadius::Variable(vec![
                (0.0, 0.5),
                (0.5, 0.7),
                (0.3, 0.6),
            ])),
        )]);
        let opts = opts_with_per_edge_profile(map);
        match validate_fillet_inputs(&model, solid_id, &edges, &opts) {
            Err(OperationError::InvalidInput { parameter, .. }) => {
                assert!(parameter.contains("PerEdgeProfile"), "{parameter}");
                assert!(parameter.contains("Variable"), "{parameter}");
                assert!(parameter.contains("station"), "{parameter}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn fillet_type_to_blend_radius_per_edge_profile_picks_entry() {
        let mut m = HashMap::new();
        m.insert(
            7_u32 as EdgeId,
            EdgeFilletProfile::Radius(BlendRadius::Constant(2.0)),
        );
        m.insert(
            13_u32 as EdgeId,
            EdgeFilletProfile::Radius(BlendRadius::Linear {
                start: 0.5,
                end: 1.5,
            }),
        );
        m.insert(
            19_u32 as EdgeId,
            EdgeFilletProfile::Radius(BlendRadius::Variable(vec![(0.0, 0.3), (1.0, 0.8)])),
        );
        let ft = FilletType::PerEdgeProfile(m);
        assert_eq!(
            fillet_type_to_blend_radius(&ft, 7),
            BlendRadius::Constant(2.0)
        );
        assert_eq!(
            fillet_type_to_blend_radius(&ft, 13),
            BlendRadius::Linear {
                start: 0.5,
                end: 1.5,
            }
        );
        assert_eq!(
            fillet_type_to_blend_radius(&ft, 19),
            BlendRadius::Variable(vec![(0.0, 0.3), (1.0, 0.8)])
        );
    }

    #[test]
    fn fillet_type_to_blend_radius_per_edge_profile_missing_key_falls_back_to_zero() {
        // Defensive fallback only — by the time this is called,
        // `validate_fillet_inputs` has already enforced coverage.
        let mut m = HashMap::new();
        m.insert(
            7_u32 as EdgeId,
            EdgeFilletProfile::Radius(BlendRadius::Constant(2.0)),
        );
        let ft = FilletType::PerEdgeProfile(m);
        assert_eq!(
            fillet_type_to_blend_radius(&ft, 99),
            BlendRadius::Constant(0.0)
        );
    }

    // -------------------------------------------------------------------
    // F5-β.5.7 — chord-in-per-edge-profile coverage
    //
    // Each new test pins one invariant of the new
    // `EdgeFilletProfile::Chord(_)` variant inside the
    // `FilletType::PerEdgeProfile` map shape:
    //   - Clone / Debug behave the same way as for radius entries.
    //   - `validate_fillet_inputs` rejects non-positive / non-finite
    //     chords with a parameter name that includes `Chord`.
    //   - A chord-and-radius mix is accepted (the variant is
    //     designed exactly for this).
    //   - `fillet_type_to_blend_radius` reports `Constant(c)` for
    //     a Chord entry (placeholder for the graph; the surgery
    //     time conversion uses the local dihedral).
    //   - `EdgeFilletProfile::max_radius_bound` / `min_radius_bound`
    //     report 0.0 / c/2 for Chord and delegate to the inner
    //     `BlendRadius` for `Radius(_)`.
    // -------------------------------------------------------------------

    #[test]
    fn per_edge_profile_chord_clone_is_deep() {
        let mut m = HashMap::new();
        m.insert(3_u32 as EdgeId, EdgeFilletProfile::Chord(0.5));
        m.insert(
            5_u32 as EdgeId,
            EdgeFilletProfile::Radius(BlendRadius::Constant(0.25)),
        );
        let ft = FilletType::PerEdgeProfile(m.clone());
        let cloned = ft.clone();
        if let FilletType::PerEdgeProfile(c) = cloned {
            assert_eq!(c, m);
        } else {
            panic!("expected PerEdgeProfile");
        }
    }

    #[test]
    fn per_edge_profile_chord_debug_is_sorted() {
        let mut m = HashMap::new();
        m.insert(11_u32 as EdgeId, EdgeFilletProfile::Chord(0.7));
        m.insert(
            2_u32 as EdgeId,
            EdgeFilletProfile::Radius(BlendRadius::Constant(0.25)),
        );
        m.insert(7_u32 as EdgeId, EdgeFilletProfile::Chord(0.4));
        let ft = FilletType::PerEdgeProfile(m);
        let s = format!("{:?}", ft);
        // edges 2, 7, 11 must appear in that order in the Debug output.
        let p2 = s.find("2").expect("edge 2 in debug");
        let p7 = s.find("7").expect("edge 7 in debug");
        let p11 = s.find("11").expect("edge 11 in debug");
        assert!(p2 < p7 && p7 < p11, "Debug output not sorted: {}", s);
    }

    #[test]
    fn validate_per_edge_profile_rejects_negative_chord() {
        let (model, solid_id, edges) = build_n_edge_model(2);
        let map = per_edge_profile_map(&[
            (edges[0], EdgeFilletProfile::Radius(BlendRadius::Constant(1.0))),
            (edges[1], EdgeFilletProfile::Chord(-0.5)),
        ]);
        let opts = opts_with_per_edge_profile(map);
        match validate_fillet_inputs(&model, solid_id, &edges, &opts) {
            Err(OperationError::InvalidInput { parameter, .. }) => {
                assert!(parameter.contains("PerEdgeProfile"), "{parameter}");
                assert!(parameter.contains("Chord"), "{parameter}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn validate_per_edge_profile_rejects_zero_chord() {
        let (model, solid_id, edges) = build_n_edge_model(2);
        let map = per_edge_profile_map(&[
            (edges[0], EdgeFilletProfile::Radius(BlendRadius::Constant(1.0))),
            (edges[1], EdgeFilletProfile::Chord(0.0)),
        ]);
        let opts = opts_with_per_edge_profile(map);
        match validate_fillet_inputs(&model, solid_id, &edges, &opts) {
            Err(OperationError::InvalidInput { parameter, .. }) => {
                assert!(parameter.contains("PerEdgeProfile"), "{parameter}");
                assert!(parameter.contains("Chord"), "{parameter}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn validate_per_edge_profile_rejects_non_finite_chord() {
        let (model, solid_id, edges) = build_n_edge_model(2);
        let map = per_edge_profile_map(&[
            (edges[0], EdgeFilletProfile::Radius(BlendRadius::Constant(1.0))),
            (edges[1], EdgeFilletProfile::Chord(f64::NAN)),
        ]);
        let opts = opts_with_per_edge_profile(map);
        match validate_fillet_inputs(&model, solid_id, &edges, &opts) {
            Err(OperationError::InvalidInput { parameter, .. }) => {
                assert!(parameter.contains("PerEdgeProfile"), "{parameter}");
                assert!(parameter.contains("Chord"), "{parameter}");
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn validate_per_edge_profile_accepts_chord_and_radius_mix() {
        let (model, solid_id, edges) = build_n_edge_model(2);
        let map = per_edge_profile_map(&[
            (edges[0], EdgeFilletProfile::Chord(0.4)),
            (edges[1], EdgeFilletProfile::Radius(BlendRadius::Constant(0.3))),
        ]);
        let opts = opts_with_per_edge_profile(map);
        assert!(validate_fillet_inputs(&model, solid_id, &edges, &opts).is_ok());
    }

    #[test]
    fn fillet_type_to_blend_radius_per_edge_profile_chord_returns_constant() {
        let mut m = HashMap::new();
        let chord_edge = 7_u32 as EdgeId;
        m.insert(chord_edge, EdgeFilletProfile::Chord(0.6));
        let ft = FilletType::PerEdgeProfile(m);
        assert_eq!(
            fillet_type_to_blend_radius(&ft, chord_edge),
            BlendRadius::Constant(0.6)
        );
    }

    #[test]
    fn edge_fillet_profile_max_radius_bound_chord_is_zero() {
        // Conservative: chord can produce arbitrarily large radii at
        // small dihedrals, so F6-α gate must be skipped.
        assert_eq!(EdgeFilletProfile::Chord(0.5).max_radius_bound(), 0.0);
    }

    #[test]
    fn edge_fillet_profile_min_radius_bound_chord_is_half() {
        let v = EdgeFilletProfile::Chord(0.6).min_radius_bound();
        assert!((v - 0.3).abs() < 1e-12);
    }

    #[test]
    fn edge_fillet_profile_radius_bounds_delegate_to_inner() {
        let p = EdgeFilletProfile::Radius(BlendRadius::Linear {
            start: 0.1,
            end: 0.5,
        });
        assert!((p.max_radius_bound() - 0.5).abs() < 1e-12);
        assert!((p.min_radius_bound() - 0.1).abs() < 1e-12);
    }

    // -------------------------------------------------------------------
    // Proptest harness for `validate_fillet_inputs` with PerEdgeConstant.
    //
    // Strategy:
    //   - `arb_radius_finite_positive`: finite radius in (tol, 100).
    //   - `arb_selection_size`: 1..=8 edges per case.
    //   - For each case, build a model with N edges, derive a
    //     correctly-shaped map, and run a focused mutation.
    //
    // Each property is *deterministic* given its random inputs: the
    // validator's expected return value is computable from the
    // mutation, not from a re-implementation of the validator. This
    // catches drift between the validator's logic and its
    // specification.
    // -------------------------------------------------------------------

    fn arb_radius_finite_positive() -> impl Strategy<Value = f64> {
        // (tol, 100). Default tolerance is ~1e-6, so 1e-3 is a safe
        // floor that always passes the `r > tol` gate without
        // collapsing into the NumericalError regime.
        (1e-3_f64..100.0_f64).prop_filter("must be finite", |r| r.is_finite())
    }

    fn arb_selection_size() -> impl Strategy<Value = usize> {
        1usize..=8
    }

    /// Build one of the three `BlendRadius` variants with a non-
    /// trivial, validation-passing shape from a `seed`. The variant
    /// kind is picked by `kind % 3`. Radii are deterministically
    /// derived from the seed so prop-test minimisation produces
    /// reproducible cases. Variable stations are fixed monotone
    /// `[0.0, 0.5, 1.0]`; radii vary by seed.
    fn build_arb_blend_radius(seed: u64, kind: u8) -> BlendRadius {
        let r0 = 0.1 + ((seed % 4000) as f64) / 1000.0;
        let r1 = 0.1 + (((seed >> 7) % 4000) as f64) / 1000.0;
        let r2 = 0.1 + (((seed >> 13) % 4000) as f64) / 1000.0;
        match kind % 3 {
            0 => BlendRadius::Constant(r0),
            1 => BlendRadius::Linear { start: r0, end: r1 },
            _ => BlendRadius::Variable(vec![
                (0.0, r0),
                (0.5, r1),
                (1.0, r2),
            ]),
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 96,
            max_global_rejects: 4096,
            ..ProptestConfig::default()
        })]

        /// Any correctly-shaped per-edge map (every selected edge
        /// has exactly one entry, every radius finite-positive)
        /// must be accepted by `validate_fillet_inputs`.
        #[test]
        fn prop_validate_fillet_inputs_per_edge_constant_accepts_any_valid_coverage(
            n in arb_selection_size(),
            seed in any::<u64>(),
        ) {
            let (model, solid_id, edges) = build_n_edge_model(n);
            // Derive deterministic radii from the seed so every
            // edge gets a distinct, finite-positive value.
            let mut map: HashMap<EdgeId, f64> = HashMap::new();
            for (i, &e) in edges.iter().enumerate() {
                // Map seed+index → (0.1, 5.1) deterministically.
                let bits = seed.wrapping_add(i as u64);
                let r = 0.1 + ((bits % 5000) as f64) / 1000.0;
                map.insert(e, r);
            }
            let opts = opts_with_per_edge(map);
            let result = validate_fillet_inputs(&model, solid_id, &edges, &opts);
            prop_assert!(result.is_ok(), "expected Ok, got {result:?}");
        }

        /// Removing any single edge's entry from an otherwise-valid
        /// map always trips the coverage check.
        #[test]
        fn prop_validate_fillet_inputs_rejects_any_missing_edge(
            n in 2usize..=8,
            drop_index in 0usize..8,
            r in arb_radius_finite_positive(),
        ) {
            let drop_index = drop_index % n;
            let (model, solid_id, edges) = build_n_edge_model(n);
            let mut map: HashMap<EdgeId, f64> = HashMap::new();
            for (i, &e) in edges.iter().enumerate() {
                if i == drop_index { continue; }
                map.insert(e, r);
            }
            let opts = opts_with_per_edge(map);
            let result = validate_fillet_inputs(&model, solid_id, &edges, &opts);
            prop_assert!(
                matches!(result, Err(OperationError::InvalidInput { .. })),
                "expected InvalidInput for missing edge, got {result:?}",
            );
        }

        /// Adding an extra map key not in the selection always
        /// trips the "no extras" check.
        #[test]
        fn prop_validate_fillet_inputs_rejects_any_extra_key(
            n in arb_selection_size(),
            r in arb_radius_finite_positive(),
            extra_offset in 1u32..=1024,
        ) {
            let (model, solid_id, edges) = build_n_edge_model(n);
            let mut map: HashMap<EdgeId, f64> = edges
                .iter()
                .map(|&e| (e, r))
                .collect();
            // Inject an edge id guaranteed not in the selection.
            let max_eid: EdgeId = *edges.iter().max().expect("non-empty by strategy");
            let extra: EdgeId = max_eid + extra_offset;
            // Re-verify it's not already in the map (only true if
            // the model happened to allocate that id, which our
            // helper doesn't — but be defensive).
            prop_assume!(!map.contains_key(&extra));
            map.insert(extra, r);
            let opts = opts_with_per_edge(map);
            let result = validate_fillet_inputs(&model, solid_id, &edges, &opts);
            prop_assert!(
                matches!(result, Err(OperationError::InvalidInput { .. })),
                "expected InvalidInput for extra key, got {result:?}",
            );
        }

        /// Replacing any one edge's radius with zero or negative
        /// must reject. Catches misses where the validator skips
        /// per-entry numeric checks after coverage passes.
        #[test]
        fn prop_validate_fillet_inputs_rejects_any_non_positive_radius(
            n in arb_selection_size(),
            bad_index in 0usize..8,
            good_r in arb_radius_finite_positive(),
            bad_r in -50.0_f64..=0.0,
        ) {
            let bad_index = bad_index % n;
            let (model, solid_id, edges) = build_n_edge_model(n);
            let mut map: HashMap<EdgeId, f64> = HashMap::new();
            for (i, &e) in edges.iter().enumerate() {
                let r = if i == bad_index { bad_r } else { good_r };
                map.insert(e, r);
            }
            let opts = opts_with_per_edge(map);
            let result = validate_fillet_inputs(&model, solid_id, &edges, &opts);
            prop_assert!(
                matches!(result, Err(OperationError::InvalidInput { .. })),
                "expected InvalidInput for non-positive radius, got {result:?}",
            );
        }

        /// Non-finite radii (NaN, +∞) at any position are rejected.
        #[test]
        fn prop_validate_fillet_inputs_rejects_any_non_finite_radius(
            n in arb_selection_size(),
            bad_index in 0usize..8,
            good_r in arb_radius_finite_positive(),
            use_nan in any::<bool>(),
        ) {
            let bad_index = bad_index % n;
            let bad_r = if use_nan { f64::NAN } else { f64::INFINITY };
            let (model, solid_id, edges) = build_n_edge_model(n);
            let mut map: HashMap<EdgeId, f64> = HashMap::new();
            for (i, &e) in edges.iter().enumerate() {
                let r = if i == bad_index { bad_r } else { good_r };
                map.insert(e, r);
            }
            let opts = opts_with_per_edge(map);
            let result = validate_fillet_inputs(&model, solid_id, &edges, &opts);
            prop_assert!(
                matches!(result, Err(OperationError::InvalidInput { .. })),
                "expected InvalidInput for non-finite radius, got {result:?}",
            );
        }

        /// Lookup faithfulness: `fillet_type_to_blend_radius` must
        /// return exactly the map's value (as `Constant(r)`) for
        /// every present key.
        #[test]
        fn prop_fillet_type_to_blend_radius_returns_map_value_for_existing_key(
            n in arb_selection_size(),
            r in arb_radius_finite_positive(),
        ) {
            let (_, _, edges) = build_n_edge_model(n);
            let map: HashMap<EdgeId, f64> = edges.iter().map(|&e| (e, r)).collect();
            let ft = FilletType::PerEdgeConstant(map);
            for &e in &edges {
                match fillet_type_to_blend_radius(&ft, e) {
                    BlendRadius::Constant(out) => prop_assert!(
                        (out - r).abs() < 1e-12,
                        "out={out} r={r}",
                    ),
                    other => prop_assert!(
                        false,
                        "expected Constant({r}), got {other:?}",
                    ),
                }
            }
        }

        /// Clone round-trip: cloning a PerEdgeConstant variant
        /// preserves every entry exactly.
        #[test]
        fn prop_per_edge_constant_clone_round_trip(
            n in arb_selection_size(),
            r in arb_radius_finite_positive(),
        ) {
            let (_, _, edges) = build_n_edge_model(n);
            let original: HashMap<EdgeId, f64> =
                edges.iter().map(|&e| (e, r)).collect();
            let ft = FilletType::PerEdgeConstant(original.clone());
            let cloned = ft.clone();
            match cloned {
                FilletType::PerEdgeConstant(c) => {
                    prop_assert_eq!(c.len(), original.len());
                    for (k, v) in &original {
                        let got = c.get(k).copied().unwrap_or(f64::NAN);
                        prop_assert!((got - v).abs() < 1e-12);
                    }
                }
                other => prop_assert!(
                    false,
                    "expected PerEdgeConstant, got {other:?}",
                ),
            }
        }

        /// Debug output is permutation-invariant: two maps with the
        /// same entries inserted in any order produce identical
        /// Debug strings. The recorded-operation JSON depends on
        /// this for stable timeline diffs.
        #[test]
        fn prop_per_edge_constant_debug_is_deterministic_under_permutations(
            entries in proptest::collection::vec(
                (0u32..1024, arb_radius_finite_positive()),
                1..=8,
            ),
        ) {
            // Dedupe by edge id — HashMap insertion would otherwise
            // collapse duplicates non-deterministically depending on
            // order.
            let mut dedup: HashMap<EdgeId, f64> = HashMap::new();
            for (e, r) in &entries {
                dedup.insert(*e as EdgeId, *r);
            }
            let mut sorted: Vec<(EdgeId, f64)> = dedup.iter().map(|(&k, &v)| (k, v)).collect();
            sorted.sort_by_key(|(k, _)| *k);
            let mut reversed = sorted.clone();
            reversed.reverse();

            let map_a: HashMap<EdgeId, f64> = sorted.into_iter().collect();
            let map_b: HashMap<EdgeId, f64> = reversed.into_iter().collect();
            prop_assert_eq!(
                format!("{:?}", FilletType::PerEdgeConstant(map_a)),
                format!("{:?}", FilletType::PerEdgeConstant(map_b)),
            );
        }

        // -------------------------------------------------------------
        // F5-β.5.6 — `PerEdgeProfile` prop tests
        //
        // Mirrors the `PerEdgeConstant` prop suite above but reaches
        // one level deeper into the per-edge profile. Each prop pins
        // an invariant of the new variant under randomised inputs.
        // The `build_arb_blend_radius` helper lives outside the
        // `proptest!` block (the macro doesn't admit free function
        // definitions inside its body).
        // -------------------------------------------------------------

        /// Every valid per-edge profile map (one entry per selected
        /// edge, finite-positive radii, valid Variable stations) is
        /// accepted by `validate_fillet_inputs`.
        #[test]
        fn prop_validate_per_edge_profile_accepts_any_valid_coverage(
            n in arb_selection_size(),
            seed in any::<u64>(),
        ) {
            let (model, solid_id, edges) = build_n_edge_model(n);
            let mut map: HashMap<EdgeId, EdgeFilletProfile> = HashMap::new();
            for (i, &e) in edges.iter().enumerate() {
                let s = seed.wrapping_add(i as u64);
                map.insert(
                    e,
                    EdgeFilletProfile::Radius(build_arb_blend_radius(s, (s & 0xff) as u8)),
                );
            }
            let opts = FilletOptions {
                radius: 1.0,
                fillet_type: FilletType::PerEdgeProfile(map),
                ..Default::default()
            };
            let result = validate_fillet_inputs(&model, solid_id, &edges, &opts);
            prop_assert!(result.is_ok(), "expected Ok, got {result:?}");
        }

        /// Removing any single edge's entry from an otherwise-valid
        /// per-edge-profile map always trips the coverage check.
        #[test]
        fn prop_validate_per_edge_profile_rejects_any_missing_edge(
            n in 2usize..=8,
            drop_index in 0usize..8,
            seed in any::<u64>(),
        ) {
            let drop_index = drop_index % n;
            let (model, solid_id, edges) = build_n_edge_model(n);
            let mut map: HashMap<EdgeId, EdgeFilletProfile> = HashMap::new();
            for (i, &e) in edges.iter().enumerate() {
                if i == drop_index { continue; }
                let s = seed.wrapping_add(i as u64);
                map.insert(
                    e,
                    EdgeFilletProfile::Radius(build_arb_blend_radius(s, (s & 0xff) as u8)),
                );
            }
            let opts = FilletOptions {
                radius: 1.0,
                fillet_type: FilletType::PerEdgeProfile(map),
                ..Default::default()
            };
            let result = validate_fillet_inputs(&model, solid_id, &edges, &opts);
            prop_assert!(result.is_err(), "expected Err, got {result:?}");
        }

        /// Adding any extra (non-selected) edge entry to an otherwise-
        /// valid map trips the no-extras check.
        #[test]
        fn prop_validate_per_edge_profile_rejects_any_extra_key(
            n in arb_selection_size(),
            seed in any::<u64>(),
        ) {
            let (model, solid_id, edges) = build_n_edge_model(n);
            let mut map: HashMap<EdgeId, EdgeFilletProfile> = HashMap::new();
            for (i, &e) in edges.iter().enumerate() {
                let s = seed.wrapping_add(i as u64);
                map.insert(
                    e,
                    EdgeFilletProfile::Radius(build_arb_blend_radius(s, (s & 0xff) as u8)),
                );
            }
            // Inject a guaranteed-extra key. `build_n_edge_model`
            // generates edge ids densely from EdgeStore allocation,
            // so a u32 well above any plausibly-allocated id will
            // not collide with the selection.
            let extra: EdgeId = 999_999_u32 as EdgeId;
            map.insert(extra, EdgeFilletProfile::Radius(BlendRadius::Constant(0.5)));
            let opts = FilletOptions {
                radius: 1.0,
                fillet_type: FilletType::PerEdgeProfile(map),
                ..Default::default()
            };
            let result = validate_fillet_inputs(&model, solid_id, &edges, &opts);
            prop_assert!(result.is_err(), "expected Err, got {result:?}");
        }

        /// A negative `Constant` radius on any single edge is always
        /// rejected.
        #[test]
        fn prop_validate_per_edge_profile_rejects_negative_constant_anywhere(
            n in 1usize..=6,
            bad_index in 0usize..8,
            r_neg in -10.0f64..=-1e-6f64,
            seed in any::<u64>(),
        ) {
            let bad_index = bad_index % n;
            let (model, solid_id, edges) = build_n_edge_model(n);
            let mut map: HashMap<EdgeId, EdgeFilletProfile> = HashMap::new();
            for (i, &e) in edges.iter().enumerate() {
                if i == bad_index {
                    map.insert(e, EdgeFilletProfile::Radius(BlendRadius::Constant(r_neg)));
                } else {
                    let s = seed.wrapping_add(i as u64);
                    map.insert(
                        e,
                        EdgeFilletProfile::Radius(build_arb_blend_radius(s, (s & 0xff) as u8)),
                    );
                }
            }
            let opts = FilletOptions {
                radius: 1.0,
                fillet_type: FilletType::PerEdgeProfile(map),
                ..Default::default()
            };
            let result = validate_fillet_inputs(&model, solid_id, &edges, &opts);
            prop_assert!(result.is_err(), "expected Err, got {result:?}");
        }

        /// A `Linear { start, end }` with a non-finite or non-positive
        /// endpoint on any single edge is always rejected.
        #[test]
        fn prop_validate_per_edge_profile_rejects_invalid_linear_endpoint(
            n in 1usize..=6,
            bad_index in 0usize..8,
            // Cover the three failure modes deterministically.
            mode in 0u8..3,
            seed in any::<u64>(),
        ) {
            let bad_index = bad_index % n;
            let (model, solid_id, edges) = build_n_edge_model(n);
            let mut map: HashMap<EdgeId, EdgeFilletProfile> = HashMap::new();
            for (i, &e) in edges.iter().enumerate() {
                if i == bad_index {
                    let bad = match mode {
                        0 => BlendRadius::Linear { start: 0.0, end: 1.0 },
                        1 => BlendRadius::Linear { start: 1.0, end: -0.5 },
                        _ => BlendRadius::Linear {
                            start: f64::NAN,
                            end: 1.0,
                        },
                    };
                    map.insert(e, EdgeFilletProfile::Radius(bad));
                } else {
                    let s = seed.wrapping_add(i as u64);
                    map.insert(
                        e,
                        EdgeFilletProfile::Radius(build_arb_blend_radius(s, (s & 0xff) as u8)),
                    );
                }
            }
            let opts = FilletOptions {
                radius: 1.0,
                fillet_type: FilletType::PerEdgeProfile(map),
                ..Default::default()
            };
            let result = validate_fillet_inputs(&model, solid_id, &edges, &opts);
            prop_assert!(result.is_err(), "expected Err, got {result:?}");
        }

        /// A `Variable` profile with non-monotone stations on any
        /// single edge is always rejected. We perturb a fixed
        /// 3-sample template so the offending edge has stations
        /// `[0.0, 0.5, p]` with `p ≤ 0.5`.
        #[test]
        fn prop_validate_per_edge_profile_rejects_non_monotone_variable(
            n in 1usize..=6,
            bad_index in 0usize..8,
            p_third in 0.0f64..=0.5f64,
            seed in any::<u64>(),
        ) {
            let bad_index = bad_index % n;
            let (model, solid_id, edges) = build_n_edge_model(n);
            let mut map: HashMap<EdgeId, EdgeFilletProfile> = HashMap::new();
            for (i, &e) in edges.iter().enumerate() {
                if i == bad_index {
                    map.insert(
                        e,
                        EdgeFilletProfile::Radius(BlendRadius::Variable(vec![
                            (0.0, 0.4),
                            (0.5, 0.6),
                            (p_third, 0.4),
                        ])),
                    );
                } else {
                    let s = seed.wrapping_add(i as u64);
                    map.insert(
                        e,
                        EdgeFilletProfile::Radius(build_arb_blend_radius(s, (s & 0xff) as u8)),
                    );
                }
            }
            let opts = FilletOptions {
                radius: 1.0,
                fillet_type: FilletType::PerEdgeProfile(map),
                ..Default::default()
            };
            let result = validate_fillet_inputs(&model, solid_id, &edges, &opts);
            prop_assert!(result.is_err(), "expected Err, got {result:?}");
        }

        /// `fillet_type_to_blend_radius` returns the map's stored
        /// profile *unchanged* for every present key. No variant
        /// transmutation, no radius drift.
        #[test]
        fn prop_fillet_type_to_blend_radius_per_edge_profile_round_trip(
            n in arb_selection_size(),
            seed in any::<u64>(),
        ) {
            let (_, _, edges) = build_n_edge_model(n);
            // Keep the inner `BlendRadius` map for round-trip
            // comparison against `fillet_type_to_blend_radius`, and
            // build the matching `EdgeFilletProfile` map by wrapping
            // each entry in `Radius(_)` for the `FilletType` value.
            let mut original: HashMap<EdgeId, BlendRadius> = HashMap::new();
            for (i, &e) in edges.iter().enumerate() {
                let s = seed.wrapping_add(i as u64);
                original.insert(e, build_arb_blend_radius(s, (s & 0xff) as u8));
            }
            let profile_map: HashMap<EdgeId, EdgeFilletProfile> = original
                .iter()
                .map(|(&k, v)| (k, EdgeFilletProfile::Radius(v.clone())))
                .collect();
            let ft = FilletType::PerEdgeProfile(profile_map);
            for &e in &edges {
                let expected = original.get(&e).cloned();
                let got = fillet_type_to_blend_radius(&ft, e);
                prop_assert_eq!(
                    expected.as_ref(),
                    Some(&got),
                    "edge {:?}: expected {:?}, got {:?}",
                    e,
                    expected,
                    got,
                );
            }
        }

        /// Cloning a `PerEdgeProfile` variant preserves every entry
        /// exactly — including the *kind* of each `BlendRadius`. We
        /// build mixed-kind maps and assert per-entry equality via
        /// `PartialEq` (already derived on `BlendRadius`).
        #[test]
        fn prop_per_edge_profile_clone_preserves_mixed_kinds(
            n in arb_selection_size(),
            seed in any::<u64>(),
        ) {
            let (_, _, edges) = build_n_edge_model(n);
            let mut original: HashMap<EdgeId, EdgeFilletProfile> = HashMap::new();
            for (i, &e) in edges.iter().enumerate() {
                let s = seed.wrapping_add(i as u64);
                original.insert(
                    e,
                    EdgeFilletProfile::Radius(build_arb_blend_radius(s, (s & 0xff) as u8)),
                );
            }
            let ft = FilletType::PerEdgeProfile(original.clone());
            let cloned = ft.clone();
            match cloned {
                FilletType::PerEdgeProfile(c) => {
                    prop_assert_eq!(c.len(), original.len());
                    for (k, v) in &original {
                        prop_assert_eq!(c.get(k), Some(v));
                    }
                }
                other => prop_assert!(
                    false,
                    "expected PerEdgeProfile, got {other:?}",
                ),
            }
        }

        /// Debug output is permutation-invariant: two maps with the
        /// same entries inserted in any order produce identical
        /// Debug strings. Required for stable timeline replay diffs.
        #[test]
        fn prop_per_edge_profile_debug_is_deterministic_under_permutations(
            entries in proptest::collection::vec(
                (0u32..1024, 0u8..3, any::<u64>()),
                1..=6,
            ),
        ) {
            // Dedupe by edge id so HashMap collisions don't make
            // the two orderings disagree about which value survived.
            let mut dedup: HashMap<EdgeId, EdgeFilletProfile> = HashMap::new();
            for (e, kind, seed) in &entries {
                dedup.insert(
                    *e as EdgeId,
                    EdgeFilletProfile::Radius(build_arb_blend_radius(*seed, *kind)),
                );
            }
            let mut sorted: Vec<(EdgeId, EdgeFilletProfile)> = dedup
                .iter()
                .map(|(&k, v)| (k, v.clone()))
                .collect();
            sorted.sort_by_key(|(k, _)| *k);
            let mut reversed = sorted.clone();
            reversed.reverse();
            let map_a: HashMap<EdgeId, EdgeFilletProfile> = sorted.into_iter().collect();
            let map_b: HashMap<EdgeId, EdgeFilletProfile> = reversed.into_iter().collect();
            prop_assert_eq!(
                format!("{:?}", FilletType::PerEdgeProfile(map_a)),
                format!("{:?}", FilletType::PerEdgeProfile(map_b)),
            );
        }

        // ----- F5-β.5.7 chord-focused property tests -----

        /// `EdgeFilletProfile::Chord(c).min_radius_bound()` is `c/2`
        /// for every positive finite chord — this is the worst-case
        /// (flat dihedral, θ=π) radius derived from a chord, and the
        /// representative-radius > 0 gate relies on it being a
        /// closed-form lower bound. Verified for any `c > 0`.
        #[test]
        fn prop_edge_fillet_profile_chord_min_bound_is_half_for_any_positive_chord(
            c in 1e-9_f64..1e6_f64,
        ) {
            let bound = EdgeFilletProfile::Chord(c).min_radius_bound();
            prop_assert!(
                (bound - c / 2.0).abs() < 1e-12,
                "Chord({c}).min_radius_bound() = {bound}, expected {expected}",
                expected = c / 2.0,
            );
        }

        /// `EdgeFilletProfile::Chord(c).max_radius_bound()` is `0.0`
        /// for any chord, matching the existing top-level
        /// `FilletType::Chord(_) => 0.0` F6-α opt-out. The actual
        /// radius produced by a chord at dihedral θ is
        /// `c / (2 sin(θ/2))`, which is unbounded as θ → 0, so the
        /// curvature gate cannot pre-screen chord requests and must
        /// no-op them.
        #[test]
        fn prop_edge_fillet_profile_chord_max_bound_is_zero_for_any_chord(
            c in 1e-9_f64..1e6_f64,
        ) {
            let bound = EdgeFilletProfile::Chord(c).max_radius_bound();
            prop_assert_eq!(bound, 0.0);
        }

        /// Wrapping any `BlendRadius` profile in
        /// `EdgeFilletProfile::Radius(...)` preserves both
        /// `min_radius_bound` and `max_radius_bound` — they delegate
        /// to the inner schedule's `min_value` / `max_value`. The
        /// chord arm is the only asymmetric path; everything else
        /// must be pass-through.
        #[test]
        fn prop_edge_fillet_profile_radius_bounds_delegate_to_inner_for_any_kind(
            seed in any::<u64>(),
            kind in 0u8..3u8,
        ) {
            let inner = build_arb_blend_radius(seed, kind);
            let wrapped = EdgeFilletProfile::Radius(inner.clone());
            prop_assert!(
                (wrapped.max_radius_bound() - inner.max_value()).abs() < 1e-12,
                "max_radius_bound mismatch: wrapped={}, inner={}",
                wrapped.max_radius_bound(),
                inner.max_value(),
            );
            prop_assert!(
                (wrapped.min_radius_bound() - inner.min_value()).abs() < 1e-12,
                "min_radius_bound mismatch: wrapped={}, inner={}",
                wrapped.min_radius_bound(),
                inner.min_value(),
            );
        }

        /// Random per-edge map where every entry is `Chord(c_i)` with
        /// `c_i` finite-positive must validate clean. Pins the
        /// chord-only fast path through the validator.
        #[test]
        fn prop_validate_per_edge_profile_accepts_any_valid_chord_only_coverage(
            n in arb_selection_size(),
            seed in any::<u64>(),
        ) {
            let (model, solid_id, edges) = build_n_edge_model(n);
            let mut map: HashMap<EdgeId, EdgeFilletProfile> = HashMap::new();
            for (i, &e) in edges.iter().enumerate() {
                let bits = seed.wrapping_add(i as u64);
                let c = 0.1 + ((bits % 5000) as f64) / 1000.0;
                map.insert(e, EdgeFilletProfile::Chord(c));
            }
            let opts = opts_with_per_edge_profile(map);
            let result = validate_fillet_inputs(&model, solid_id, &edges, &opts);
            prop_assert!(result.is_ok(), "expected Ok, got {result:?}");
        }

        /// Random per-edge map where every entry is *either* a valid
        /// chord *or* a valid radius profile must validate clean.
        /// Pins the mixed chord+radius path — the variant that F5-β.5.7
        /// exists to enable.
        #[test]
        fn prop_validate_per_edge_profile_accepts_any_valid_chord_radius_mix(
            n in arb_selection_size(),
            seed in any::<u64>(),
        ) {
            let (model, solid_id, edges) = build_n_edge_model(n);
            let mut map: HashMap<EdgeId, EdgeFilletProfile> = HashMap::new();
            for (i, &e) in edges.iter().enumerate() {
                let bits = seed.wrapping_add(i as u64);
                // Cycle the kind: chord on every 4th edge, otherwise
                // a deterministic radius profile.
                if i % 4 == 0 {
                    let c = 0.1 + ((bits % 5000) as f64) / 1000.0;
                    map.insert(e, EdgeFilletProfile::Chord(c));
                } else {
                    let r = build_arb_blend_radius(bits, (i % 3) as u8);
                    map.insert(e, EdgeFilletProfile::Radius(r));
                }
            }
            let opts = opts_with_per_edge_profile(map);
            let result = validate_fillet_inputs(&model, solid_id, &edges, &opts);
            prop_assert!(result.is_ok(), "expected Ok, got {result:?}");
        }

        /// Injecting a non-positive chord at any single position of an
        /// otherwise-valid mixed map must trip the per-edge chord
        /// positivity check. The failing parameter must mention
        /// `Chord` so callers can localise the bad entry.
        #[test]
        fn prop_validate_per_edge_profile_rejects_non_positive_chord_anywhere(
            n in 1usize..=8,
            bad_index in 0usize..8,
            bad_value in proptest::sample::select(vec![0.0_f64, -1e-6, -0.5, -42.0]),
        ) {
            prop_assume!(n >= 1);
            let bad_index = bad_index % n;
            let (model, solid_id, edges) = build_n_edge_model(n);
            let mut map: HashMap<EdgeId, EdgeFilletProfile> = HashMap::new();
            for (i, &e) in edges.iter().enumerate() {
                if i == bad_index {
                    map.insert(e, EdgeFilletProfile::Chord(bad_value));
                } else {
                    map.insert(e, EdgeFilletProfile::Radius(BlendRadius::Constant(0.5)));
                }
            }
            let opts = opts_with_per_edge_profile(map);
            let result = validate_fillet_inputs(&model, solid_id, &edges, &opts);
            match result {
                Err(OperationError::InvalidInput { parameter, .. }) => {
                    prop_assert!(
                        parameter.contains("Chord"),
                        "expected parameter to mention Chord, got {parameter}",
                    );
                }
                other => prop_assert!(
                    false,
                    "expected InvalidInput citing Chord, got {other:?}",
                ),
            }
        }

        /// Injecting a non-finite chord (NaN / +∞ / -∞) at any single
        /// position must trip the finite check. Mirrors the
        /// `..._non_finite_constant_anywhere` test for the chord arm.
        #[test]
        fn prop_validate_per_edge_profile_rejects_non_finite_chord_anywhere(
            n in 1usize..=8,
            bad_index in 0usize..8,
            mode in 0u8..3u8,
        ) {
            let bad_index = bad_index % n;
            let bad_value = match mode {
                0 => f64::NAN,
                1 => f64::INFINITY,
                _ => f64::NEG_INFINITY,
            };
            let (model, solid_id, edges) = build_n_edge_model(n);
            let mut map: HashMap<EdgeId, EdgeFilletProfile> = HashMap::new();
            for (i, &e) in edges.iter().enumerate() {
                if i == bad_index {
                    map.insert(e, EdgeFilletProfile::Chord(bad_value));
                } else {
                    map.insert(e, EdgeFilletProfile::Radius(BlendRadius::Constant(0.5)));
                }
            }
            let opts = opts_with_per_edge_profile(map);
            let result = validate_fillet_inputs(&model, solid_id, &edges, &opts);
            match result {
                Err(OperationError::InvalidInput { parameter, .. }) => {
                    prop_assert!(
                        parameter.contains("Chord"),
                        "expected parameter to mention Chord, got {parameter}",
                    );
                }
                other => prop_assert!(
                    false,
                    "expected InvalidInput citing Chord, got {other:?}",
                ),
            }
        }

        /// For any per-edge chord entry, `fillet_type_to_blend_radius`
        /// returns `BlendRadius::Constant(c)` — the placeholder used
        /// to feed `blend_graph::build`'s edge classification when no
        /// closed-form radius is available without dihedral context.
        /// Required so chord edges still appear in the blend graph
        /// (and therefore in the surgery pipeline) on equal footing
        /// with constant-radius edges.
        #[test]
        fn prop_fillet_type_to_blend_radius_chord_per_edge_returns_constant(
            c in 1e-9_f64..1e6_f64,
            eid in 0u32..1024,
        ) {
            let mut m = HashMap::new();
            m.insert(eid as EdgeId, EdgeFilletProfile::Chord(c));
            let ft = FilletType::PerEdgeProfile(m);
            match fillet_type_to_blend_radius(&ft, eid as EdgeId) {
                BlendRadius::Constant(out) => prop_assert!(
                    (out - c).abs() < 1e-12,
                    "expected Constant({c}), got Constant({out})",
                ),
                other => prop_assert!(
                    false,
                    "expected Constant({c}), got {other:?}",
                ),
            }
        }

        /// Cloning a `PerEdgeProfile` map that mixes chord + radius
        /// entries preserves every value (deep clone semantics). The
        /// pre-F5-β.5.7 prop test only covered radius profiles; this
        /// extends it to the new chord arm.
        #[test]
        fn prop_per_edge_profile_chord_clone_preserves_mixed_kinds(
            entries in proptest::collection::vec(
                (0u32..1024, 0u8..4, any::<u64>()),
                1..=8,
            ),
        ) {
            let mut original: HashMap<EdgeId, EdgeFilletProfile> = HashMap::new();
            for (e, kind, seed) in &entries {
                let profile = if *kind == 3 {
                    // Map kind=3 to chord; the other three kinds go
                    // through the existing arbitrary BlendRadius
                    // builder.
                    let c = 0.1 + ((seed % 5000) as f64) / 1000.0;
                    EdgeFilletProfile::Chord(c)
                } else {
                    EdgeFilletProfile::Radius(build_arb_blend_radius(*seed, *kind))
                };
                original.insert(*e as EdgeId, profile);
            }
            let ft = FilletType::PerEdgeProfile(original.clone());
            let cloned = ft.clone();
            match cloned {
                FilletType::PerEdgeProfile(c) => {
                    prop_assert_eq!(c.len(), original.len());
                    for (k, v) in &original {
                        prop_assert_eq!(c.get(k), Some(v));
                    }
                }
                other => prop_assert!(
                    false,
                    "expected PerEdgeProfile, got {other:?}",
                ),
            }
        }
    }

    // -------------------------------------------------------------------
    // F5-β.5.5 — synthetic non-orthogonal three-cylinder corner fixture
    //
    // The mixed-radii NURBS triangular-patch emission path inside
    // `apply_triangular_nurbs_corner` cannot fire end-to-end through
    // `fillet_edges` today: rectilinear box corners overdetermine the
    // pairwise cap-circle intersection (proof + worked example in the
    // module-doc of `tests/fillet_three_edge_corner_mixed_radii.rs`),
    // and `TopologyBuilder` does not yet construct tetrahedral / skew-
    // prism solids whose three corner edges are non-orthogonal.
    //
    // This fixture hand-builds the *output* of the per-edge fillet
    // pass (three cylindrical fillet faces with V-side cap arcs) on a
    // synthetic three-cylinder corner whose axes are non-coplanar:
    //
    //     u_0 = +Y           q_0 = C_0 = (3, 0, 4)   r_0 = 5
    //     u_1 = +X           q_1 = C_1 = (0, 1, 4)   r_1 = √17
    //     u_2 = (1,1,1)/√3   q_2 = C_2 = (3, 1, 4)   r_2 = √26
    //
    // with corner_apex A = (0, 0, 8) — the unique intersection of the
    // three cap planes y=0, x=0, x+y+z=8. The pairwise cap-circle
    // intersections (V-side, picked against vertex_pos=(3,2,4) and
    // vertex_outward=-Z) are exact rational points:
    //
    //     P_{01} = (0, 0, 0)    [cap0 ∩ cap1]
    //     P_{12} = (0, 5, 3)    [cap1 ∩ cap2]
    //     P_{20} = (7, 0, 1)    [cap2 ∩ cap0]
    //
    // All six radial-distance and axial-projection checks satisfy
    // |Δ| = 0 exactly, so the fixture never lands on
    // `trim_cap_arc_in_place`'s tolerance gate.
    //
    // The far-side cap arcs + seam Lines exist only so each fillet
    // face's outer loop is closeable; they sit at C_i + 100·u_i so
    // `find_cap_arc_edge_by_cylinder_axis` (which prefers the cap
    // nearest `corner_apex`) reliably picks the V-side.
    // -------------------------------------------------------------------

    /// Returns `(model, solid_id, vertex_id, vertex_pos, [fillet_face;3],
    /// [(q_i,u_i,r_i);3], corner_apex, vertex_outward)` ready to feed
    /// into [`apply_triangular_nurbs_corner`].
    #[allow(clippy::too_many_lines)]
    fn build_synthetic_three_cylinder_corner() -> (
        BRepModel,
        SolidId,
        VertexId,
        Point3,
        [FaceId; 3],
        [(Point3, Vector3, f64); 3],
        Point3,
        Vector3,
    ) {
        use crate::primitives::face::FaceOrientation;
        use crate::primitives::shell::{Shell, ShellType};
        use crate::primitives::solid::Solid;
        use crate::primitives::surface::Cylinder;

        let mut model = BRepModel::new();

        // Three cylinder axes (q_i = C_i picks t=0 on the axis line so
        // the function's recomputed C_i = q + ((A-q)·u)·u lands on q
        // exactly — verified by (A-C_i)·u_i = 0 for all three).
        let u_0 = Vector3::new(0.0, 1.0, 0.0);
        let u_1 = Vector3::new(1.0, 0.0, 0.0);
        let inv_sqrt3 = 1.0 / 3.0_f64.sqrt();
        let u_2 = Vector3::new(inv_sqrt3, inv_sqrt3, inv_sqrt3);
        let c_0 = Point3::new(3.0, 0.0, 4.0);
        let c_1 = Point3::new(0.0, 1.0, 4.0);
        let c_2 = Point3::new(3.0, 1.0, 4.0);
        let r_0 = 5.0;
        let r_1 = 17.0_f64.sqrt();
        let r_2 = 26.0_f64.sqrt();

        let cylinder_axes: [(Point3, Vector3, f64); 3] =
            [(c_0, u_0, r_0), (c_1, u_1, r_1), (c_2, u_2, r_2)];
        let corner_apex = Point3::new(0.0, 0.0, 8.0);
        let vertex_pos = Point3::new(3.0, 2.0, 4.0);
        let vertex_outward = Vector3::new(0.0, 0.0, -1.0);
        let corner_vertex = model
            .vertices
            .add(vertex_pos.x, vertex_pos.y, vertex_pos.z);

        // Register the three cylindrical surfaces.
        let mut fillet_face_ids = [0u32; 3];
        let far_offset = 100.0;

        for i in 0..3 {
            let (q, u, r) = cylinder_axes[i];
            let cyl = Cylinder::new(q, u, r).expect("synthetic cylinder");
            let surface_id = model.surfaces.add(Box::new(cyl));

            // V-side cap arc: full 90° sweep at center C_i (the cap is
            // the circle in the plane through C_i perpendicular to u_i,
            // radius r_i). Pre-trim sweep is irrelevant — the trim
            // pass overwrites both start and sweep — but the arc must
            // exist with the right (center, normal, radius).
            let arc_v = Arc::new(q, u, r, 0.0, std::f64::consts::FRAC_PI_2)
                .expect("synthetic V-side cap arc");
            // Pre-trim arc endpoints (will be discarded by trim, but
            // the seam Lines need to anchor at the right positions
            // so the outer loop closes geometrically).
            let p_v_start = arc_v.evaluate(0.0).expect("V-arc start").position;
            let p_v_end = arc_v.evaluate(1.0).expect("V-arc end").position;
            let curve_v = model.curves.add(Box::new(arc_v));

            // Far-side cap arc: at q + far_offset·u, same normal +
            // radius. Sits beyond the V-side cap so the
            // nearest-to-corner_apex tie-breaker in
            // `find_cap_arc_edge_by_cylinder_axis` always picks the
            // V-side cap.
            let far_center = Point3::new(
                q.x + far_offset * u.x,
                q.y + far_offset * u.y,
                q.z + far_offset * u.z,
            );
            let arc_far = Arc::new(far_center, u, r, 0.0, std::f64::consts::FRAC_PI_2)
                .expect("synthetic far-side cap arc");
            let p_f_start = arc_far.evaluate(0.0).expect("far-arc start").position;
            let p_f_end = arc_far.evaluate(1.0).expect("far-arc end").position;
            let curve_far = model.curves.add(Box::new(arc_far));

            // V-side vertices.
            let v_start = model.vertices.add(p_v_start.x, p_v_start.y, p_v_start.z);
            let v_end = model.vertices.add(p_v_end.x, p_v_end.y, p_v_end.z);
            // Far-side vertices.
            let f_start = model.vertices.add(p_f_start.x, p_f_start.y, p_f_start.z);
            let f_end = model.vertices.add(p_f_end.x, p_f_end.y, p_f_end.z);

            // V-side cap arc edge.
            let v_edge = model.edges.add(Edge::new_auto_range(
                0,
                v_start,
                v_end,
                curve_v,
                EdgeOrientation::Forward,
            ));
            // Far-side cap arc edge.
            let f_edge = model.edges.add(Edge::new_auto_range(
                0,
                f_start,
                f_end,
                curve_far,
                EdgeOrientation::Forward,
            ));

            // Two straight seam Lines connecting V-side to far-side.
            // seam_a: v_end → f_end (one cylinder generator).
            let seam_a_line = Line::new(p_v_end, p_f_end);
            let seam_a_curve = model.curves.add(Box::new(seam_a_line));
            let seam_a = model.edges.add(Edge::new_auto_range(
                0,
                v_end,
                f_end,
                seam_a_curve,
                EdgeOrientation::Forward,
            ));
            // seam_b: f_start → v_start (closes the loop).
            let seam_b_line = Line::new(p_f_start, p_v_start);
            let seam_b_curve = model.curves.add(Box::new(seam_b_line));
            let seam_b = model.edges.add(Edge::new_auto_range(
                0,
                f_start,
                v_start,
                seam_b_curve,
                EdgeOrientation::Forward,
            ));

            // Outer loop: V-cap (fwd) → seam_a (fwd) → far-cap (rev)
            // → seam_b (fwd). The far-cap is traversed backwards
            // because v_end→f_end→f_start→v_start is the cycle.
            let mut outer = Loop::new(0, LoopType::Outer);
            outer.add_edge(v_edge, true);
            outer.add_edge(seam_a, true);
            outer.add_edge(f_edge, false);
            outer.add_edge(seam_b, true);
            let loop_id = model.loops.add(outer);

            let face = Face::new(0, surface_id, loop_id, FaceOrientation::Forward);
            fillet_face_ids[i] = model.faces.add(face);
        }

        // Outer shell + solid wrapping the three fillet faces. The
        // shell is `Open` because the fixture is not a closed
        // manifold; `apply_triangular_nurbs_corner` does not require
        // closure, only that `solid.outer_shell` exists.
        let mut shell = Shell::new(0, ShellType::Open);
        for &face_id in &fillet_face_ids {
            shell.add_face(face_id);
        }
        let shell_id = model.shells.add(shell);
        let solid = Solid::new(0, shell_id);
        let solid_id = model.solids.add(solid);

        (
            model,
            solid_id,
            corner_vertex,
            vertex_pos,
            fillet_face_ids,
            cylinder_axes,
            corner_apex,
            vertex_outward,
        )
    }

    /// Precondition: the three cap circles pairwise intersect at the
    /// derived exact-rational points `(0,0,0)`, `(0,5,3)`, `(7,0,1)`.
    /// If this fails the fixture geometry is wrong; everything
    /// downstream depending on it is moot.
    #[test]
    fn synthetic_three_cylinder_corner_fixture_satisfies_pairwise_caps() {
        let (_, _, _, vertex_pos, _, cylinder_axes, corner_apex, vertex_outward) =
            build_synthetic_three_cylinder_corner();

        // Compute cap centres exactly as apply_triangular_nurbs_corner
        // does at Step 2.
        let mut cap_centres = [Point3::new(0.0, 0.0, 0.0); 3];
        for i in 0..3 {
            let (q, u, _) = cylinder_axes[i];
            let v = corner_apex - q;
            let t = v.x * u.x + v.y * u.y + v.z * u.z;
            cap_centres[i] = Point3::new(q.x + t * u.x, q.y + t * u.y, q.z + t * u.z);
        }
        let (_, u_0, r_0) = cylinder_axes[0];
        let (_, u_1, r_1) = cylinder_axes[1];
        let (_, u_2, r_2) = cylinder_axes[2];

        let p_01 = intersect_two_caps(
            cap_centres[0],
            u_0,
            r_0,
            cap_centres[1],
            u_1,
            r_1,
            vertex_pos,
            vertex_outward,
        )
        .expect("cap 0 ∩ cap 1 must intersect");
        let p_12 = intersect_two_caps(
            cap_centres[1],
            u_1,
            r_1,
            cap_centres[2],
            u_2,
            r_2,
            vertex_pos,
            vertex_outward,
        )
        .expect("cap 1 ∩ cap 2 must intersect");
        let p_20 = intersect_two_caps(
            cap_centres[2],
            u_2,
            r_2,
            cap_centres[0],
            u_0,
            r_0,
            vertex_pos,
            vertex_outward,
        )
        .expect("cap 2 ∩ cap 0 must intersect");

        let approx_eq = |a: Point3, b: Point3| -> bool {
            (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9 && (a.z - b.z).abs() < 1e-9
        };
        assert!(
            approx_eq(p_01, Point3::new(0.0, 0.0, 0.0)),
            "p_01 = {:?}, expected (0,0,0)",
            p_01
        );
        assert!(
            approx_eq(p_12, Point3::new(0.0, 5.0, 3.0)),
            "p_12 = {:?}, expected (0,5,3)",
            p_12
        );
        assert!(
            approx_eq(p_20, Point3::new(7.0, 0.0, 1.0)),
            "p_20 = {:?}, expected (7,0,1)",
            p_20
        );

        // The three intersection points must form a non-degenerate
        // triangle (no two coincide).
        assert!(!approx_eq(p_01, p_12));
        assert!(!approx_eq(p_12, p_20));
        assert!(!approx_eq(p_20, p_01));
    }

    /// Precondition: each of the three fillet faces has exactly one
    /// V-side cap arc resolvable by
    /// `find_cap_arc_edge_by_cylinder_axis`. If the lookup ever
    /// returns the far-side arc instead, the trim pass writes to the
    /// wrong edge and the test below silently corrupts the topology.
    #[test]
    fn synthetic_three_cylinder_corner_fixture_cap_arcs_resolvable() {
        let (model, _, _, _, fillet_face_ids, cylinder_axes, corner_apex, _) =
            build_synthetic_three_cylinder_corner();

        for i in 0..3 {
            let (q, u, _r) = cylinder_axes[i];
            let edge_id =
                find_cap_arc_edge_by_cylinder_axis(&model, fillet_face_ids[i], q, u, corner_apex)
                    .unwrap_or_else(|| {
                        panic!("cap arc lookup failed on synthetic face {}", i)
                    });
            // The picked arc's centre must be the V-side cap centre
            // (= q, since we placed q at C_i), not the far-side one
            // (q + 100·u).
            let edge = model.edges.get(edge_id).expect("edge");
            let curve = model.curves.get(edge.curve_id).expect("curve");
            let arc = curve
                .as_any()
                .downcast_ref::<Arc>()
                .expect("cap arc must be an Arc curve");
            let dx = arc.center.x - q.x;
            let dy = arc.center.y - q.y;
            let dz = arc.center.z - q.z;
            assert!(
                dx * dx + dy * dy + dz * dz < 1e-18,
                "cap arc lookup picked the far-side arc on face {} \
                 (centre = {:?}, expected V-side at {:?})",
                i,
                arc.center,
                q
            );
        }
    }

    /// F5-β.5.5 — the headline test. Feeds the synthetic non-
    /// orthogonal three-cylinder corner directly into
    /// `apply_triangular_nurbs_corner` and pins the four invariants
    /// of a successful NURBS-emission path:
    ///
    /// 1. Exactly one new face is added to the model.
    /// 2. The new face is backed by a `GeneralNurbsSurface`.
    /// 3. The new face's outer loop has exactly three edges (the
    ///    three trimmed cap arcs).
    /// 4. The new face is registered on the solid's outer shell.
    ///
    /// Together (1-4) prove the kernel can close a mixed-radii
    /// triangular corner hole with a rational bi-quadratic NURBS
    /// patch. End-to-end coverage through `fillet_edges` is deferred
    /// to whenever `TopologyBuilder` grows a non-rectilinear solid
    /// constructor (tetrahedron / skew wedge) — see the
    /// `#[ignore]` reason on
    /// `mixed_radii_synthetic_corner_emits_general_nurbs_face` in
    /// `tests/fillet_three_edge_corner_mixed_radii.rs`.
    #[test]
    fn apply_triangular_nurbs_corner_emits_general_nurbs_face_on_synthetic_corner() {
        use crate::primitives::surface::GeneralNurbsSurface;

        let (
            mut model,
            solid_id,
            vertex_id,
            vertex_pos,
            fillet_face_ids,
            cylinder_axes,
            corner_apex,
            vertex_outward,
        ) = build_synthetic_three_cylinder_corner();

        let pre_face_count = model.faces.len();

        let new_face_id = apply_triangular_nurbs_corner(
            &mut model,
            solid_id,
            vertex_id,
            vertex_pos,
            &fillet_face_ids,
            &cylinder_axes,
            corner_apex,
            vertex_outward,
        )
        .expect("synthetic three-cylinder corner must emit a NURBS face");

        // (1) ΔF = +1.
        assert_eq!(
            model.faces.len(),
            pre_face_count + 1,
            "expected exactly one new face after NURBS-corner emission"
        );

        // (2) New face's surface is GeneralNurbsSurface.
        let new_face = model.faces.get(new_face_id).expect("new face exists");
        let surf = model
            .surfaces
            .get(new_face.surface_id)
            .expect("new face surface exists");
        assert!(
            surf.as_any().downcast_ref::<GeneralNurbsSurface>().is_some(),
            "corner face must be backed by GeneralNurbsSurface"
        );

        // (3) Outer loop has exactly three edges (one per trimmed
        // cap arc).
        let outer_loop = model
            .loops
            .get(new_face.outer_loop)
            .expect("new face outer loop exists");
        assert_eq!(
            outer_loop.edges.len(),
            3,
            "corner-patch outer loop must hold three cap-arc edges"
        );

        // (4) The new face is registered on the solid's outer shell.
        let solid = model.solids.get(solid_id).expect("solid exists");
        let shell = model
            .shells
            .get(solid.outer_shell)
            .expect("outer shell exists");
        assert!(
            shell.faces.contains(&new_face_id),
            "new corner face must be registered on solid's outer shell"
        );
    }
}

