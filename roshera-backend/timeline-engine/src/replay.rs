//! Replay a sequence of timeline events back into a fresh `BRepModel`.
//!
//! ## Why this module exists
//!
//! `Timeline::add_operation` is a *write-only* ledger by itself — events go
//! in, but rebuilding the kernel state from those events is a separate
//! concern that lives here. The api-server's `/api/timeline/replay`,
//! `/undo`, and `/redo` handlers all need a way to take a chronologically
//! ordered slice of [`crate::types::TimelineEvent`] and re-execute each
//! `Operation::Generic { command_type, parameters }` against a real
//! `BRepModel`, exactly the way the original kernel call would have.
//!
//! ## Mapping back to kernel calls
//!
//! Every successful kernel operation passes through `BRepModel::record_operation`,
//! which forwards a [`geometry_engine::operations::recorder::RecordedOperation`]
//! to the attached recorder (`TimelineRecorder` in production). The bridge
//! turns that into `Operation::Generic { command_type: kind, parameters:
//! { params, inputs, outputs } }` — see [`crate::recorder_bridge::to_timeline_operation`].
//!
//! Replay reverses that mapping: it routes on `command_type` and reads the
//! original `params` payload to reconstruct the kernel call. New entity
//! IDs are kept in an `id_remap` table so subsequent operations that
//! reference earlier outputs (e.g. boolean operands, fillet edges) can be
//! routed to the freshly created topology rather than to dangling
//! original-IDs from the recorded log.
//!
//! ## Coverage
//!
//! Every operation kind the kernel currently emits is handled where the
//! recorded payload is sufficient to rebuild the call:
//!
//! - **Primitives** (via `TopologyBuilder`): `create_{point,line,circle,
//!   rectangle}_2d`, `create_{box,sphere,cylinder,cone,plane}_3d`
//! - **Direct ops**: `extrude_face`, `revolve_face`,
//!   `boolean_{union,intersection,difference}`, `fillet_edges`,
//!   `chamfer_edges`, `transform_{solid,faces,edges}`
//! - **Lossy-record ops**: `sweep_profile` and `loft_profiles` are
//!   skipped with a structured error because the kernel currently records
//!   profile *edges*, not the parent profile *face* — which is what the
//!   replay would need. Tracking this as a future kernel-side fix.
//! - Anything else is logged via `tracing::warn!` and counted as
//!   `events_skipped`. Replay never panics on an unknown kind.
//!
//! ## Recorder detachment
//!
//! Replay temporarily detaches whatever recorder is on the model so that
//! re-applying events does not double-record them into the timeline. The
//! original recorder is reattached before the function returns.

use std::collections::HashMap;

use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::{
    boolean::{boolean_operation, BooleanOp, BooleanOptions},
    chamfer::{chamfer_edges, ChamferOptions, ChamferType},
    extrude::{extrude_face, ExtrudeOptions},
    fillet::{fillet_edges, FilletOptions, FilletType},
    revolve::{revolve_face, RevolveOptions},
    transform::{transform_edges, transform_faces, transform_solid, TransformOptions},
};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use serde_json::Value;

use crate::types::{Operation, TimelineEvent};

/// Errors that can be raised while replaying a single timeline event.
///
/// The top-level [`rebuild_model_from_events`] function does **not**
/// propagate these — it logs them and continues. Callers that want
/// per-event diagnostics should iterate themselves and call
/// [`apply_event`] directly.
#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    /// `Operation::Generic.command_type` did not match any known kernel
    /// operation. The dispatcher logs and skips.
    #[error("unknown operation kind: {0}")]
    UnknownKind(String),

    /// The recorded `params` payload is missing a required field or has
    /// an unexpected shape (e.g. wrong type, malformed enum stringly).
    #[error("invalid parameters for {kind}: {reason}")]
    InvalidParameters {
        /// The operation kind that failed.
        kind: String,
        /// Human-readable detail about what was wrong.
        reason: String,
    },

    /// The kernel rejected the replayed call (e.g. missing parent solid,
    /// degenerate input). The original error message is preserved for
    /// debugging.
    #[error("kernel rejected operation {kind}: {message}")]
    KernelError {
        /// The operation kind whose kernel call failed.
        kind: String,
        /// String form of the kernel-side error.
        message: String,
    },
}

