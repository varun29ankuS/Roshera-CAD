//! The one channel: kernel wire entity refs ⇄ the event's typed lineage fields.
//!
//! # The problem this module exists to close
//!
//! A [`TimelineEvent`](crate::TimelineEvent) carries two descriptions of what an
//! operation touched:
//!
//! 1. the **wire envelope** — `Operation::Generic { parameters }` with
//!    `parameters.inputs` / `.outputs` / `.deleted`, each a list of the kernel's
//!    canonical `"<kind>:<id>"` refs, written verbatim by
//!    [`recorder_bridge::to_timeline_operation`](crate::recorder_bridge) from what
//!    the kernel's `RecordedOperation` actually declared;
//! 2. the **typed fields** — [`OperationInputs`] / [`OperationOutputs`], which
//!    ~19 production consumers (the storage entity indices, merge's affected-set,
//!    undo/redo's affected list, entity-state reconstruction, the operation cache)
//!    read as the truth about the event.
//!
//! Historically only (1) was populated on the kernel path and (2) was built empty,
//! so every one of those consumers read "this operation affected nothing" for
//! every real geometry operation — an answer that raises no error and that no gate
//! can see. This module makes (2) a faithful re-presentation of (1) rather than a
//! second, emptier story.
//!
//! # Why an encoding rather than a hash
//!
//! [`EntityId`] is a 128-bit UUID; a kernel ref is a `kind` tag plus an integer
//! that lives in a per-kind counter namespace (`face:1` and `solid:1` are
//! different entities). A one-way hash of the ref string would give a usable id,
//! but it would lose two things that matter:
//!
//! * [`OperationOutputs::modified`] and [`OperationOutputs::deleted`] are bare
//!   `Vec<EntityId>` — **no `EntityType` travels with them**. Under a hash, the
//!   kind of a deleted `face:7` would be unrecoverable, and requirement "preserve
//!   entity kinds" would fail on exactly the channel that cannot carry a kind.
//! * [`lineage`](crate::lineage) renders refs as `"<kind>:<id>"` strings and
//!   unions the wire and typed channels. Under a hash the two channels would
//!   render *different strings for the same entity*, doubling every node in the
//!   lineage DAG.
//!
//! So a ref whose id is an integer is encoded **losslessly and reversibly** into
//! the UUID: the kind and the integer are both recoverable from the `EntityId`
//! alone ([`decode`]). The typed channel then renders byte-identically to the wire
//! channel, the union in `lineage` becomes a no-op, and the bare channels carry
//! their kind inside the id.
//!
//! # Layout
//!
//! ```text
//! byte  0 1 2 3 4   5      6 7      8 .. 15
//!      [ MAGIC   ][kind][ 0x0000 ][ id: u64 big-endian ]
//! ```
//!
//! Bytes 6–7 are zero, which puts `0` in the UUID version nibble. No RFC 4122
//! generator emits version 0, so an encoded ref can never collide with a `v4`
//! (viewport / DTO) or `v5` (derived) id; the 5-byte magic makes an accidental
//! collision with a hand-supplied UUID a `2^-56` event. The kind codes are part of
//! the serialized event and are therefore **stable forever** — append, never
//! renumber.
//!
//! # The one case that cannot be encoded
//!
//! Assembly-side refs (`assembly:<uuid>`, `component:<uuid>`, `mate:<uuid>`) carry
//! a 128-bit UUID id. A kind plus 128 bits does not fit in 128 bits, so those refs
//! are carried as the UUID **itself** — faithful, and still rendering back to the
//! exact wire string — but their kind then lives only in the channel's
//! `EntityType`. Such a ref is therefore admissible in the kind-carrying channels
//! ([`OperationInputs::required_entities`], [`OperationOutputs::created`]) and
//! **refused** in the bare ones, rather than being silently stripped of its kind.
//!
//! # All-or-nothing
//!
//! [`project_envelope`] fails the **whole event** if any single ref cannot be
//! represented. A partially populated typed channel is the original defect at
//! smaller scale: a consumer reading two of three `required_entities` has no way
//! to know one is missing. On refusal the caller leaves the typed fields empty and
//! logs why — the envelope stays the event's sole lineage channel, exactly as it
//! was before this module existed.

