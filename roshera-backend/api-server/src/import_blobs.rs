//! Imported files as part of a document's history.
//!
//! `POST /api/geometry/import_step` and `/import_ros` splice parts no kernel
//! operation built. Before this module, the timeline recorded only their
//! `set_name` events, so every rebuild of the document from its log — re-open,
//! tab switch, restart, undo — silently dropped every imported part.
//!
//! Now an import:
//! 1. stores the file's exact bytes with the document, addressed by sha256
//!    (`DatabasePersistence::save_import_blob`) — awaited BEFORE the model is
//!    touched, so a part never lands that the store could not keep;
//! 2. records one import step (`timeline_engine::import_replay`) naming that
//!    sha256 and listing the solids it produced, ahead of the `set_name`
//!    steps that name them.
//!
//! Replay re-runs the import from the bytes through [`StoredImportReplayer`],
//! installed once per process. Replay is synchronous and the store is not, so
//! the bytes replay reads come from an in-memory, content-addressed map: the
//! live import puts them there, and [`preload`] fills it from the store before
//! a document's history is replayed (boot and document open). A digest the
//! map does not hold is a typed refusal (`ImportUnreplayable`), never a skip.

use std::sync::{Arc, LazyLock};

use dashmap::DashMap;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;
use timeline_engine::import_replay::{
    digest, ImportRecord, ImportReplayError, ImportReplayer, IMPORT_ROS_KIND, IMPORT_STEP_KIND,
};
use timeline_engine::{Operation, TimelineEvent};

use crate::error_catalog::{ApiError, ErrorCode};
use crate::AppState;

/// Bytes of every imported file this process has stored or loaded, by
/// sha256. Content-addressed, so one entry serves every document that
/// imported the same file.
static BLOBS: LazyLock<DashMap<String, Arc<Vec<u8>>>> = LazyLock::new(DashMap::new);

/// Install [`StoredImportReplayer`] as this process's import replayer.
/// Idempotent: the first install wins and every later call is a no-op.
pub(crate) fn install() {
    timeline_engine::import_replay::install_import_replayer(Arc::new(StoredImportReplayer));
}

/// The file format an import step records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImportFormat {
    Step,
    Ros,
}

impl ImportFormat {
    fn tag(self) -> &'static str {
        match self {
            ImportFormat::Step => "step",
            ImportFormat::Ros => "ros",
        }
    }

    fn kind(self) -> &'static str {
        match self {
            ImportFormat::Step => IMPORT_STEP_KIND,
            ImportFormat::Ros => IMPORT_ROS_KIND,
        }
    }
}

/// Re-import the bytes of `format` exactly as the live route does, into a
/// fresh model (no file I/O). Shared by the replayer so the two cannot drift.
pub(crate) fn import_bytes(
    format: ImportFormat,
    bytes: &[u8],
    source_label: &str,
) -> Result<BRepModel, String> {
    match format {
        ImportFormat::Step => {
            let text = export_engine::formats::step::parser::decode_step_bytes(bytes.to_vec());
            export_engine::formats::step::import_step_text_uncertified(&text, source_label)
                .map(|(model, _report)| model)
                .map_err(|e| format!("{e:?}"))
        }
        ImportFormat::Ros => export_engine::formats::ros::import_ros_bytes(bytes.to_vec(), None)
            .and_then(|ros| ros.into_model())
            .map_err(|e| e.to_string()),
    }
}

/// An import whose bytes are stored and which is ready to be recorded once
/// its parts are spliced.
#[derive(Debug, Clone)]
pub(crate) struct StoredImport {
    format: ImportFormat,
    record: ImportRecord,
}

/// Step 1: keep the file's exact bytes with the document the import lands
/// in. Awaited before the model is touched — a store that cannot keep the
/// file refuses the import (a part that would vanish on the next open is not
/// imported). With durability off nothing is promised and nothing is stored,
/// but the bytes still serve this process's replays (undo, scrub).
pub(crate) async fn store(
    state: &AppState,
    format: ImportFormat,
    bytes: Vec<u8>,
    source_label: &str,
    name: Option<&str>,
) -> Result<StoredImport, ApiError> {
    // Any process that records an import step can replay it.
    install();
    let sha256 = digest(&bytes);
    let byte_len = bytes.len() as u64;
    let bytes = if crate::durability::durability_enabled() {
        let document = crate::durability::write_document(state).await;
        let blob = session_manager::ImportBlobRecord {
            session_id: document,
            sha256: sha256.clone(),
            bytes,
            created_at: chrono::Utc::now().timestamp_millis(),
        };
        state.database.save_import_blob(&blob).await.map_err(|e| {
            ApiError::new(
                ErrorCode::Internal,
                format!(
                    "the imported file could not be stored with the document, so its \
                     parts would not survive a re-open; nothing was imported: {e}"
                ),
            )
        })?;
        blob.bytes
    } else {
        bytes
    };
    BLOBS
        .entry(sha256.clone())
        .or_insert_with(|| Arc::new(bytes));
    Ok(StoredImport {
        format,
        record: ImportRecord {
            format: format.tag().to_string(),
            sha256,
            byte_len,
            source_label: source_label.to_string(),
            name: name.map(str::to_string),
            solid_pids: Vec::new(),
        },
    })
}