/// Outcome of a [`rebuild_model_from_events`] run.
#[derive(Debug, Clone, Default)]
pub struct ReplayOutcome {
    /// Number of events that successfully re-executed against the model.
    pub events_applied: usize,
    /// Number of events that were skipped (unknown kind, invalid params,
    /// or kernel rejection). See `tracing::warn!` for per-event detail.
    pub events_skipped: usize,
    /// Final remap from original-recorded entity IDs to current-model
    /// entity IDs. Useful for callers who want to translate event-log
    /// references (e.g. an event's `outputs.created`) into live IDs.
    pub id_remap: HashMap<u64, u64>,
}

/// Replay a chronologically ordered slice of events into the given model.
///
/// The model is mutated in place. Callers that want a "from scratch"
/// rebuild should pass a freshly constructed `BRepModel::new()`.
///
/// Returns an aggregate [`ReplayOutcome`]. Per-event failures are logged
/// via `tracing::warn!` (target: `timeline.replay`) and counted as
/// skipped — replay never aborts on a single bad event.
///
/// The recorder currently attached to `model` (if any) is detached for
/// the duration of the replay so that re-executed operations do not
/// double-record into the timeline; it is reattached before this
/// function returns.
pub fn rebuild_model_from_events(
    model: &mut BRepModel,
    events: &[TimelineEvent],
) -> ReplayOutcome {
    // Detach any attached recorder so replayed operations do not
    // double-record. We reattach unconditionally before returning so the
    // caller's recorder wiring is preserved.
    let saved_recorder = model.attach_recorder(None);

    let mut outcome = ReplayOutcome::default();

    for event in events {
        match apply_event(model, event, &mut outcome.id_remap) {
            Ok(()) => outcome.events_applied += 1,
            Err(err) => {
                tracing::warn!(
                    target: "timeline.replay",
                    event_id = %event.id,
                    sequence = event.sequence_number,
                    error = %err,
                    "replay step failed; skipping"
                );
                outcome.events_skipped += 1;
            }
        }
    }

    // Reattach. If `saved_recorder` was None we still call this so the
    // model ends up in the exact state we found it in.
    let _ = model.attach_recorder(saved_recorder);

    outcome
}

/// Apply a single event to the model, threading the entity-ID remap.
///
/// Only `Operation::Generic` is dispatched — that is the canonical
/// envelope the kernel's recorder bridge emits. Other `Operation`
/// variants are produced solely by the api-server's DTO layer and have
/// no replay path here.
pub fn apply_event(
    model: &mut BRepModel,
    event: &TimelineEvent,
    id_remap: &mut HashMap<u64, u64>,
) -> Result<(), ReplayError> {
    match &event.operation {
        Operation::Generic {
            command_type,
            parameters,
        } => dispatch_generic(model, command_type, parameters, id_remap),
        other => Err(ReplayError::UnknownKind(format!(
            "non-Generic operation variant: {:?}",
            std::mem::discriminant(other)
        ))),
    }
}

