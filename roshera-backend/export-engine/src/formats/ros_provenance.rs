//! Derive the `.ros` PROV chunk's [`AICommandTracker`] from the timeline.
//!
//! PROV is a mandatory chunk, but until 2026-08 no live `AICommandTracker`
//! existed anywhere in the server, so every exported file carried a real
//! session id and ZERO commands. That absence was honest then — there was
//! no command source. It stopped being honest the moment intent became a
//! recorded fact: every mutating kernel operation can carry a
//! `roshera.intent` facet (`geometry_engine::operations::recorder::IntentFacet`)
//! inside its recorded operation's `parameters.facets` envelope, and the
//! HIST chunk already transports it verbatim (pinned by
//! `tests/ros_provenance_wire.rs`).
//!
//! [`ai_tracker_from_timeline`] projects that recorded history into the
//! PROV shape: **one [`AICommand`] per recorded operation, in sequence
//! order**, with `timestamp` / `sequence_num` taken from the event and the
//! session id freshly opened at export time (events carry no session of
//! their own; the write-time session is the file's session, same as the
//! empty-PROV path).
//!
//! # Honesty contract
//!
//! * An operation that recorded NO intent still gets its command — the
//!   operation happened — but with **no prompt and no prompt hash**. The
//!   prompt is never synthesised from the op kind, the parameters, or
//!   anything else: "this happened, and no reason was stated" is a true
//!   and useful fact; an invented reason is a lie in exactly the chunk an
//!   IP claim would rest on.
//! * The `command_type` mapping ([`command_type_for_operation`]) only
//!   claims a [`CommandType`] it can honestly stand behind; every op kind
//!   outside the known taxonomy maps to `CommandType::Custom`, never to a
//!   wrong-but-plausible classification.
//! * The prompt HASH is computed **before** the tracking-level gate —
//!   the same order as `AICommandTracker::track_command` in
//!   `ros-format/src/aipr.rs` — so a `TrackingLevel::Basic` file withholds
//!   the intent text yet still carries a SHA-256 commitment to it:
//!   redaction and provability are not in tension.
//! * A present-but-malformed intent facet yields no prompt and no hash
//!   (we cannot attest text we could not parse) but is flagged with the
//!   `roshera.intent:unparseable` tag so it never reads as "no intent was
//!   recorded".

use geometry_engine::operations::recorder::IntentFacet;
use ros_format::util::sha256;
use ros_format::{AICommand, AICommandTracker, CommandType, PrivacySettings, TrackingLevel};
use timeline_engine::{Author, Operation, TimelineEvent};

/// Tag attached to a command whose event carried a `roshera.intent` facet
/// that failed to parse as [`IntentFacet`]. Distinguishes "intent present
/// but unreadable" from "no intent recorded" (which carries no tag).
pub const INTENT_UNPARSEABLE_TAG: &str = "roshera.intent:unparseable";

