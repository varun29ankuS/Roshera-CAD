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
    loft::{loft_profiles, LoftOptions, LoftType},
    revolve::{revolve_face, RevolveOptions},
    sweep::{sweep_profile, SweepOptions, SweepQuality, SweepType},
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
pub fn rebuild_model_from_events(model: &mut BRepModel, events: &[TimelineEvent]) -> ReplayOutcome {
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
    // Persistent-id lineage (#11 slice 40-G): drive the kernel's root-pid seed
    // from this event's STABLE sequence number, so a replay re-derives identical
    // persistent-ids for the same timeline — even after a parameter edit (mould).
    // The sequence number is stable across replays (events replay in order), so
    // two replays of the same timeline assign the same PIDs, and a moulded event
    // keeps its key (only its parameters change) → references survive.
    model.set_event_key(Some(format!("evt:{}", event.sequence_number)));
    let result = match &event.operation {
        Operation::Generic {
            command_type,
            parameters,
        } => dispatch_generic(model, command_type, parameters, id_remap),
        other => Err(ReplayError::UnknownKind(format!(
            "non-Generic operation variant: {:?}",
            std::mem::discriminant(other)
        ))),
    };
    model.set_event_key(None);
    result
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
        .map(|a| a.iter().filter_map(parse_any_entity_ref).collect())
        .unwrap_or_default();

    // Translate a recorded ID into the live-model ID, falling back to
    // the original ID when no remap entry exists (first-reference case
    // where the recorder didn't observe the producer).
    let remap_id = |id: u64, remap: &HashMap<u64, u64>| -> u64 { *remap.get(&id).unwrap_or(&id) };

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
                .create_cylinder_3d(Point3::new(bx, by, bz), Vector3::new(ax, ay, az), r, h)
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
                .create_cone_3d(Point3::new(bx, by, bz), Vector3::new(ax, ay, az), br, tr, h)
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

        "sketch_extrude" => {
            // Self-contained sketch extrusion: frame + per-loop
            // payloads. Replays through the SAME kernel helper the
            // live api-server bridges use (extrude_profile_regions),
            // so replay cannot drift from live behaviour.
            //
            // Loop payload schema (SKETCH-DCM #45 Slice 5): a plain
            // vertex array `[[u, v], ...]` is a materialised polygon
            // (the legacy shape — every pre-slice event replays
            // unchanged), while `{"edges": [...]}` carries typed
            // analytic `ProfileEdge`s so replayed bores stay TRUE
            // cylinders, byte-equivalent to the live build.
            let vec3 = |key: &str| -> Option<geometry_engine::math::Vector3> {
                let a = inner.get(key)?.as_array()?;
                Some(geometry_engine::math::Vector3::new(
                    a.first()?.as_f64()?,
                    a.get(1)?.as_f64()?,
                    a.get(2)?.as_f64()?,
                ))
            };
            let point3 = |key: &str| -> Option<geometry_engine::math::Point3> {
                let a = inner.get(key)?.as_array()?;
                Some(geometry_engine::math::Point3::new(
                    a.first()?.as_f64()?,
                    a.get(1)?.as_f64()?,
                    a.get(2)?.as_f64()?,
                ))
            };
            let polygon = |v: &serde_json::Value| -> Option<Vec<[f64; 2]>> {
                v.as_array()?
                    .iter()
                    .map(|p| {
                        let pa = p.as_array()?;
                        Some([pa.first()?.as_f64()?, pa.get(1)?.as_f64()?])
                    })
                    .collect()
            };
            let profile_loop =
                |v: &serde_json::Value| -> Option<geometry_engine::operations::extrude::ProfileLoop> {
                    if v.is_array() {
                        return Some(
                            geometry_engine::operations::extrude::ProfileLoop::Polygon(polygon(v)?),
                        );
                    }
                    let edges = v.get("edges")?.clone();
                    let edges: Vec<geometry_engine::sketch2d::sketch_topology::ProfileEdge> =
                        serde_json::from_value(edges).ok()?;
                    Some(geometry_engine::operations::extrude::ProfileLoop::Edges(edges))
                };
            let parsed = (|| -> Option<_> {
                let origin = point3("origin")?;
                let u_axis = vec3("u_axis")?;
                let v_axis = vec3("v_axis")?;
                let distance = inner.get("distance")?.as_f64()?;
                let direction = vec3("direction");
                let regions: Option<Vec<geometry_engine::operations::extrude::ProfileRegion>> =
                    inner
                        .get("regions")?
                        .as_array()?
                        .iter()
                        .map(|r| {
                            let outer = profile_loop(r.get("outer")?)?;
                            let holes: Option<Vec<_>> = r
                                .get("holes")?
                                .as_array()?
                                .iter()
                                .map(&profile_loop)
                                .collect();
                            Some(geometry_engine::operations::extrude::ProfileRegion {
                                outer,
                                holes: holes?,
                            })
                        })
                        .collect();
                Some((origin, u_axis, v_axis, regions?, distance, direction))
            })();
            match parsed {
                Some((origin, u_axis, v_axis, regions, distance, direction)) => {
                    geometry_engine::operations::extrude::extrude_profile_regions(
                        model,
                        origin,
                        u_axis,
                        v_axis,
                        &regions,
                        distance,
                        direction,
                        geometry_engine::math::Tolerance::default(),
                    )
                    .map(|_| ())
                    .map_err(|e| kernel_err(kind, &e))
                }
                None => Err(ReplayError::InvalidParameters {
                    kind: kind.to_string(),
                    reason: "missing/malformed sketch_extrude payload".to_string(),
                }),
            }
        }

        "sketch_revolve" => {
            // Self-contained sketch revolution (SKETCH-DCM #45
            // follow-ups B, item 5): frame + per-loop payloads + the
            // IN-PLANE axis. Same loop payload schema as
            // `sketch_extrude` (legacy polygon arrays and typed
            // `{"edges": [...]}` both accepted); replays through the
            // SAME kernel entry the live csketch route uses
            // (`revolve_profile_regions`) — no live/replay drift.
            let vec3 = |key: &str| -> Option<geometry_engine::math::Vector3> {
                let a = inner.get(key)?.as_array()?;
                Some(geometry_engine::math::Vector3::new(
                    a.first()?.as_f64()?,
                    a.get(1)?.as_f64()?,
                    a.get(2)?.as_f64()?,
                ))
            };
            let point3 = |key: &str| -> Option<geometry_engine::math::Point3> {
                let a = inner.get(key)?.as_array()?;
                Some(geometry_engine::math::Point3::new(
                    a.first()?.as_f64()?,
                    a.get(1)?.as_f64()?,
                    a.get(2)?.as_f64()?,
                ))
            };
            let vec2 = |key: &str| -> Option<[f64; 2]> {
                let a = inner.get(key)?.as_array()?;
                Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?])
            };
            let polygon = |v: &serde_json::Value| -> Option<Vec<[f64; 2]>> {
                v.as_array()?
                    .iter()
                    .map(|p| {
                        let pa = p.as_array()?;
                        Some([pa.first()?.as_f64()?, pa.get(1)?.as_f64()?])
                    })
                    .collect()
            };
            let profile_loop =
                |v: &serde_json::Value| -> Option<geometry_engine::operations::extrude::ProfileLoop> {
                    if v.is_array() {
                        return Some(
                            geometry_engine::operations::extrude::ProfileLoop::Polygon(polygon(v)?),
                        );
                    }
                    let edges = v.get("edges")?.clone();
                    let edges: Vec<geometry_engine::sketch2d::sketch_topology::ProfileEdge> =
                        serde_json::from_value(edges).ok()?;
                    Some(geometry_engine::operations::extrude::ProfileLoop::Edges(edges))
                };
            let parsed = (|| -> Option<_> {
                let origin = point3("origin")?;
                let u_axis = vec3("u_axis")?;
                let v_axis = vec3("v_axis")?;
                let axis_origin = vec2("axis_origin")?;
                let axis_direction = vec2("axis_direction")?;
                let angle = inner.get("angle")?.as_f64()?;
                let segments = inner.get("segments").and_then(|v| v.as_u64()).unwrap_or(48) as u32;
                let regions: Option<Vec<geometry_engine::operations::extrude::ProfileRegion>> =
                    inner
                        .get("regions")?
                        .as_array()?
                        .iter()
                        .map(|r| {
                            let outer = profile_loop(r.get("outer")?)?;
                            let holes: Option<Vec<_>> = r
                                .get("holes")?
                                .as_array()?
                                .iter()
                                .map(&profile_loop)
                                .collect();
                            Some(geometry_engine::operations::extrude::ProfileRegion {
                                outer,
                                holes: holes?,
                            })
                        })
                        .collect();
                Some((
                    origin,
                    u_axis,
                    v_axis,
                    regions?,
                    axis_origin,
                    axis_direction,
                    angle,
                    segments,
                ))
            })();
            match parsed {
                Some((
                    origin,
                    u_axis,
                    v_axis,
                    regions,
                    axis_origin,
                    axis_direction,
                    angle,
                    segments,
                )) => geometry_engine::operations::revolve::revolve_profile_regions(
                    model,
                    origin,
                    u_axis,
                    v_axis,
                    &regions,
                    axis_origin,
                    axis_direction,
                    angle,
                    segments,
                    geometry_engine::math::Tolerance::default(),
                )
                .map(|_| ())
                .map_err(|e| kernel_err(kind, &e)),
                None => Err(ReplayError::InvalidParameters {
                    kind: kind.to_string(),
                    reason: "missing/malformed sketch_revolve payload".to_string(),
                }),
            }
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
            let solid_raw = parse_entity_ref(&inputs[0], "solid").ok_or_else(|| {
                ReplayError::InvalidParameters {
                    kind: kind.to_string(),
                    reason: "inputs[0] expected `solid:<id>`".to_string(),
                }
            })?;
            let edge_ids: Vec<EdgeId> = inputs
                .iter()
                .skip(1)
                .filter_map(|v| parse_entity_ref(v, "edge"))
                .map(|id| remap_id(id, id_remap) as EdgeId)
                .collect();
            let solid = remap_id(solid_raw, id_remap) as SolidId;
            // Prefer the structured `radius` field added in 2026-05-10;
            // fall back to parsing the Debug-formatted `fillet_type`
            // string for events recorded by older builds. Final fallback
            // is the FilletOptions default radius.
            let radius = inner
                .get("radius")
                .and_then(|v| v.as_f64())
                .or_else(|| parse_fillet_constant_radius(inner))
                .unwrap_or(1.0);
            let options = FilletOptions {
                fillet_type: FilletType::Constant(radius),
                radius,
                ..FilletOptions::default()
            };
            let _faces =
                fillet_edges(model, solid, edge_ids, options).map_err(|e| kernel_err(kind, &e))?;
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
            let solid_raw = parse_entity_ref(&inputs[0], "solid").ok_or_else(|| {
                ReplayError::InvalidParameters {
                    kind: kind.to_string(),
                    reason: "inputs[0] expected `solid:<id>`".to_string(),
                }
            })?;
            let edge_ids: Vec<EdgeId> = inputs
                .iter()
                .skip(1)
                .filter_map(|v| parse_entity_ref(v, "edge"))
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
            let _faces =
                chamfer_edges(model, solid, edge_ids, options).map_err(|e| kernel_err(kind, &e))?;
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
                .filter_map(|v| parse_entity_ref(v, "face"))
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
                .filter_map(|v| parse_entity_ref(v, "edge"))
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
            let new_solid =
                revolve_face(model, face_id, options).map_err(|e| kernel_err(kind, &e))?;
            stamp_outputs(new_solid as u64, &recorded_outputs, id_remap);
            Ok(())
        }

        // ----------------------------------------------------------------
        // Sweep — `inputs` is `[profile_edge_0, ..., profile_edge_{n-1},
        // path_edge]`. `params.profile_edge_count = n` partitions them.
        // The kernel ignores the bulk of `SweepOptions` apart from
        // `sweep_type` / `quality` / scaling; the replayed call uses
        // defaults for path-tangent / twist / quality controls. Lossy
        // for those — but the **topology** is reproducible, which is
        // what timeline replay must preserve.
        // ----------------------------------------------------------------
        "sweep_profile" => {
            let inputs_arr = parameters
                .get("inputs")
                .and_then(|v| v.as_array())
                .ok_or_else(|| missing_inputs(kind))?;
            let profile_edge_count = inner
                .get("profile_edge_count")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize)
                .ok_or_else(|| ReplayError::InvalidParameters {
                    kind: kind.to_string(),
                    reason: "missing `profile_edge_count`".to_string(),
                })?;
            if inputs_arr.len() != profile_edge_count + 1 {
                return Err(ReplayError::InvalidParameters {
                    kind: kind.to_string(),
                    reason: format!(
                        "inputs length {} does not match profile_edge_count + 1 = {}",
                        inputs_arr.len(),
                        profile_edge_count + 1
                    ),
                });
            }
            let raw_inputs: Vec<u64> = inputs_arr
                .iter()
                .filter_map(|v| parse_entity_ref(v, "edge"))
                .collect();
            if raw_inputs.len() != inputs_arr.len() {
                return Err(ReplayError::InvalidParameters {
                    kind: kind.to_string(),
                    reason: "inputs[] contains non-`edge:<id>` entries".to_string(),
                });
            }
            let profile: Vec<EdgeId> = raw_inputs[..profile_edge_count]
                .iter()
                .map(|&id| remap_id(id, id_remap) as EdgeId)
                .collect();
            let path_raw = raw_inputs[profile_edge_count];
            let path = remap_id(path_raw, id_remap) as EdgeId;
            // Recover sweep_type / quality from their Debug-formatted
            // strings (the original options carry non-Serialize
            // function-pointer fields, hence the Debug round-trip).
            let sweep_type = inner
                .get("sweep_type")
                .and_then(|v| v.as_str())
                .map(parse_sweep_type)
                .unwrap_or(SweepType::Path);
            let quality = inner
                .get("quality")
                .and_then(|v| v.as_str())
                .map(parse_sweep_quality)
                .unwrap_or(SweepQuality::Standard);
            let options = SweepOptions {
                sweep_type,
                quality,
                ..SweepOptions::default()
            };
            let new_solid =
                sweep_profile(model, profile, path, options).map_err(|e| kernel_err(kind, &e))?;
            stamp_outputs(new_solid as u64, &recorded_outputs, id_remap);
            Ok(())
        }

        // ----------------------------------------------------------------
        // Loft — `inputs` is the flat concatenation of profile edges,
        // partitioned by `params.profile_edge_counts: [usize; n]`.
        // Lossy for guide curves and explicit vertex correspondence
        // (those carry types that don't round-trip through JSON);
        // replay falls back to the kernel's automatic correspondence.
        // ----------------------------------------------------------------
        "loft_profiles" => {
            let inputs_arr = parameters
                .get("inputs")
                .and_then(|v| v.as_array())
                .ok_or_else(|| missing_inputs(kind))?;
            let counts: Vec<usize> = inner
                .get("profile_edge_counts")
                .and_then(|v| v.as_array())
                .ok_or_else(|| ReplayError::InvalidParameters {
                    kind: kind.to_string(),
                    reason: "missing `profile_edge_counts`".to_string(),
                })?
                .iter()
                .filter_map(|v| v.as_u64().map(|n| n as usize))
                .collect();
            let raw_inputs: Vec<u64> = inputs_arr
                .iter()
                .filter_map(|v| parse_entity_ref(v, "edge"))
                .collect();
            if raw_inputs.len() != counts.iter().sum::<usize>() {
                return Err(ReplayError::InvalidParameters {
                    kind: kind.to_string(),
                    reason: format!(
                        "inputs length {} does not match Σ profile_edge_counts {:?}",
                        raw_inputs.len(),
                        counts
                    ),
                });
            }
            let mut cursor = 0usize;
            let mut profiles: Vec<Vec<EdgeId>> = Vec::with_capacity(counts.len());
            for c in &counts {
                let slice: Vec<EdgeId> = raw_inputs[cursor..cursor + c]
                    .iter()
                    .map(|&id| remap_id(id, id_remap) as EdgeId)
                    .collect();
                profiles.push(slice);
                cursor += c;
            }
            let loft_type = inner
                .get("loft_type")
                .and_then(|v| v.as_str())
                .map(parse_loft_type)
                .unwrap_or(LoftType::Linear);
            let closed = inner
                .get("closed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let create_solid = inner
                .get("create_solid")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let options = LoftOptions {
                loft_type,
                closed,
                create_solid,
                ..LoftOptions::default()
            };
            let new_solid =
                loft_profiles(model, profiles, options).map_err(|e| kernel_err(kind, &e))?;
            stamp_outputs(new_solid as u64, &recorded_outputs, id_remap);
            Ok(())
        }

        // ----------------------------------------------------------------
        // Sketch-domain operations (SKETCH-DCM #45 Slice 6): trim /
        // extend / offset / mirror / patterns / construction-flag edits
        // on a live csketch. These events are DESIGN-HISTORY records —
        // the sketch container lives in the api-server, not in the
        // B-Rep model, so their model effect is nil BY CONSTRUCTION:
        // the downstream `sketch_extrude` event is fully
        // self-contained (frame + materialised profile loops) and
        // rebuilds the identical solid whether or not the sketch ops
        // that shaped the profile are replayed. Validating the payload
        // shape (rather than erroring UnknownKind) keeps full-timeline
        // replays of sketch-op sessions at `events_skipped == 0`.
        "csketch_trim"
        | "csketch_extend"
        | "csketch_offset"
        | "csketch_mirror"
        | "csketch_pattern_linear"
        | "csketch_pattern_circular"
        | "csketch_pattern_curve"
        | "csketch_pattern_phyllotaxis"
        | "csketch_construction" => {
            if inner.get("csketch_id").and_then(|v| v.as_str()).is_none() {
                return Err(ReplayError::InvalidParameters {
                    kind: kind.to_string(),
                    reason: "missing `csketch_id`".to_string(),
                });
            }
            Ok(())
        }

        unknown => Err(ReplayError::UnknownKind(unknown.to_string())),
    }
}