/// Step 2: record the import step, naming the stored blob and every solid
/// the splice produced (in splice order). Call right after the merge, before
/// any `set_name` of those solids, so the step's sequence precedes them.
/// Durability-gated like `record_set_name`.
pub(crate) fn record(
    state: &AppState,
    stored: &StoredImport,
    model: &BRepModel,
    solids: &[SolidId],
) {
    if !crate::durability::durability_enabled() {
        return;
    }
    use geometry_engine::operations::recorder::OperationRecorder as _;
    // Each part's persistent id, aligned with the outputs: replay pairs the
    // re-imported parts with these by id, not by emission order.
    let mut record = stored.record.clone();
    record.solid_pids = solids
        .iter()
        .map(|&id| {
            model
                .solid_pid(id)
                .map(timeline_engine::import_replay::pid_hex)
                .unwrap_or_default()
        })
        .collect();
    let op = geometry_engine::operations::recorder::RecordedOperation::new(stored.format.kind())
        .with_parameters(record.to_params())
        .with_output_solids(solids.iter().map(|&id| id as u64));
    if let Err(e) = state.timeline_recorder.record(op) {
        tracing::warn!(target: "durability", "import step not recorded: {e}");
    }
}

/// Make the bytes of every import step in `events` replayable in this
/// process: each digest not already held is read from document `document_id`'s
/// store and verified. A blob the store does not hold (or that no longer
/// hashes to its address) is left out — the replay of that step then refuses
/// with a typed reason naming it.
pub(crate) async fn preload<'a>(
    state: &AppState,
    document_id: &str,
    events: impl IntoIterator<Item = &'a TimelineEvent>,
) {
    // Any process that replays a history can replay its import steps.
    install();
    let wanted: Vec<(u64, ImportRecord)> = events
        .into_iter()
        .filter_map(|event| {
            let Operation::Generic {
                command_type,
                parameters,
            } = &event.operation
            else {
                return None;
            };
            if !timeline_engine::import_replay::is_import_kind(command_type) {
                return None;
            }
            let params = parameters.get("params").unwrap_or(parameters);
            let record = ImportRecord::from_params(params).ok()?;
            (!BLOBS.contains_key(&record.sha256)).then_some((event.sequence_number, record))
        })
        .collect();
    for (sequence, record) in wanted {
        if BLOBS.contains_key(&record.sha256) {
            continue;
        }
        match state
            .database
            .load_import_blob(document_id, &record.sha256)
            .await
        {
            Ok(Some(blob)) => {
                let actual = digest(&blob.bytes);
                if actual == record.sha256 {
                    BLOBS.insert(record.sha256.clone(), Arc::new(blob.bytes));
                } else {
                    tracing::error!(
                        target: "durability",
                        sequence,
                        recorded = %record.sha256,
                        actual = %actual,
                        "stored import blob does not hash to its address; the import will refuse to replay"
                    );
                }
            }
            Ok(None) => tracing::error!(
                target: "durability",
                sequence,
                sha256 = %record.sha256,
                "import blob missing from the document store; the import will refuse to replay"
            ),
            Err(e) => tracing::error!(
                target: "durability",
                sequence,
                sha256 = %record.sha256,
                error = %e,
                "import blob could not be read; the import will refuse to replay"
            ),
        }
    }
}

/// Forget a digest's bytes in this process (a test simulating a fresh
/// process, which must then read them from the store).
#[cfg(test)]
pub(crate) fn forget(sha256: &str) {
    BLOBS.remove(sha256);
}

/// The replayer: bytes from [`BLOBS`], verified against the recorded digest,
/// re-imported with the route's own importer, spliced with the route's own
/// merge.
struct StoredImportReplayer;

impl ImportReplayer for StoredImportReplayer {
    fn replay_import(
        &self,
        model: &mut BRepModel,
        record: &ImportRecord,
        recorded_parts: usize,
    ) -> Result<Vec<SolidId>, ImportReplayError> {
        let format = match record.format.as_str() {
            "step" => ImportFormat::Step,
            "ros" => ImportFormat::Ros,
            other => {
                return Err(ImportReplayError::ImportFailed(format!(
                    "unknown import format {other:?}"
                )))
            }
        };
        let bytes = BLOBS
            .get(&record.sha256)
            .map(|b| Arc::clone(b.value()))
            .ok_or(ImportReplayError::BlobMissing)?;
        let actual = digest(&bytes);
        if actual != record.sha256 {
            return Err(ImportReplayError::BlobCorrupt { actual });
        }
        let imported = import_bytes(format, &bytes, &record.source_label)
            .map_err(ImportReplayError::ImportFailed)?;
        if imported.solids.len() != recorded_parts {
            return Err(ImportReplayError::PartCountChanged {
                found: imported.solids.len(),
                recorded: recorded_parts,
            });
        }
        Ok(export_engine::formats::step::merge_solids_into(
            model, &imported,
        ))
    }
}
