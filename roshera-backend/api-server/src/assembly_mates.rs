//! Mate connectors + mates + solve on the instanced-assembly document
//! (kinematic-assembly campaign, Slice 2).
//!
//! The document (`InstancedAssembly`) carries connectors and mates as pure
//! description; this module is the seam that RESOLVES connector anchors
//! against the live kernel model (the durability ladder: PID → label →
//! fingerprint → raw), maps document mates into the assembly-engine's
//! borrowed `SolveInput` view, and writes solved poses back.
//!
//! # Anchoring discipline (spec §3.3)
//!
//! Connector frames are RE-DERIVED from their anchored faces on every
//! solve — an edited bore MOVES its axis and the mate follows (pinned by
//! `mate_follows_relabelled_bore_after_geometry_edit`). A resolution
//! failure marks the connector STALE and the solve REFUSES with typed
//! facts — never a silent re-anchor (the labeller `Holds | Stale`
//! contract). Anchor provenance (pid/label/fingerprint/raw) is reported
//! per connector so degradation (e.g. a fillet face with no PID yet —
//! #11 slices 40-E/F) is visible, not hidden.

use crate::error_catalog::{ApiError, ErrorCode};
use crate::AppState;
use axum::{
    extract::{Path, State},
    response::Json,
};
use geometry_engine::assembly::instancing::{InstanceId, InstancedAssembly};
use geometry_engine::assembly::mates::{
    derive_frame_for_face, fingerprint_for_face, resolve_face_by_fingerprint, AnchorProvenance,
    ConnectorAnchor, ConnectorFrame, DocMate, DocMateId, DocMateKind, MateConnector,
    MateConnectorId,
};
use geometry_engine::math::{Matrix4, Quaternion};
use geometry_engine::operations::recorder::RecordedOperation;
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::persistent_id::PersistentId;
use geometry_engine::primitives::topology_builder::BRepModel;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Wire types ──────────────────────────────────────────────────────