/// Dispatch on the kernel-side `kind` string emitted by the recorder
/// bridge. Each arm reads the recorded `params` payload and reconstructs
/// the original kernel call.
fn dispatch_generic(
    model: &mut BRepModel,
    kind: &str,
    parameters: &Value,
    id_remap: &mut HashMap<u64, u64>,
) -> Result<(), ReplayError> {
    // The recorder bridge wraps the original payload as
    // `{ "params": <inner>, "inputs": [...], "outputs": [...] }`. The
    // `outputs` list pairs positionally with the new IDs the kernel
    // returns so we can populate the remap.
    let inner = parameters.get("params").unwrap_or(parameters);
    let recorded_outputs: Vec<u64> = parameters
        .get("outputs")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_u64()).collect())
        .unwrap_or_default();

    // Translate a recorded ID into the live-model ID, falling back to
    // the original ID when no remap entry exists (first-reference case
    // where the recorder didn't observe the producer).
    let remap_id =
        |id: u64, remap: &HashMap<u64, u64>| -> u64 { *remap.get(&id).unwrap_or(&id) };

    match kind {
        // ----------------------------------------------------------------
        // 2D primitives — recorded as `TimelineOperation::Create2D` (an
        // externally-tagged enum) with parameters in a HashMap<String,f64>.
        // ----------------------------------------------------------------
        "create_point_2d" => {
            let params = extract_create_params(inner, "Create2D")?;
            let x = num_field(params, "x", kind)?;
            let y = num_field(params, "y", kind)?;
            let mut builder = TopologyBuilder::new(model);
            let id = builder
                .create_point_2d(x, y)
                .map_err(|e| kernel_err(kind, &e))?;
            stamp_outputs(geometry_id_to_u64(id), &recorded_outputs, id_remap);
            Ok(())
        }
        "create_line_2d" => {
            let params = extract_create_params(inner, "Create2D")?;
            let sx = num_field(params, "start_x", kind)?;
            let sy = num_field(params, "start_y", kind)?;
            let ex = num_field(params, "end_x", kind)?;
            let ey = num_field(params, "end_y", kind)?;
            let mut builder = TopologyBuilder::new(model);
            let id = builder
                .create_line_2d(Point3::new(sx, sy, 0.0), Point3::new(ex, ey, 0.0))
                .map_err(|e| kernel_err(kind, &e))?;
            stamp_outputs(geometry_id_to_u64(id), &recorded_outputs, id_remap);
            Ok(())
        }
        "create_circle_2d" => {
            let params = extract_create_params(inner, "Create2D")?;
            let cx = num_field(params, "center_x", kind)?;
            let cy = num_field(params, "center_y", kind)?;
            let r = num_field(params, "radius", kind)?;
            let mut builder = TopologyBuilder::new(model);
            let id = builder
                .create_circle_2d(Point3::new(cx, cy, 0.0), r)
                .map_err(|e| kernel_err(kind, &e))?;
            stamp_outputs(geometry_id_to_u64(id), &recorded_outputs, id_remap);
            Ok(())
        }
        "create_rectangle_2d" => {
            let params = extract_create_params(inner, "Create2D")?;
            let cx = num_field(params, "corner_x", kind)?;
            let cy = num_field(params, "corner_y", kind)?;
            let w = num_field(params, "width", kind)?;
            let h = num_field(params, "height", kind)?;
            let mut builder = TopologyBuilder::new(model);
            let id = builder
                .create_rectangle_2d(Point3::new(cx, cy, 0.0), w, h)
                .map_err(|e| kernel_err(kind, &e))?;
            stamp_outputs(geometry_id_to_u64(id), &recorded_outputs, id_remap);
            Ok(())
        }

        // ----------------------------------------------------------------
        // 3D primitives — recorded as `TimelineOperation::Create3D`.
        // ----------------------------------------------------------------
        "create_box_3d" => {
            let params = extract_create_params(inner, "Create3D")?;
            let w = num_field(params, "width", kind)?;
            let h = num_field(params, "height", kind)?;
            let d = num_field(params, "depth", kind)?;
            let mut builder = TopologyBuilder::new(model);
            let id = builder
                .create_box_3d(w, h, d)
                .map_err(|e| kernel_err(kind, &e))?;
            stamp_outputs(geometry_id_to_u64(id), &recorded_outputs, id_remap);
            Ok(())
        }
        "create_sphere_3d" => {
            let params = extract_create_params(inner, "Create3D")?;
            let cx = num_field(params, "center_x", kind)?;
            let cy = num_field(params, "center_y", kind)?;
            let cz = num_field(params, "center_z", kind)?;
            let r = num_field(params, "radius", kind)?;
            let mut builder = TopologyBuilder::new(model);
            let id = builder
                .create_sphere_3d(Point3::new(cx, cy, cz), r)
                .map_err(|e| kernel_err(kind, &e))?;
            stamp_outputs(geometry_id_to_u64(id), &recorded_outputs, id_remap);
            Ok(())
        }
        "create_cylinder_3d" => {
            let params = extract_create_params(inner, "Create3D")?;
            let bx = num_field(params, "base_x", kind)?;
            let by = num_field(params, "base_y", kind)?;
            let bz = num_field(params, "base_z", kind)?;
            let ax = num_field(params, "axis_x", kind)?;
            let ay = num_field(params, "axis_y", kind)?;
            let az = num_field(params, "axis_z", kind)?;
            let r = num_field(params, "radius", kind)?;
            let h = num_field(params, "height", kind)?;
            let mut builder = TopologyBuilder::new(model);
            let id = builder
                .create_cylinder_3d(
                    Point3::new(bx, by, bz),
                    Vector3::new(ax, ay, az),
                    r,
                    h,
                )
                .map_err(|e| kernel_err(kind, &e))?;
            stamp_outputs(geometry_id_to_u64(id), &recorded_outputs, id_remap);
            Ok(())
        }
        "create_cone_3d" => {
            let params = extract_create_params(inner, "Create3D")?;
            let bx = num_field(params, "base_x", kind)?;
            let by = num_field(params, "base_y", kind)?;
            let bz = num_field(params, "base_z", kind)?;
            let ax = num_field(params, "axis_x", kind)?;
            let ay = num_field(params, "axis_y", kind)?;
            let az = num_field(params, "axis_z", kind)?;
            let br = num_field(params, "base_radius", kind)?;
            let tr = num_field(params, "top_radius", kind)?;
            let h = num_field(params, "height", kind)?;
            let mut builder = TopologyBuilder::new(model);
            let id = builder
                .create_cone_3d(
                    Point3::new(bx, by, bz),
                    Vector3::new(ax, ay, az),
                    br,
                    tr,
                    h,
                )
                .map_err(|e| kernel_err(kind, &e))?;
            stamp_outputs(geometry_id_to_u64(id), &recorded_outputs, id_remap);
            Ok(())
        }
        "create_plane_3d" => {
            let params = extract_create_params(inner, "Create3D")?;
            let ox = num_field(params, "origin_x", kind)?;
            let oy = num_field(params, "origin_y", kind)?;
            let oz = num_field(params, "origin_z", kind)?;
            let nx = num_field(params, "normal_x", kind)?;
            let ny = num_field(params, "normal_y", kind)?;
            let nz = num_field(params, "normal_z", kind)?;
            let w = num_field(params, "width", kind)?;
            let h = num_field(params, "height", kind)?;
            let t = num_field(params, "thickness", kind)?;
            // The kernel re-orthogonalizes whatever u_dir we hand it; pick
            // any axis not parallel to the normal.
            let normal = Vector3::new(nx, ny, nz);
            let candidate = if normal.x.abs() < 0.9 {
                Vector3::X
            } else {
                Vector3::Y
            };
            let mut builder = TopologyBuilder::new(model);
            let id = builder
                .plane_primitive(Point3::new(ox, oy, oz), normal, candidate, w, h, t)
                .map_err(|e| kernel_err(kind, &e))?;
            stamp_outputs(id as u64, &recorded_outputs, id_remap);
            Ok(())
        }

        // ----------------------------------------------------------------
        // Operations
        // ----------------------------------------------------------------
        "extrude_face" => {
            let face_raw = num_field(inner, "face_id", kind)? as u64;
            let mapped = remap_id(face_raw, id_remap) as FaceId;
            let distance = num_field(inner, "distance", kind)?;
            let dir = vec3_field(inner, "direction").unwrap_or(Vector3::Z);
            let cap_ends = inner
                .get("cap_ends")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let draft_angle = inner
                .get("draft_angle")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let twist_angle = inner
                .get("twist_angle")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let end_scale = inner
                .get("end_scale")
                .and_then(|v| v.as_f64())
                .unwrap_or(1.0);

            let options = ExtrudeOptions {
                direction: dir,
                distance,
                cap_ends,
                draft_angle,
                twist_angle,
                end_scale,
                ..ExtrudeOptions::default()
            };
            let new_solid =
                extrude_face(model, mapped, options).map_err(|e| kernel_err(kind, &e))?;
            stamp_outputs(new_solid as u64, &recorded_outputs, id_remap);
            Ok(())
        }

        "boolean_union" | "boolean_intersection" | "boolean_difference" => {
            let op = match kind {
                "boolean_union" => BooleanOp::Union,
                "boolean_intersection" => BooleanOp::Intersection,
                "boolean_difference" => BooleanOp::Difference,
                _ => unreachable!(),
            };
            let a_raw = num_field(inner, "solid_a", kind)? as u64;
            let b_raw = num_field(inner, "solid_b", kind)? as u64;
            let a = remap_id(a_raw, id_remap) as SolidId;
            let b = remap_id(b_raw, id_remap) as SolidId;
            let new_solid = boolean_operation(model, a, b, op, BooleanOptions::default())
                .map_err(|e| kernel_err(kind, &e))?;
            stamp_outputs(new_solid as u64, &recorded_outputs, id_remap);
            Ok(())
        }

        "fillet_edges" => {
            // Recorded as inputs[0] = solid_id, inputs[1..] = edge ids
            // (see fillet.rs:187-188).
            let inputs = parameters
                .get("inputs")
                .and_then(|v| v.as_array())
                .ok_or_else(|| missing_inputs(kind))?;
            if inputs.is_empty() {
                return Err(ReplayError::InvalidParameters {
                    kind: kind.to_string(),
                    reason: "empty inputs[]".to_string(),
                });
            }
            let solid_raw = inputs[0]
                .as_u64()
                .ok_or_else(|| ReplayError::InvalidParameters {
                    kind: kind.to_string(),
                    reason: "inputs[0] (solid_id) not u64".to_string(),
                })?;
            let edge_ids: Vec<EdgeId> = inputs
                .iter()
                .skip(1)
                .filter_map(|v| v.as_u64())
                .map(|id| remap_id(id, id_remap) as EdgeId)
                .collect();
            let solid = remap_id(solid_raw, id_remap) as SolidId;
            let radius = parse_fillet_constant_radius(inner).unwrap_or(1.0);
            let options = FilletOptions {
                fillet_type: FilletType::Constant(radius),
                ..FilletOptions::default()
            };
            let _faces = fillet_edges(model, solid, edge_ids, options)
                .map_err(|e| kernel_err(kind, &e))?;
            // Fillet outputs are face IDs; downstream replay does not
            // currently reference fillet faces by recorded ID, so we
            // leave the remap untouched here.
            Ok(())
        }

        "chamfer_edges" => {
            let inputs = parameters
                .get("inputs")
                .and_then(|v| v.as_array())
                .ok_or_else(|| missing_inputs(kind))?;
            if inputs.is_empty() {
                return Err(ReplayError::InvalidParameters {
                    kind: kind.to_string(),
                    reason: "empty inputs[]".to_string(),
                });
            }
            let solid_raw = inputs[0]
                .as_u64()
                .ok_or_else(|| ReplayError::InvalidParameters {
                    kind: kind.to_string(),
                    reason: "inputs[0] (solid_id) not u64".to_string(),
                })?;
            let edge_ids: Vec<EdgeId> = inputs
                .iter()
                .skip(1)
                .filter_map(|v| v.as_u64())
                .map(|id| remap_id(id, id_remap) as EdgeId)
                .collect();
            let solid = remap_id(solid_raw, id_remap) as SolidId;
            let distance = inner
                .get("distance1")
                .and_then(|v| v.as_f64())
                .unwrap_or(1.0);
            let options = ChamferOptions {
                chamfer_type: ChamferType::EqualDistance(distance),
                ..ChamferOptions::default()
            };
            let _faces = chamfer_edges(model, solid, edge_ids, options)
                .map_err(|e| kernel_err(kind, &e))?;
            Ok(())
        }

        "transform_solid" => {
            let solid_raw = num_field(inner, "solid_id", kind)? as u64;
            let solid = remap_id(solid_raw, id_remap) as SolidId;
            let transform = matrix4_field(inner, "transform", kind)?;
            transform_solid(model, solid, transform, TransformOptions::default())
                .map_err(|e| kernel_err(kind, &e))?;
            Ok(())
        }

        "transform_faces" => {
            let inputs = parameters
                .get("inputs")
                .and_then(|v| v.as_array())
                .ok_or_else(|| missing_inputs(kind))?;
            let face_ids: Vec<FaceId> = inputs
                .iter()
                .filter_map(|v| v.as_u64())
                .map(|id| remap_id(id, id_remap) as FaceId)
                .collect();
            let transform = matrix4_field(inner, "transform", kind)?;
            transform_faces(model, face_ids, transform, TransformOptions::default())
                .map_err(|e| kernel_err(kind, &e))?;
            Ok(())
        }

        "transform_edges" => {
            let inputs = parameters
                .get("inputs")
                .and_then(|v| v.as_array())
                .ok_or_else(|| missing_inputs(kind))?;
            let edge_ids: Vec<EdgeId> = inputs
                .iter()
                .filter_map(|v| v.as_u64())
                .map(|id| remap_id(id, id_remap) as EdgeId)
                .collect();
            let transform = matrix4_field(inner, "transform", kind)?;
            transform_edges(model, edge_ids, transform, TransformOptions::default())
                .map_err(|e| kernel_err(kind, &e))?;
            Ok(())
        }

        "revolve_face" => {
            let face_raw = num_field(inner, "face_id", kind)? as u64;
            let face_id = remap_id(face_raw, id_remap) as FaceId;
            let axis_origin_v = vec3_field(inner, "axis_origin").unwrap_or(Vector3::ZERO);
            let axis_direction = vec3_field(inner, "axis_direction").unwrap_or(Vector3::Z);
            let angle = num_field(inner, "angle", kind).unwrap_or(std::f64::consts::TAU);
            let pitch = inner.get("pitch").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let segments = inner
                .get("segments")
                .and_then(|v| v.as_u64())
                .map(|n| n as u32)
                .unwrap_or(32);
            let cap_ends = inner
                .get("cap_ends")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let options = RevolveOptions {
                axis_origin: Point3::new(axis_origin_v.x, axis_origin_v.y, axis_origin_v.z),
                axis_direction,
                angle,
                pitch,
                segments,
                cap_ends,
                ..RevolveOptions::default()
            };
            let new_solid = revolve_face(model, face_id, options)
                .map_err(|e| kernel_err(kind, &e))?;
            stamp_outputs(new_solid as u64, &recorded_outputs, id_remap);
            Ok(())
        }

        // The kernel currently records sweep / loft with profile *edges*
        // in `inputs`, not the parent profile face. Replay needs the
        // face, so until the recorder is enriched these are skipped with
        // a structured error rather than executed against the wrong
        // entity type.
        "sweep_profile" | "loft_profiles" => Err(ReplayError::InvalidParameters {
            kind: kind.to_string(),
            reason: "kernel-side recorder is currently lossy for this op \
                     (profile edges recorded, not face) — replay deferred"
                .to_string(),
        }),

        unknown => Err(ReplayError::UnknownKind(unknown.to_string())),
    }
}