use crate::types::{
    CreatedEntity, EntityId, EntityReference, EntityType, OperationInputs, OperationOutputs,
    ValidationRequirement,
};
use std::collections::HashSet;
use thiserror::Error;
use uuid::Uuid;

/// Magic prefix marking an [`EntityId`] as an encoded kernel wire ref.
const KERNEL_REF_MAGIC: [u8; 5] = *b"Roshk";

/// A wire ref parsed into the typed channel's representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedRef {
    /// The kind the ref named, as an [`EntityType`].
    pub entity_type: EntityType,
    /// The id the typed channels carry.
    pub id: EntityId,
    /// `true` when [`decode`] can recover the kind from `id` alone — i.e. the
    /// ref may travel in a bare `Vec<EntityId>` channel without losing its kind.
    pub kind_in_id: bool,
}

/// Why a single kernel wire ref could not be carried by the typed channels.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum KernelRefError {
    /// Not of the canonical `"<kind>:<id>"` shape.
    #[error("entity ref {reference:?} is not of the canonical \"<kind>:<id>\" form")]
    Malformed {
        /// The offending ref, verbatim.
        reference: String,
    },

    /// The kind tag is not one the timeline's [`EntityType`] can express. Stated
    /// rather than coerced — a ref of an unknown kind is never re-labelled.
    #[error("entity ref {reference:?} names kind {kind:?}, which EntityType cannot express")]
    UnknownKind {
        /// The offending ref, verbatim.
        reference: String,
        /// The kind tag that has no [`EntityType`].
        kind: String,
    },

    /// The id is neither an integer nor a UUID, so no [`EntityId`] represents it.
    #[error("entity ref {reference:?} has an id that is neither an integer nor a UUID")]
    UnrepresentableId {
        /// The offending ref, verbatim.
        reference: String,
    },

    /// A UUID-valued ref whose UUID happens to be shaped like an encoded ref.
    /// Refused rather than decoded as some other entity.
    #[error("entity ref {reference:?} carries a UUID that collides with the kernel-ref encoding")]
    AmbiguousId {
        /// The offending ref, verbatim.
        reference: String,
    },
}

/// Why a kernel envelope could not be projected onto the typed channels.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum EnvelopeError {
    /// A ref in `channel` could not be represented.
    #[error("{channel} channel: {source}")]
    Ref {
        /// `"inputs"`, `"outputs"`, or `"deleted"`.
        channel: &'static str,
        /// The per-ref reason.
        #[source]
        source: KernelRefError,
    },

    /// A lineage channel held something that is not a ref string.
    #[error("{channel} channel contains a non-string entry")]
    NonStringRef {
        /// `"inputs"`, `"outputs"`, or `"deleted"`.
        channel: &'static str,
    },

    /// The ref would land in `outputs.modified` or `outputs.deleted`, which carry
    /// no [`EntityType`], yet its kind is not recoverable from its id. Refused
    /// rather than filed under a kind it does not have.
    #[error(
        "entity ref {reference:?} would lose its kind in the untyped {channel} channel \
         (its id carries no kind and the channel carries no EntityType)"
    )]
    KindLostInBareChannel {
        /// The offending ref, verbatim.
        reference: String,
        /// `"modified"` or `"deleted"`.
        channel: &'static str,
    },

    /// The same ref is claimed as both produced and deleted by one operation.
    /// `ExecutionEngine::validate_outputs` rejects such an event, so authoring
    /// one would put an invalid event in the log.
    #[error("entity ref {reference:?} appears in both the outputs and deleted channels")]
    OutputAlsoDeleted {
        /// The offending ref, verbatim.
        reference: String,
    },
}