/// How the caller names the face a connector anchors on. Exactly one of
/// the fields must be set.
#[derive(Debug, Clone, Deserialize)]
pub struct FaceSelector {
    /// A label name (label → PID → face; the strongest agent anchor).
    pub label: Option<String>,
    /// A raw persistent id.
    pub pid: Option<u128>,
    /// A live kernel face id. Anchor durability is derived: the face's
    /// PID when it has one, else a geometric FINGERPRINT (degraded —
    /// reported, not hidden).
    pub face_id: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FrameSpec {
    pub origin: [f64; 3],
    pub z_axis: [f64; 3],
    pub x_axis: [f64; 3],
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateConnectorRequest {
    pub instance_id: Uuid,
    /// Anchor on a face (durability ladder) …
    pub face: Option<FaceSelector>,
    /// … or declare an explicit raw frame (datum-style; the engine's
    /// anchor probe is the anti-fabrication check at certify time).
    pub frame: Option<FrameSpec>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectorResponse {
    pub connector_id: Uuid,
    pub instance_id: Uuid,
    pub provenance: AnchorProvenance,
    pub frame: ConnectorFrame,
    pub radius: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateMateRequest {
    pub kind: DocMateKind,
    pub a: Uuid,
    pub b: Uuid,
    /// For coupling kinds: the mate ids whose joint parameters are coupled
    /// (2 for GearRatio/RackPinion, 1 for Screw). The reference joint
    /// parameters (`at`) are captured from the CURRENT configuration.
    pub couples: Option<Vec<Uuid>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PatchMateRequest {
    pub kind: DocMateKind,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SolveRequest {
    /// The grounded instance (never moves). Defaults to the first instance.
    pub ground: Option<Uuid>,
}

/// A connector whose anchor no longer resolves — surfaced, never silently
/// re-anchored.
#[derive(Debug, Clone, Serialize)]
pub struct StaleConnector {
    pub connector_id: Uuid,
    pub reason: String,
}

/// Per-mate fact in a solve response.
#[derive(Debug, Clone, Serialize)]
pub struct MateFact {
    pub mate_id: Uuid,
    pub kind: DocMateKind,
    /// Whether the solver numerically enforces this mate; `reason` names
    /// why not (typed refuse set / feature mismatch / broken coupling).
    pub enforced: bool,
    pub reason: Option<String>,
    /// Residual norm at the SOLVED poses (0 = satisfied).
    pub violation: f64,
    /// Anchor provenance of the two connectors (durability rung).
    pub provenance: [AnchorProvenance; 2],
}

#[derive(Debug, Clone, Serialize)]
pub struct SolveResponse {
    /// False when the solve REFUSED (stale anchors) — typed facts below.
    pub solved: bool,
    pub refused_reason: Option<String>,
    pub stale: Vec<StaleConnector>,
    pub converged: Option<bool>,
    pub iterations: Option<usize>,
    pub residual_norm: Option<f64>,
    pub dof: Option<usize>,
    pub rank: Option<usize>,
    pub mates: Vec<MateFact>,
    /// Solved world transform per instance (written back to the document).
    pub poses: Vec<PoseOut>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PoseOut {
    pub instance_id: Uuid,
    pub transform: [[f64; 4]; 4],
}

// ── Event builders (replayed by timeline-engine `dispatch_assembly`) ──

pub(crate) fn op_connector_add(assembly_id: Uuid, connector: &MateConnector) -> RecordedOperation {
    RecordedOperation::new("assembly.connector_add")
        .with_parameters(serde_json::json!({
            "assembly_id": assembly_id,
            "connector": connector,
        }))
        .with_input_assembly(assembly_id)
}

pub(crate) fn op_connector_remove(assembly_id: Uuid, connector_id: Uuid) -> RecordedOperation {
    RecordedOperation::new("assembly.connector_remove")
        .with_parameters(serde_json::json!({
            "assembly_id": assembly_id,
            "connector_id": connector_id,
        }))
        .with_input_assembly(assembly_id)
}

pub(crate) fn op_mate_add(assembly_id: Uuid, mate: &DocMate) -> RecordedOperation {
    RecordedOperation::new("assembly.mate_add")
        .with_parameters(serde_json::json!({
            "assembly_id": assembly_id,
            "mate": mate,
        }))
        .with_input_assembly(assembly_id)
}

pub(crate) fn op_mate_edit(
    assembly_id: Uuid,
    mate_id: Uuid,
    kind: &DocMateKind,
) -> RecordedOperation {
    RecordedOperation::new("assembly.mate_edit")
        .with_parameters(serde_json::json!({
            "assembly_id": assembly_id,
            "mate_id": mate_id,
            "kind": kind,
        }))
        .with_input_assembly(assembly_id)
}

pub(crate) fn op_mate_remove(assembly_id: Uuid, mate_id: Uuid) -> RecordedOperation {
    RecordedOperation::new("assembly.mate_remove")
        .with_parameters(serde_json::json!({
            "assembly_id": assembly_id,
            "mate_id": mate_id,
        }))
        .with_input_assembly(assembly_id)
}

pub(crate) fn op_solve(assembly_id: Uuid, poses: &[PoseOut]) -> RecordedOperation {
    RecordedOperation::new("assembly.solve")
        .with_parameters(serde_json::json!({
            "assembly_id": assembly_id,
            "poses": poses.iter().map(|p| serde_json::json!({
                "instance_id": p.instance_id,
                "transform": p.transform,
            })).collect::<Vec<_>>(),
        }))
        .with_input_assembly(assembly_id)
}

// ── Core resolution (sync, testable without AppState) ───────────────

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::InvalidParameter, msg.into())
}

/// Does `face` belong to `solid` (outer or inner shells)?
fn face_in_solid(model: &BRepModel, solid_id: u32, face: FaceId) -> bool {
    let Some(solid) = model.solids.get(solid_id) else {
        return false;
    };
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    shells.into_iter().any(|sh| {
        model
            .shells
            .get(sh)
            .is_some_and(|shell| shell.faces.contains(&face))
    })
}

/// Decompose a document transform into a rigid pose (translation + unit
/// quaternion `[x,y,z,w]`). Refuses non-rigid transforms (scale/shear) —
/// the mate solver moves RIGID bodies.
fn rigid_pose_of(m: &Matrix4) -> Result<([f64; 3], [f64; 4]), String> {
    // Orthonormality: R·Rᵀ = I within tolerance, det(R) = +1.
    let r = [
        [m[(0, 0)], m[(0, 1)], m[(0, 2)]],
        [m[(1, 0)], m[(1, 1)], m[(1, 2)]],
        [m[(2, 0)], m[(2, 1)], m[(2, 2)]],
    ];
    const TOL: f64 = 1e-6;
    for i in 0..3 {
        for j in 0..3 {
            let dot: f64 = (0..3).map(|k| r[i][k] * r[j][k]).sum();
            let expect = if i == j { 1.0 } else { 0.0 };
            if (dot - expect).abs() > TOL {
                return Err(format!(
                    "instance transform is not rigid (row {i}·row {j} = {dot:.9}); \
                     the mate solver moves rigid bodies — bake scale into the part"
                ));
            }
        }
    }
    let det = r[0][0] * (r[1][1] * r[2][2] - r[1][2] * r[2][1])
        - r[0][1] * (r[1][0] * r[2][2] - r[1][2] * r[2][0])
        + r[0][2] * (r[1][0] * r[2][1] - r[1][1] * r[2][0]);
    if (det - 1.0).abs() > TOL {
        return Err(format!(
            "instance transform is not a proper rotation (det = {det:.9})"
        ));
    }
    let q = Quaternion::from_matrix4(m);
    Ok(([m[(0, 3)], m[(1, 3)], m[(2, 3)]], [q.x, q.y, q.z, q.w]))
}

/// Re-derive one connector's frame from its anchor against the LIVE model
/// — the durability ladder. `Err(reason)` = STALE.
pub(crate) fn resolve_connector(
    model: &mut BRepModel,
    connector: &MateConnector,
) -> Result<(ConnectorFrame, Option<f64>), String> {
    let face = match &connector.anchor {
        ConnectorAnchor::RawFrame => return Ok((connector.frame, connector.radius)),
        ConnectorAnchor::FacePid { pid } => model
            .face_by_pid(PersistentId(*pid))
            .ok_or_else(|| format!("face PID {pid:#x} no longer resolves to a live face"))?,
        ConnectorAnchor::Label { name } => model
            .resolve_label_face(name)
            .map_err(|e| format!("label '{name}' no longer resolves: {e:?}"))?,
        ConnectorAnchor::Fingerprint {
            position,
            radius,
            size,
            ..
        } => {
            // Position tolerance: generous but local — 5% of the fingerprint
            // position magnitude, floored at 0.5 model units.
            let scale = (position[0].powi(2) + position[1].powi(2) + position[2].powi(2)).sqrt();
            let tol = (0.05 * scale).max(0.5);
            resolve_face_by_fingerprint(model, *position, *radius, *size, tol).ok_or_else(|| {
                "fingerprint anchor no longer matches a unique live face \
                     (moved, deleted, or ambiguous)"
                    .to_string()
            })?
        }
    };
    let derived = derive_frame_for_face(model, face).ok_or_else(|| {
        "anchored face has no analytic connector frame (plane/cylinder/sphere required)".to_string()
    })?;
    Ok((derived.frame, derived.radius))
}

/// Everything the engine needs, resolved from the document + model.
pub(crate) struct EngineView {
    pub poses: Vec<assembly_engine::InputPose>,
    pub mates: Vec<assembly_engine::Mate>,
    pub ground: assembly_engine::InstanceId,
    pub instance_ids: Vec<Uuid>,
    pub stale: Vec<StaleConnector>,
}

/// Build the borrowed solve view: resolve every connector (refreshing the
/// document's stored frames), map document mates onto engine mates, and
/// decompose instance transforms into rigid poses.
pub(crate) fn build_engine_view(
    model: &mut BRepModel,
    doc: &mut InstancedAssembly,
    ground: Option<Uuid>,
) -> Result<EngineView, ApiError> {
    if doc.instance_count() == 0 {
        return Err(bad("assembly has no instances to solve"));
    }
    // Instance index map + rigid poses.
    let mut instance_ids: Vec<Uuid> = Vec::with_capacity(doc.instance_count());
    let mut poses = Vec::with_capacity(doc.instance_count());
    for (idx, inst) in doc.instances().iter().enumerate() {
        let (t, r) = rigid_pose_of(&inst.transform).map_err(bad)?;
        instance_ids.push(inst.id.0);
        poses.push(assembly_engine::InputPose {
            id: assembly_engine::InstanceId(idx as u32),
            translation: t,
            rotation: r,
        });
    }
    let ground_idx = match ground {
        Some(g) => instance_ids
            .iter()
            .position(|id| *id == g)
            .ok_or_else(|| bad(format!("ground instance {g} not found in assembly")))?,
        None => 0,
    };

    // Resolve connectors (durability ladder) and refresh stored frames.
    let mut stale: Vec<StaleConnector> = Vec::new();
    let connector_list: Vec<MateConnector> = doc.connectors().to_vec();
    let mut resolved: std::collections::HashMap<MateConnectorId, (ConnectorFrame, Option<f64>)> =
        std::collections::HashMap::new();
    for connector in &connector_list {
        match resolve_connector(model, connector) {
            Ok((frame, radius)) => {
                doc.set_connector_frame(connector.id, frame, radius);
                resolved.insert(connector.id, (frame, radius));
            }
            Err(reason) => stale.push(StaleConnector {
                connector_id: connector.id.0,
                reason,
            }),
        }
    }

    // Map document mates → engine mates (same order, so coupling indices
    // are document mate positions).
    let mate_index_of = |id: DocMateId| -> Option<u32> {
        doc.mates()
            .iter()
            .position(|m| m.id == id)
            .map(|i| i as u32)
    };
    let mut mates = Vec::with_capacity(doc.mates().len());
    for mate in doc.mates() {
        let (Some(ca), Some(cb)) = (doc.connector(mate.a), doc.connector(mate.b)) else {
            return Err(bad(format!(
                "mate {} references a missing connector",
                mate.id.0
            )));
        };
        let ia = doc
            .instances()
            .iter()
            .position(|i| i.id == ca.instance)
            .ok_or_else(|| bad("connector instance vanished"))?;
        let ib = doc
            .instances()
            .iter()
            .position(|i| i.id == cb.instance)
            .ok_or_else(|| bad("connector instance vanished"))?;
        let (fa, ra) = resolved
            .get(&ca.id)
            .copied()
            .unwrap_or((ca.frame, ca.radius));
        let (fb, rb) = resolved
            .get(&cb.id)
            .copied()
            .unwrap_or((cb.frame, cb.radius));

        // Tangent: the engine kind carries the radius of whichever side is
        // curved; the PLANE side plays frame A. Swap if needed; refuse when
        // neither connector is curved.
        let (kind, swap) =
            engine_kind(&mate.kind, &mate.couples, &mate.at, ra, rb, mate_index_of).map_err(bad)?;
        let (ia, ib, fa, fb) = if swap {
            (ib, ia, fb, fa)
        } else {
            (ia, ib, fa, fb)
        };
        mates.push(assembly_engine::Mate {
            kind,
            a: assembly_engine::InstanceId(ia as u32),
            feature_a: frame_feature(&fa),
            b: assembly_engine::InstanceId(ib as u32),
            feature_b: frame_feature(&fb),
        });
    }

    Ok(EngineView {
        poses,
        mates,
        ground: assembly_engine::InstanceId(ground_idx as u32),
        instance_ids,
        stale,
    })
}

fn frame_feature(f: &ConnectorFrame) -> assembly_engine::FeatureRef {
    assembly_engine::FeatureRef::Frame {
        origin: f.origin,
        z_axis: f.z_axis,
        x_axis: f.x_axis,
    }
}

/// Map a document mate kind onto the engine kind. Returns `(kind,
/// swap_sides)`; `swap_sides` is used by Tangent to put the planar
/// connector on side A.
fn engine_kind(
    kind: &DocMateKind,
    couples: &[DocMateId],
    at: &[f64],
    radius_a: Option<f64>,
    radius_b: Option<f64>,
    mate_index_of: impl Fn(DocMateId) -> Option<u32>,
) -> Result<(assembly_engine::MateKind, bool), String> {
    use assembly_engine::MateKind as EK;
    let couple_idx = |slot: usize| -> Result<u32, String> {
        let id = couples
            .get(slot)
            .copied()
            .ok_or_else(|| format!("coupling requires couples[{slot}]"))?;
        mate_index_of(id).ok_or_else(|| format!("coupled mate {} not found", id.0))
    };
    let at2 = |k: &str| -> Result<[f64; 2], String> {
        if at.len() >= 2 {
            Ok([at[0], at[1]])
        } else {
            Err(format!("{k} coupling is missing its reference parameters"))
        }
    };
    let kind = match kind {
        DocMateKind::Fastened => EK::Fastened,
        DocMateKind::Revolute { limits } => EK::Revolute { limits: *limits },
        DocMateKind::Slider { limits } => EK::Slider { limits: *limits },
        DocMateKind::Cylindrical {
            rot_limits,
            trans_limits,
        } => EK::Cylindrical {
            rot_limits: *rot_limits,
            trans_limits: *trans_limits,
        },
        DocMateKind::Planar => EK::Planar,
        DocMateKind::Ball => EK::Ball,
        DocMateKind::PinSlot { slot_dir_x, limits } => EK::PinSlot {
            slot_dir_x: *slot_dir_x,
            limits: *limits,
        },
        DocMateKind::Distance { value } => EK::Distance { value: *value },
        DocMateKind::Angle { value } => EK::Angle { value: *value },
        DocMateKind::Parallel => EK::Parallel,
        DocMateKind::Tangent => {
            // The curved side supplies the radius; the planar side is frame A.
            return match (radius_a, radius_b) {
                (None, Some(r)) => Ok((EK::Tangent { radius: r }, false)),
                (Some(r), None) => Ok((EK::Tangent { radius: r }, true)),
                (None, None) => Err(
                    "Tangent requires one cylindrical/spherical connector (with a radius) \
                     and one planar connector"
                        .to_string(),
                ),
                (Some(_), Some(_)) => Err(
                    "Tangent between two curved connectors is not supported yet — \
                     refused honestly (plane↔cylinder/sphere only)"
                        .to_string(),
                ),
            };
        }
        DocMateKind::GearRatio { ratio } => EK::GearRatio {
            ratio: *ratio,
            at: at2("GearRatio")?,
            couples: [couple_idx(0)?, couple_idx(1)?],
        },
        DocMateKind::RackPinion { pinion_radius } => EK::RackPinion {
            pinion_radius: *pinion_radius,
            at: at2("RackPinion")?,
            couples: [couple_idx(0)?, couple_idx(1)?],
        },
        DocMateKind::Screw { lead } => EK::Screw {
            lead: *lead,
            at: at2("Screw")?,
            couples: couple_idx(0)?,
        },
        DocMateKind::Cam => EK::Cam,
        DocMateKind::Path => EK::Path,
        DocMateKind::Symmetric => EK::Symmetric,
    };
    Ok((kind, false))
}

/// Create-connector core: resolve the request against the model and store
/// the connector on the document.
pub(crate) fn create_connector_core<F>(
    model: &mut BRepModel,
    resolve_part: F,
    doc: &mut InstancedAssembly,
    req: &CreateConnectorRequest,
) -> Result<MateConnector, ApiError>
where
    F: Fn(Uuid) -> Option<u32>,
{
    let instance = doc
        .instances()
        .iter()
        .find(|i| i.id.0 == req.instance_id)
        .cloned()
        .ok_or_else(|| bad(format!("instance {} not found", req.instance_id)))?;

    let (anchor, frame, radius) = match (&req.face, &req.frame) {
        (Some(_), Some(_)) | (None, None) => {
            return Err(bad(
                "exactly one of `face` (anchored) or `frame` (raw) must be given",
            ));
        }
        (None, Some(spec)) => {
            let z = norm3(spec.z_axis).ok_or_else(|| bad("frame z_axis is degenerate"))?;
            let x0 = norm3(spec.x_axis).ok_or_else(|| bad("frame x_axis is degenerate"))?;
            let dot = z[0] * x0[0] + z[1] * x0[1] + z[2] * x0[2];
            if dot.abs() > 1e-6 {
                return Err(bad(format!(
                    "frame axes must be perpendicular (z·x = {dot:.9})"
                )));
            }
            (
                ConnectorAnchor::RawFrame,
                ConnectorFrame {
                    origin: spec.origin,
                    z_axis: z,
                    x_axis: x0,
                },
                None,
            )
        }
        (Some(sel), None) => {
            let solid = resolve_part(instance.part_id).ok_or_else(|| {
                bad(format!(
                    "instance part {} does not resolve to a live solid",
                    instance.part_id
                ))
            })?;
            let selector_count = [
                sel.label.is_some(),
                sel.pid.is_some(),
                sel.face_id.is_some(),
            ]
            .iter()
            .filter(|b| **b)
            .count();
            if selector_count != 1 {
                return Err(bad(
                    "face selector must set exactly one of label / pid / face_id",
                ));
            }
            let (face, anchor) = if let Some(name) = &sel.label {
                let face = model
                    .resolve_label_face(name)
                    .map_err(|e| bad(format!("label '{name}' does not resolve: {e:?}")))?;
                (face, ConnectorAnchor::Label { name: name.clone() })
            } else if let Some(pid) = sel.pid {
                let face = model
                    .face_by_pid(PersistentId(pid))
                    .ok_or_else(|| bad(format!("PID {pid:#x} does not resolve to a face")))?;
                (face, ConnectorAnchor::FacePid { pid })
            } else {
                // face_id path: durability ladder decides — PID when the
                // face has one, else the DEGRADED fingerprint (reported).
                let fid = sel.face_id.unwrap_or_default() as FaceId;
                if model.faces.get(fid).is_none() {
                    return Err(bad(format!("face {fid} does not exist")));
                }
                match model.face_pid(fid) {
                    Some(pid) => (fid, ConnectorAnchor::FacePid { pid: pid.0 }),
                    None => {
                        let fp = fingerprint_for_face(model, fid).ok_or_else(|| {
                            bad("face has no computable fingerprint to anchor on")
                        })?;
                        (fid, fp)
                    }
                }
            };
            if !face_in_solid(model, solid, face) {
                return Err(bad(format!(
                    "face {face} does not belong to instance {}'s part — a connector \
                     must sit on its own instance",
                    req.instance_id
                )));
            }
            let derived = derive_frame_for_face(model, face).ok_or_else(|| {
                bad("face has no analytic connector frame (plane/cylinder/sphere required)")
            })?;
            (anchor, derived.frame, derived.radius)
        }
    };

    let connector = MateConnector {
        id: MateConnectorId::new(),
        instance: InstanceId(req.instance_id),
        anchor,
        frame,
        radius,
    };
    if !doc.add_connector(connector.clone()) {
        return Err(bad("connector rejected by the document (duplicate id?)"));
    }
    Ok(connector)
}

fn norm3(v: [f64; 3]) -> Option<[f64; 3]> {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if n < 1e-12 {
        return None;
    }
    Some([v[0] / n, v[1] / n, v[2] / n])
}

/// Create-mate core: validate, capture coupling reference parameters from
/// the CURRENT configuration, store on the document.
pub(crate) fn create_mate_core(
    model: &mut BRepModel,
    doc: &mut InstancedAssembly,
    req: &CreateMateRequest,
) -> Result<DocMate, ApiError> {
    let a = MateConnectorId(req.a);
    let b = MateConnectorId(req.b);
    let couples: Vec<DocMateId> = req
        .couples
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(DocMateId)
        .collect();

    let is_coupling = matches!(
        req.kind,
        DocMateKind::GearRatio { .. } | DocMateKind::RackPinion { .. } | DocMateKind::Screw { .. }
    );
    let expected_couples = match req.kind {
        DocMateKind::GearRatio { .. } | DocMateKind::RackPinion { .. } => 2,
        DocMateKind::Screw { .. } => 1,
        _ => 0,
    };
    if couples.len() != expected_couples {
        return Err(bad(format!(
            "{:?} requires exactly {expected_couples} coupled mate id(s), got {}",
            req.kind,
            couples.len()
        )));
    }

    // Capture `at` — the coupled mates' joint parameters at declaration —
    // through the same resolved engine view the solve uses.
    let mut at: Vec<f64> = Vec::new();
    if is_coupling {
        let view = build_engine_view(model, doc, None)?;
        if !view.stale.is_empty() {
            return Err(bad(format!(
                "cannot capture coupling reference parameters: {} stale connector(s)",
                view.stale.len()
            )));
        }
        let mut engine = assembly_engine::Assembly::new(view.ground);
        for pose in &view.poses {
            let mut inst = assembly_engine::Instance::new(
                pose.id,
                format!("i{}", pose.id.0),
                assembly_engine::Mesh::default(),
            );
            inst.translation = pose.translation;
            inst.rotation = pose.rotation;
            engine.add_instance(inst);
        }
        for m in &view.mates {
            engine.add_mate(m.clone());
        }
        for cid in &couples {
            let idx = doc
                .mates()
                .iter()
                .position(|m| m.id == *cid)
                .ok_or_else(|| bad(format!("coupled mate {} not found", cid.0)))?;
            let (theta, s) = engine.joint_parameters_of(idx as u32).ok_or_else(|| {
                bad(format!(
                    "coupled mate {} has no frame pair to read joint parameters from",
                    cid.0
                ))
            })?;
            match req.kind {
                // Gear couples two rotations; RackPinion takes θ from the
                // first and s from the second; Screw takes (θ, s) from one.
                DocMateKind::GearRatio { .. } => at.push(theta),
                DocMateKind::RackPinion { .. } => {
                    if at.is_empty() {
                        at.push(theta)
                    } else {
                        at.push(s)
                    }
                }
                DocMateKind::Screw { .. } => {
                    at.push(theta);
                    at.push(s);
                }
                _ => {}
            }
        }
    }

    let mate = DocMate {
        id: DocMateId::new(),
        kind: req.kind.clone(),
        a,
        b,
        couples,
        at,
    };
    if !doc.add_mate(mate.clone()) {
        return Err(bad(
            "mate rejected by the document: connectors must exist, sit on two DIFFERENT \
             instances, and coupling references must name existing mates",
        ));
    }
    Ok(mate)
}

/// Solve core: resolve, refuse on stale, solve over the borrowed view,
/// write poses back, report typed per-mate facts.
pub(crate) fn solve_core(
    model: &mut BRepModel,
    doc: &mut InstancedAssembly,
    ground: Option<Uuid>,
) -> Result<SolveResponse, ApiError> {
    let view = build_engine_view(model, doc, ground)?;

    // Provenance per mate (before any refusal, so facts are always typed).
    let provenance_of = |mate: &DocMate| -> [AnchorProvenance; 2] {
        let pa = doc
            .connector(mate.a)
            .map(|c| c.anchor.provenance())
            .unwrap_or(AnchorProvenance::Raw);
        let pb = doc
            .connector(mate.b)
            .map(|c| c.anchor.provenance())
            .unwrap_or(AnchorProvenance::Raw);
        [pa, pb]
    };

    if !view.stale.is_empty() {
        let mates = doc
            .mates()
            .iter()
            .map(|m| MateFact {
                mate_id: m.id.0,
                kind: m.kind.clone(),
                enforced: false,
                reason: Some("solve refused: stale connector anchors".to_string()),
                violation: f64::NAN,
                provenance: provenance_of(m),
            })
            .collect();
        return Ok(SolveResponse {
            solved: false,
            refused_reason: Some(format!(
                "{} connector anchor(s) no longer resolve — a mate is never silently \
                 re-anchored (Holds|Stale contract); re-anchor or delete the stale \
                 connectors",
                view.stale.len()
            )),
            stale: view.stale,
            converged: None,
            iterations: None,
            residual_norm: None,
            dof: None,
            rank: None,
            mates,
            poses: Vec::new(),
        });
    }

    // Solve over the borrowed view.
    let input = assembly_engine::SolveInput {
        ground: view.ground,
        poses: &view.poses,
        mates: &view.mates,
    };
    let (report, solved_poses) = input.solved_poses();

    // Post-solve facts: rebuild the meshless engine assembly AT the solved
    // poses for per-mate violations, enforcement, and DOF.
    let mut solved_engine = assembly_engine::Assembly::new(view.ground);
    for pose in &solved_poses {
        let mut inst = assembly_engine::Instance::new(
            pose.instance,
            format!("i{}", pose.instance.0),
            assembly_engine::Mesh::default(),
        );
        inst.translation = pose.translation;
        inst.rotation = pose.rotation;
        solved_engine.add_instance(inst);
    }
    for m in &view.mates {
        solved_engine.add_mate(m.clone());
    }
    let enforcement = solved_engine.mate_enforcement_report();
    let dof = solved_engine.dof_analysis();

    let mates: Vec<MateFact> = doc
        .mates()
        .iter()
        .enumerate()
        .map(|(idx, m)| {
            let violation = solved_engine
                .mates
                .get(idx)
                .map(|em| solved_engine.mate_violation(em))
                .unwrap_or(f64::NAN);
            let (enforced, reason) = enforcement
                .mates
                .get(idx)
                .map(|e| (e.enforced, e.reason.clone()))
                .unwrap_or((false, Some("enforcement not evaluated".to_string())));
            MateFact {
                mate_id: m.id.0,
                kind: m.kind.clone(),
                enforced,
                reason,
                violation,
                provenance: provenance_of(m),
            }
        })
        .collect();

    // Write solved poses back into the document (world transforms).
    let mut poses_out = Vec::with_capacity(solved_poses.len());
    for pose in &solved_poses {
        let uuid = view
            .instance_ids
            .get(pose.instance.0 as usize)
            .copied()
            .ok_or_else(|| bad("solver returned an unknown instance index"))?;
        let q = Quaternion::new(
            pose.rotation[3],
            pose.rotation[0],
            pose.rotation[1],
            pose.rotation[2],
        );
        let mut m = q.to_matrix4();
        m[(0, 3)] = pose.translation[0];
        m[(1, 3)] = pose.translation[1];
        m[(2, 3)] = pose.translation[2];
        doc.transform_instance(InstanceId(uuid), m);
        let mut arr = [[0.0_f64; 4]; 4];
        for r in 0..4 {
            for c in 0..4 {
                arr[r][c] = m[(r, c)];
            }
        }
        poses_out.push(PoseOut {
            instance_id: uuid,
            transform: arr,
        });
    }

    Ok(SolveResponse {
        solved: true,
        refused_reason: None,
        stale: Vec::new(),
        converged: Some(report.converged),
        iterations: Some(report.iterations),
        residual_norm: Some(report.final_residual_norm),
        dof: Some(dof.dof),
        rank: Some(dof.rank),
        mates,
        poses: poses_out,
    })
}

// ── Route handlers ──────────────────────────────────────────────────

fn assembly_not_found(id: Uuid) -> ApiError {
    ApiError::new(
        ErrorCode::SolidNotFound,
        format!("instanced assembly {id} not found"),
    )
}

/// `POST /api/assembly/{id}/connector`
pub async fn create_connector(
    State(state): State<AppState>,
    crate::part_mgr::ActiveModel(model_handle): crate::part_mgr::ActiveModel,
    Path(id): Path<Uuid>,
    Json(req): Json<CreateConnectorRequest>,
) -> Result<Json<ConnectorResponse>, ApiError> {
    let handle = state
        .instanced_assemblies
        .get(&id)
        .ok_or_else(|| assembly_not_found(id))?;
    let mut doc = handle.write().await;
    let mut model = model_handle.write().await;
    let connector =
        create_connector_core(&mut model, |part| state.get_local_id(&part), &mut doc, &req)?;
    drop(model);
    drop(doc);
    state
        .instanced_assemblies
        .record_event(op_connector_add(id, &connector));
    Ok(Json(ConnectorResponse {
        connector_id: connector.id.0,
        instance_id: connector.instance.0,
        provenance: connector.anchor.provenance(),
        frame: connector.frame,
        radius: connector.radius,
    }))
}

/// `DELETE /api/assembly/{id}/connector/{cid}`
pub async fn delete_connector(
    State(state): State<AppState>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let handle = state
        .instanced_assemblies
        .get(&id)
        .ok_or_else(|| assembly_not_found(id))?;
    let mut doc = handle.write().await;
    if !doc.remove_connector(MateConnectorId(cid)) {
        return Err(bad(format!(
            "connector {cid} not found or still referenced by a mate (delete the mate first)"
        )));
    }
    drop(doc);
    state
        .instanced_assemblies
        .record_event(op_connector_remove(id, cid));
    Ok(Json(
        serde_json::json!({ "success": true, "connector_id": cid }),
    ))
}

/// `POST /api/assembly/{id}/mate`
pub async fn create_mate(
    State(state): State<AppState>,
    crate::part_mgr::ActiveModel(model_handle): crate::part_mgr::ActiveModel,
    Path(id): Path<Uuid>,
    Json(req): Json<CreateMateRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let handle = state
        .instanced_assemblies
        .get(&id)
        .ok_or_else(|| assembly_not_found(id))?;
    let mut doc = handle.write().await;
    let mut model = model_handle.write().await;
    let mate = create_mate_core(&mut model, &mut doc, &req)?;
    drop(model);
    drop(doc);
    state
        .instanced_assemblies
        .record_event(op_mate_add(id, &mate));
    Ok(Json(serde_json::json!({
        "mate_id": mate.id.0,
        "kind": mate.kind,
        "a": mate.a.0,
        "b": mate.b.0,
        "at": mate.at,
    })))
}

/// `PATCH /api/assembly/{id}/mate/{mid}` — edit value/limits (the kind).
pub async fn patch_mate(
    State(state): State<AppState>,
    Path((id, mid)): Path<(Uuid, Uuid)>,
    Json(req): Json<PatchMateRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let handle = state
        .instanced_assemblies
        .get(&id)
        .ok_or_else(|| assembly_not_found(id))?;
    let mut doc = handle.write().await;
    if !doc.set_mate_kind(DocMateId(mid), req.kind.clone()) {
        return Err(bad(format!("mate {mid} not found")));
    }
    drop(doc);
    state
        .instanced_assemblies
        .record_event(op_mate_edit(id, mid, &req.kind));
    Ok(Json(
        serde_json::json!({ "success": true, "mate_id": mid, "kind": req.kind }),
    ))
}

/// `DELETE /api/assembly/{id}/mate/{mid}`
pub async fn delete_mate(
    State(state): State<AppState>,
    Path((id, mid)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let handle = state
        .instanced_assemblies
        .get(&id)
        .ok_or_else(|| assembly_not_found(id))?;
    let mut doc = handle.write().await;
    if !doc.remove_mate(DocMateId(mid)) {
        return Err(bad(format!(
            "mate {mid} not found or referenced by a coupling (remove the coupling first)"
        )));
    }
    drop(doc);
    state
        .instanced_assemblies
        .record_event(op_mate_remove(id, mid));
    Ok(Json(serde_json::json!({ "success": true, "mate_id": mid })))
}

// (tests at the bottom of this file)

/// `POST /api/assembly/{id}/solve`
pub async fn solve(
    State(state): State<AppState>,
    crate::part_mgr::ActiveModel(model_handle): crate::part_mgr::ActiveModel,
    Path(id): Path<Uuid>,
    body: Option<Json<SolveRequest>>,
) -> Result<Json<SolveResponse>, ApiError> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let handle = state
        .instanced_assemblies
        .get(&id)
        .ok_or_else(|| assembly_not_found(id))?;
    let mut doc = handle.write().await;
    let mut model = model_handle.write().await;
    let response = solve_core(&mut model, &mut doc, req.ground)?;
    drop(model);
    drop(doc);
    if response.solved {
        state
            .instanced_assemblies
            .record_event(op_solve(id, &response.poses));
    }
    Ok(Json(response))
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use geometry_engine::labels::{Fingerprint, Label, LabelAssertion, LabelKind, LabelTarget};
    use geometry_engine::math::{Point3, Vector3};
    use geometry_engine::primitives::surface::Cylinder;
    use geometry_engine::primitives::topology_builder::{GeometryId, TopologyBuilder};
    use std::collections::HashMap;

    fn solid_id_of(g: GeometryId) -> u32 {
        match g {
            GeometryId::Solid(s) => s,
            other => {
                assert!(false, "expected a solid, got {other:?}");
                0
            }
        }
    }

    /// The cylindrical SIDE face of a solid (the bore/boss wall).
    fn cylindrical_face(model: &BRepModel, solid: u32) -> Option<FaceId> {
        let s = model.solids.get(solid)?;
        let mut shells = vec![s.outer_shell];
        shells.extend_from_slice(&s.inner_shells);
        for sh in shells {
            let shell = model.shells.get(sh)?;
            for &fid in &shell.faces {
                let face = model.faces.get(fid)?;
                if let Some(surf) = model.surfaces.get(face.surface_id) {
                    if surf.as_any().downcast_ref::<Cylinder>().is_some() {
                        return Some(fid);
                    }
                }
            }
        }
        None
    }

    /// Any planar face of a solid.
    fn planar_face(model: &BRepModel, solid: u32) -> Option<FaceId> {
        use geometry_engine::primitives::surface::Plane;
        let s = model.solids.get(solid)?;
        let shell = model.shells.get(s.outer_shell)?;
        for &fid in &shell.faces {
            let face = model.faces.get(fid)?;
            if let Some(surf) = model.surfaces.get(face.surface_id) {
                if surf.as_any().downcast_ref::<Plane>().is_some() {
                    return Some(fid);
                }
            }
        }
        None
    }

    struct Rig {
        model: BRepModel,
        doc: InstancedAssembly,
        parts: HashMap<Uuid, u32>,
        host_instance: Uuid,
        bracket_instance: Uuid,
        host_solid: u32,
    }

    /// Ground host = a Ø8×20 boss (cylinder), bracket = a 4mm cube.
    fn boss_and_bracket() -> Rig {
        let mut model = BRepModel::new();
        let (boss, cube) = {
            let mut b = TopologyBuilder::new(&mut model);
            let boss = b
                .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, 4.0, 20.0)
                .map(solid_id_of);
            let cube = b.create_box_3d(4.0, 4.0, 4.0).map(solid_id_of);
            (boss, cube)
        };
        let (Ok(boss), Ok(cube)) = (boss, cube) else {
            unreachable!("primitive construction is infallible here");
        };
        let host_part = Uuid::new_v4();
        let bracket_part = Uuid::new_v4();
        let mut parts = HashMap::new();
        parts.insert(host_part, boss);
        parts.insert(bracket_part, cube);
        let mut doc = InstancedAssembly::new("durability-rig");
        let host_instance = doc
            .add_instance(host_part, Matrix4::IDENTITY, Some("host".into()))
            .0;
        let mut away = Matrix4::IDENTITY;
        away[(0, 3)] = 30.0; // bracket starts far off the boss axis
        let bracket_instance = doc
            .add_instance(bracket_part, away, Some("bracket".into()))
            .0;
        Rig {
            model,
            doc,
            parts,
            host_instance,
            bracket_instance,
            host_solid: boss,
        }
    }

    /// Label the boss wall and mate the bracket to it by LABEL. Returns
    /// (host connector id, mate id).
    fn label_and_mate(rig: &mut Rig) -> (Uuid, Uuid) {
        let wall = cylindrical_face(&rig.model, rig.host_solid);
        let Some(wall) = wall else {
            unreachable!("a cylinder always has its side wall");
        };
        let pid = rig.model.face_pid(wall);
        let Some(pid) = pid else {
            unreachable!("primitive faces mint PIDs");
        };
        let attach = rig.model.labels.attach(
            "boss_wall",
            Label {
                target: LabelTarget::Entity {
                    kind: LabelKind::Face,
                    pid,
                },
                assertion: Some(LabelAssertion::Fingerprint(Fingerprint {
                    kind: LabelKind::Face,
                    position: [0.0, 0.0, 10.0],
                    normal: None,
                    radius: Some(4.0),
                    size: None,
                })),
                description: None,
            },
        );
        assert!(attach.is_ok(), "label attach failed: {attach:?}");

        let parts = rig.parts.clone();
        let conn_host = create_connector_core(
            &mut rig.model,
            |p| parts.get(&p).copied(),
            &mut rig.doc,
            &CreateConnectorRequest {
                instance_id: rig.host_instance,
                face: Some(FaceSelector {
                    label: Some("boss_wall".into()),
                    pid: None,
                    face_id: None,
                }),
                frame: None,
            },
        );
        let Ok(conn_host) = conn_host else {
            unreachable!("host connector must resolve: {conn_host:?}");
        };
        assert_eq!(conn_host.anchor.provenance(), AnchorProvenance::Label);
        assert_eq!(conn_host.radius, Some(4.0), "boss wall radius carried");

        let conn_bracket = create_connector_core(
            &mut rig.model,
            |p| parts.get(&p).copied(),
            &mut rig.doc,
            &CreateConnectorRequest {
                instance_id: rig.bracket_instance,
                face: None,
                frame: Some(FrameSpec {
                    origin: [0.0, 0.0, 0.0],
                    z_axis: [0.0, 0.0, 1.0],
                    x_axis: [1.0, 0.0, 0.0],
                }),
            },
        );
        let Ok(conn_bracket) = conn_bracket else {
            unreachable!("raw connector is always valid: {conn_bracket:?}");
        };

        let mate = create_mate_core(
            &mut rig.model,
            &mut rig.doc,
            &CreateMateRequest {
                kind: DocMateKind::Cylindrical {
                    rot_limits: None,
                    trans_limits: None,
                },
                a: conn_host.id.0,
                b: conn_bracket.id.0,
                couples: None,
            },
        );
        let Ok(mate) = mate else {
            unreachable!("cylindrical mate must be accepted: {mate:?}");
        };
        (conn_host.id.0, mate.id.0)
    }

    fn bracket_xy(rig: &Rig) -> (f64, f64) {
        let inst = rig
            .doc
            .instances()
            .iter()
            .find(|i| i.id.0 == rig.bracket_instance)
            .cloned();
        match inst {
            Some(i) => (i.transform[(0, 3)], i.transform[(1, 3)]),
            None => (f64::NAN, f64::NAN),
        }
    }

    #[test]
    fn mate_follows_relabelled_bore_after_geometry_edit() {
        // THE durability RED (spec Slice 2 gate): mate a bracket to a boss
        // wall BY LABEL, solve; then MOVE the boss geometry (the part edit)
        // and re-solve — the connector frame is RE-DERIVED through
        // label → PID → face at solve time, so the mate FOLLOWS the moved
        // axis. Impossible with frozen coordinates (§2.6): the pre-fix
        // behaviour (a stored frame never re-derived) leaves the bracket
        // at the ORIGINAL axis.
        let mut rig = boss_and_bracket();
        label_and_mate(&mut rig);

        let host = rig.host_instance;
        let first = solve_core(&mut rig.model, &mut rig.doc, Some(host));
        let Ok(first) = first else {
            unreachable!("solve must run: {first:?}");
        };
        assert!(first.solved && first.converged == Some(true), "{first:?}");
        let (x0, y0) = bracket_xy(&rig);
        assert!(
            x0.abs() < 1e-6 && y0.abs() < 1e-6,
            "bracket seated on the boss axis at the origin, got ({x0}, {y0})"
        );

        // The part edit: shift the boss geometry +12 in x. Face PIDs are
        // untouched by a transform — identity survives, geometry moves.
        let mut shift = Matrix4::IDENTITY;
        shift[(0, 3)] = 12.0;
        let moved = geometry_engine::operations::transform::transform_solid(
            &mut rig.model,
            rig.host_solid,
            shift,
            geometry_engine::operations::transform::TransformOptions::default(),
        );
        assert!(moved.is_ok(), "transform_solid failed: {moved:?}");

        let second = solve_core(&mut rig.model, &mut rig.doc, Some(host));
        let Ok(second) = second else {
            unreachable!("re-solve must run: {second:?}");
        };
        assert!(
            second.solved && second.converged == Some(true),
            "{second:?}"
        );
        let (x1, y1) = bracket_xy(&rig);
        assert!(
            (x1 - 12.0).abs() < 1e-6 && y1.abs() < 1e-6,
            "the mate must FOLLOW the moved bore axis to x=12, got ({x1}, {y1})"
        );
    }

    #[test]
    fn stale_label_refuses_solve_with_typed_facts() {
        // Anchor durability's other half: when the anchor STOPS resolving,
        // the solve REFUSES with named stale connectors — never a silent
        // re-anchor, never a solve against a frozen frame.
        let mut rig = boss_and_bracket();
        label_and_mate(&mut rig);
        rig.model.labels.remove("boss_wall");

        let host = rig.host_instance;
        let outcome = solve_core(&mut rig.model, &mut rig.doc, Some(host));
        let Ok(outcome) = outcome else {
            unreachable!("refusal is a RESPONSE, not an error: {outcome:?}");
        };
        assert!(!outcome.solved, "stale anchor must refuse the solve");
        assert_eq!(outcome.stale.len(), 1);
        assert!(
            outcome.stale[0].reason.contains("boss_wall"),
            "the stale fact names the label: {:?}",
            outcome.stale[0]
        );
        assert!(outcome.poses.is_empty(), "no poses from a refused solve");
    }

    #[test]
    fn blend_face_anchor_degrades_to_fingerprint_and_is_reported() {
        // Anchor-provenance degradation RED (spec Slice 2 gate): faces of
        // ops that don't mint PID lineage yet (#11 slices 40-E/F —
        // fillet/chamfer/PATTERN) force the anchor to degrade to a
        // FINGERPRINT and say so — never silently pretend PID durability.
        // The carrier is a PATTERN-copied planar face (analytic frame
        // exists, no PID); fillet/chamfer blends are additionally
        // non-analytic surfaces and are asserted below as the honest
        // REFUSAL case.
        let mut model = BRepModel::new();
        let boxed = {
            let mut b = TopologyBuilder::new(&mut model);
            b.create_box_3d(10.0, 10.0, 10.0).map(solid_id_of)
        };
        let Ok(solid) = boxed else {
            unreachable!("box builds");
        };
        // Loft a square prism next to the box: loft is OUTSIDE the
        // PID-minting set (primitives/extrude/revolve/boolean mint; loft
        // does not — the same #11 40-E/F lineage gap as fillet/chamfer/
        // pattern), so its planar CAP faces are live analytic faces with
        // NO persistent id — exactly the degradation state.
        let lofted = {
            use geometry_engine::primitives::curve::Line;
            use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
            let mut square = |z: f64| -> Vec<geometry_engine::primitives::edge::EdgeId> {
                let h = 3.0;
                let v0 = model.vertices.add(20.0 - h, -h, z);
                let v1 = model.vertices.add(20.0 + h, -h, z);
                let v2 = model.vertices.add(20.0 + h, h, z);
                let v3 = model.vertices.add(20.0 - h, h, z);
                let mut line_edge = |a: u32, b: u32| {
                    let pa = model
                        .vertices
                        .get(a)
                        .map(|v| v.point())
                        .unwrap_or(Point3::ORIGIN);
                    let pb = model
                        .vertices
                        .get(b)
                        .map(|v| v.point())
                        .unwrap_or(Point3::ORIGIN);
                    let cid = model.curves.add(Box::new(Line::new(pa, pb)));
                    model
                        .edges
                        .add(Edge::new_auto_range(0, a, b, cid, EdgeOrientation::Forward))
                };
                vec![
                    line_edge(v0, v1),
                    line_edge(v1, v2),
                    line_edge(v2, v3),
                    line_edge(v3, v0),
                ]
            };
            let p0 = square(0.0);
            let p1 = square(8.0);
            geometry_engine::operations::loft::loft_profiles(
                &mut model,
                vec![p0, p1],
                geometry_engine::operations::loft::LoftOptions::default(),
            )
        };
        let Ok(lofted) = lofted else {
            unreachable!("square loft builds: {lofted:?}");
        };
        let _ = lofted;
        // Find a PID-less planar face and the solid that owns it.
        let is_planar = |m: &BRepModel, f: geometry_engine::primitives::face::FaceId| {
            m.faces.get(f).is_some_and(|face| {
                m.surfaces.get(face.surface_id).is_some_and(|surf| {
                    surf.as_any()
                        .downcast_ref::<geometry_engine::primitives::surface::Plane>()
                        .is_some()
                })
            })
        };
        let mut found: Option<(u32, FaceId)> = None;
        let solid_ids: Vec<u32> = model.solids.iter().map(|(sid, _)| sid).collect();
        'outer: for sid in solid_ids {
            let Some(s) = model.solids.get(sid) else {
                continue;
            };
            let mut shells = vec![s.outer_shell];
            shells.extend_from_slice(&s.inner_shells);
            for sh in shells {
                let Some(shell) = model.shells.get(sh) else {
                    continue;
                };
                for &f in &shell.faces {
                    if model.face_pid(f).is_none() && is_planar(&model, f) {
                        found = Some((sid, f));
                        break 'outer;
                    }
                }
            }
        }
        let Some((owner, copied)) = found else {
            let n_solids = model.solids.iter().count();
            let n_faces = model.faces.iter().count();
            let n_pidless = model
                .faces
                .iter()
                .filter(|(f, _)| model.face_pid(*f).is_none())
                .count();
            unreachable!(
                "the lofted solid carries PID-less planar caps today (loft is outside \
                 the #11 minting set) — when this starts failing, loft PID minting \
                 landed and this test should assert Pid provenance instead \
                 [probe: solids={n_solids} faces={n_faces} pidless={n_pidless}]"
            );
        };

        let part = Uuid::new_v4();
        let mut doc = InstancedAssembly::new("degradation-rig");
        let inst = doc.add_instance(part, Matrix4::IDENTITY, None).0;
        let connector = create_connector_core(
            &mut model,
            |p| if p == part { Some(owner) } else { None },
            &mut doc,
            &CreateConnectorRequest {
                instance_id: inst,
                face: Some(FaceSelector {
                    label: None,
                    pid: None,
                    face_id: Some(copied),
                }),
                frame: None,
            },
        );
        let Ok(connector) = connector else {
            unreachable!("pattern-face connector must be accepted: {connector:?}");
        };
        assert_eq!(
            connector.anchor.provenance(),
            AnchorProvenance::Fingerprint,
            "degraded provenance must be REPORTED, not hidden"
        );

        // Contrast: a pristine box face carries a PID → Pid provenance.
        let plain = planar_face(&model, solid);
        let Some(plain) = plain else {
            unreachable!("box has planar faces");
        };
        let pid_connector = create_connector_core(
            &mut model,
            |p| if p == part { Some(solid) } else { None },
            &mut doc,
            &CreateConnectorRequest {
                instance_id: inst,
                face: Some(FaceSelector {
                    label: None,
                    pid: None,
                    face_id: Some(plain),
                }),
                frame: None,
            },
        );
        let Ok(pid_connector) = pid_connector else {
            unreachable!("plain-face connector resolves: {pid_connector:?}");
        };
        assert_eq!(pid_connector.anchor.provenance(), AnchorProvenance::Pid);
    }

    #[test]
    fn non_analytic_blend_face_is_refused_typed() {
        // A fillet blend surface carries no analytic connector frame
        // (plane/cylinder/sphere) in the current kernel — a connector on
        // it must REFUSE with a typed error, never fabricate a frame.
        let mut model = BRepModel::new();
        let boxed = {
            let mut b = TopologyBuilder::new(&mut model);
            b.create_box_3d(10.0, 10.0, 10.0).map(solid_id_of)
        };
        let Ok(solid) = boxed else {
            unreachable!("box builds");
        };
        let edge = model.edges.iter().map(|(id, _)| id).next();
        let Some(edge) = edge else {
            unreachable!("a box has edges");
        };
        let blends = geometry_engine::operations::fillet::fillet_edges(
            &mut model,
            solid,
            vec![edge],
            geometry_engine::operations::fillet::FilletOptions {
                radius: 1.0,
                ..Default::default()
            },
        );
        let Ok(blends) = blends else {
            unreachable!("fillet on a box edge succeeds: {blends:?}");
        };
        let blend = blends.first().copied();
        let Some(blend) = blend else {
            unreachable!("fillet returns its blend faces");
        };
        let part = Uuid::new_v4();
        let mut doc = InstancedAssembly::new("refusal-rig");
        let inst = doc.add_instance(part, Matrix4::IDENTITY, None).0;
        let connector = create_connector_core(
            &mut model,
            |p| if p == part { Some(solid) } else { None },
            &mut doc,
            &CreateConnectorRequest {
                instance_id: inst,
                face: Some(FaceSelector {
                    label: None,
                    pid: None,
                    face_id: Some(blend),
                }),
                frame: None,
            },
        );
        match connector {
            Err(e) => assert!(
                e.error.contains("no analytic connector frame"),
                "typed refusal names the reason: {e:?}"
            ),
            Ok(c) => {
                // If the kernel starts producing analytic cylindrical blend
                // surfaces, the connector becomes legitimate — then the
                // frame must be genuinely cylindrical (radius carried).
                assert!(
                    c.radius.is_some(),
                    "an accepted blend connector must carry its radius: {c:?}"
                );
            }
        }
    }

    #[test]
    fn refused_kind_returns_typed_fact_from_solve() {
        // Slice-2 gate: refusal paths return TYPED facts. A Cam mate rides
        // through the solve response with enforced=false + reason, and the
        // solve still reports honestly on everything else.
        let mut rig = boss_and_bracket();
        let (host_conn, _mate) = label_and_mate(&mut rig);
        // Second connector on the bracket for the Cam declaration.
        let parts = rig.parts.clone();
        let extra = create_connector_core(
            &mut rig.model,
            |p| parts.get(&p).copied(),
            &mut rig.doc,
            &CreateConnectorRequest {
                instance_id: rig.bracket_instance,
                face: None,
                frame: Some(FrameSpec {
                    origin: [0.0, 0.0, 2.0],
                    z_axis: [0.0, 0.0, 1.0],
                    x_axis: [1.0, 0.0, 0.0],
                }),
            },
        );
        let Ok(extra) = extra else {
            unreachable!("raw connector valid: {extra:?}");
        };
        let cam = create_mate_core(
            &mut rig.model,
            &mut rig.doc,
            &CreateMateRequest {
                kind: DocMateKind::Cam,
                a: host_conn,
                b: extra.id.0,
                couples: None,
            },
        );
        let Ok(cam) = cam else {
            unreachable!("Cam is TYPED (declarable): {cam:?}");
        };

        let host = rig.host_instance;
        let outcome = solve_core(&mut rig.model, &mut rig.doc, Some(host));
        let Ok(outcome) = outcome else {
            unreachable!("solve runs: {outcome:?}");
        };
        assert!(outcome.solved);
        let cam_fact = outcome.mates.iter().find(|f| f.mate_id == cam.id.0);
        let Some(cam_fact) = cam_fact else {
            unreachable!("every mate gets a fact");
        };
        assert!(!cam_fact.enforced, "Cam must be refused: {cam_fact:?}");
        assert!(
            cam_fact
                .reason
                .as_deref()
                .is_some_and(|r| r.contains("not numerically enforced")),
            "refusal reason is typed and named: {cam_fact:?}"
        );
        // The healthy cylindrical mate is enforced and satisfied.
        let healthy = outcome.mates.iter().find(|f| f.mate_id != cam.id.0);
        assert!(
            healthy.is_some_and(|f| f.enforced && f.violation < 1e-6),
            "the cylindrical mate stays enforced + satisfied: {:?}",
            outcome.mates
        );
    }

    #[test]
    fn non_rigid_instance_transform_is_refused() {
        let mut rig = boss_and_bracket();
        label_and_mate(&mut rig);
        // Scale the bracket's transform — no longer rigid.
        let mut scaled = Matrix4::IDENTITY;
        scaled[(0, 0)] = 2.0;
        let bracket = rig.bracket_instance;
        assert!(rig.doc.transform_instance(InstanceId(bracket), scaled));
        let host = rig.host_instance;
        let outcome = solve_core(&mut rig.model, &mut rig.doc, Some(host));
        assert!(
            outcome.is_err(),
            "a scaled instance transform must be refused with a typed error"
        );
    }

    #[test]
    fn mate_events_replay_into_identical_documents() {
        // Slice-2 event round-trip: connector/mate/solve events emitted by
        // this surface rebuild the document through timeline replay — the
        // slice-1 pin extended to the mate layer.
        let mut rig = boss_and_bracket();
        let (_conn, mate_id) = label_and_mate(&mut rig);
        let host = rig.host_instance;
        let outcome = solve_core(&mut rig.model, &mut rig.doc, Some(host));
        let Ok(outcome) = outcome else {
            unreachable!("solve runs: {outcome:?}");
        };
        assert!(outcome.solved);

        // Emit the exact production ops for the whole session.
        let doc = &rig.doc;
        let aid = doc.id;
        let mut ops: Vec<RecordedOperation> =
            vec![crate::assembly_instances::op_create(aid, &doc.name)];
        for inst in doc.instances() {
            ops.push(crate::assembly_instances::op_add_instance(aid, inst));
        }
        for connector in doc.connectors() {
            ops.push(op_connector_add(aid, connector));
        }
        for mate in doc.mates() {
            ops.push(op_mate_add(aid, mate));
        }
        ops.push(op_solve(aid, &outcome.poses));

        let events: Vec<timeline_engine::TimelineEvent> = ops
            .iter()
            .enumerate()
            .map(|(i, op)| timeline_engine::TimelineEvent {
                id: timeline_engine::EventId(Uuid::new_v4()),
                sequence_number: i as u64,
                timestamp: chrono::Utc::now(),
                author: timeline_engine::Author::System,
                operation: timeline_engine::Operation::Generic {
                    command_type: op.kind.clone(),
                    parameters: serde_json::json!({
                        "params": op.parameters,
                        "inputs": op.inputs,
                        "outputs": op.outputs,
                    }),
                },
                inputs: Default::default(),
                outputs: Default::default(),
                metadata: Default::default(),
            })
            .collect();

        let mut scratch = geometry_engine::primitives::topology_builder::BRepModel::new();
        let replayed = timeline_engine::rebuild_model_from_events(&mut scratch, &events);
        assert_eq!(
            replayed.events_skipped, 0,
            "every mate-layer event must replay"
        );
        let rebuilt = replayed.assemblies.get(&aid).cloned();
        let Some(rebuilt) = rebuilt else {
            unreachable!("document reconstructed");
        };
        assert_eq!(rebuilt.connectors().len(), doc.connectors().len());
        assert_eq!(rebuilt.mates().len(), doc.mates().len());
        assert!(rebuilt
            .mate(geometry_engine::assembly::mates::DocMateId(mate_id))
            .is_some());
        // Solved poses survive the replay byte-for-byte.
        for inst in doc.instances() {
            let r = rebuilt.instance(inst.id).cloned();
            let Some(r) = r else {
                unreachable!("instance {} reconstructed", inst.id.0);
            };
            for row in 0..4 {
                for col in 0..4 {
                    assert!(
                        (r.transform[(row, col)] - inst.transform[(row, col)]).abs() < 1e-12,
                        "transform cell ({row},{col}) must round-trip"
                    );
                }
            }
        }
    }
}
