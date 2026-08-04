//! Operation recording abstraction (dependency-inversion boundary).
//!
//! Geometry operations need to emit a record of "what happened" so that the
//! timeline engine (and future orchestration layers) can build an
//! event-sourced history. `geometry-engine` cannot depend on `timeline-engine`
//! directly — the dependency already goes the other way — so recording is
//! performed through this trait, which any downstream crate may implement.
//!
//! # Usage
//!
//! A caller (api-server, tests, a script driving the kernel) constructs a
//! concrete recorder, wraps it in `Arc<dyn OperationRecorder>`, and attaches
//! it to the `BRepModel` via `BRepModel::attach_recorder`. Operations then
//! call `model.record(...)` on success; if no recorder is attached the call
//! is a no-op.
//!
//! Failures in the recorder never propagate back into the geometry operation
//! — the operation has already mutated the model successfully. A failed
//! record is logged via `tracing::warn!` so the issue is visible without
//! breaking the kernel.
//!
//! # Lineage ID namespacing
//!
//! `inputs`, `outputs`, and `deleted` are `Vec<String>` whose entries follow the
//! canonical wire form `"<kind>:<id>"`, where `<kind>` is one of
//! [`ENTITY_SOLID`], [`ENTITY_FACE`], [`ENTITY_EDGE`], [`ENTITY_VERTEX`],
//! [`ENTITY_LOOP`], [`ENTITY_CURVE`], [`ENTITY_DATUM`]. Each kernel ID
//! counter (`solid_id`, `face_id`, `edge_id`, …) lives in its own
//! integer namespace inside `BRepModel`, so a bare integer is ambiguous
//! — `face:1` and `solid:1` are distinct entities that previously
//! collided in the lineage graph and produced incorrect parent-child
//! edges in the operation tree. The typed `with_input_*` / `with_output_*` /
//! `with_deleted_*` builders below are the only sanctioned construction
//! sites; callers never assemble the `kind:id` string by hand.

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Entity-kind tag for a solid (`BRepModel::solids`).
pub const ENTITY_SOLID: &str = "solid";
/// Entity-kind tag for a face (`BRepModel::faces`).
pub const ENTITY_FACE: &str = "face";
/// Entity-kind tag for an edge (`BRepModel::edges`).
pub const ENTITY_EDGE: &str = "edge";
/// Entity-kind tag for a vertex (`BRepModel::vertices`).
pub const ENTITY_VERTEX: &str = "vertex";
/// Entity-kind tag for a loop (`BRepModel::loops`).
pub const ENTITY_LOOP: &str = "loop";
/// Entity-kind tag for a curve (`BRepModel::curves`).
pub const ENTITY_CURVE: &str = "curve";
/// Entity-kind tag for a user-authored datum (`BRepModel::datums`).
pub const ENTITY_DATUM: &str = "datum";
/// Entity-kind tag for a top-level assembly (`AssemblyManager`-owned).
/// Assemblies live outside `BRepModel` but share the recorder so their
/// mutations appear in the same timeline / audit stream as kernel ops.
pub const ENTITY_ASSEMBLY: &str = "assembly";
/// Entity-kind tag for an assembly component (one occurrence of a solid
/// inside an assembly, identified by `ComponentId`).
pub const ENTITY_COMPONENT: &str = "component";
/// Entity-kind tag for an assembly mate (one constraint between two
/// `MateReference`s, identified by `MateId`).
pub const ENTITY_MATE: &str = "mate";

/// Format a single entity reference as `"<kind>:<id>"`. The numeric
/// `id` is widened to `u64` so all kernel counter widths fit without
/// loss of information.
#[inline]
pub fn entity_ref(kind: &str, id: u64) -> String {
    format!("{}:{}", kind, id)
}

/// Parse a `"solid:<id>"` wire reference back to a [`SolidId`](crate::primitives::solid::SolidId).
///
/// Total and non-panicking: returns `None` for any reference whose kind tag
/// is not [`ENTITY_SOLID`], whose `<id>` is missing, or whose `<id>` does not
/// parse as a `SolidId` (or overflows it). Inverse of
/// `entity_ref(ENTITY_SOLID, id)`. Used by the certificate-invalidation funnel
/// in `BRepModel::record_operation` to recover the solids an operation
/// touched from its `inputs` / `outputs` lists.
#[inline]
pub fn parse_solid_ref(reference: &str) -> Option<crate::primitives::solid::SolidId> {
    let (kind, id) = reference.split_once(':')?;
    if kind != ENTITY_SOLID {
        return None;
    }
    id.parse::<crate::primitives::solid::SolidId>().ok()
}

/// The kernel proof for the solid a recorded operation produced — the
/// serialization-friendly projection of a
/// [`ValidityCertificate`](crate::primitives::provenance::ValidityCertificate)
/// that a recording handler attaches to a [`RecordedOperation`] so the
/// timeline can store a per-event certificate at record time.
///
/// # Honesty contract
///
/// [`RecordedSolidCertificate::from_validity`] is the only sanctioned
/// construction site: it delegates `is_sound` to the FULL
/// [`ValidityCertificate::is_sound`](crate::primitives::provenance::ValidityCertificate::is_sound)
/// — never a subset of cheap checks — and copies the per-check breakdown
/// verbatim. Volume and face count are the cheap structural facts the
/// handler already computed alongside certification; `None` when
/// unavailable, never fabricated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordedSolidCertificate {
    /// The honest AND verdict — `ValidityCertificate::is_sound()` verbatim.
    pub is_sound: bool,
    /// Tessellated-mesh Euler characteristic (V − E + F).
    pub euler_characteristic: i64,
    /// `validate_solid_scoped` Standard topology verdict.
    pub brep_valid: bool,
    /// Mesh closes (no boundary edges) at the certification chord.
    pub watertight: bool,
    /// Every edge bordered by exactly two faces.
    pub manifold: bool,
    /// Consistently wound, correctly-oriented closed surface.
    pub oriented: bool,
    /// No two non-adjacent faces cross.
    pub self_intersection_free: bool,
    /// Signed volume in model units³, when available.
    pub volume: Option<f64>,
    /// Outer-shell face count, when available.
    pub face_count: Option<usize>,
}

impl RecordedSolidCertificate {
    /// Project the kernel certificate the operation actually proved, plus
    /// the cheap structural facts computed alongside it. `is_sound` is the
    /// full [`ValidityCertificate::is_sound`](crate::primitives::provenance::ValidityCertificate::is_sound)
    /// — the projection never re-derives its own verdict.
    pub fn from_validity(
        cert: &crate::primitives::provenance::ValidityCertificate,
        volume: Option<f64>,
        face_count: Option<usize>,
    ) -> Self {
        Self {
            is_sound: cert.is_sound(),
            euler_characteristic: cert.euler_characteristic,
            brep_valid: cert.brep_valid,
            watertight: cert.watertight,
            manifold: cert.manifold,
            oriented: cert.oriented,
            self_intersection_free: cert.self_intersection_free,
            volume,
            face_count,
        }
    }
}

/// Namespaced, typed, optional metadata attached to a [`RecordedOperation`]
/// — the facet envelope (OpenLineage's model). New provenance dimensions
/// attach as new facet names with **no schema migration**.
///
/// Facets are annotations *about* an operation (intent, cost, session, …).
/// What an operation did to the entity graph — `inputs` / `outputs` /
/// `deleted` — is the operation's core shape and stays first-class, never
/// a facet.
///
/// # Container contract
///
/// 1. **Forward compatible** — an unknown facet name round-trips
///    byte-identically; a newer producer and an older reader lose nothing.
/// 2. **Backward compatible** — events serialized before this container
///    existed deserialize with an empty `Facets` (`serde(default)` on the
///    field), and an empty container serializes NO key at all.
/// 3. **Deterministic** — storage is a `BTreeMap`, never a `HashMap`;
///    the same facets always serialize to identical bytes.
/// 4. **Absence is honest** — a missing facet means *not recorded*, never
///    *false* / *empty*. [`facet`](Self::facet) returns `None` for absent
///    and `Some(...)` for present, so readers can always distinguish.
/// 5. **Typed at the edges** — storage is `serde_json::Value`; producers
///    and consumers go through the typed accessors below so each facet's
///    shape is defined in exactly one place (e.g. [`IntentFacet`]).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Facets(BTreeMap<String, serde_json::Value>);