/// Build an [`AICommandTracker`] whose command log mirrors the given
/// timeline events, one command per event in slice order.
///
/// Uses `PrivacySettings::default()` (no anonymisation, no hash-only
/// mode); the caller-chosen `tracking_level` is the gate that decides
/// whether intent text is stored as the command's `prompt`. The hash
/// commitment is written regardless of the gate — see the module docs.
pub fn ai_tracker_from_timeline(
    events: &[TimelineEvent],
    tracking_level: TrackingLevel,
) -> AICommandTracker {
    let mut tracker = AICommandTracker::new(tracking_level, PrivacySettings::default(), None);
    let session_id = tracker.current_session;

    for event in events {
        let mut cmd = AICommand::new(
            command_type_for_operation(&event.operation),
            model_id_for_author(&event.author),
            // Model VERSION is not recorded on timeline events; 0 states
            // "unknown", it is never invented.
            0,
            session_id,
            // The event's own sequence number, saturating into the PROV
            // u32 field (a >4-billion-event document is beyond the wire
            // format; saturation is visible in the file, never silent
            // renumbering).
            u32::try_from(event.sequence_number).unwrap_or(u32::MAX),
        );

        // Timestamp from the event, not the export wall clock. Clamped at
        // 0 for a (corrupt) pre-epoch timestamp rather than wrapping.
        cmd.timestamp = u64::try_from(event.timestamp.timestamp_millis()).unwrap_or(0);

        // `affected_objects` = the operation's outputs: the recorder
        // bridge's `"<kind>:<id>"` refs for kernel ops, the DTO layer's
        // created-entity ids otherwise.
        cmd.affected_objects = operation_outputs(event);

        match &event.author {
            Author::User { id, .. } => {
                cmd.user_id = Some(id.clone());
            }
            Author::AIAgent { id, model } => {
                cmd.model_name = Some(model.clone());
                cmd.tags.push(format!("agent:{id}"));
            }
            Author::System => {}
        }

        match intent_of(event) {
            // No intent recorded: no prompt, no hash, no tag. The
            // command still exists because the operation did.
            None => {}
            Some(Ok(intent)) => {
                // Commitment FIRST, gate SECOND — mirrors the hash-then-
                // gate order in `AICommandTracker::track_command`
                // (aipr.rs), so a Basic-level file still carries a
                // commitment to the withheld text.
                cmd.prompt_hash = sha256(intent.text.as_bytes());
                if tracking_level.should_track_prompts() {
                    cmd.prompt = Some(intent.text);
                }
                if let Some(turn) = intent.turn_id {
                    cmd.tags.push(format!("turn:{turn}"));
                }
                cmd.tags.push(format!("intent_source:{}", intent.source));
            }
            // Present but unreadable: we cannot attest text we could not
            // parse, so no prompt and no hash — but the malformation is
            // flagged, never silently coerced to "not recorded".
            Some(Err(_)) => {
                cmd.tags.push(INTENT_UNPARSEABLE_TAG.to_string());
            }
        }

        tracker.header.command_count += 1;
        if let Some(session) = tracker.sessions.get_mut(&session_id) {
            session.command_count += 1;
        }
        tracker.commands.push(cmd);
    }

    // Header timestamps reflect the recorded history when there is one;
    // an empty timeline keeps the tracker-creation timestamps.
    if let Some(first) = tracker.commands.first() {
        tracker.header.first_timestamp = first.timestamp;
    }
    if let Some(last) = tracker.commands.last() {
        tracker.header.last_timestamp = last.timestamp;
    }

    tracker
}

/// Map a timeline [`Operation`] onto the PROV [`CommandType`] taxonomy.
///
/// DTO variants map structurally; the kernel recorder's `Generic`
/// envelope maps by its recorded `kind` string via
/// [`command_type_for_kind`]. `CreateCheckpoint` and `Batch` have no
/// honest single classification and fall to `Custom`.
pub fn command_type_for_operation(operation: &Operation) -> CommandType {
    match operation {
        Operation::CreateSketch { .. }
        | Operation::CreatePrimitive { .. }
        | Operation::Extrude { .. }
        | Operation::Revolve { .. }
        | Operation::Loft { .. }
        | Operation::Sweep { .. } => CommandType::Create,
        Operation::BooleanUnion { .. }
        | Operation::BooleanIntersection { .. }
        | Operation::BooleanDifference { .. }
        | Operation::Boolean { .. }
        | Operation::Fillet { .. }
        | Operation::Chamfer { .. }
        | Operation::Pattern { .. }
        | Operation::Modify { .. } => CommandType::Modify,
        Operation::Transform { .. } => CommandType::Transform,
        Operation::Delete { .. } => CommandType::Delete,
        Operation::CreateCheckpoint { .. } | Operation::Batch { .. } => CommandType::Custom(0),
        Operation::Generic { command_type, .. } => command_type_for_kind(command_type),
    }
}

