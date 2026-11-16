//! Kernel Lens — the kernel shows its work.
//!
//! Every decision the kernel makes about a solid that a person or an agent
//! would want to know about — a loop walked reversed on import, an edge gap
//! repaired, a certificate check a face blocks — is recorded as a
//! [`LensEvent`] on the entity it concerns, with the rule it followed and the
//! numbers it measured. Events are emitted by the code that makes the
//! decision, never reconstructed afterwards, and are keyed by
//! [`PersistentId`]: stable across runs and across merges into a session,
//! where numeric face ids are not.
//!
//! The person sees a [`LensJournal`] in the app (faces coloured by
//! [`FaceStatus`], a story per face); the agent reads the same journal over
//! MCP.

use serde::{Deserialize, Serialize};

use crate::primitives::persistent_id::PersistentId;

/// What an event is about. Persistent ids serialise as 32-digit hex strings
/// (a 128-bit number does not survive JSON in a browser).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityRef {
    Face(#[serde(with = "pid_hex")] PersistentId),
    Solid(#[serde(with = "pid_hex")] PersistentId),
}

/// Which part of the kernel made the decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Import,
    Repair,
    Mesh,
    Certify,
}

/// What the kernel decided.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// A face bound marked `.F.` walked reversed (ISO 10303-42).
    LoopReversed,
    /// The face's outer loop chosen geometrically, not as the first bound.
    OuterLoopChosen,
    /// A swept surface's profile put on [0, 1].
    ProfileRescaled,
    /// An edge's tolerance raised to its measured gap from the face.
    EdgeToleranceRaised,
    /// The face's loops reversed to match its neighbours.
    LoopReversedToMatchNeighbours,
    /// A void shell turned inside out (`ORIENTED_CLOSED_SHELL .F.`).
    VoidShellReversed,
    /// Two vertices at one point, which the face's loop passed between, made
    /// one so the loop chains.
    VerticesMerged,
    /// A certificate check this entity blocks.
    Blocking { conjunct: String },
}

impl EventKind {
    /// The kind's name as serialised (`"loop_reversed"`, `"blocking"`, ...).
    pub fn name(&self) -> &'static str {
        match self {
            EventKind::LoopReversed => "loop_reversed",
            EventKind::OuterLoopChosen => "outer_loop_chosen",
            EventKind::ProfileRescaled => "profile_rescaled",
            EventKind::EdgeToleranceRaised => "edge_tolerance_raised",
            EventKind::LoopReversedToMatchNeighbours => "loop_reversed_to_match_neighbours",
            EventKind::VoidShellReversed => "void_shell_reversed",
            EventKind::VerticesMerged => "vertices_merged",
            EventKind::Blocking { .. } => "blocking",
        }
    }
}

/// One decision, on one entity, with its reason and evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LensEvent {
    pub entity: EntityRef,
    pub stage: Stage,
    pub kind: EventKind,
    /// The source STEP entity `#N` the decision concerned, when imported.
    pub step_instance: Option<u64>,
    /// The rule followed, in one line.
    pub why: String,
    /// The numbers measured, by name (e.g. `("gap_mm", 0.0032)`).
    pub measured: Vec<(String, f64)>,
}

/// How a face reads in the Lens. Blocking outranks repaired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaceStatus {
    AsWritten,
    Repaired,
    Blocking,
}

/// Every event recorded about one solid (or an import's solids).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LensJournal {
    events: Vec<LensEvent>,
}

impl LensJournal {
    /// Record one event.
    pub fn push(&mut self, event: LensEvent) {
        self.events.push(event);
    }

    /// Every event, in the order recorded.
    pub fn events(&self) -> &[LensEvent] {
        &self.events
    }

    /// The events about face `pid`.
    pub fn for_face(&self, pid: PersistentId) -> impl Iterator<Item = &LensEvent> {
        self.events
            .iter()
            .filter(move |e| e.entity == EntityRef::Face(pid))
    }

    /// The events about solid `pid` itself (not its faces).
    pub fn for_solid(&self, pid: PersistentId) -> impl Iterator<Item = &LensEvent> {
        self.events
            .iter()
            .filter(move |e| e.entity == EntityRef::Solid(pid))
    }

    /// How face `pid` reads: blocking if any event blocks a check, repaired
    /// if the kernel changed anything on it, else as written.
    pub fn status(&self, pid: PersistentId) -> FaceStatus {
        let mut status = FaceStatus::AsWritten;
        for e in self.for_face(pid) {
            if matches!(e.kind, EventKind::Blocking { .. }) {
                return FaceStatus::Blocking;
            }
            status = FaceStatus::Repaired;
        }
        status
    }

    /// Event counts by kind, in first-seen order.
    pub fn overview(&self) -> Vec<(String, usize)> {
        let mut out: Vec<(String, usize)> = Vec::new();
        for e in &self.events {
            let name = e.kind.name();
            match out.iter_mut().find(|(k, _)| k == name) {
                Some((_, n)) => *n += 1,
                None => out.push((name.to_string(), 1)),
            }
        }
        out
    }
}

/// Serialise a [`PersistentId`] as a 32-digit lower-case hex string.
mod pid_hex {
    use super::PersistentId;
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(pid: &PersistentId, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{:032x}", pid.0))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<PersistentId, D::Error> {
        let text = String::deserialize(d)?;
        u128::from_str_radix(&text, 16)
            .map(PersistentId)
            .map_err(|e| D::Error::custom(format!("persistent id {text:?}: {e}")))
    }
}