/// The canonical wire tag for an [`EntityType`] — the exact string the kernel's
/// `RecordedOperation` uses (`geometry_engine::operations::recorder::ENTITY_*`).
///
/// Total and injective, so [`entity_type_for_tag`] is its exact inverse.
pub fn wire_tag(entity_type: EntityType) -> &'static str {
    match entity_type {
        EntityType::Sketch => "sketch",
        EntityType::Solid => "solid",
        EntityType::Surface => "surface",
        EntityType::Curve => "curve",
        EntityType::Point => "point",
        EntityType::Edge => "edge",
        EntityType::Face => "face",
        EntityType::Vertex => "vertex",
        EntityType::Loop => "loop",
        EntityType::Datum => "datum",
        EntityType::Assembly => "assembly",
        EntityType::Component => "component",
        EntityType::Mate => "mate",
    }
}

/// The [`EntityType`] a wire kind tag names, or `None` when the tag is not one
/// the timeline can express. Exact inverse of [`wire_tag`].
pub fn entity_type_for_tag(tag: &str) -> Option<EntityType> {
    Some(match tag {
        "sketch" => EntityType::Sketch,
        "solid" => EntityType::Solid,
        "surface" => EntityType::Surface,
        "curve" => EntityType::Curve,
        "point" => EntityType::Point,
        "edge" => EntityType::Edge,
        "face" => EntityType::Face,
        "vertex" => EntityType::Vertex,
        "loop" => EntityType::Loop,
        "datum" => EntityType::Datum,
        "assembly" => EntityType::Assembly,
        "component" => EntityType::Component,
        "mate" => EntityType::Mate,
        _ => return None,
    })
}

/// Stable on-the-wire code for a kind. **Never renumber**: these bytes live
/// inside serialized [`EntityId`]s in persisted events.
fn kind_code(entity_type: EntityType) -> u8 {
    match entity_type {
        EntityType::Sketch => 1,
        EntityType::Solid => 2,
        EntityType::Surface => 3,
        EntityType::Curve => 4,
        EntityType::Point => 5,
        EntityType::Edge => 6,
        EntityType::Face => 7,
        EntityType::Vertex => 8,
        EntityType::Loop => 9,
        EntityType::Datum => 10,
        EntityType::Assembly => 11,
        EntityType::Component => 12,
        EntityType::Mate => 13,
    }
}

/// Inverse of [`kind_code`]; `None` for a code this build does not know.
fn kind_for_code(code: u8) -> Option<EntityType> {
    Some(match code {
        1 => EntityType::Sketch,
        2 => EntityType::Solid,
        3 => EntityType::Surface,
        4 => EntityType::Curve,
        5 => EntityType::Point,
        6 => EntityType::Edge,
        7 => EntityType::Face,
        8 => EntityType::Vertex,
        9 => EntityType::Loop,
        10 => EntityType::Datum,
        11 => EntityType::Assembly,
        12 => EntityType::Component,
        13 => EntityType::Mate,
        _ => return None,
    })
}

/// Encode an integer-valued kernel ref into an [`EntityId`] that carries both
/// its kind and its id (see the module docs for the layout).
pub fn encode(entity_type: EntityType, id: u64) -> EntityId {
    let mut bytes = [0u8; 16];
    bytes[..5].copy_from_slice(&KERNEL_REF_MAGIC);
    bytes[5] = kind_code(entity_type);
    // bytes[6..8] deliberately left zero — UUID version nibble 0.
    bytes[8..].copy_from_slice(&id.to_be_bytes());
    EntityId(Uuid::from_bytes(bytes))
}

/// Recover `(kind, id)` from an [`EntityId`] produced by [`encode`].
///
/// `None` for every other id — a viewport `v4`, a DTO-supplied UUID, a derived
/// `v5` — which is exactly the structural discriminator between the two id
/// spaces: an id that decodes is a kernel entity, an id that does not is not.
pub fn decode(id: EntityId) -> Option<(EntityType, u64)> {
    let bytes = id.0.as_bytes();
    if bytes[..5] != KERNEL_REF_MAGIC || bytes[6] != 0 || bytes[7] != 0 {
        return None;
    }
    let entity_type = kind_for_code(bytes[5])?;
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&bytes[8..]);
    Some((entity_type, u64::from_be_bytes(raw)))
}