// =====================================================================
// Helpers
// =====================================================================

/// Pull the inner parameter object out of a `record_and_push`-style
/// payload. The recorder serializes `TimelineOperation::CreateNd` as the
/// externally tagged `{ "<Variant>": { "primitive_type": ..., "parameters": {...} } }`.
fn extract_create_params<'a>(inner: &'a Value, variant: &str) -> Result<&'a Value, ReplayError> {
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

/// Recover a `SweepType` from its Debug-formatted string. Sweep records
/// use `format!("{:?}", options.sweep_type)` because `SweepOptions`
/// carries non-`Serialize` callback fields and cannot round-trip through
/// JSON directly. Unknown strings fall back to `Path` (the default).
fn parse_sweep_type(s: &str) -> SweepType {
    match s {
        "Path" => SweepType::Path,
        "MultiGuide" => SweepType::MultiGuide,
        "Rail" => SweepType::Rail,
        "BiRail" => SweepType::BiRail,
        _ => SweepType::Path,
    }
}

/// Recover a `SweepQuality` from its Debug string. Unknown → `Standard`.
fn parse_sweep_quality(s: &str) -> SweepQuality {
    match s {
        "Draft" => SweepQuality::Draft,
        "Standard" => SweepQuality::Standard,
        "High" => SweepQuality::High,
        _ => SweepQuality::Standard,
    }
}