/// Map a recorded operation `kind` string (the kernel recorder's stable
/// identifiers — see `geometry_engine::operations::recorder` module docs)
/// onto [`CommandType`].
///
/// The match is an EXACT allowlist of kinds the production recorders emit
/// today (kernel ops, `assembly.*`, `drawing.*`, `part.*`, `datum_*`,
/// `param.*`). Anything not listed — including future kinds — maps to
/// `Custom(0)`: an unclassified op is stated as unclassified, never
/// guessed into a plausible bucket by substring heuristics.
pub fn command_type_for_kind(kind: &str) -> CommandType {
    match kind {
        // New geometry / entities come into existence.
        "create_box_3d"
        | "extrude_face"
        | "sketch_extrude"
        | "sketch_revolve"
        | "revolve_face"
        | "revolve_meridian"
        | "revolve_typed"
        | "loft_profiles"
        | "nurbs_loft"
        | "sweep_profile"
        | "csketch_construction"
        | "imprint_curves_on_face"
        | "datum_create"
        | "datum_create_derived"
        | "part.create"
        | "drawing.create"
        | "drawing.create_from_part"
        | "drawing.add_view"
        | "assembly.create"
        | "assembly.add_component"
        | "assembly.add_instance"
        | "assembly.add_mate"
        | "assembly.mate_add"
        | "assembly.connector_add"
        | "assembly.register_mate_reference" => CommandType::Create,

        // Existing entities are reshaped or re-described. Booleans and
        // offset mint a fresh result solid but are destructive of their
        // operands — Modify (`is_destructive() == true`) is the honest
        // classification, not Create.
        "boolean_union"
        | "boolean_intersection"
        | "boolean_difference"
        | "fillet_edges"
        | "chamfer_edges"
        | "offset_solid"
        | "set_color"
        | "set_name"
        | "solid_reanchor"
        | "part.rename"
        | "datum_rename"
        | "datum_set_visibility"
        | "drawing.rename"
        | "drawing.title_block.update"
        | "assembly.mate_edit"
        | "assembly.patch_mate"
        | "param.mould"
        | "param.name" => CommandType::Modify,

        // Rigid-motion / placement changes.
        "transform_solid"
        | "transform_faces"
        | "transform_edges"
        | "datum_set_transform"
        | "assembly.set_component_transform"
        | "assembly.transform_instance" => CommandType::Transform,

        // Entities cease to exist.
        "delete_solid"
        | "clear_geometry"
        | "datum_delete"
        | "part.delete"
        | "drawing.delete"
        | "drawing.remove_view"
        | "assembly.delete"
        | "assembly.remove_component"
        | "assembly.remove_instance"
        | "assembly.remove_mate"
        | "assembly.mate_remove"
        | "assembly.connector_remove" => CommandType::Delete,

        // Motion study.
        "assembly.simulate_motion" => CommandType::Simulate,

        // Everything else (incl. `assembly.solve`, `assembly.explode`,
        // `*.noop`, and any future kind): unclassified, stated as such.
        _ => CommandType::Custom(0),
    }
}

/// The operation's outputs as PROV `affected_objects` strings.
///
/// Kernel-recorded events (`Operation::Generic`) carry their outputs in
/// the recorder bridge's envelope (`parameters.outputs`, canonical
/// `"<kind>:<id>"` refs) — used verbatim. DTO-layer events carry them in
/// the event's structured `outputs.created` list instead.
fn operation_outputs(event: &TimelineEvent) -> Vec<String> {
    if let Operation::Generic { parameters, .. } = &event.operation {
        if let Some(outputs) = parameters.get("outputs").and_then(|v| v.as_array()) {
            return outputs
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
        }
    }
    event
        .outputs
        .created
        .iter()
        .map(|created| created.id.to_string())
        .collect()
}

/// The event's `roshera.intent` facet, read from the recorder bridge's
/// envelope (`parameters.facets["roshera.intent"]`).
///
/// * `None` — no intent was recorded (or the event is not a kernel
///   `Generic` envelope, which never carries facets).
/// * `Some(Ok(_))` — present and well-formed.
/// * `Some(Err(_))` — present but malformed; surfaced, never coerced to
///   absence.
fn intent_of(event: &TimelineEvent) -> Option<Result<IntentFacet, serde_json::Error>> {
    let Operation::Generic { parameters, .. } = &event.operation else {
        return None;
    };
    let facet = parameters.get("facets")?.get(IntentFacet::NAME)?;
    Some(serde_json::from_value(facet.clone()))
}

fn model_id_for_author(author: &Author) -> [u8; 32] {
    match author {
        // A deterministic commitment to the RECORDED model name — the
        // same recorded agent always yields the same id, and the id is
        // derived from a fact the event actually carries (never an
        // invented registry handle).
        Author::AIAgent { model, .. } => sha256(model.as_bytes()),
        // No model involved / not recorded: all-zero, the tracker's own
        // "unknown" convention.
        Author::User { .. } | Author::System => [0u8; 32],
    }
}