// =====================================================================
// Helpers
// =====================================================================

/// Pull the inner parameter object out of a `record_and_push`-style
/// payload. The recorder serializes `TimelineOperation::CreateNd` as the
/// externally tagged `{ "<Variant>": { "primitive_type": ..., "parameters": {...} } }`.
fn extract_create_params<'a>(
    inner: &'a Value,
    variant: &str,
) -> Result<&'a Value, ReplayError> {
    let v = inner
        .get(variant)
        .ok_or_else(|| ReplayError::InvalidParameters {
            kind: variant.to_string(),
            reason: format!("missing top-level {} variant", variant),
        })?;
    v.get("parameters")
        .ok_or_else(|| ReplayError::InvalidParameters {
            kind: variant.to_string(),
            reason: "missing nested parameters object".to_string(),
        })
}

fn num_field(v: &Value, name: &str, kind: &str) -> Result<f64, ReplayError> {
    v.get(name)
        .and_then(|x| x.as_f64())
        .ok_or_else(|| ReplayError::InvalidParameters {
            kind: kind.to_string(),
            reason: format!("missing or non-numeric field `{}`", name),
        })
}

fn vec3_field(v: &Value, name: &str) -> Option<Vector3> {
    let arr = v.get(name)?.as_array()?;
    if arr.len() != 3 {
        return None;
    }
    Some(Vector3::new(
        arr[0].as_f64()?,
        arr[1].as_f64()?,
        arr[2].as_f64()?,
    ))
}

