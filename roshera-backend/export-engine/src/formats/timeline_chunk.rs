//! .ros v3.1 HIST chunk — event-sourced timeline + branch manifest.
//!
//! Slice 2 elevated the timeline to a co-equal first-class chunk
//! alongside provenance. Every .ros v3.1 file MUST carry a HIST chunk;
//! an empty `events` vector is valid (the file simply has no recorded
//! history yet) but the chunk itself is mandatory.
//!
//! The on-disk schema mirrors a stripped-down view of the
//! timeline-engine `Branch` so we capture the manifest without
//! pulling the live `DashMap<EventIndex, TimelineEvent>` (which is
//! `#[serde(skip)]` on the kernel side anyway). Events are stored as
//! a flat `Vec<TimelineEvent>` keyed on each event's own
//! `metadata.branch_id`; readers reconstruct per-branch indices by
//! grouping on that field.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use timeline_engine::{
    BranchId, BranchMetadata, BranchState, EventIndex, ForkPoint, TimelineEvent,
};

use ros_format::{Result, RosFileError};

/// Compact branch manifest — everything the on-disk format needs to
/// reconstruct a branch's identity without the runtime event map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchManifest {
    /// Branch identifier.
    pub id: BranchId,
    /// Human-readable name.
    pub name: String,
    /// Parent branch id, if any.
    pub parent: Option<BranchId>,
    /// Where this branch forked off its parent.
    pub fork_point: ForkPoint,
    /// Lifecycle state at write time.
    pub state: BranchState,
    /// Creation/purpose metadata.
    pub metadata: BranchMetadata,
    /// Branch protection flag (matches `Branch.protected`).
    #[serde(default)]
    pub protected: bool,
    /// Hidden flag (matches `Branch.hidden`).
    #[serde(default)]
    pub hidden: bool,
}

impl BranchManifest {
    /// Convenience: the sequence number where this branch diverged.
    pub fn fork_point_seq(&self) -> EventIndex {
        self.fork_point.event_index
    }

    /// Build a manifest from a live `timeline_engine::Branch`.
    pub fn from_branch(branch: &timeline_engine::Branch) -> Self {
        BranchManifest {
            id: branch.id,
            name: branch.name.clone(),
            parent: branch.parent,
            fork_point: branch.fork_point.clone(),
            state: branch.state.clone(),
            metadata: branch.metadata.clone(),
            protected: branch.protected,
            hidden: branch.hidden,
        }
    }
}

/// HIST chunk payload — branches and a flat list of timeline events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistChunk {
    /// On-disk schema version of the HIST chunk body.
    pub schema_version: u32,
    /// Wall-clock the file was written (independent of file header
    /// `creation_time` so HIST stays self-describing if extracted).
    pub written_at: DateTime<Utc>,
    /// All branches present in the timeline at write time.
    pub branches: Vec<BranchManifest>,
    /// Flat event list. Per-branch grouping is derived from
    /// `event.metadata.branch_id`.
    pub events: Vec<TimelineEvent>,
}

impl HistChunk {
    /// Current HIST schema version.
    pub const SCHEMA_VERSION: u32 = 1;

    /// Build an empty HIST chunk (no branches, no events).
    ///
    /// Valid on disk per the v3.1 spec — a file may be saved before
    /// any events are recorded; the chunk's presence is the
    /// contract, not its contents.
    pub fn empty() -> Self {
        HistChunk {
            schema_version: Self::SCHEMA_VERSION,
            written_at: Utc::now(),
            branches: Vec::new(),
            events: Vec::new(),
        }
    }

    /// Build a HIST chunk from concrete branch and event slices.
    pub fn new(branches: Vec<BranchManifest>, events: Vec<TimelineEvent>) -> Self {
        HistChunk {
            schema_version: Self::SCHEMA_VERSION,
            written_at: Utc::now(),
            branches,
            events,
        }
    }

    /// Encode to MessagePack.
    pub fn serialize(&self) -> Result<Vec<u8>> {
        rmp_serde::to_vec_named(self).map_err(|e| RosFileError::Other {
            message: format!("Failed to serialize HIST chunk: {}", e),
            source: None,
        })
    }

    /// Decode from MessagePack.
    pub fn deserialize(bytes: &[u8]) -> Result<Self> {
        rmp_serde::from_slice(bytes).map_err(|e| RosFileError::Other {
            message: format!("Failed to deserialize HIST chunk: {}", e),
            source: None,
        })
    }

    /// Group events by branch id, preserving original order within each
    /// branch. Useful for replay drivers that operate per-branch.
    pub fn events_by_branch(&self) -> std::collections::HashMap<BranchId, Vec<&TimelineEvent>> {
        let mut grouped: std::collections::HashMap<BranchId, Vec<&TimelineEvent>> =
            std::collections::HashMap::new();
        for event in &self.events {
            grouped
                .entry(event.metadata.branch_id)
                .or_default()
                .push(event);
        }
        grouped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use timeline_engine::{
        Author, BranchPurpose, EventId, EventIndex, EventMetadata, Operation, OperationInputs,
        OperationOutputs, PrimitiveType, TimelineEvent,
    };

    fn synth_event(branch: BranchId, seq: EventIndex) -> TimelineEvent {
        TimelineEvent {
            id: EventId::new(),
            sequence_number: seq,
            timestamp: Utc::now(),
            author: Author::System,
            operation: Operation::CreatePrimitive {
                primitive_type: PrimitiveType::Box,
                parameters: serde_json::json!({}),
            },
            inputs: OperationInputs::default(),
            outputs: OperationOutputs::default(),
            metadata: EventMetadata {
                description: None,
                branch_id: branch,
                tags: vec![],
                properties: Default::default(),
            },
        }
    }

    #[test]
    fn test_empty_round_trip() {
        let chunk = HistChunk::empty();
        let bytes = chunk.serialize().unwrap();
        let decoded = HistChunk::deserialize(&bytes).unwrap();
        assert!(decoded.branches.is_empty());
        assert!(decoded.events.is_empty());
        assert_eq!(decoded.schema_version, HistChunk::SCHEMA_VERSION);
    }

    #[test]
    fn test_events_by_branch_grouping() {
        let main = BranchId::main();
        let alt = BranchId::new();

        let manifest = BranchManifest {
            id: main,
            name: "main".to_string(),
            parent: None,
            fork_point: ForkPoint {
                branch_id: main,
                event_index: 0,
                timestamp: Utc::now(),
            },
            state: BranchState::Active,
            metadata: BranchMetadata {
                created_by: Author::System,
                created_at: Utc::now(),
                purpose: BranchPurpose::UserExploration {
                    description: "init".to_string(),
                },
                ai_context: None,
                checkpoints: vec![],
            },
            protected: true,
            hidden: false,
        };

        let chunk = HistChunk::new(
            vec![manifest],
            vec![
                synth_event(main, 0),
                synth_event(alt, 0),
                synth_event(main, 1),
            ],
        );

        let bytes = chunk.serialize().unwrap();
        let decoded = HistChunk::deserialize(&bytes).unwrap();

        let grouped = decoded.events_by_branch();
        assert_eq!(grouped.get(&main).map(|v| v.len()).unwrap_or(0), 2);
        assert_eq!(grouped.get(&alt).map(|v| v.len()).unwrap_or(0), 1);
        assert_eq!(decoded.branches.len(), 1);
        assert_eq!(decoded.branches[0].fork_point_seq(), 0);
    }
}