impl Facets {
    /// True when no facet is attached. Doubles as the
    /// `skip_serializing_if` predicate that keeps an empty container off
    /// the wire entirely (absent key, not `{}`).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Number of attached facets.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether a facet with this name is present — the raw absence /
    /// presence signal, independent of whether the payload parses.
    pub fn contains(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }

    /// The raw JSON payload of a facet, untyped. `None` = not recorded.
    pub fn get_raw(&self, name: &str) -> Option<&serde_json::Value> {
        self.0.get(name)
    }

    /// Attach a raw JSON payload under `name`, replacing any previous
    /// value. Prefer the typed setters ([`set_intent`](Self::set_intent))
    /// for known facets; this is the escape hatch that keeps unknown /
    /// pass-through facets representable.
    pub fn set_raw(&mut self, name: impl Into<String>, value: serde_json::Value) {
        self.0.insert(name.into(), value);
    }

    /// Typed read of a facet.
    ///
    /// * `None` — the facet is **absent** (not recorded).
    /// * `Some(Ok(t))` — present and parsed as `T`.
    /// * `Some(Err(e))` — present but its payload does not match `T`'s
    ///   shape; surfaced as a typed error, never silently coerced to
    ///   absence — a malformed facet must not read back as "not recorded".
    pub fn facet<T: DeserializeOwned>(&self, name: &str) -> Option<Result<T, serde_json::Error>> {
        self.0.get(name).map(|v| serde_json::from_value(v.clone()))
    }

    /// Typed write of a facet. Fails only if `T`'s `Serialize` impl fails
    /// (e.g. a map with non-string keys) — surfaced, never swallowed.
    pub fn set_facet<T: Serialize>(
        &mut self,
        name: impl Into<String>,
        value: &T,
    ) -> Result<(), serde_json::Error> {
        let v = serde_json::to_value(value)?;
        self.0.insert(name.into(), v);
        Ok(())
    }

    /// Typed read of the [`IntentFacet`] (`roshera.intent`). Absence
    /// semantics identical to [`facet`](Self::facet).
    pub fn intent(&self) -> Option<Result<IntentFacet, serde_json::Error>> {
        self.facet(IntentFacet::NAME)
    }

    /// Typed write of the [`IntentFacet`] (`roshera.intent`).
    pub fn set_intent(&mut self, intent: &IntentFacet) -> Result<(), serde_json::Error> {
        self.set_facet(IntentFacet::NAME, intent)
    }

    /// Typed read of the [`OriginFacet`] (`roshera.origin`). Absence
    /// semantics identical to [`facet`](Self::facet).
    pub fn origin(&self) -> Option<Result<OriginFacet, serde_json::Error>> {
        self.facet(OriginFacet::NAME)
    }

    /// Typed write of the [`OriginFacet`] (`roshera.origin`).
    pub fn set_origin(&mut self, origin: &OriginFacet) -> Result<(), serde_json::Error> {
        self.set_facet(OriginFacet::NAME, origin)
    }

    /// Iterate facets in deterministic (name-sorted) order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &serde_json::Value)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// The whole container as one JSON object value, in deterministic
    /// (name-sorted) order. Infallible — the storage already is JSON.
    /// Used by bridges that embed the facets in a larger envelope.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::Value::Object(self.0.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
    }
}

/// The `roshera.intent` facet: the natural-language request this operation
/// was serving — *what was being asked for*, alongside the event's *what
/// happened* and *who did it*.
///
/// This type is the single definition of the facet's shape; producers and
/// consumers go through [`Facets::set_intent`] / [`Facets::intent`] rather
/// than assembling JSON at call sites.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntentFacet {
    /// The natural-language turn text (Varun 2026-08-03: text, not a
    /// reference — the corpus carries the intent → action → verdict triple
    /// directly).
    pub text: String,
    /// Foreign key to the conversation turn that produced this op, when
    /// the caller has one. `None` = not recorded, never an empty string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    /// Where the text came from, e.g. `"agent_turn"` or `"user_prompt"`.
    pub source: String,
}

impl IntentFacet {
    /// The facet's namespaced wire name.
    pub const NAME: &'static str = "roshera.intent";
}

/// The closed set of channels an operation can enter the kernel through.
///
/// Intent is MCP-only (the MCP intent gate is the only thing that forces a
/// declaration before a mutating call); origin is universal — every
/// mutating channel, gated or not, is one of these. A free-form string
/// would let a producer invent a new, un-auditable category; a closed enum
/// cannot. `NotDetermined` is a first-class member, not an omission: a
/// code path that cannot establish its channel must say so explicitly
/// rather than default to a plausible-looking one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// The Roshera MCP server (`roshera-mcp`), whose intent gate forces a
    /// design-intent declaration before any solid-mutating tool call.
    Mcp,
    /// A direct REST call that did not present the MCP client's wire
    /// signature (see [`OriginFacet`]'s doc comment).
    Rest,
    /// The `/ws` WebSocket protocol (`TimelineWSCommand` / `GeometryWSCommand`).
    Websocket,
    /// The `/ws/viewport-bridge` connection.
    ViewportBridge,
    /// A kernel operation re-executed during replay
    /// (`timeline_engine::replay::rebuild_model_from_events`). Reserved for
    /// a future replay path that legitimately re-emits events; today's
    /// replay detaches the recorder for its entire duration (see that
    /// module's doc comment), so no live path produces this value yet.
    Replay,
    /// The channel could not be established. Honest, not a default: every
    /// other variant must be earned by an explicit, scoped signal.
    NotDetermined,
}

/// How confidently [`OriginFacet::channel`] is known.
///
/// [`Origin::Mcp`] is a claim the CLIENT makes on the wire (the MCP client
/// sends its own identifying headers; nothing server-side cryptographically
/// verifies that a caller presenting the same headers actually IS the MCP
/// server — see [`OriginFacet`]'s doc comment) — `ClientHeader`. Every other
/// channel is something the server itself observed structurally (which
/// route accepted the request, which task is handling the WebSocket
/// connection) — `ServerObserved`. Collapsing this distinction into the
/// channel alone would let a self-reported claim and a server-verified fact
/// read as equally certain, which is exactly the kind of confidently-wrong
/// provenance this feature exists to avoid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginBasis {
    /// Self-reported by the caller via request headers, never verified.
    ClientHeader,
    /// Established by the server from the transport/route it received the
    /// call on (or from the explicit absence of any such signal, for
    /// [`Origin::NotDetermined`]).
    ServerObserved,
}