fn matrix4_field(v: &Value, name: &str, kind: &str) -> Result<Matrix4, ReplayError> {
    let raw = v
        .get(name)
        .cloned()
        .ok_or_else(|| ReplayError::InvalidParameters {
            kind: kind.to_string(),
            reason: format!("missing `{}` field", name),
        })?;
    serde_json::from_value(raw).map_err(|e| ReplayError::InvalidParameters {
        kind: kind.to_string(),
        reason: format!("`{}` deserialize: {}", name, e),
    })
}

fn missing_inputs(kind: &str) -> ReplayError {
    ReplayError::InvalidParameters {
        kind: kind.to_string(),
        reason: "missing inputs[]".to_string(),
    }
}

fn kernel_err<E: std::fmt::Display>(kind: &str, e: &E) -> ReplayError {
    ReplayError::KernelError {
        kind: kind.to_string(),
        message: e.to_string(),
    }
}

/// Pair the new ID(s) returned by the kernel call with the recorded
/// outputs. Most operations produce exactly one entity — the recorder
/// puts one ID in `outputs` — so a positional pairing is sufficient.
fn stamp_outputs(new_id: u64, recorded: &[u64], id_remap: &mut HashMap<u64, u64>) {
    if let Some(&recorded_id) = recorded.first() {
        id_remap.insert(recorded_id, new_id);
    }
}

