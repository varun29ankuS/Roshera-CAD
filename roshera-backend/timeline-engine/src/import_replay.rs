//! Import steps: a file imported into a document is part of its history.
//!
//! An import (`POST /api/geometry/import_step`, `/import_ros`) splices parts
//! that no kernel operation built, so no recorded kernel call can rebuild
//! them. The history therefore records the import itself: one
//! `Operation::Generic` step whose `params` are an [`ImportRecord`] — the
//! file's sha256, its format, the source label the importer was given, and
//! the caller's name — and whose `outputs` are the `solid:<id>` of every part
//! it produced, in splice order. The file's exact bytes are stored with the
//! document, addressed by that sha256.
//!
//! Replaying the step re-runs the same import from those bytes and splices
//! the result exactly as the live route did. The importer is deterministic
//! (face and solid persistent ids derive from the file content), so the same
//! solids, ids and names come back, and the `set_name` steps that follow
//! still apply.
//!
//! This crate cannot run the importer itself (`export-engine` depends on
//! `timeline-engine`, not the reverse), so the process that owns the blob
//! store installs an [`ImportReplayer`] once at start-up. Every replay path
//! (`rebuild_model_from_events`, the rebuild certificate, frames, undo/redo,
//! branch switch) goes through `apply_event`, which reaches the installed
//! replayer — none of them can skip an import.
//!
//! When a step cannot be re-run — no replayer, the bytes are missing or no
//! longer hash to the recorded sha256, the re-import yields a different part
//! count — replay fails with the typed
//! [`ReplayError::ImportUnreplayable`](crate::replay::ReplayError::ImportUnreplayable).
//! A rebuild quarantines there and names the file. It never drops the parts
//! silently.

use std::sync::{Arc, OnceLock};

use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;
use serde::{Deserialize, Serialize};

/// The timeline kind of a STEP import step.
pub const IMPORT_STEP_KIND: &str = "import_step";
/// The timeline kind of a `.ros` geometry import step.
pub const IMPORT_ROS_KIND: &str = "import_ros";

/// The address of a file's bytes: their sha256, lowercase hex. The one
/// digest both the recording side and the replaying side use.
pub fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// True for the kinds this module replays.
pub fn is_import_kind(kind: &str) -> bool {
    kind == IMPORT_STEP_KIND || kind == IMPORT_ROS_KIND
}

/// What an import step records: everything the re-import needs, and a real
/// reference (`sha256`) to the stored bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportRecord {
    /// `"step"` or `"ros"`.
    pub format: String,
    /// Lowercase-hex sha256 of the imported file's exact bytes — the blob's
    /// address in the document's store.
    pub sha256: String,
    /// Length of those bytes (a cheap cross-check beside the digest).
    pub byte_len: u64,
    /// The label the importer was given (the file's name, or the caller's
    /// `filename`). It feeds the importer, so it is part of the recipe.
    pub source_label: String,
    /// The name the caller gave the import (`name`), if any.
    #[serde(default)]
    pub name: Option<String>,
    /// The persistent id of each part the import produced, aligned with the
    /// step's `outputs` (lowercase hex of the 128-bit id; empty string for a
    /// part that had none). The importer derives these from the file's
    /// content, so they — not the order the importer happens to emit parts
    /// in — say which re-imported part is which recorded part.
    #[serde(default)]
    pub solid_pids: Vec<String>,
}

/// The hex form [`ImportRecord::solid_pids`] holds.
pub fn pid_hex(pid: geometry_engine::primitives::persistent_id::PersistentId) -> String {
    format!("{:032x}", pid.as_u128())
}

impl ImportRecord {
    /// The record as the step's `params` payload.
    pub fn to_params(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    /// Read a record back from a step's `params` payload.
    pub fn from_params(params: &serde_json::Value) -> Result<Self, String> {
        let record: Self = serde_json::from_value(params.clone())
            .map_err(|e| format!("malformed import record: {e}"))?;
        if record.sha256.len() != 64 || !record.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!(
                "import record sha256 {:?} is not a 64-digit hex digest",
                record.sha256
            ));
        }
        Ok(record)
    }
}

/// Why an installed replayer could not re-run an import.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImportReplayError {
    /// The document's store holds no blob with this digest.
    #[error("the imported file's bytes are not stored with this document")]
    BlobMissing,
    /// The stored bytes no longer hash to the recorded digest.
    #[error("the stored bytes hash to {actual}, not the recorded digest")]
    BlobCorrupt {
        /// The digest the stored bytes actually have.
        actual: String,
    },
    /// The importer refused the bytes.
    #[error("the re-import failed: {0}")]
    ImportFailed(String),
    /// The bytes re-import, but not into the parts the step recorded.
    #[error("the re-import yields {found} parts, the step recorded {recorded}")]
    PartCountChanged {
        /// Parts the re-import produced.
        found: usize,
        /// Parts the step recorded.
        recorded: usize,
    },
}

/// Re-runs a recorded import against a model. Installed once per process by
/// the owner of the blob store (the api-server).
pub trait ImportReplayer: Send + Sync {
    /// Re-import the file `record` names and splice its parts into `model`
    /// exactly as the live import route did, returning the new solid ids in
    /// splice order. `recorded_parts` is how many parts the step recorded:
    /// a re-import that yields any other count is refused BEFORE `model` is
    /// touched ([`ImportReplayError::PartCountChanged`]).
    fn replay_import(
        &self,
        model: &mut BRepModel,
        record: &ImportRecord,
        recorded_parts: usize,
    ) -> Result<Vec<SolidId>, ImportReplayError>;
}

static REPLAYER: OnceLock<Arc<dyn ImportReplayer>> = OnceLock::new();

/// Install the process's import replayer. The first install wins; a later
/// call returns `false` and changes nothing (one blob store per process).
pub fn install_import_replayer(replayer: Arc<dyn ImportReplayer>) -> bool {
    REPLAYER.set(replayer).is_ok()
}

/// The installed replayer, if any.
pub fn import_replayer() -> Option<&'static Arc<dyn ImportReplayer>> {
    REPLAYER.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> ImportRecord {
        ImportRecord {
            format: "step".into(),
            sha256: "ab".repeat(32),
            byte_len: 10,
            source_label: "part.step".into(),
            name: Some("Bracket".into()),
            solid_pids: vec!["0".repeat(32)],
        }
    }

    #[test]
    fn record_round_trips_through_params() {
        let r = record();
        assert_eq!(ImportRecord::from_params(&r.to_params()), Ok(r));
    }

    /// `dispatch_generic` matches these kinds as literals (the arm-coverage
    /// gate reads literal arms); they must stay the constants' values.
    #[test]
    fn the_import_kinds_are_the_replay_arm_literals() {
        assert_eq!(IMPORT_STEP_KIND, "import_step");
        assert_eq!(IMPORT_ROS_KIND, "import_ros");
        assert!(is_import_kind("import_step") && is_import_kind("import_ros"));
        assert!(!is_import_kind("import.step"));
    }

    #[test]
    fn digest_is_the_sha256_of_the_bytes() {
        // FIPS 180-2 test vector.
        assert_eq!(
            digest(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_record_without_a_real_digest_is_refused() {
        let mut r = record();
        r.sha256 = "not-a-digest".into();
        assert!(ImportRecord::from_params(&r.to_params()).is_err());
        let mut short = record();
        short.sha256 = "ab".into();
        assert!(ImportRecord::from_params(&short.to_params()).is_err());
    }
}