/// The `roshera.origin` facet: which channel initiated this operation.
///
/// Unlike [`IntentFacet`] (present only when the MCP gate's checkpoint was
/// open), this facet is attached to EVERY operation `TimelineRecorder`
/// records — see that type's `record()` — because a channel is always
/// structurally determinable to at least the honest "not determined"
/// level, so leaving it off entirely would make an untracked channel
/// indistinguishable from one this build simply forgot to stamp.
///
/// # Distinguishing `mcp` from `rest`
///
/// The MCP client (`roshera-mcp/src/core.ts`) is the only caller that sends
/// BOTH `X-Roshera-Agent` (unconditionally, every call) AND a decodable
/// `X-Roshera-Intent` (whenever its intent gate has an open checkpoint) —
/// no other channel sends either. `main.rs`'s `agent_origin_layer` stamps
/// `Mcp` only on that conjunction; a request with neither, or only one, is
/// `Rest`. This is a claim the CLIENT can fabricate (any REST caller could
/// send the same two headers), which is why it is recorded with
/// `basis: ClientHeader` rather than silently presented as equally certain
/// as a server-observed channel like `Websocket`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OriginFacet {
    /// Which of the closed [`Origin`] set initiated the operation.
    pub channel: Origin,
    /// How confidently `channel` is known.
    pub basis: OriginBasis,
}

impl OriginFacet {
    /// The facet's namespaced wire name.
    pub const NAME: &'static str = "roshera.origin";
}

/// A structured description of one geometry operation that has just
/// completed successfully.
///
/// The shape is deliberately minimal and serialization-friendly so that any
/// recorder — timeline, audit log, network mirror — can consume it without
/// reaching into kernel-specific types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedOperation {
    /// Short stable identifier for the operation, e.g. `"extrude_face"`,
    /// `"boolean_union"`, `"create_box"`. Downstream recorders use this to
    /// dispatch to their own operation taxonomy.
    pub kind: String,

    /// Free-form JSON payload describing operation parameters. Must contain
    /// enough information to deterministically replay the operation when fed
    /// back into the same kernel version.
    pub parameters: serde_json::Value,

    /// Entity references consumed by this operation, each in the canonical
    /// `"<kind>:<id>"` wire form (see module docs). Empty when the operation
    /// is purely constructive.
    pub inputs: Vec<String>,

    /// Entity references produced by this operation, each in the canonical
    /// `"<kind>:<id>"` wire form (see module docs). Empty when the operation
    /// is purely destructive.
    pub outputs: Vec<String>,

    /// Entity references this operation REMOVED from the model, each in the
    /// canonical `"<kind>:<id>"` wire form (see module docs). A first-class
    /// third ref channel alongside `inputs` / `outputs` — PROV models
    /// invalidation as a core relation (`wasInvalidatedBy`), not an
    /// annotation. Without it a deletion records its victim as an input
    /// with no outputs, structurally identical to merely *reading* it, and
    /// nothing can ever leave the lineage frontier. Deletion must be stated
    /// structurally here, never inferred from the `kind` string.
    /// Empty when the operation removed nothing. Absent on the wire when
    /// empty, so pre-change events round-trip unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deleted: Vec<String>,

    /// Namespaced, typed, optional annotations *about* this operation (see
    /// [`Facets`]). Empty for every producer until a facet is adopted;
    /// absent on the wire when empty, so pre-facets events round-trip
    /// unchanged.
    #[serde(default, skip_serializing_if = "Facets::is_empty")]
    pub facets: Facets,

    /// The kernel proof for the solid this operation produced, attached by
    /// the recording handler AFTER certification and BEFORE `record(...)`.
    /// `None` means the op was not certified at record time (e.g. a
    /// fast-path op, or a producer not yet wired) — downstream recorders
    /// store no per-event certificate for it, an honest absence, never a
    /// fabricated one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solid_certificate: Option<RecordedSolidCertificate>,

    /// A timeline sequence number this operation's root persistent-ids were
    /// minted under, if any (`BRepModel::next_root_seed`'s on-demand
    /// reservation via [`OperationRecorder::reserve_event_key`]). Set by
    /// `BRepModel::record_operation` immediately before forwarding to the
    /// recorder — never by a caller constructing a `RecordedOperation`
    /// directly. `None` is the honest default for every operation that did
    /// not go through an on-demand reservation (no recorder attached, the
    /// recorder's `reserve_event_key` is the no-op default, or root-pid
    /// minting was suppressed inside a discard scope).
    ///
    /// This is a live-append INSTRUCTION, not replay data: a recorder that
    /// honours it (the timeline bridge) must append this exact record at
    /// this exact sequence number rather than burning a fresh one, so the
    /// key a live operation minted its root pids under (`"evt:{this}"`)
    /// matches what a later replay of the SAME event re-derives. It is
    /// deliberately excluded from the replay envelope
    /// (`recorder_bridge::to_timeline_operation`) — a value meaningful only
    /// to the live append path, never persisted as event `parameters`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserved_sequence: Option<u64>,
}