/// Render a typed `(kind, id)` pair back to its wire ref string.
///
/// For an encoded ref this reproduces the kernel's original string exactly
/// (`solid:42`), which is what makes the typed and wire lineage channels
/// render identically. For any other id the UUID is rendered as-is, matching
/// the pre-existing behaviour for DTO-layer events.
pub fn render_ref(entity_type: EntityType, id: EntityId) -> String {
    match decode(id) {
        Some((_, numeric)) => format!("{}:{}", wire_tag(entity_type), numeric),
        None => format!("{}:{}", wire_tag(entity_type), id),
    }
}

/// Render a bare [`EntityId`] (one travelling in a channel that carries no
/// `EntityType`) to its wire ref string, when its kind is recoverable from the
/// id itself. `None` when it is not — the caller must then say so rather than
/// guess a kind.
pub fn render_bare(id: EntityId) -> Option<String> {
    decode(id).map(|(entity_type, numeric)| format!("{}:{}", wire_tag(entity_type), numeric))
}

/// Parse one canonical `"<kind>:<id>"` kernel wire ref.
pub fn parse_ref(reference: &str) -> Result<ParsedRef, KernelRefError> {
    let malformed = || KernelRefError::Malformed {
        reference: reference.to_string(),
    };
    let (tag, rest) = reference.split_once(':').ok_or_else(malformed)?;
    if tag.is_empty() || rest.is_empty() {
        return Err(malformed());
    }
    let entity_type = entity_type_for_tag(tag).ok_or_else(|| KernelRefError::UnknownKind {
        reference: reference.to_string(),
        kind: tag.to_string(),
    })?;

    if let Ok(numeric) = rest.parse::<u64>() {
        return Ok(ParsedRef {
            entity_type,
            id: encode(entity_type, numeric),
            kind_in_id: true,
        });
    }

    if let Ok(uuid) = Uuid::parse_str(rest) {
        let id = EntityId(uuid);
        // A UUID that decodes as an encoded ref would be two entities with one
        // id. Refuse it; never resolve the ambiguity by picking one.
        if decode(id).is_some() {
            return Err(KernelRefError::AmbiguousId {
                reference: reference.to_string(),
            });
        }
        return Ok(ParsedRef {
            entity_type,
            id,
            kind_in_id: false,
        });
    }

    Err(KernelRefError::UnrepresentableId {
        reference: reference.to_string(),
    })
}

/// Read one lineage channel out of a kernel envelope.
///
/// An absent key, or a key whose value is not an array, is **no lineage claim**
/// and yields the empty list: `Operation::Generic` is also used by non-kernel
/// producers (the AI command processor's `export` / `change_view` events) whose
/// `parameters` are unrelated JSON. A key that *is* an array of refs is a claim,
/// and every element of it must be a string.
fn wire_list(
    parameters: &serde_json::Value,
    channel: &'static str,
) -> Result<Vec<String>, EnvelopeError> {
    let Some(array) = parameters.get(channel).and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    array
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .map(str::to_string)
                .ok_or(EnvelopeError::NonStringRef { channel })
        })
        .collect()
}

/// Deduplicate refs, preserving first-occurrence order (the order the kernel
/// recorded them in), and parse each exactly once.
fn parse_channel(
    refs: &[String],
    channel: &'static str,
) -> Result<Vec<(String, ParsedRef)>, EnvelopeError> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out: Vec<(String, ParsedRef)> = Vec::with_capacity(refs.len());
    for reference in refs {
        if !seen.insert(reference.as_str()) {
            continue;
        }
        let parsed =
            parse_ref(reference).map_err(|source| EnvelopeError::Ref { channel, source })?;
        out.push((reference.clone(), parsed));
    }
    Ok(out)
}