/// Best-effort parse of a Debug-formatted `FilletType::Constant(<r>)`
/// string back into the radius value. Fillet records use
/// `format!("{:?}", options.fillet_type)`, which is lossy by nature.
/// Variable / Function / Chord variants fall back to the constant
/// default at the call site.
fn parse_fillet_constant_radius(params: &Value) -> Option<f64> {
    let s = params.get("fillet_type")?.as_str()?;
    let inner = s.strip_prefix("Constant(")?.strip_suffix(')')?;
    inner.trim().parse::<f64>().ok()
}

/// Erase the `GeometryId` discriminant — see the matching kernel-side
/// helper in `topology_builder.rs`. Used here because `TopologyBuilder`
/// methods return typed `GeometryId` and the remap stores raw `u64`.
fn geometry_id_to_u64(id: GeometryId) -> u64 {
    match id {
        GeometryId::Face(i) => i as u64,
        GeometryId::Solid(i) => i as u64,
        GeometryId::Edge(i) => i as u64,
        GeometryId::Vertex(i) => i as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Author, EventId, EventMetadata};
    use chrono::Utc;
    use uuid::Uuid;

    fn mk_event(kind: &str, params: Value) -> TimelineEvent {
        TimelineEvent {
            id: EventId(Uuid::new_v4()),
            sequence_number: 0,
            timestamp: Utc::now(),
            author: Author::System,
            operation: Operation::Generic {
                command_type: kind.to_string(),
                parameters: params,
            },
            inputs: Default::default(),
            outputs: Default::default(),
            metadata: EventMetadata::default(),
        }
    }

    #[test]
    fn replay_create_box_3d() {
        let mut model = BRepModel::new();
        let event = mk_event(
            "create_box_3d",
            serde_json::json!({
                "params": {
                    "Create3D": {
                        "primitive_type": "box",
                        "parameters": {
                            "width": 10.0,
                            "height": 20.0,
                            "depth": 30.0
                        },
                        "timestamp": 0
                    }
                },
                "inputs": [],
                "outputs": [42]
            }),
        );
        let outcome = rebuild_model_from_events(&mut model, &[event]);
        assert_eq!(outcome.events_applied, 1);
        assert_eq!(outcome.events_skipped, 0);
        assert!(model.solids.len() >= 1, "expected at least one solid");
        // Recorded output 42 should be remapped to whatever solid index
        // the fresh model assigned.
        assert!(outcome.id_remap.contains_key(&42));
    }

    #[test]
    fn replay_create_sphere_then_cylinder_remap() {
        let mut model = BRepModel::new();
        let events = vec![
            mk_event(
                "create_sphere_3d",
                serde_json::json!({
                    "params": {
                        "Create3D": {
                            "primitive_type": "sphere",
                            "parameters": {
                                "center_x": 0.0, "center_y": 0.0, "center_z": 0.0,
                                "radius": 5.0
                            },
                            "timestamp": 0
                        }
                    },
                    "inputs": [],
                    "outputs": [0]
                }),
            ),
            mk_event(
                "create_cylinder_3d",
                serde_json::json!({
                    "params": {
                        "Create3D": {
                            "primitive_type": "cylinder",
                            "parameters": {
                                "base_x": 0.0, "base_y": 0.0, "base_z": 0.0,
                                "axis_x": 0.0, "axis_y": 0.0, "axis_z": 1.0,
                                "radius": 2.0, "height": 10.0
                            },
                            "timestamp": 0
                        }
                    },
                    "inputs": [],
                    "outputs": [1]
                }),
            ),
        ];
        let outcome = rebuild_model_from_events(&mut model, &events);
        assert_eq!(outcome.events_applied, 2);
        assert!(model.solids.len() >= 2);
    }

    #[test]
    fn unknown_kind_is_skipped_not_panic() {
        let mut model = BRepModel::new();
        let event = mk_event(
            "totally_made_up_kind",
            serde_json::json!({"params": {}, "inputs": [], "outputs": []}),
        );
        let outcome = rebuild_model_from_events(&mut model, &[event]);
        assert_eq!(outcome.events_applied, 0);
        assert_eq!(outcome.events_skipped, 1);
    }

    #[test]
    fn parse_fillet_radius_constant() {
        let p = serde_json::json!({"fillet_type": "Constant(2.5)"});
        assert_eq!(parse_fillet_constant_radius(&p), Some(2.5));
        let p2 = serde_json::json!({"fillet_type": "Variable(1.0, 2.0)"});
        assert_eq!(parse_fillet_constant_radius(&p2), None);
    }
}