impl RecordedOperation {
    /// Start building a record for an operation of the given kind.
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            parameters: serde_json::Value::Null,
            inputs: Vec::new(),
            outputs: Vec::new(),
            deleted: Vec::new(),
            facets: Facets::default(),
            solid_certificate: None,
            reserved_sequence: None,
        }
    }

    /// Attach a JSON parameter payload.
    pub fn with_parameters(mut self, parameters: serde_json::Value) -> Self {
        self.parameters = parameters;
        self
    }

    /// Attach the kernel proof for the solid this operation produced (see
    /// [`RecordedSolidCertificate`]). Called by the recording handler after
    /// it has certified the result, so the certificate rides on the same
    /// record the timeline turns into the event.
    pub fn with_solid_certificate(mut self, certificate: RecordedSolidCertificate) -> Self {
        self.solid_certificate = Some(certificate);
        self
    }

    /// Append pre-formatted input entity references. Callers that already
    /// hold `"<kind>:<id>"` strings (typically because they assembled a
    /// heterogeneous list from multiple typed helpers) use this builder
    /// instead of one of the kind-specific ones below.
    pub fn with_input_refs<I, S>(mut self, refs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.inputs.extend(refs.into_iter().map(Into::into));
        self
    }

    /// Append pre-formatted output entity references — counterpart of
    /// [`with_input_refs`](Self::with_input_refs).
    pub fn with_output_refs<I, S>(mut self, refs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.outputs.extend(refs.into_iter().map(Into::into));
        self
    }

    /// Append solid inputs (`solid:<id>`).
    pub fn with_input_solids<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_input_refs(ids.into_iter().map(|i| entity_ref(ENTITY_SOLID, i.into())))
    }

    /// Append face inputs (`face:<id>`).
    pub fn with_input_faces<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_input_refs(ids.into_iter().map(|i| entity_ref(ENTITY_FACE, i.into())))
    }

    /// Append edge inputs (`edge:<id>`).
    pub fn with_input_edges<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_input_refs(ids.into_iter().map(|i| entity_ref(ENTITY_EDGE, i.into())))
    }

    /// Append vertex inputs (`vertex:<id>`).
    pub fn with_input_vertices<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_input_refs(ids.into_iter().map(|i| entity_ref(ENTITY_VERTEX, i.into())))
    }

    /// Append loop inputs (`loop:<id>`).
    pub fn with_input_loops<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_input_refs(ids.into_iter().map(|i| entity_ref(ENTITY_LOOP, i.into())))
    }

    /// Append curve inputs (`curve:<id>`).
    pub fn with_input_curves<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_input_refs(ids.into_iter().map(|i| entity_ref(ENTITY_CURVE, i.into())))
    }

    /// Append datum inputs (`datum:<id>`).
    pub fn with_input_datums<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_input_refs(ids.into_iter().map(|i| entity_ref(ENTITY_DATUM, i.into())))
    }

    /// Append solid outputs (`solid:<id>`).
    pub fn with_output_solids<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_output_refs(ids.into_iter().map(|i| entity_ref(ENTITY_SOLID, i.into())))
    }

    /// Append face outputs (`face:<id>`).
    pub fn with_output_faces<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_output_refs(ids.into_iter().map(|i| entity_ref(ENTITY_FACE, i.into())))
    }

    /// Append edge outputs (`edge:<id>`).
    pub fn with_output_edges<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_output_refs(ids.into_iter().map(|i| entity_ref(ENTITY_EDGE, i.into())))
    }

    /// Append vertex outputs (`vertex:<id>`).
    pub fn with_output_vertices<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_output_refs(ids.into_iter().map(|i| entity_ref(ENTITY_VERTEX, i.into())))
    }

    /// Append loop outputs (`loop:<id>`).
    pub fn with_output_loops<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_output_refs(ids.into_iter().map(|i| entity_ref(ENTITY_LOOP, i.into())))
    }

    /// Append curve outputs (`curve:<id>`).
    pub fn with_output_curves<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_output_refs(ids.into_iter().map(|i| entity_ref(ENTITY_CURVE, i.into())))
    }

    /// Append datum outputs (`datum:<id>`).
    pub fn with_output_datums<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_output_refs(ids.into_iter().map(|i| entity_ref(ENTITY_DATUM, i.into())))
    }

    /// Append pre-formatted deleted entity references — counterpart of
    /// [`with_input_refs`](Self::with_input_refs) for the deletion channel.
    pub fn with_deleted_refs<I, S>(mut self, refs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.deleted.extend(refs.into_iter().map(Into::into));
        self
    }

    /// Append deleted solids (`solid:<id>`).
    pub fn with_deleted_solids<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_deleted_refs(ids.into_iter().map(|i| entity_ref(ENTITY_SOLID, i.into())))
    }

    /// Append deleted faces (`face:<id>`).
    pub fn with_deleted_faces<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_deleted_refs(ids.into_iter().map(|i| entity_ref(ENTITY_FACE, i.into())))
    }

    /// Append deleted edges (`edge:<id>`).
    pub fn with_deleted_edges<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_deleted_refs(ids.into_iter().map(|i| entity_ref(ENTITY_EDGE, i.into())))
    }

    /// Append deleted vertices (`vertex:<id>`).
    pub fn with_deleted_vertices<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_deleted_refs(ids.into_iter().map(|i| entity_ref(ENTITY_VERTEX, i.into())))
    }

    /// Append deleted loops (`loop:<id>`).
    pub fn with_deleted_loops<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_deleted_refs(ids.into_iter().map(|i| entity_ref(ENTITY_LOOP, i.into())))
    }

    /// Append deleted curves (`curve:<id>`).
    pub fn with_deleted_curves<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_deleted_refs(ids.into_iter().map(|i| entity_ref(ENTITY_CURVE, i.into())))
    }

    /// Append deleted datums (`datum:<id>`).
    pub fn with_deleted_datums<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_deleted_refs(ids.into_iter().map(|i| entity_ref(ENTITY_DATUM, i.into())))
    }

    /// Append assembly inputs (`assembly:<uuid-as-u128>`). Assembly /
    /// component / mate identifiers are UUIDs rather than counters, so
    /// callers pass `Uuid::as_u128()` widened to two `u64`s — but for
    /// recording purposes we collapse to a single `u128`-shaped `u64`
    /// pair encoded via `Uuid::to_string()`. To keep the canonical
    /// `<kind>:<id>` form we instead accept the already-formatted
    /// string. Use [`with_input_refs`](Self::with_input_refs) with
    /// [`entity_ref`] for any kind that needs a non-`u64` identifier.
    pub fn with_input_assembly(self, uuid: impl fmt::Display) -> Self {
        self.with_input_refs([format!("{}:{}", ENTITY_ASSEMBLY, uuid)])
    }

    /// Append assembly outputs (`assembly:<uuid>`). See
    /// [`with_input_assembly`](Self::with_input_assembly).
    pub fn with_output_assembly(self, uuid: impl fmt::Display) -> Self {
        self.with_output_refs([format!("{}:{}", ENTITY_ASSEMBLY, uuid)])
    }

    /// Append component inputs (`component:<uuid>`).
    pub fn with_input_component(self, uuid: impl fmt::Display) -> Self {
        self.with_input_refs([format!("{}:{}", ENTITY_COMPONENT, uuid)])
    }

    /// Append component outputs (`component:<uuid>`).
    pub fn with_output_component(self, uuid: impl fmt::Display) -> Self {
        self.with_output_refs([format!("{}:{}", ENTITY_COMPONENT, uuid)])
    }

    /// Append mate inputs (`mate:<uuid>`).
    pub fn with_input_mate(self, uuid: impl fmt::Display) -> Self {
        self.with_input_refs([format!("{}:{}", ENTITY_MATE, uuid)])
    }

    /// Append mate outputs (`mate:<uuid>`).
    pub fn with_output_mate(self, uuid: impl fmt::Display) -> Self {
        self.with_output_refs([format!("{}:{}", ENTITY_MATE, uuid)])
    }
}

/// Errors a recorder may surface. Geometry operations do not propagate
/// these — the operation is already complete — but the recorder layer
/// reports them so orchestration code can decide how to react.
#[derive(Debug, Clone)]
pub enum RecorderError {
    /// The recorder was configured but is temporarily unable to accept
    /// events (queue full, downstream unreachable, etc.).
    Unavailable(String),
    /// The recorded operation failed validation (unknown kind, malformed
    /// parameters, etc.).
    InvalidOperation(String),
    /// Any other failure. Free-form description.
    Other(String),
}

impl fmt::Display for RecorderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecorderError::Unavailable(msg) => write!(f, "recorder unavailable: {}", msg),
            RecorderError::InvalidOperation(msg) => write!(f, "invalid operation: {}", msg),
            RecorderError::Other(msg) => write!(f, "recorder error: {}", msg),
        }
    }
}

impl std::error::Error for RecorderError {}

/// Receives one record per successful geometry operation.
///
/// Implementations must be `Send + Sync` so a single recorder can be shared
/// across threads holding `BRepModel`s.
///
/// # Transactional staging
///
/// The trio [`begin_pending`](Self::begin_pending) /
/// [`commit_pending`](Self::commit_pending) /
/// [`abort_pending`](Self::abort_pending) lets a transactional caller
/// (typically `operations::lifecycle::with_rollback`) defer event
/// commitment until the surrounding operation's success is known. When
/// the operation fails and its model mutations are rolled back via
/// `ModelSnapshot::restore`, the staged events must be discarded so the
/// timeline never holds a record of an operation that "never happened".
///
/// The default implementations are no-ops: a recorder that commits
/// events immediately at `record()` time will continue to do so. Only
/// recorders backed by a remote / async sink (e.g. `TimelineRecorder`)
/// need to override the staging hooks. This is what keeps `NullRecorder`,
/// test captures, and audit-log recorders source-compatible.
pub trait OperationRecorder: Send + Sync + fmt::Debug {
    /// Record a completed operation. Called after the `BRepModel` has
    /// already been mutated successfully. When a transactional scope is
    /// active (see [`begin_pending`](Self::begin_pending)), the event is
    /// staged in-memory and only forwarded downstream on
    /// [`commit_pending`](Self::commit_pending).
    fn record(&self, operation: RecordedOperation) -> Result<(), RecorderError>;

    /// Enter a transactional recording scope. Subsequent `record` calls
    /// are staged until either [`commit_pending`](Self::commit_pending)
    /// or [`abort_pending`](Self::abort_pending) resolves the scope.
    /// Default impl: no-op (recorder commits immediately).
    fn begin_pending(&self) {}

    /// Commit and forward every event staged since the matching
    /// [`begin_pending`](Self::begin_pending). Default impl: no-op.
    fn commit_pending(&self) {}

    /// Discard every event staged since the matching
    /// [`begin_pending`](Self::begin_pending). Called by
    /// `with_rollback` when the wrapped operation returned `Err` and
    /// the model snapshot is about to be restored. Default impl: no-op.
    fn abort_pending(&self) {}