/// Recover a `LoftType` from its Debug string. Unknown → `Linear`.
fn parse_loft_type(s: &str) -> LoftType {
    match s {
        "Linear" => LoftType::Linear,
        "Cubic" => LoftType::Cubic,
        "MinimalTwist" => LoftType::MinimalTwist,
        "Guided" => LoftType::Guided,
        _ => LoftType::Linear,
    }
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

/// Parse a recorded entity reference (`"<kind>:<id>"` form, as emitted
/// by `geometry-engine/src/operations/recorder.rs::entity_ref`) and
/// return the numeric id when the kind matches. Pre-2026-05-10 events
/// recorded inputs as bare numeric `u64` values; for backward
/// compatibility we accept those too (the bare form has no kind tag,
/// so we trust the caller's positional contract).
///
/// Returning `None` for a kind mismatch lets call sites use
/// `filter_map` to drop entries of the wrong kind silently — useful
/// when `inputs[]` interleaves multiple kinds and only one is wanted
/// (e.g. fillet recording `[solid, edge, edge, ...]`).
fn parse_entity_ref(v: &Value, expected_kind: &str) -> Option<u64> {
    if let Some(s) = v.as_str() {
        let (kind, id) = s.split_once(':')?;
        if kind != expected_kind {
            return None;
        }
        id.parse::<u64>().ok()
    } else {
        v.as_u64()
    }
}

/// Extract the numeric id from any recorded entity reference, ignoring
/// its kind. Used for `recorded_outputs` because `stamp_outputs` keys
/// the remap by the recorded id only — the kind is fixed by the kernel
/// call's return type at the matching site.
fn parse_any_entity_ref(v: &Value) -> Option<u64> {
    if let Some(s) = v.as_str() {
        let (_, id) = s.split_once(':')?;
        id.parse::<u64>().ok()
    } else {
        v.as_u64()
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

    // ---- #11 slice 40-G: replay-driven persistent-id lineage ----

    fn box_event(width: f64, seq: u64) -> TimelineEvent {
        let mut e = mk_event(
            "create_box_3d",
            serde_json::json!({
                "params": { "Create3D": {
                    "primitive_type": "box",
                    "parameters": { "width": width, "height": 10.0, "depth": 10.0 },
                    "timestamp": 0
                }},
                "inputs": [],
                "outputs": [0]
            }),
        );
        e.sequence_number = seq;
        e
    }

    fn only_solid(m: &BRepModel) -> SolidId {
        m.solids.iter().next().map(|(id, _)| id).expect("one solid")
    }

    fn max_abs_x(m: &BRepModel) -> f64 {
        let mut mx = 0.0_f64;
        for vid in 0..m.vertices.len() as u32 {
            if let Some(p) = m.vertices.get_position(vid) {
                mx = mx.max(p[0].abs());
            }
        }
        mx
    }

    #[test]
    fn replay_assigns_stable_pids_and_mould_preserves_them() {
        // Replay is PID-deterministic: the SAME timeline → the SAME persistent
        // ids, because the kernel seeds root PIDs from each event's stable
        // sequence number (set by apply_event).
        let mut m1 = BRepModel::new();
        rebuild_model_from_events(&mut m1, &[box_event(10.0, 5)]);
        let s1 = only_solid(&m1);
        let solid_pid = m1.solid_pid(s1).expect("solid has a persistent id");
        let face0 = {
            let solid = m1.solids.get(s1).unwrap();
            m1.shells.get(solid.outer_shell).unwrap().faces[0]
        };
        let face_pid = m1.face_pid(face0).expect("face has a persistent id");

        let mut m2 = BRepModel::new();
        rebuild_model_from_events(&mut m2, &[box_event(10.0, 5)]);
        let s2 = only_solid(&m2);
        assert_eq!(
            m2.solid_pid(s2),
            Some(solid_pid),
            "replaying the same timeline re-derives the same solid PID"
        );

        // MOULD: same event (sequence 5), width 10 -> 25. Re-evaluate.
        let mut m3 = BRepModel::new();
        rebuild_model_from_events(&mut m3, &[box_event(25.0, 5)]);
        let s3 = only_solid(&m3);

        // The agent's durable references SURVIVE the dimension edit.
        assert_eq!(
            m3.solid_pid(s3),
            Some(solid_pid),
            "solid PID survives the mould (depends on the event, not the dimension)"
        );
        assert!(
            m3.face_by_pid(face_pid).is_some(),
            "the face PID still resolves after the mould"
        );

        // And the edit actually took effect: the box really is wider.
        assert!((max_abs_x(&m1) - 5.0).abs() < 1e-6, "original half-width 5");
        assert!(
            (max_abs_x(&m3) - 12.5).abs() < 1e-6,
            "moulded half-width 12.5"
        );
    }

    /// Analytic cylinder faces on the outer shell of `solid`, as radii
    /// (SKETCH-DCM #45 Slice 5 replay assertions).
    fn cylinder_face_radii(m: &BRepModel, solid: SolidId) -> Vec<f64> {
        let solid_ref = m.solids.get(solid).expect("solid");
        let shell = m.shells.get(solid_ref.outer_shell).expect("shell");
        let mut radii = Vec::new();
        for &fid in &shell.faces {
            let face = m.faces.get(fid).expect("face");
            let surface = m.surfaces.get(face.surface_id).expect("surface");
            if let Some(cyl) = surface
                .as_any()
                .downcast_ref::<geometry_engine::primitives::surface::Cylinder>()
            {
                radii.push(cyl.radius);
            }
        }
        radii
    }

    /// SKETCH-DCM #45 Slice 5: a `sketch_extrude` event whose hole
    /// loop carries typed analytic edges (`{"edges": [...]}`) replays
    /// to a solid with a TRUE cylindrical bore face — byte-equivalent
    /// to the live analytic build, not a re-sampled 64-gon.
    #[test]
    fn replay_sketch_extrude_typed_edges_rebuilds_analytic_bore() {
        let mut model = BRepModel::new();
        let event = mk_event(
            "sketch_extrude",
            serde_json::json!({
                "params": {
                    "origin": [0.0, 0.0, 0.0],
                    "u_axis": [1.0, 0.0, 0.0],
                    "v_axis": [0.0, 1.0, 0.0],
                    "regions": [{
                        // Mixed schema on purpose: legacy polygon outer
                        // + typed analytic hole in ONE event.
                        "outer": [[0.0, 0.0], [40.0, 0.0], [40.0, 30.0], [0.0, 30.0]],
                        "holes": [{ "edges": [
                            { "kind": "circle", "center": [20.0, 15.0], "radius": 6.0 }
                        ]}],
                    }],
                    "distance": 10.0,
                    "direction": [0.0, 0.0, 1.0],
                },
                "inputs": [],
                "outputs": [99]
            }),
        );
        let outcome = rebuild_model_from_events(&mut model, &[event]);
        assert_eq!(outcome.events_applied, 1, "event must apply");
        assert_eq!(outcome.events_skipped, 0);
        let solid = only_solid(&model);
        let radii = cylinder_face_radii(&model, solid);
        assert_eq!(
            radii.len(),
            1,
            "typed-edge replay must rebuild ONE analytic cylinder bore face"
        );
        let radius = radii.first().copied().expect("one bore radius");
        assert!(
            (radius - 6.0).abs() < 1e-9,
            "replayed bore radius must be exact: {radius}"
        );
    }

    /// Pre-Slice-5 `sketch_extrude` events (plain polygon arrays) must
    /// keep replaying — and must reproduce the RECORDED polygonal
    /// geometry, not be silently upgraded to analytic faces.
    #[test]
    fn replay_sketch_extrude_legacy_polygon_payload_unchanged() {
        let mut model = BRepModel::new();
        // A coarse 8-gon "circle" hole, exactly as an old event would
        // have materialised it.
        let hole: Vec<[f64; 2]> = (0..8)
            .map(|i| {
                let a = (i as f64) * std::f64::consts::TAU / 8.0;
                [20.0 + 6.0 * a.cos(), 15.0 + 6.0 * a.sin()]
            })
            .collect();
        let event = mk_event(
            "sketch_extrude",
            serde_json::json!({
                "params": {
                    "origin": [0.0, 0.0, 0.0],
                    "u_axis": [1.0, 0.0, 0.0],
                    "v_axis": [0.0, 1.0, 0.0],
                    "regions": [{
                        "outer": [[0.0, 0.0], [40.0, 0.0], [40.0, 30.0], [0.0, 30.0]],
                        "holes": [hole],
                    }],
                    "distance": 10.0,
                    "direction": [0.0, 0.0, 1.0],
                },
                "inputs": [],
                "outputs": [99]
            }),
        );
        let outcome = rebuild_model_from_events(&mut model, &[event]);
        assert_eq!(outcome.events_applied, 1, "legacy event must apply");
        assert_eq!(outcome.events_skipped, 0);
        let solid = only_solid(&model);
        assert!(
            cylinder_face_radii(&model, solid).is_empty(),
            "legacy polygon payloads replay as recorded (planar facets), \
             never silently upgraded to analytic faces"
        );
    }

    /// SKETCH-DCM #45 Slice 6: sketch-op events (trim / offset /
    /// mirror / patterns / construction) are design-history records —
    /// the sketch lives in the api-server, and the downstream
    /// `sketch_extrude` event is fully self-contained. A timeline
    /// carrying them must replay with `events_skipped == 0` and a
    /// model state identical to the extrude event alone.
    #[test]
    fn replay_csketch_op_events_are_design_history_records_with_nil_model_effect() {
        let op_event = |kind: &str| {
            mk_event(
                kind,
                serde_json::json!({
                    "params": {
                        "csketch_id": "6a1f0c9e-2c1e-4b3a-9a53-1de1cbbf0000",
                        "distance": 5.0,
                    },
                    "inputs": [],
                    "outputs": []
                }),
            )
        };
        let extrude_event = mk_event(
            "sketch_extrude",
            serde_json::json!({
                "params": {
                    "origin": [0.0, 0.0, 0.0],
                    "u_axis": [1.0, 0.0, 0.0],
                    "v_axis": [0.0, 1.0, 0.0],
                    "regions": [{
                        "outer": [[0.0, 0.0], [40.0, 0.0], [40.0, 30.0], [0.0, 30.0]],
                        "holes": [{ "edges": [
                            { "kind": "circle", "center": [20.0, 15.0], "radius": 6.0 }
                        ]}],
                    }],
                    "distance": 10.0,
                    "direction": [0.0, 0.0, 1.0],
                },
                "inputs": [],
                "outputs": [99]
            }),
        );

        // Sketch-op events alone leave the model untouched.
        let mut ops_only = BRepModel::new();
        let events: Vec<_> = [
            "csketch_trim",
            "csketch_extend",
            "csketch_offset",
            "csketch_mirror",
            "csketch_pattern_linear",
            "csketch_pattern_circular",
            "csketch_pattern_curve",
            "csketch_pattern_phyllotaxis",
            "csketch_construction",
        ]
        .iter()
        .map(|k| op_event(k))
        .collect();
        let outcome = rebuild_model_from_events(&mut ops_only, &events);
        assert_eq!(outcome.events_applied, 9, "all op kinds must apply");
        assert_eq!(
            outcome.events_skipped, 0,
            "no skips — full-timeline honesty"
        );
        assert_eq!(ops_only.solids.len(), 0, "nil model effect by construction");

        // Ops + extrude replay to the SAME state as the extrude alone.
        let mut with_ops = BRepModel::new();
        let mut sequence = events;
        sequence.push(extrude_event.clone());
        let outcome = rebuild_model_from_events(&mut with_ops, &sequence);
        assert_eq!(outcome.events_applied, 10);
        assert_eq!(outcome.events_skipped, 0);

        let mut extrude_only = BRepModel::new();
        rebuild_model_from_events(&mut extrude_only, &[extrude_event]);
        let (s1, s2) = (only_solid(&with_ops), only_solid(&extrude_only));
        assert_eq!(
            cylinder_face_radii(&with_ops, s1),
            cylinder_face_radii(&extrude_only, s2),
            "identical replayed state with or without the sketch-op events"
        );
    }

    /// SKETCH-DCM #45 Slice 7: a `sketch_extrude` event whose outer
    /// loop carries a typed NURBS edge (`{"kind": "nurbs", ...}`)
    /// replays through the SAME kernel entry as the live analytic
    /// build — an organic (spline-walled) profile round-trips through
    /// the timeline as exact geometry, never a re-sampled polygon.
    #[test]
    fn replay_sketch_extrude_typed_nurbs_edges_rebuilds_solid() {
        let mut model = BRepModel::new();
        // Base line (0,0)->(30,0) plus a clamped cubic arch back from
        // (30,0) to (0,0) — a closed two-edge organic profile.
        let event = mk_event(
            "sketch_extrude",
            serde_json::json!({
                "params": {
                    "origin": [0.0, 0.0, 0.0],
                    "u_axis": [1.0, 0.0, 0.0],
                    "v_axis": [0.0, 1.0, 0.0],
                    "regions": [{
                        "outer": { "edges": [
                            { "kind": "line", "start": [0.0, 0.0], "end": [30.0, 0.0] },
                            { "kind": "nurbs",
                              "degree": 3,
                              "control_points": [[30.0, 0.0], [28.0, 12.0], [2.0, 12.0], [0.0, 0.0]],
                              "weights": null,
                              "knots": [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0] }
                        ]},
                        "holes": [],
                    }],
                    "distance": 5.0,
                    "direction": [0.0, 0.0, 1.0],
                },
                "inputs": [],
                "outputs": [99]
            }),
        );
        let outcome = rebuild_model_from_events(&mut model, &[event]);
        assert_eq!(outcome.events_applied, 1, "typed NURBS event must apply");
        assert_eq!(outcome.events_skipped, 0);
        let solid = only_solid(&model);
        // 2 caps + 1 planar wall + 1 NURBS ruled wall.
        let face_count = model
            .solid_outer_face_count(solid)
            .expect("outer face count");
        assert_eq!(face_count, 4, "2 caps + line wall + NURBS wall");
        let volume = model
            .calculate_solid_volume(solid)
            .expect("volume computable");
        // Green's-theorem area of the arch profile is 208.8 (dense
        // boundary quadrature over the exact cubic, converged to
        // 1e-8); the mesh volume oracle tessellates the true NURBS
        // ruled wall adaptively — 2e-3 relative bounds it well clear
        // of any sampled-polygon signature.
        let expected = 208.8 * 5.0;
        let rel = (volume - expected).abs() / expected;
        assert!(
            rel < 2e-3,
            "replayed organic volume must match the boundary oracle: {volume} vs {expected} (rel {rel})"
        );
    }

    /// SKETCH-DCM #45 follow-ups B (item 2): a `sketch_extrude` event
    /// whose outer loop is ONE CLOSED typed NURBS edge (first CP ==
    /// last CP) replays to a SOUND solid — the kernel seam-splits the
    /// closed edge into two open exact halves, so the old
    /// zero-triangle closed-ruled refusal is retired and typed closed
    /// blobs round-trip through the timeline as exact geometry.
    #[test]
    fn replay_sketch_extrude_closed_nurbs_edge_rebuilds_seam_split_solid() {
        let mut model = BRepModel::new();
        let event = mk_event(
            "sketch_extrude",
            serde_json::json!({
                "params": {
                    "origin": [0.0, 0.0, 0.0],
                    "u_axis": [1.0, 0.0, 0.0],
                    "v_axis": [0.0, 1.0, 0.0],
                    "regions": [{
                        "outer": { "edges": [
                            { "kind": "nurbs",
                              "degree": 3,
                              "control_points": [
                                  [10.0, 0.0], [14.0, 9.0], [-2.0, 12.0],
                                  [-8.0, 2.0], [2.0, -7.0], [10.0, 0.0]
                              ],
                              "weights": null,
                              "knots": [0.0, 0.0, 0.0, 0.0, 1.0/3.0, 2.0/3.0,
                                        1.0, 1.0, 1.0, 1.0] }
                        ]},
                        "holes": [],
                    }],
                    "distance": 5.0,
                    "direction": [0.0, 0.0, 1.0],
                },
                "inputs": [],
                "outputs": [99]
            }),
        );
        let outcome = rebuild_model_from_events(&mut model, &[event]);
        assert_eq!(outcome.events_applied, 1, "closed-NURBS event must apply");
        assert_eq!(outcome.events_skipped, 0);
        let solid = only_solid(&model);
        let gt = model.ground_truth(solid).expect("ground truth");
        assert!(
            gt.certificate.is_sound(),
            "replayed closed-blob solid must be SOUND: {:?}",
            gt.certificate
        );
        let face_count = model
            .solid_outer_face_count(solid)
            .expect("outer face count");
        assert_eq!(face_count, 4, "2 caps + 2 seam-split NURBS walls");
    }

    /// SKETCH-DCM #45 follow-ups B (item 5): a `sketch_revolve` event
    /// with typed Line edges replays through the SAME kernel entry the
    /// live csketch route uses — the washer rebuilds as 4 analytic
    /// faces (2 Cylinder bands at the exact radii + 2 planar annuli),
    /// never a band explosion or a sampled ring.
    #[test]
    fn replay_sketch_revolve_typed_edges_rebuilds_analytic_washer() {
        let mut model = BRepModel::new();
        let event = mk_event(
            "sketch_revolve",
            serde_json::json!({
                "params": {
                    "origin": [0.0, 0.0, 0.0],
                    "u_axis": [1.0, 0.0, 0.0],
                    "v_axis": [0.0, 1.0, 0.0],
                    "regions": [{
                        "outer": { "edges": [
                            { "kind": "line", "start": [5.0, 0.0], "end": [8.0, 0.0] },
                            { "kind": "line", "start": [8.0, 0.0], "end": [8.0, 2.0] },
                            { "kind": "line", "start": [8.0, 2.0], "end": [5.0, 2.0] },
                            { "kind": "line", "start": [5.0, 2.0], "end": [5.0, 0.0] }
                        ]},
                        "holes": [],
                    }],
                    "axis_origin": [0.0, 0.0],
                    "axis_direction": [0.0, 1.0],
                    "angle": std::f64::consts::TAU,
                    "segments": 48,
                },
                "inputs": [],
                "outputs": [99]
            }),
        );
        let outcome = rebuild_model_from_events(&mut model, &[event]);
        assert_eq!(outcome.events_applied, 1, "revolve event must apply");
        assert_eq!(outcome.events_skipped, 0);
        let solid = only_solid(&model);
        let radii = cylinder_face_radii(&model, solid);
        assert_eq!(
            radii.len(),
            2,
            "washer replay must rebuild TWO analytic cylinder bands, got {radii:?}"
        );
        let mut sorted = radii.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        assert!(
            (sorted[0] - 5.0).abs() < 1e-9 && (sorted[1] - 8.0).abs() < 1e-9,
            "replayed band radii must be exact: {sorted:?}"
        );
        let face_count = model
            .solid_outer_face_count(solid)
            .expect("outer face count");
        assert_eq!(face_count, 4, "2 cylinder bands + 2 planar annuli");
        let gt = model.ground_truth(solid).expect("ground truth");
        assert!(
            gt.certificate.is_sound(),
            "replayed washer must be SOUND: {:?}",
            gt.certificate
        );
    }

    /// SKETCH-DCM #45 follow-ups B (item 6): the payload shape the
    /// CLICK-DRAFT extrude now records for a circle shape — a typed
    /// analytic circle OUTER loop — replays to the analytic cylinder
    /// solid (2 caps + 1 Cylinder lateral at the exact radius), i.e.
    /// the SAME solid the live click-draft build produced. Pre-item-6
    /// click-draft events recorded a 64-gon polygon here and replay
    /// rebuilt a 66-face prism — live-vs-replay drift, now retired
    /// (old polygon events still replay unchanged: the legacy pin
    /// above stands).
    #[test]
    fn replay_click_draft_circle_event_rebuilds_analytic_cylinder() {
        let mut model = BRepModel::new();
        let event = mk_event(
            "sketch_extrude",
            serde_json::json!({
                "params": {
                    "origin": [0.0, 0.0, 0.0],
                    "u_axis": [1.0, 0.0, 0.0],
                    "v_axis": [0.0, 1.0, 0.0],
                    "regions": [{
                        "outer": { "edges": [
                            { "kind": "circle", "center": [12.5, -3.0], "radius": 4.0 }
                        ]},
                        "holes": [],
                    }],
                    "distance": 6.0,
                    "direction": [0.0, 0.0, 1.0],
                },
                "inputs": [],
                "outputs": [99]
            }),
        );
        let outcome = rebuild_model_from_events(&mut model, &[event]);
        assert_eq!(
            outcome.events_applied, 1,
            "click-draft circle event applies"
        );
        assert_eq!(outcome.events_skipped, 0);
        let solid = only_solid(&model);
        let radii = cylinder_face_radii(&model, solid);
        assert_eq!(
            radii.len(),
            1,
            "the replayed boss lateral is ONE analytic cylinder face"
        );
        let radius = radii.first().copied().expect("radius");
        assert!(
            (radius - 4.0).abs() < 1e-9,
            "replayed radius must be exact: {radius}"
        );
        let face_count = model
            .solid_outer_face_count(solid)
            .expect("outer face count");
        assert_eq!(
            face_count, 3,
            "2 caps + 1 cylinder lateral (not a 64-gon prism)"
        );
    }

    /// A sketch-op event with no `csketch_id` is malformed and must be
    /// rejected (skipped with a logged error), not silently accepted.
    #[test]
    fn replay_csketch_op_event_without_sketch_id_is_invalid() {
        let mut model = BRepModel::new();
        let event = mk_event(
            "csketch_offset",
            serde_json::json!({ "params": { "distance": 5.0 }, "inputs": [], "outputs": [] }),
        );
        let outcome = rebuild_model_from_events(&mut model, &[event]);
        assert_eq!(outcome.events_applied, 0);
        assert_eq!(
            outcome.events_skipped, 1,
            "malformed op event must be skipped"
        );
    }
}