/// Project a kernel envelope's recorded lineage onto the event's typed channels.
///
/// This **reads** what the kernel recorded; it never re-derives lineage from the
/// operation's parameters. The mapping is total and rule-based:
///
/// | recorded | typed |
/// |---|---|
/// | `inputs` | [`OperationInputs::required_entities`], each [`ValidationRequirement::MustExist`] |
/// | `outputs` ∖ `inputs` | [`OperationOutputs::created`] |
/// | `outputs` ∩ `inputs` | [`OperationOutputs::modified`] |
/// | `deleted` | [`OperationOutputs::deleted`] |
///
/// The created/modified split is the **only** inference here, and it is the same
/// rule the existing lineage projection already applies: `lineage::LineageGraph::build`
/// treats a ref that is both an input and an output as a modification (it skips
/// the degenerate self-edge rather than recording production). An entity the
/// operation both consumed and produced was modified, not created; one it produced
/// without consuming was created. Nothing else is inferred:
///
/// * `optional_entities` stays empty — the kernel records no optional channel, and
///   inventing one would be a claim it never made.
/// * `side_effects` stays empty — nothing recorded maps onto it.
/// * `inputs.parameters` stays `Null` — the parameters already live, verbatim, in
///   the operation's own envelope. Copying them here would create a second copy
///   that can drift, which is the very failure mode this module exists to remove.
pub fn project_envelope(
    parameters: &serde_json::Value,
) -> Result<(OperationInputs, OperationOutputs), EnvelopeError> {
    let input_refs = parse_channel(&wire_list(parameters, "inputs")?, "inputs")?;
    let output_refs = parse_channel(&wire_list(parameters, "outputs")?, "outputs")?;
    let deleted_refs = parse_channel(&wire_list(parameters, "deleted")?, "deleted")?;

    let input_keys: HashSet<&str> = input_refs.iter().map(|(key, _)| key.as_str()).collect();
    let output_keys: HashSet<&str> = output_refs.iter().map(|(key, _)| key.as_str()).collect();

    let mut created: Vec<CreatedEntity> = Vec::new();
    let mut modified: Vec<EntityId> = Vec::new();
    for (key, parsed) in &output_refs {
        if input_keys.contains(key.as_str()) {
            if !parsed.kind_in_id {
                return Err(EnvelopeError::KindLostInBareChannel {
                    reference: key.clone(),
                    channel: "modified",
                });
            }
            modified.push(parsed.id);
        } else {
            created.push(CreatedEntity {
                id: parsed.id,
                entity_type: parsed.entity_type,
                name: None,
            });
        }
    }

    let mut deleted: Vec<EntityId> = Vec::new();
    for (key, parsed) in &deleted_refs {
        if output_keys.contains(key.as_str()) {
            return Err(EnvelopeError::OutputAlsoDeleted {
                reference: key.clone(),
            });
        }
        if !parsed.kind_in_id {
            return Err(EnvelopeError::KindLostInBareChannel {
                reference: key.clone(),
                channel: "deleted",
            });
        }
        deleted.push(parsed.id);
    }

    let required_entities = input_refs
        .iter()
        .map(|(_, parsed)| EntityReference {
            id: parsed.id,
            expected_type: parsed.entity_type,
            validation: ValidationRequirement::MustExist,
        })
        .collect();

    Ok((
        OperationInputs {
            required_entities,
            optional_entities: Vec::new(),
            parameters: serde_json::Value::Null,
        },
        OperationOutputs {
            created,
            modified,
            deleted,
            side_effects: Vec::new(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `EntityType` round-trips tag → type → tag and code → type → code.
    /// A new variant added without extending both tables fails here.
    #[test]
    fn every_entity_type_round_trips() {
        let all = [
            EntityType::Sketch,
            EntityType::Solid,
            EntityType::Surface,
            EntityType::Curve,
            EntityType::Point,
            EntityType::Edge,
            EntityType::Face,
            EntityType::Vertex,
            EntityType::Loop,
            EntityType::Datum,
            EntityType::Assembly,
            EntityType::Component,
            EntityType::Mate,
        ];
        let mut codes: HashSet<u8> = HashSet::new();
        let mut tags: HashSet<&str> = HashSet::new();
        for entity_type in all {
            let tag = wire_tag(entity_type);
            assert!(tags.insert(tag), "duplicate wire tag {tag:?}");
            assert_eq!(entity_type_for_tag(tag), Some(entity_type));
            let code = kind_code(entity_type);
            assert!(codes.insert(code), "duplicate kind code {code}");
            assert_eq!(kind_for_code(code), Some(entity_type));
        }
    }

    /// The encoding is reversible: kind AND numeric id come back out of the id
    /// alone, which is what lets the bare `modified`/`deleted` channels keep a
    /// kind they have no field for.
    #[test]
    fn encoding_is_reversible_including_the_kind() {
        for entity_type in [EntityType::Solid, EntityType::Face, EntityType::Datum] {
            for numeric in [0u64, 1, 42, u64::MAX] {
                let id = encode(entity_type, numeric);
                assert_eq!(decode(id), Some((entity_type, numeric)));
                assert_eq!(
                    render_bare(id),
                    Some(format!("{}:{}", wire_tag(entity_type), numeric))
                );
            }
        }
    }

    /// A `face:9` must never read back as a solid.
    #[test]
    fn a_face_ref_stays_a_face() {
        let parsed = parse_ref("face:9").expect("face ref parses");
        assert_eq!(parsed.entity_type, EntityType::Face);
        assert_ne!(parsed.entity_type, EntityType::Solid);
        assert!(parsed.kind_in_id);
        assert_eq!(render_ref(parsed.entity_type, parsed.id), "face:9");
        // Same integer, different kind ⇒ different entity.
        assert_ne!(parsed.id, parse_ref("solid:9").expect("solid ref").id);
    }

    /// No generated UUID can be mistaken for an encoded ref: v4 and v5 both put
    /// a non-zero version nibble where the encoding keeps a zero byte.
    #[test]
    fn generated_uuids_never_decode_as_kernel_refs() {
        for _ in 0..256 {
            assert_eq!(decode(EntityId(Uuid::new_v4())), None);
        }
        assert_eq!(
            decode(EntityId(Uuid::new_v5(&Uuid::NAMESPACE_URL, b"solid:1"))),
            None
        );
    }

    /// An unknown kind is reported, never coerced to `Solid`.
    #[test]
    fn unknown_kind_is_stated_not_coerced() {
        let err = parse_ref("gremlin:3").expect_err("unknown kind refused");
        assert_eq!(
            err,
            KernelRefError::UnknownKind {
                reference: "gremlin:3".to_string(),
                kind: "gremlin".to_string(),
            }
        );
    }

    /// A UUID-valued assembly ref is carried as the UUID itself and still
    /// renders back to its exact wire string.
    #[test]
    fn uuid_valued_ref_is_carried_verbatim() {
        let uuid = Uuid::new_v4();
        let reference = format!("assembly:{uuid}");
        let parsed = parse_ref(&reference).expect("assembly ref parses");
        assert_eq!(parsed.entity_type, EntityType::Assembly);
        assert_eq!(parsed.id, EntityId(uuid));
        assert!(
            !parsed.kind_in_id,
            "a raw UUID cannot carry its own kind — the bare channels must refuse it"
        );
        assert_eq!(render_ref(parsed.entity_type, parsed.id), reference);
    }

    /// The recorded envelope becomes the typed channels verbatim, with the
    /// created/modified split following input∩output.
    #[test]
    fn envelope_projects_onto_typed_channels() {
        let parameters = serde_json::json!({
            "params": { "distance": 5.0 },
            "inputs": ["solid:1", "face:9"],
            "outputs": ["solid:1", "edge:4"],
            "deleted": ["face:7"],
        });
        let (inputs, outputs) = project_envelope(&parameters).expect("envelope projects");

        let required: Vec<String> = inputs
            .required_entities
            .iter()
            .map(|r| render_ref(r.expected_type, r.id))
            .collect();
        assert_eq!(required, vec!["solid:1".to_string(), "face:9".to_string()]);
        assert!(inputs.optional_entities.is_empty());
        assert_eq!(inputs.parameters, serde_json::Value::Null);

        let created: Vec<String> = outputs
            .created
            .iter()
            .map(|c| render_ref(c.entity_type, c.id))
            .collect();
        assert_eq!(
            created,
            vec!["edge:4".to_string()],
            "an output that was not also an input was created"
        );
        assert_eq!(
            outputs
                .modified
                .iter()
                .filter_map(|id| render_bare(*id))
                .collect::<Vec<_>>(),
            vec!["solid:1".to_string()],
            "an entity both consumed and produced was modified, not created"
        );
        assert_eq!(
            outputs
                .deleted
                .iter()
                .filter_map(|id| render_bare(*id))
                .collect::<Vec<_>>(),
            vec!["face:7".to_string()],
            "the deleted channel keeps its kind through the untyped Vec<EntityId>"
        );
        assert!(outputs.side_effects.is_empty());
    }

    /// A non-kernel `Operation::Generic` (the AI command processor's `export`
    /// event) carries no lineage arrays and yields empty channels — an honest
    /// "this event affected no entities", not a refusal.
    #[test]
    fn envelope_without_lineage_arrays_is_empty_not_an_error() {
        let parameters = serde_json::json!({ "format": "stl", "inputs": "not-an-array" });
        let (inputs, outputs) = project_envelope(&parameters).expect("no lineage claim");
        assert!(inputs.required_entities.is_empty());
        assert!(outputs.created.is_empty());
    }

    /// A UUID-valued ref that would land in a bare channel is refused for the
    /// whole event rather than filed without its kind.
    #[test]
    fn uuid_ref_in_a_bare_channel_refuses_the_event() {
        let uuid = Uuid::new_v4();
        let parameters = serde_json::json!({
            "inputs": [format!("assembly:{uuid}")],
            "outputs": [format!("assembly:{uuid}")],
        });
        let err = project_envelope(&parameters).expect_err("bare channel refuses a kindless id");
        assert_eq!(
            err,
            EnvelopeError::KindLostInBareChannel {
                reference: format!("assembly:{uuid}"),
                channel: "modified",
            }
        );
    }

    /// One bad ref refuses the WHOLE event — a partially populated typed
    /// channel is the original defect at smaller scale.
    #[test]
    fn one_bad_ref_refuses_the_whole_event() {
        let parameters = serde_json::json!({
            "inputs": ["solid:1", "gremlin:2"],
            "outputs": ["solid:3"],
        });
        let err = project_envelope(&parameters).expect_err("whole event refused");
        assert!(
            matches!(
                err,
                EnvelopeError::Ref {
                    channel: "inputs",
                    ..
                }
            ),
            "expected an inputs-channel ref error, got {err:?}"
        );
    }

    /// An event claiming to both produce and delete one entity is refused —
    /// `ExecutionEngine::validate_outputs` would reject it.
    #[test]
    fn produced_and_deleted_is_refused() {
        let parameters = serde_json::json!({
            "outputs": ["solid:3"],
            "deleted": ["solid:3"],
        });
        assert_eq!(
            project_envelope(&parameters).expect_err("contradiction refused"),
            EnvelopeError::OutputAlsoDeleted {
                reference: "solid:3".to_string(),
            }
        );
    }

    /// Duplicate refs collapse to one entry, in recorded order.
    #[test]
    fn duplicate_refs_are_deduplicated_in_recorded_order() {
        let parameters = serde_json::json!({
            "inputs": ["solid:2", "solid:1", "solid:2"],
            "outputs": [],
        });
        let (inputs, _) = project_envelope(&parameters).expect("envelope projects");
        let required: Vec<String> = inputs
            .required_entities
            .iter()
            .map(|r| render_ref(r.expected_type, r.id))
            .collect();
        assert_eq!(required, vec!["solid:2".to_string(), "solid:1".to_string()]);
    }
}