    /// Enter a scope whose records are GUARANTEED to be discarded and
    /// never reach the timeline — as opposed to the
    /// [`begin_pending`](Self::begin_pending) trio, whose staged events
    /// may still be forwarded on [`commit_pending`](Self::commit_pending)
    /// (e.g. `with_rollback`'s success path). The api-server's
    /// `RecorderSuppressGuard` is the caller: it hides a composite op's
    /// kernel-internal sub-events behind one consolidated event by
    /// staging them with `begin_pending` and then UNCONDITIONALLY
    /// `abort_pending`-ing on drop, win or lose. `BRepModel::record_operation`
    /// checks [`records_are_discarded`](Self::records_are_discarded) to skip
    /// the certify-at-record computation while this scope is open — that
    /// work would otherwise be thrown away with the record itself. Default
    /// impl: no-op, so only a recorder that needs the distinction overrides
    /// it (`begin_pending`'s default no-op recorders never call this either).
    fn begin_discard_scope(&self) {}

    /// Close a scope opened by [`begin_discard_scope`](Self::begin_discard_scope).
    /// Default impl: no-op.
    fn end_discard_scope(&self) {}

    /// `true` while inside a [`begin_discard_scope`](Self::begin_discard_scope)
    /// window (nested scopes count as one, matching `begin_pending`'s depth
    /// semantics). Default impl: `false`, so a recorder that never overrides
    /// the discard-scope trio always allows certification to proceed.
    fn records_are_discarded(&self) -> bool {
        false
    }

    /// Reserve a stable event key for the operation about to run, so every
    /// root persistent-id minted during it (`BRepModel::next_root_seed`)
    /// derives from the SAME key a later replay of this exact event will
    /// re-derive (`format!("evt:{sequence_number}")`, `replay::apply_event`).
    ///
    /// The live-authoring counterpart of that replay seed: without it, a
    /// live root pid was minted from a process-local fallback
    /// (`root_counter`) that a subsequent replay could never reproduce,
    /// because replay always seeds from the event's own burned sequence
    /// number. A recorder backed by a real, appendable timeline overrides
    /// this to actually reserve a sequence (see
    /// `timeline-engine::recorder_bridge::TimelineRecorder`); the returned
    /// key must correspond to a sequence number the SAME recorder will
    /// later honour when this operation's record reaches it (see
    /// [`RecordedOperation::reserved_sequence`]).
    ///
    /// Default: `None` — `NullRecorder`, test capture recorders, and any
    /// recorder not backed by a timeline sequence. `BRepModel::next_root_seed`
    /// falls back to its process-local `root_counter` seed in that case,
    /// which is exactly today's (pre-existing) behaviour — this default
    /// keeps every existing recorder source-compatible and byte-identical.
    fn reserve_event_key(&self) -> Option<String> {
        None
    }
}

/// Recorder that drops every event. Useful as a default for tests and
/// unattached models.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullRecorder;

impl OperationRecorder for NullRecorder {
    fn record(&self, _operation: RecordedOperation) -> Result<(), RecorderError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Default)]
    struct CaptureRecorder {
        events: Mutex<Vec<RecordedOperation>>,
    }

    impl OperationRecorder for CaptureRecorder {
        fn record(&self, operation: RecordedOperation) -> Result<(), RecorderError> {
            self.events
                .lock()
                .expect("CaptureRecorder mutex poisoned")
                .push(operation);
            Ok(())
        }
    }

    #[test]
    fn null_recorder_never_fails() {
        let r = NullRecorder;
        assert!(r
            .record(RecordedOperation::new("noop").with_parameters(serde_json::json!({})))
            .is_ok());
    }

    #[test]
    fn capture_recorder_stores_events_in_order() {
        let r = Arc::new(CaptureRecorder::default());
        r.record(RecordedOperation::new("a")).expect("a");
        r.record(RecordedOperation::new("b")).expect("b");
        let captured = r.events.lock().expect("mutex").clone();
        assert_eq!(captured.len(), 2);
        assert_eq!(captured[0].kind, "a");
        assert_eq!(captured[1].kind, "b");
    }

    #[test]
    fn recorded_operation_builder_captures_all_fields() {
        let op = RecordedOperation::new("extrude_face")
            .with_parameters(serde_json::json!({ "distance": 5.0 }))
            .with_input_faces([1u64, 2])
            .with_output_solids([10u64, 11, 12]);
        assert_eq!(op.kind, "extrude_face");
        assert_eq!(op.parameters["distance"], 5.0);
        assert_eq!(op.inputs, vec!["face:1", "face:2"]);
        assert_eq!(op.outputs, vec!["solid:10", "solid:11", "solid:12"]);
    }

    #[test]
    fn entity_ref_uses_canonical_wire_form() {
        assert_eq!(entity_ref(ENTITY_SOLID, 7), "solid:7");
        assert_eq!(entity_ref(ENTITY_FACE, 42), "face:42");
        assert_eq!(entity_ref(ENTITY_EDGE, 0), "edge:0");
        assert_eq!(
            entity_ref(ENTITY_VERTEX, u64::MAX),
            format!("vertex:{}", u64::MAX)
        );
    }

    #[test]
    fn mixed_kind_builder_chain_preserves_namespaces() {
        // Common chamfer / fillet pattern: solid plus edges on the input
        // side, solid plus new faces on the output side. The lineage graph
        // must keep all four kinds distinct.
        let op = RecordedOperation::new("chamfer_edges")
            .with_input_solids([5u64])
            .with_input_edges([10u64, 11, 12])
            .with_output_solids([5u64])
            .with_output_faces([20u64, 21, 22]);
        assert_eq!(op.inputs, vec!["solid:5", "edge:10", "edge:11", "edge:12"]);
        assert_eq!(op.outputs, vec!["solid:5", "face:20", "face:21", "face:22"]);
    }

    #[test]
    fn assembly_entity_tags_are_canonical_wire_form() {
        assert_eq!(ENTITY_ASSEMBLY, "assembly");
        assert_eq!(ENTITY_COMPONENT, "component");
        assert_eq!(ENTITY_MATE, "mate");
    }

    #[test]
    fn assembly_builders_emit_uuid_styled_refs() {
        let asm_uuid = "550e8400-e29b-41d4-a716-446655440000";
        let comp_uuid = "550e8400-e29b-41d4-a716-446655440001";
        let mate_uuid = "550e8400-e29b-41d4-a716-446655440002";
        let op = RecordedOperation::new("assembly.add_mate")
            .with_input_assembly(asm_uuid)
            .with_input_component(comp_uuid)
            .with_output_mate(mate_uuid);
        assert_eq!(
            op.inputs,
            vec![
                format!("assembly:{}", asm_uuid),
                format!("component:{}", comp_uuid)
            ]
        );
        assert_eq!(op.outputs, vec![format!("mate:{}", mate_uuid)]);
    }

    #[test]
    fn with_input_refs_passes_through_preformatted_strings() {
        let pre: Vec<String> = vec!["solid:1".into(), "face:2".into()];
        let op = RecordedOperation::new("custom").with_input_refs(pre.clone());
        assert_eq!(op.inputs, pre);
    }

    // ──────────── Deletion channel + facet envelope ────────────

    /// Every `with_deleted_*` builder emits the canonical `<kind>:<id>`
    /// wire form, exactly mirroring the input/output families.
    #[test]
    fn deleted_builders_emit_canonical_wire_form() {
        let op = RecordedOperation::new("delete_solid")
            .with_deleted_solids([7u64])
            .with_deleted_faces([12u64])
            .with_deleted_edges([3u64])
            .with_deleted_vertices([4u64])
            .with_deleted_loops([5u64])
            .with_deleted_curves([6u64])
            .with_deleted_datums([8u64])
            .with_deleted_refs(["mate:550e8400-e29b-41d4-a716-446655440002".to_string()]);
        assert_eq!(
            op.deleted,
            vec![
                "solid:7",
                "face:12",
                "edge:3",
                "vertex:4",
                "loop:5",
                "curve:6",
                "datum:8",
                "mate:550e8400-e29b-41d4-a716-446655440002",
            ]
        );
        assert!(
            op.inputs.is_empty(),
            "deletion channel must not leak into inputs"
        );
        assert!(
            op.outputs.is_empty(),
            "deletion channel must not leak into outputs"
        );
    }

    /// An operation carrying deletions round-trips through JSON: `deleted`
    /// survives as its own channel, structurally distinct from an input
    /// with no outputs — the whole point of the third channel.
    #[test]
    fn operation_carrying_deletions_round_trips() {
        let op = RecordedOperation::new("delete_solid")
            .with_parameters(serde_json::json!({ "cascade": true }))
            .with_input_solids([9u64])
            .with_deleted_solids([9u64])
            .with_deleted_faces([1u64, 2]);
        let json = serde_json::to_string(&op).expect("serialize");
        let back: RecordedOperation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.kind, op.kind);
        assert_eq!(back.parameters, op.parameters);
        assert_eq!(back.inputs, op.inputs);
        assert_eq!(back.outputs, op.outputs);
        assert_eq!(back.deleted, vec!["solid:9", "face:1", "face:2"]);
        assert_eq!(back.facets, op.facets);
    }

    /// Wire stability: an operation with no deletions and no facets
    /// serializes with NO `deleted` / `facets` keys at all — the wire
    /// bytes of pre-change events are reproduced exactly.
    #[test]
    fn empty_deleted_and_facets_serialize_no_keys() {
        let op = RecordedOperation::new("noop");
        let v = serde_json::to_value(&op).expect("serialize");
        let obj = v.as_object().expect("an object");
        assert!(
            !obj.contains_key("deleted"),
            "empty deleted must be absent: {v}"
        );
        assert!(
            !obj.contains_key("facets"),
            "empty facets must be absent: {v}"
        );
    }

    /// Backward compatibility, proven against a LITERAL pre-change fixture
    /// (the exact wire shape emitted before `deleted` / `facets` existed —
    /// hand-written verbatim, not constructed and re-serialized). It must
    /// still deserialize, with both new channels honestly empty.
    #[test]
    fn pre_change_event_fixture_still_deserialises() {
        let fixture = r#"{"kind":"extrude_face","parameters":{"distance":5.0},"inputs":["face:1","edge:2"],"outputs":["solid:42"]}"#;
        let op: RecordedOperation =
            serde_json::from_str(fixture).expect("a pre-change event must deserialize unchanged");
        assert_eq!(op.kind, "extrude_face");
        assert_eq!(op.parameters["distance"], 5.0);
        assert_eq!(op.inputs, vec!["face:1", "edge:2"]);
        assert_eq!(op.outputs, vec!["solid:42"]);
        assert!(op.deleted.is_empty(), "no deleted key reads back as empty");
        assert!(op.facets.is_empty(), "no facets key reads back as empty");
        assert!(op.solid_certificate.is_none());
    }

    /// Forward compatibility: a facet name THIS build knows nothing about
    /// round-trips byte-identically — an older reader must never drop a
    /// newer producer's data.
    #[test]
    fn unknown_facet_round_trips_byte_identically() {
        let mut op = RecordedOperation::new("noop");
        op.facets.set_raw(
            "vendor.future_dimension",
            serde_json::json!({ "cost_usd": 0.0125, "nested": { "a": [1, 2, 3] } }),
        );
        let first = serde_json::to_string(&op).expect("serialize");
        let back: RecordedOperation = serde_json::from_str(&first).expect("deserialize");
        let second = serde_json::to_string(&back).expect("re-serialize");
        assert_eq!(
            first, second,
            "an unknown facet must round-trip byte-identically, never be dropped"
        );
        assert!(back.facets.contains("vendor.future_dimension"));
    }

    /// Determinism: the same facets attached in DIFFERENT insertion orders
    /// serialize to identical bytes — BTreeMap ordering, the property any
    /// future content addressing depends on.
    #[test]
    fn facet_serialisation_is_deterministic_across_insertion_order() {
        let mut a = RecordedOperation::new("noop");
        a.facets.set_raw("z.last", serde_json::json!(1));
        a.facets.set_raw("a.first", serde_json::json!(2));
        a.facets.set_raw("m.middle", serde_json::json!(3));

        let mut b = RecordedOperation::new("noop");
        b.facets.set_raw("m.middle", serde_json::json!(3));
        b.facets.set_raw("a.first", serde_json::json!(2));
        b.facets.set_raw("z.last", serde_json::json!(1));

        let bytes_a = serde_json::to_string(&a).expect("serialize a");
        let bytes_b = serde_json::to_string(&b).expect("serialize b");
        assert_eq!(
            bytes_a, bytes_b,
            "insertion order must not leak into the wire bytes"
        );
    }

    /// Absence is honest: a reader can distinguish a facet that was never
    /// recorded (`None`) from one recorded with an empty payload
    /// (`Some(...)`). "Missing" must never collapse into "empty".
    #[test]
    fn facet_absence_is_distinguishable_from_present_but_empty() {
        let mut op = RecordedOperation::new("noop");
        assert!(
            op.facets.get_raw("roshera.marker").is_none(),
            "absent = None"
        );
        assert!(op
            .facets
            .facet::<serde_json::Value>("roshera.marker")
            .is_none());

        op.facets.set_raw("roshera.marker", serde_json::json!({}));
        let raw = op.facets.get_raw("roshera.marker");
        assert!(
            matches!(raw, Some(v) if v.as_object().is_some_and(|o| o.is_empty())),
            "present-but-empty = Some(empty object), got {raw:?}"
        );

        // The distinction survives a wire round-trip.
        let json = serde_json::to_string(&op).expect("serialize");
        let back: RecordedOperation = serde_json::from_str(&json).expect("deserialize");
        assert!(back.facets.get_raw("roshera.marker").is_some());
        assert!(back.facets.get_raw("roshera.never_recorded").is_none());
    }

    /// The `roshera.intent` typed facet: written and read through the ONE
    /// definition of its shape, round-trips intact, and a present-but-
    /// malformed payload surfaces as `Some(Err(..))` — never coerced to
    /// absence, never silently defaulted.
    #[test]
    fn intent_facet_round_trips_through_typed_accessor() {
        let mut op = RecordedOperation::new("sketch_extrude");
        assert!(op.facets.intent().is_none(), "no intent recorded = None");

        let intent = IntentFacet {
            text: "make the base plate 5mm thicker".to_string(),
            turn_id: Some("turn-91".to_string()),
            source: "agent_turn".to_string(),
        };
        op.facets.set_intent(&intent).expect("set intent");

        let json = serde_json::to_string(&op).expect("serialize");
        let back: RecordedOperation = serde_json::from_str(&json).expect("deserialize");
        let read = back
            .facets
            .intent()
            .expect("intent facet is present")
            .expect("intent facet parses");
        assert_eq!(read, intent);

        // A present-but-malformed intent is a typed error, not absence.
        let mut bad = RecordedOperation::new("noop");
        bad.facets
            .set_raw(IntentFacet::NAME, serde_json::json!({ "text": 42 }));
        let result = bad.facets.intent().expect("facet is present");
        assert!(
            result.is_err(),
            "malformed intent must surface as Err, not vanish"
        );
    }

    /// The `roshera.origin` typed facet: written and read through the ONE
    /// definition of its shape, round-trips intact, uses the closed-set
    /// snake_case wire form, and a present-but-malformed payload surfaces
    /// as `Some(Err(..))` — never coerced to absence.
    #[test]
    fn origin_facet_round_trips_through_typed_accessor() {
        let mut op = RecordedOperation::new("boolean_union");
        assert!(op.facets.origin().is_none(), "no origin recorded = None");

        let origin = OriginFacet {
            channel: Origin::Websocket,
            basis: OriginBasis::ServerObserved,
        };
        op.facets.set_origin(&origin).expect("set origin");

        let json = serde_json::to_string(&op).expect("serialize");
        assert!(
            json.contains(r#""channel":"websocket""#)
                && json.contains(r#""basis":"server_observed""#),
            "wire form must be closed-set snake_case, got {json}"
        );
        let back: RecordedOperation = serde_json::from_str(&json).expect("deserialize");
        let read = back
            .facets
            .origin()
            .expect("origin facet is present")
            .expect("origin facet parses");
        assert_eq!(read, origin);

        // A present-but-malformed origin is a typed error, not absence.
        let mut bad = RecordedOperation::new("noop");
        bad.facets.set_raw(
            OriginFacet::NAME,
            serde_json::json!({ "channel": "not_a_real_channel" }),
        );
        let result = bad.facets.origin().expect("facet is present");
        assert!(
            result.is_err(),
            "malformed/unknown origin must surface as Err, not vanish or coerce to NotDetermined"
        );
    }

    /// An all-sound kernel certificate, hand-built so individual checks can
    /// be flipped to make `brep_valid` and `is_sound()` diverge.
    fn sound_validity_certificate() -> crate::primitives::provenance::ValidityCertificate {
        use crate::primitives::provenance::{
            ConstructionConsistency, EyesConsistency, LabelsConsistency, MeshQuality,
            TessellationQuality, ValidityCertificate,
        };
        ValidityCertificate {
            brep_valid: true,
            watertight: true,
            manifold: true,
            euler_characteristic: 2,
            boundary_edges: 0,
            nonmanifold_edges: 0,
            oriented: true,
            inconsistent_directed_edges: 0,
            self_intersection_free: true,
            construction_consistent: ConstructionConsistency::NotApplicable,
            labels_consistent: LabelsConsistency::NotApplicable,
            eyes_consistent: EyesConsistency::Consistent,
            tessellation: TessellationQuality::empty(),
            mesh_quality: MeshQuality::empty(),
            errors: vec![],
            model_debris_orphan_faces: 0,
        }
    }

    /// Honesty-contract rule 2 pin: `is_sound` on the recorded projection is
    /// the FULL `ValidityCertificate::is_sound()`, never a cheap subset. A
    /// cert that is `brep_valid` yet NOT watertight is UNSOUND; a projection
    /// that read `brep_valid` into `is_sound` reports `true` here and fails.
    #[test]
    fn recorded_solid_certificate_is_sound_is_the_full_and() {
        let mut cert = sound_validity_certificate();
        cert.watertight = false;
        assert!(cert.brep_valid, "guard: brep_valid stays true");
        assert!(!cert.is_sound(), "guard: the cert is unsound");

        let rec = RecordedSolidCertificate::from_validity(&cert, Some(1.0), Some(6));
        assert!(
            !rec.is_sound,
            "is_sound must be the full AND, not brep_valid"
        );
        assert!(!rec.watertight);
        assert!(rec.brep_valid);
    }

    /// The projection mirrors the certificate's per-check breakdown and the
    /// cheap structural facts verbatim — no field is re-derived or invented.
    #[test]
    fn recorded_solid_certificate_mirrors_the_validity_certificate() {
        let cert = sound_validity_certificate();
        let rec = RecordedSolidCertificate::from_validity(&cert, Some(1000.0), Some(6));
        assert_eq!(rec.is_sound, cert.is_sound());
        assert_eq!(rec.euler_characteristic, cert.euler_characteristic);
        assert_eq!(rec.brep_valid, cert.brep_valid);
        assert_eq!(rec.watertight, cert.watertight);
        assert_eq!(rec.manifold, cert.manifold);
        assert_eq!(rec.oriented, cert.oriented);
        assert_eq!(rec.self_intersection_free, cert.self_intersection_free);
        assert_eq!(rec.volume, Some(1000.0));
        assert_eq!(rec.face_count, Some(6));
    }

    /// Wire-format honesty: a record without a certificate serializes with
    /// NO `solid_certificate` key at all (absent, not `null`), and an old
    /// payload lacking the key deserializes back to `None` — an uncertified
    /// record can never read back as a certified one.
    #[test]
    fn solid_certificate_absent_round_trips_as_absent() {
        let op = RecordedOperation::new("noop");
        let v = serde_json::to_value(&op).expect("serialize");
        assert!(
            v.as_object()
                .is_some_and(|o| !o.contains_key("solid_certificate")),
            "absent certificate must not serialize a key; got {v}"
        );
        let back: RecordedOperation = serde_json::from_value(v).expect("deserialize");
        assert!(back.solid_certificate.is_none());
    }

    // ──────────── Certify-at-record: runtime coverage ratchet ────────────
    //
    // The source-scanner ratchet below proves every production record SITE
    // declares inputs; these tests prove certificate coverage BEHAVIOURALLY:
    // real kernel ops against a real `BRepModel`, a capture recorder
    // attached, and emitted records that carry the kernel's verdict for the
    // solid they produced (`BRepModel::record_operation` →
    // `attach_record_time_certificate`). Before that seam existed, every
    // kernel-side record was emitted with `solid_certificate: None`.

    /// Fresh model with a capture recorder attached and one real box built
    /// through the recording kernel path (`TopologyBuilder::create_box_3d`).
    fn box_solid_with_capture() -> (
        crate::primitives::topology_builder::BRepModel,
        Arc<CaptureRecorder>,
        crate::primitives::solid::SolidId,
    ) {
        use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
        let capture = Arc::new(CaptureRecorder::default());
        let mut model = BRepModel::new();
        model.attach_recorder(Some(capture.clone()));
        let sid = match TopologyBuilder::new(&mut model)
            .create_box_3d(20.0, 14.0, 10.0)
            .expect("box creation succeeds")
        {
            GeometryId::Solid(s) => s,
            other => panic!("expected a solid, got {other:?}"),
        };
        (model, capture, sid)
    }

    /// A primitive-creation record must carry the certificate of the solid
    /// it created, computed at record time — and the contents must reflect
    /// the REAL box (a default-constructed / fabricated projection cannot
    /// satisfy soundness, χ = 2, and face_count = 6 at once).
    #[test]
    fn primitive_creation_record_carries_the_solids_certificate() {
        let (_model, capture, sid) = box_solid_with_capture();
        let events = capture.events.lock().expect("mutex").clone();
        let solid_ref = entity_ref(ENTITY_SOLID, sid as u64);
        let record = events
            .iter()
            .find(|e| e.outputs.contains(&solid_ref))
            .expect("a record naming the created solid as an output");
        let cert = record
            .solid_certificate
            .as_ref()
            .expect("the creation record must carry a record-time certificate");
        assert!(cert.is_sound, "a clean box certifies sound: {cert:?}");
        assert!(cert.brep_valid, "box B-Rep is valid: {cert:?}");
        assert!(cert.watertight && cert.manifold, "{cert:?}");
        assert!(cert.oriented && cert.self_intersection_free, "{cert:?}");
        assert_eq!(cert.euler_characteristic, 2, "closed box mesh has χ = 2");
        assert_eq!(cert.face_count, Some(6), "a box has 6 outer faces");
    }

    /// A MODIFYING op through the real kernel path (`transform_solid`, which
    /// runs inside `with_rollback`'s transactional recording scope) must
    /// also emit its record with a post-op certificate attached.
    #[test]
    fn modifying_operation_record_carries_a_post_op_certificate() {
        let (mut model, capture, sid) = box_solid_with_capture();
        let translation = crate::math::Matrix4::translation(5.0, -3.0, 2.0);
        crate::operations::transform::transform_solid(
            &mut model,
            sid,
            translation,
            crate::operations::transform::TransformOptions::default(),
        )
        .expect("transform succeeds");
        let events = capture.events.lock().expect("mutex").clone();
        let record = events
            .iter()
            .find(|e| e.kind == "transform_solid")
            .expect("the transform must have recorded");
        let cert = record
            .solid_certificate
            .as_ref()
            .expect("a modifying op's record must carry a record-time certificate");
        assert!(cert.is_sound, "a translated box is still sound: {cert:?}");
        assert_eq!(cert.euler_characteristic, 2, "{cert:?}");
        assert_eq!(
            cert.face_count,
            Some(6),
            "translation preserves the 6 faces: {cert:?}"
        );
    }

    /// Multi-output rule pin: `solid_certificate` holds ONE certificate, so
    /// a record naming several distinct output solids must stay honestly
    /// uncertified — silently attaching the first solid's verdict would
    /// ascribe one solid's soundness to an event that produced several. The
    /// single-output control on the same model proves the absence is the
    /// rule firing, not a broken seam.
    #[test]
    fn multi_output_record_stays_honestly_uncertified() {
        use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
        let capture = Arc::new(CaptureRecorder::default());
        let mut model = BRepModel::new();
        let a = match TopologyBuilder::new(&mut model)
            .create_box_3d(10.0, 10.0, 10.0)
            .expect("box a")
        {
            GeometryId::Solid(s) => s,
            other => panic!("expected a solid, got {other:?}"),
        };
        let b = match TopologyBuilder::new(&mut model)
            .create_box_3d(8.0, 8.0, 8.0)
            .expect("box b")
        {
            GeometryId::Solid(s) => s,
            other => panic!("expected a solid, got {other:?}"),
        };
        // Attach AFTER creation so only the two manual records are captured.
        model.attach_recorder(Some(capture.clone()));
        model.record_operation(
            RecordedOperation::new("test.split")
                .with_input_solids([a as u64])
                .with_output_solids([a as u64, b as u64]),
        );
        model.record_operation(
            RecordedOperation::new("test.touch")
                .with_input_solids([a as u64])
                .with_output_solids([a as u64]),
        );
        let events = capture.events.lock().expect("mutex").clone();
        assert_eq!(events.len(), 2);
        assert!(
            events[0].solid_certificate.is_none(),
            "two distinct output solids must record an honest absence"
        );
        let cert = events[1]
            .solid_certificate
            .as_ref()
            .expect("single-output control on the same model must certify");
        assert!(cert.is_sound, "{cert:?}");
        assert_eq!(cert.face_count, Some(6), "{cert:?}");
    }

    /// An output solid that does not exist cannot be certified: honest
    /// absence, never a fabricated verdict.
    #[test]
    fn record_naming_a_missing_output_solid_stays_uncertified() {
        let capture = Arc::new(CaptureRecorder::default());
        let mut model = crate::primitives::topology_builder::BRepModel::new();
        model.attach_recorder(Some(capture.clone()));
        model.record_operation(
            RecordedOperation::new("test.ghost")
                .with_input_solids([99u64])
                .with_output_solids([99u64]),
        );
        let events = capture.events.lock().expect("mutex").clone();
        assert_eq!(events.len(), 1);
        assert!(
            events[0].solid_certificate.is_none(),
            "a missing output solid cannot be certified — honest absence required"
        );
    }

    // ───────────────────────── Lineage ratchet ─────────────────────────

    /// Recursively collect every `.rs` file under `dir`.
    fn collect_rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read_dir under geometry-engine src") {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                collect_rs_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// Lineage ratchet: every PRODUCTION `RecordedOperation::new(` site must
    /// attach at least one `.with_input_*` reference, unless its op kind is a
    /// genuinely constructive root on the explicit allowlist below.
    ///
    /// Why this is load-bearing: the durable event stream is the ONLY thing
    /// that survives replay. An in-memory graph can know the true parents,
    /// but if the recorded event carries no inputs, lineage rebuilt from
    /// events has a hole — the result appears from nowhere. That is a SILENT
    /// failure mode: nothing errors, the part's history just splits in two.
    #[test]
    fn every_production_recorded_operation_declares_inputs_or_is_a_root() {
        // Constructive roots — operations that genuinely consume no model
        // entities. Every entry MUST carry a justification; do not add one
        // to silence the test for a consuming operation.
        const ALLOWLIST: &[&str] = &[
            // datum_create (3 sites): plane / axis / point authored from a
            // user-supplied transform — there is no antecedent model entity
            // to reference.
            "datum_create",
            // nurbs_loft (1 site): the signature is
            // `sections: Vec<Vec<Point3>>` — raw point rings, not model
            // entities, so there is nothing to reference.
            "nurbs_loft",
        ];

        let src_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect_rs_files(&src_root, &mut files);
        assert!(
            !files.is_empty(),
            "found no .rs files under {}",
            src_root.display()
        );

        const NEEDLE: &str = "RecordedOperation::new(";
        let mut sites = 0usize;
        let mut violations: Vec<String> = Vec::new();

        for path in &files {
            let text = std::fs::read_to_string(path).expect("read source file");
            // Repo convention: everything before a file's first
            // `#[cfg(test)]` is production code; everything after is test
            // code (and is exempt — tests may build minimal records).
            let production = match text.find("#[cfg(test)]") {
                Some(i) => &text[..i],
                None => text.as_str(),
            };
            let mut from = 0usize;
            while let Some(rel) = production[from..].find(NEEDLE) {
                let at = from + rel;
                let after = at + NEEDLE.len();
                let line = production[..at].matches('\n').count() + 1;
                // The fluent builder chain runs from the constructor to the
                // first `;` terminating the statement.
                let chain_end = production[after..]
                    .find(';')
                    .map_or(production.len(), |i| after + i);
                let chain = &production[at..chain_end];
                // Op kind: a string-literal argument, or a variable (a
                // dynamic kind — those sites must still carry inputs, they
                // just cannot be allowlisted by name).
                let arg = production[after..chain_end].trim_start();
                let kind = if let Some(rest) = arg.strip_prefix('"') {
                    rest.split('"').next().unwrap_or("<unterminated>")
                } else {
                    "<dynamic>"
                };
                sites += 1;
                let has_inputs = chain.contains(".with_input_");
                if !has_inputs && !ALLOWLIST.contains(&kind) {
                    violations.push(format!(
                        "{}:{} op kind `{}` records NO `.with_input_*` references. \
                         A consuming operation that records no inputs is a SILENT \
                         lineage break: the durable event stream — the only thing \
                         that survives replay — says its result appeared from \
                         nowhere, so lineage rebuilt from events splits one part \
                         into two with nothing failing. Attach the real input \
                         references. If (and only if) this operation is a genuinely \
                         constructive root that consumes no model entities, add its \
                         op kind to the allowlist in this test with a justification.",
                        path.display(),
                        line,
                        kind
                    ));
                }
                from = after;
            }
        }

        // Vacuity guard: the kernel has dozens of recording sites. If the
        // scanner suddenly sees almost none, the scan itself has rotted and
        // a green result would be meaningless.
        assert!(
            sites >= 20,
            "scanner found only {sites} `RecordedOperation::new(` sites — the scan is broken"
        );
        assert!(
            violations.is_empty(),
            "lineage ratchet violations:\n{}",
            violations.join("\n")
        );
    }
}
