//! .ros v3.1 file format export/import.
//!
//! Slice 2 (2026-05-06) reshaped the format so timeline (HIST) and AI
//! provenance (PROV) are mandatory first-class chunks; geometry (GEOM)
//! is now optional cache that readers can regenerate from HIST events
//! via `timeline_engine::rebuild_model_from_events`.
//!
//! ## Layout
//! ```text
//! Header (128 bytes, v3.1)
//! ├── META  (JSON)         — author, units, software, vertex/face counts,
//! │                          replay_status (the file's own statement of
//! │                          whether its HIST events replay — see
//! │                          [`RosReplayStatus`])
//! ├── HIST  (MessagePack)  — timeline events + branch manifest. MANDATORY.
//! ├── PROV  (MessagePack)  — AI command log + privacy. MANDATORY.
//! ├── GEOM  (MessagePack)  — BRepSnapshot. OPTIONAL cache.
//! └── SIGN  (MessagePack)  — Ed25519 signature. OPTIONAL.
//! ```
//!
//! ## Signing
//!
//! When `RosExportOptions::sign` is true the writer computes a SHA-256
//! Merkle root over the ON-DISK bytes of every content chunk (META,
//! HIST, PROV, GEOM — in chunk-table order), signs that root with the
//! caller-supplied Ed25519 key, and stores the signature in a SIGN
//! chunk. The header's signature claim is set in the same function that
//! writes the chunk, after it is written — no code path can produce a
//! header that claims a signature the file does not carry. On import
//! the root is recomputed from the raw file bytes and the signature is
//! verified; the result is a three-state [`RosSignatureVerdict`], never
//! a bool, because "unsigned" and "signature failed" are different
//! facts. A header that claims a signature over a file with no SIGN
//! chunk is a hard error — that is a forged claim, not a warning.

use crate::formats::ros_snapshot::BRepSnapshot;
use crate::formats::timeline_chunk::{BranchManifest, HistChunk};
use geometry_engine::primitives::topology_builder::BRepModel;
use ros_format::keys::{KeyManager, SoftwareKeyManager};
use ros_format::merkle::{HashAlgorithm, MerkleTree};
use ros_format::signature::{FileSigner, SignatureAlgorithm, SignatureChunk, SignatureVerifier};
use ros_format::util::{current_time_ms, sha256, to_hex};
use ros_format::{
    self, AICommandTracker, Chunk, ChunkType, PrivacySettings, ProvChunk, TrackingLevel,
    CHUNK_INDEX_ENTRY_SIZE,
};
use shared_types::*;
use std::io::Cursor;
use std::path::Path;
use timeline_engine::TimelineEvent;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Convert `ros_format::RosFileError` to `shared_types::ExportError`.
///
/// Both types live in foreign crates so the orphan rule prevents an
/// `impl From<…> for …`; callers `.map_err(ros_err)` instead.
fn ros_err(err: ros_format::error::RosFileError) -> ExportError {
    ExportError::ExportFailed {
        reason: format!("ROS file error: {}", err),
    }
}

/// Timeline payload destined for the HIST chunk.
///
/// Built by callers that have access to a `timeline_engine::Timeline`;
/// `RosExportPayload::history` is `None` when the writer simply wants
/// an empty manifest (still produces a valid v3.1 file).
#[derive(Debug, Clone, Default)]
pub struct HistData {
    pub branches: Vec<BranchManifest>,
    pub events: Vec<TimelineEvent>,
}

impl HistData {
    pub fn new(branches: Vec<BranchManifest>, events: Vec<TimelineEvent>) -> Self {
        HistData { branches, events }
    }
}

/// Everything the writer needs in addition to the file path.
pub struct RosExportPayload<'a> {
    /// Geometry source. Always supplied so callers can opt out of the
    /// snapshot via `RosExportOptions::include_snapshot`.
    pub model: &'a BRepModel,
    /// Timeline data destined for HIST. `None` writes an empty
    /// manifest (which is still valid).
    pub history: Option<HistData>,
    /// AI tracker destined for PROV. `None` writes an empty tracker
    /// using `options.tracking_level` and a default privacy policy.
    pub aipr: Option<AICommandTracker>,
}

/// Export options for .ros v3.1.
///
/// `track_ai` and `ai_tracking_level: u8` from v3.0 are gone — PROV is
/// mandatory and the tracking level is a typed enum. `include_snapshot`
/// replaces the implicit "always write GEOM" behaviour.
#[derive(Debug, Clone)]
pub struct RosExportOptions {
    /// Write a GEOM cache chunk. Default true. When false, readers
    /// must rebuild geometry from HIST events.
    pub include_snapshot: bool,

    /// AI provenance tracking level for the PROV chunk header.
    pub tracking_level: TrackingLevel,

    /// Verify at export time that the HIST events actually replay into a
    /// fresh model, and record the honest outcome in META
    /// (`replay_status`). Default true. When false the file states
    /// `"unverified"` — the field is never absent and never defaults to
    /// a pass; see [`RosReplayStatus`].
    pub verify_replay: bool,

    /// Sign the file with Ed25519. Requires `signing_key`; `sign: true`
    /// with no key is a typed refusal, not a silently minted throwaway
    /// key — a signature from a per-file ephemeral key proves nothing
    /// about who authored the file, and an IP-attribution artifact must
    /// not carry a meaningless signature dressed up as a real one.
    pub sign: bool,

    /// Ed25519 signing key (32 bytes). Mandatory when `sign` is true.
    pub signing_key: Option<[u8; 32]>,

    /// Encrypt chunks with the given password. `Some(_)` enables
    /// AES-256-GCM with PBKDF2 key derivation; `None` writes plain.
    pub password: Option<String>,

    /// Author/creator name (META).
    pub author: String,

    /// Software string (META).
    pub software: String,

    /// Units (META).
    pub units: String,
}

impl Default for RosExportOptions {
    fn default() -> Self {
        Self {
            include_snapshot: true,
            // Detailed, not Basic: `Basic.should_track_prompts()` is
            // false, so a Basic default would strip every recorded
            // `roshera.intent` text on the way out — the authorship
            // record an IP claim rests on. Callers wanting redaction opt
            // into Basic explicitly, and lose nothing provable:
            // `aipr.rs` (and `ros_provenance::ai_tracker_from_timeline`)
            // compute the prompt HASH *before* the privacy gate, so a
            // redacted file still carries a commitment to the text —
            // redaction and provability are not in tension here.
            tracking_level: TrackingLevel::Detailed,
            verify_replay: true,
            sign: false,
            signing_key: None,
            password: None,
            author: "Roshera CAD".to_string(),
            software: "Roshera Export Engine v1.0".to_string(),
            units: "millimeters".to_string(),
        }
    }
}

/// Signature verdict for an imported .ros file.
///
/// Deliberately an enum, never a bool: "the file carries no signature"
/// and "the file carries a signature that does not verify" are
/// different facts and must not collapse into each other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RosSignatureVerdict {
    /// The file neither claims nor carries a signature.
    Unsigned,
    /// The SIGN chunk's Ed25519 signature verifies against the Merkle
    /// root of the file's on-disk chunk bytes.
    ///
    /// `public_key` (hex) is the authenticated fact: the holder of this
    /// key signed exactly these bytes. `signer_id` (hex) is metadata
    /// CARRIED by the signature record, not independently proven.
    Verified {
        signer_id: String,
        public_key: String,
    },
    /// A signature is present but does not verify — the file was
    /// modified after signing, the signature was transplanted, or the
    /// SIGN chunk is malformed.
    Invalid { reason: String },
}

/// What the writer reports about signing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RosWriteSignature {
    /// `options.sign` was false; no SIGN chunk, no header claim.
    Unsigned,
    /// A SIGN chunk was written and the header claims it. Both hex.
    Signed {
        signer_id: String,
        public_key: String,
    },
}

/// The failing event of an export-time replay-verification pass —
/// [`timeline_engine::ReplayFailure`] in its META wire shape.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RosReplayFailure {
    /// The failing event's stable sequence number.
    pub sequence_number: u64,
    /// The failing event's id (stringified UUID).
    pub event_id: String,
    /// The replay error, verbatim.
    pub error: String,
}

/// The file's own statement about whether its HIST events replay.
///
/// Written into META (`replay_status`) at export time so the caveat
/// travels WITH the file: whoever opens it in two years sees the verdict,
/// not a conversation that has evaporated. Three states, never two —
/// same rule as [`RosSignatureVerdict`]: "unverified" and "failed" are
/// different facts and must not collapse into each other, and the field
/// is never absent and never defaults to a pass.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum RosReplayStatus {
    /// Every HIST event re-applied cleanly into a fresh `BRepModel` at
    /// export time (`events_applied` of them; 0 for an empty timeline —
    /// vacuously clean, and the count makes that legible).
    Verified { events_applied: usize },
    /// Replay was attempted and could NOT fully re-apply. The file's
    /// geometry cache (GEOM, if present) is still exact; what this
    /// records is that the event history alone does not reproduce it.
    Incomplete {
        events_applied: usize,
        events_skipped: usize,
        first_failure: RosReplayFailure,
    },
    /// Replay was not attempted (`RosExportOptions::verify_replay` was
    /// false). Serialized as `"unverified"` — an explicit statement of
    /// no claim, distinct from both `verified` and `incomplete`.
    #[serde(rename = "unverified")]
    NotAttempted,
}

/// Reader output. `snapshot` is `None` when the file omitted the
/// optional GEOM chunk; callers may rebuild geometry by replaying
/// `timeline` against a fresh `BRepModel`.
pub struct RosImport {
    pub timeline: Vec<TimelineEvent>,
    pub branches: Vec<BranchManifest>,
    pub aipr: ProvChunk,
    pub snapshot: Option<BRepSnapshot>,
    /// Signature verdict, computed against the raw on-disk bytes before
    /// any chunk is decrypted or parsed.
    pub signature: RosSignatureVerdict,
    /// The file's own replay-status claim, read from META. Files written
    /// before the field existed made no claim, which reads back as
    /// [`RosReplayStatus::NotAttempted`] — the safe direction (absence
    /// never becomes a pass).
    pub replay_status: RosReplayStatus,
}

impl RosImport {
    /// Materialise a `BRepModel` from this import: use the GEOM cache
    /// when the file carried one, otherwise rebuild by replaying HIST
    /// events. The single materialisation path shared by
    /// [`import_ros_to_brep`] and the api-server import route.
    pub fn into_model(self) -> Result<BRepModel, shared_types::ExportError> {
        if let Some(snapshot) = self.snapshot {
            return Ok(snapshot.to_model());
        }
        let mut model = BRepModel::new();
        let outcome = timeline_engine::rebuild_model_from_events(&mut model, &self.timeline);
        if outcome.events_skipped > 0 {
            return Err(ExportError::ExportFailed {
                reason: format!(
                    "Failed to rebuild geometry from {} HIST events: {} skipped",
                    self.timeline.len(),
                    outcome.events_skipped
                ),
            });
        }
        Ok(model)
    }
}

/// What [`export_brep_to_ros`] actually wrote into the file's mandatory
/// HIST/PROV chunks — returned from the write site itself so an export
/// response can state the file's contents (event/branch/command counts,
/// PROV session id) without re-opening the file. A provenance-bearing
/// file and a bare geometry snapshot must be distinguishable from the
/// writer's own report, never inferred.
#[derive(Debug, Clone)]
pub struct RosWriteSummary {
    /// Timeline events written into HIST.
    pub hist_event_count: usize,
    /// Branch manifests written into HIST.
    pub hist_branch_count: usize,
    /// AI commands written into PROV.
    pub prov_command_count: usize,
    /// Session id recorded in PROV (freshly opened when no tracker was
    /// supplied — still a real, file-carried id).
    pub prov_session_id: u64,
    /// Whether a SIGN chunk was written, and with which key. This is
    /// the writer's own report from the write site — a file whose
    /// header claims a signature always has the chunk this reports.
    pub signature: RosWriteSignature,
    /// The replay-status verdict written into META — the writer's own
    /// report of what the file now states about itself.
    pub replay_status: RosReplayStatus,
}

/// Export a B-Rep model + timeline + provenance to .ros v3.1.
///
/// Returns a [`RosWriteSummary`] stating what the mandatory HIST/PROV
/// chunks actually carry.
pub async fn export_brep_to_ros(
    payload: RosExportPayload<'_>,
    path: &Path,
    options: RosExportOptions,
) -> Result<RosWriteSummary, shared_types::ExportError> {
    let mut file = File::create(path)
        .await
        .map_err(|_e| ExportError::FileWriteError {
            path: path.to_string_lossy().to_string(),
        })?;

    // Encryption setup -------------------------------------------------
    let encrypt = options.password.is_some();
    let (key_set, salt, file_iv) = if encrypt {
        let password = options
            .password
            .as_deref()
            .ok_or_else(|| ExportError::ExportFailed {
                reason: "Password required for encryption".to_string(),
            })?;

        let salt = ros_format::random_16();
        let file_iv: [u8; 8] =
            ros_format::random_bytes(8)
                .try_into()
                .map_err(|_| ExportError::ExportFailed {
                    reason: "random_bytes(8) did not return 8 bytes".to_string(),
                })?;
        let key_manager = SoftwareKeyManager::default();
        let key_set = key_manager.generate_key_set(password, &salt).map_err(|e| {
            ExportError::ExportFailed {
                reason: format!("Key generation failed: {}", e),
            }
        })?;
        (Some(key_set), salt, file_iv)
    } else {
        (None, [0u8; 16], [0u8; 8])
    };

    // Header -----------------------------------------------------------
    let mut header = ros_format::FileHeader::builder();
    if encrypt {
        // KDF id 2 = Argon2id; the third arg is Argon2's t_cost (passes),
        // recorded so the importer reproduces the exact derivation.
        header =
            header.with_encryption(1, 2, ros_format::keys::ROSHERA_KDF_TIME_COST, salt, file_iv);
    }
    // PROV is always present in v3.1, so the AI-provenance flag is
    // unconditionally set. The tracking level is taken from options.
    header = header.with_ai_tracking(options.tracking_level as u8);
    // NOTE: the header's signature claim is NOT set here. It is set by
    // `append_sign_chunk`, in the same statement group that pushes the
    // SIGN chunk, so the claim cannot exist without the chunk.
    let mut header = header.build();

    let mut chunks: Vec<Chunk> = Vec::new();

    // HIST payload (built before META: the replay-verification pass and
    // META's `replay_status` field both need the final event list) -----
    let hist_chunk = match payload.history {
        Some(data) => HistChunk::new(data.branches, data.events),
        None => HistChunk::empty(),
    };
    let hist_event_count = hist_chunk.events.len();
    let hist_branch_count = hist_chunk.branches.len();

    // Replay verification ---------------------------------------------
    // Attempt the replay the file's own readers would perform: rebuild a
    // model from the HIST events into a fresh `BRepModel` and record the
    // honest outcome in META. Opt-out via `options.verify_replay`, in
    // which case the file says exactly that ("unverified") — the field
    // is never absent and never defaults to a pass.
    let replay_status = if options.verify_replay {
        let mut replica = BRepModel::new();
        let outcome = timeline_engine::rebuild_model_from_events(&mut replica, &hist_chunk.events);
        if outcome.events_skipped == 0 {
            RosReplayStatus::Verified {
                events_applied: outcome.events_applied,
            }
        } else {
            match outcome.first_failure {
                Some(failure) => RosReplayStatus::Incomplete {
                    events_applied: outcome.events_applied,
                    events_skipped: outcome.events_skipped,
                    first_failure: RosReplayFailure {
                        sequence_number: failure.sequence_number,
                        event_id: failure.event_id,
                        error: failure.error,
                    },
                },
                // `rebuild_model_from_events` records the failure in the
                // same arm that increments `events_skipped`; reaching
                // this state means that invariant broke. Refuse rather
                // than write an Incomplete verdict with a fabricated
                // failure detail.
                None => {
                    return Err(ExportError::ExportFailed {
                        reason: format!(
                            "replay verification skipped {} of {} HIST events but \
                             ReplayOutcome carried no first-failure detail — \
                             timeline-engine invariant broken (every skip must \
                             record its failure); refusing to write a verdict \
                             with an invented failure",
                            outcome.events_skipped, hist_event_count
                        ),
                    })
                }
            }
        }
    } else {
        RosReplayStatus::NotAttempted
    };
    let replay_status_json =
        serde_json::to_value(&replay_status).map_err(|e| ExportError::ExportFailed {
            reason: format!("Failed to serialize replay_status for META: {}", e),
        })?;

    // META chunk -------------------------------------------------------
    let meta_data = serde_json::json!({
        "name": "Roshera CAD Model",
        "author": options.author,
        "created": current_time_ms(),
        "software": options.software,
        "units": options.units,
        "vertices": payload.model.vertices.len(),
        "edges": payload.model.edges.len(),
        "faces": payload.model.faces.len(),
        "solids": payload.model.solids.len(),
        "include_snapshot": options.include_snapshot,
        "replay_status": replay_status_json,
    })
    .to_string();
    chunks.push(Chunk::new(ChunkType::META, meta_data.into_bytes()));

    // HIST chunk (mandatory) ------------------------------------------
    let hist_bytes = hist_chunk.serialize().map_err(ros_err)?;
    chunks.push(encrypt_if_enabled(
        Chunk::new(ChunkType::HIST, hist_bytes),
        key_set.as_ref(),
        file_iv,
        chunks.len(),
    )?);

    // PROV chunk (mandatory) ------------------------------------------
    let prov_chunk = match &payload.aipr {
        Some(tracker) => ProvChunk::from_tracker(tracker),
        None => ProvChunk::empty(options.tracking_level, PrivacySettings::default()),
    };
    let prov_command_count = prov_chunk.commands.len();
    let prov_session_id = prov_chunk.session;
    let prov_bytes = prov_chunk.serialize().map_err(ros_err)?;
    chunks.push(encrypt_if_enabled(
        Chunk::new(ChunkType::PROV, prov_bytes),
        key_set.as_ref(),
        file_iv,
        chunks.len(),
    )?);

    // GEOM chunk (optional cache) -------------------------------------
    if options.include_snapshot {
        let snapshot = BRepSnapshot::from_model(payload.model)?;
        let geom_bytes =
            rmp_serde::to_vec_named(&snapshot).map_err(|e| ExportError::ExportFailed {
                reason: format!("Failed to serialize geometry: {}", e),
            })?;
        chunks.push(encrypt_if_enabled(
            Chunk::new(ChunkType::GEOM, geom_bytes),
            key_set.as_ref(),
            file_iv,
            chunks.len(),
        )?);
    }

    // SIGN chunk (optional) -------------------------------------------
    let signature = match (options.sign, options.signing_key) {
        (false, _) => RosWriteSignature::Unsigned,
        // REFUSAL, not a convenience fallback: minting a fresh per-file
        // key here (the old behaviour) produces a signature that proves
        // the bytes are self-consistent and proves NOTHING about who
        // authored them. For an IP-attribution artifact that is an
        // approximation labelled as exact, so it is refused outright.
        (true, None) => {
            return Err(ExportError::ExportFailed {
                reason: "REFUSED: sign=true requires a caller-supplied Ed25519 \
                         signing key (RosExportOptions::signing_key). A freshly \
                         minted per-file key would prove nothing about \
                         authorship, so no signature is emitted rather than a \
                         meaningless one."
                    .to_string(),
            })
        }
        (true, Some(key_bytes)) => append_sign_chunk(&mut chunks, &mut header, &key_bytes)?,
    };

    // Layout -----------------------------------------------------------
    let mut current_offset: u64 = 128;
    for chunk in &mut chunks {
        chunk.index.offset = current_offset;
        current_offset += chunk.data.len() as u64;
    }
    header.index_offset = current_offset;
    header.index_entry_count = chunks.len() as u32;
    header.file_size = current_offset + (chunks.len() * CHUNK_INDEX_ENTRY_SIZE) as u64;

    // Write ------------------------------------------------------------
    let mut buffer = Vec::new();
    {
        let mut cursor = Cursor::new(&mut buffer);
        header
            .write_to(&mut cursor)
            .map_err(|e| ExportError::ExportFailed {
                reason: format!("Failed to write header: {}", e),
            })?;
    }
    for chunk in &chunks {
        buffer.extend_from_slice(&chunk.data);
    }
    {
        let mut index_buf = Vec::new();
        let mut cursor = Cursor::new(&mut index_buf);
        for chunk in &chunks {
            chunk
                .index
                .write_to(&mut cursor)
                .map_err(|e| ExportError::ExportFailed {
                    reason: format!("Failed to write chunk index: {}", e),
                })?;
        }
        buffer.extend_from_slice(&index_buf);
    }

    file.write_all(&buffer)
        .await
        .map_err(|_e| ExportError::FileWriteError {
            path: path.to_string_lossy().to_string(),
        })?;

    // Flush AND fsync before reporting success. `tokio::fs::File` buffers,
    // and dropping it does not guarantee the bytes reach the disk — the
    // drop-time flush is best-effort and its result is discarded. Without
    // this, `export_brep_to_ros` returns `Ok` on a file that may still be
    // partially written, so a caller that immediately reads it back (an
    // agent verifying its own export, a user copying the artifact) can get
    // a truncated file while having been told the export succeeded.
    //
    // Observed, not theorised: `ros_sign_chunk::tampered_hist_byte_flips_
    // verdict_to_invalid` failed once under parallel load because the
    // marker it plants inside HIST was absent from the file it read back
    // immediately after export — the same binary passing 7 subsequent runs.
    //
    // `sync_all` rather than `flush` alone: this file is the durable
    // artifact an IP claim rests on, and a signature over bytes that never
    // reached the platter is not evidence of anything. One fsync per export
    // is a trivial cost against silently truncating the thing we just told
    // the caller we wrote.
    file.flush()
        .await
        .map_err(|_e| ExportError::FileWriteError {
            path: path.to_string_lossy().to_string(),
        })?;
    file.sync_all()
        .await
        .map_err(|_e| ExportError::FileWriteError {
            path: path.to_string_lossy().to_string(),
        })?;

    Ok(RosWriteSummary {
        hist_event_count,
        hist_branch_count,
        prov_command_count,
        prov_session_id,
        signature,
        replay_status,
    })
}

/// Sign the content chunks and append the SIGN chunk.
///
/// This is the ONLY site that sets the header's signature claim, and it
/// does so immediately after the chunk is pushed, with no fallible call
/// in between — so no reachable state has the claim without the chunk.
///
/// What is signed: the SHA-256 Merkle root over the ON-DISK bytes of
/// every chunk already in `chunks` (META, HIST, PROV, GEOM), in
/// chunk-table order. For encrypted files these are the
/// POST-ENCRYPTION bytes as they land on disk — chosen deliberately so
/// a verifier can check the signature against the raw file WITHOUT the
/// password. The SIGN chunk itself is never encrypted for the same
/// reason.
fn append_sign_chunk(
    chunks: &mut Vec<Chunk>,
    header: &mut ros_format::FileHeader,
    key_bytes: &[u8; 32],
) -> Result<RosWriteSignature, ExportError> {
    let leaves: Vec<Vec<u8>> = chunks.iter().map(|c| c.data.clone()).collect();
    let tree = MerkleTree::from_leaves(leaves, HashAlgorithm::Sha256).map_err(ros_err)?;
    let root = tree
        .root_hash()
        .ok_or_else(|| ExportError::ExportFailed {
            reason: "cannot sign a file with no content chunks (empty Merkle tree)".to_string(),
        })?
        .to_vec();

    // The signer id is derived from the public key (first 16 bytes of
    // SHA-256 of the verifying key) so the same key always names the
    // same signer, with no separate identity registry to drift.
    let probe = FileSigner::from_bytes(key_bytes, [0u8; 16]).map_err(ros_err)?;
    let public_key = probe.verifying_key_bytes();
    let mut signer_id = [0u8; 16];
    signer_id.copy_from_slice(&sha256(&public_key)[..16]);
    let signer = FileSigner::from_bytes(key_bytes, signer_id).map_err(ros_err)?;

    let record = signer.sign_file(&root, header.file_uuid).map_err(ros_err)?;
    let sign_bytes = SignatureChunk::new(record).serialize();
    if sign_bytes.is_empty() {
        return Err(ExportError::ExportFailed {
            reason: "SIGN chunk serialization produced no bytes".to_string(),
        });
    }

    // Chunk first, claim second — adjacent, infallible, same function.
    chunks.push(Chunk::new(ChunkType::SIGN, sign_bytes));
    header.signature_algo = SignatureAlgorithm::Ed25519 as u8;
    header.feature_flags = header.feature_flags.with_signature();

    Ok(RosWriteSignature::Signed {
        signer_id: to_hex(&signer_id),
        public_key: to_hex(&public_key),
    })
}

/// Encrypt a chunk in place when a key set is supplied.
fn encrypt_if_enabled(
    mut chunk: Chunk,
    key_set: Option<&ros_format::KeySet>,
    file_iv: [u8; 8],
    chunk_index: usize,
) -> Result<Chunk, ExportError> {
    if let Some(keys) = key_set {
        let encryptor = ros_format::ChunkEncryptor::new(
            ros_format::EncryptionAlgorithm::AES256GCM,
            keys.clone(),
            file_iv,
        );
        let encrypted = encryptor
            .encrypt_chunk(
                &chunk.index.chunk_type,
                &chunk.data,
                chunk_index as u32,
                None,
            )
            .map_err(|e| ExportError::ExportFailed {
                reason: format!("Encryption failed: {}", e),
            })?;
        chunk.data = encrypted;
        chunk.index.encrypted = true;
        chunk.index.enc_algo = 1;
    }
    chunk.index.uncompressed_size = chunk.data.len() as u64;
    chunk.update_crc();
    Ok(chunk)
}

/// Read a .ros v3.1 file into the structured `RosImport`.
pub async fn import_ros(
    path: &Path,
    password: Option<&str>,
) -> Result<RosImport, shared_types::ExportError> {
    let mut file = File::open(path)
        .await
        .map_err(|_e| ExportError::ExportFailed {
            reason: format!("Failed to read file: {}", path.to_string_lossy()),
        })?;

    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .await
        .map_err(|_e| ExportError::ExportFailed {
            reason: format!("Failed to read file: {}", path.to_string_lossy()),
        })?;

    let mut cursor = Cursor::new(buffer);
    let header =
        ros_format::FileHeader::read_from(&mut cursor).map_err(|e| ExportError::ExportFailed {
            reason: format!("Failed to read header: {}", e),
        })?;

    let key_set = if header.feature_flags.encrypted() {
        let password = password.ok_or_else(|| ExportError::ExportFailed {
            reason: "Password required for encrypted file".to_string(),
        })?;
        // Reproduce the derivation recorded in the header. The t_cost is
        // clamped against a hostile/corrupt header so import fails cleanly
        // (key mismatch) rather than hanging on an absurd pass count.
        let key_manager = SoftwareKeyManager::with_clamped_time_cost(header.kdf_iterations);
        let key_set = key_manager
            .generate_key_set(password, &header.kdf_salt)
            .map_err(|e| ExportError::ExportFailed {
                reason: format!("Key derivation failed: {}", e),
            })?;
        Some(key_set)
    } else {
        None
    };

    let chunk_table = ros_format::chunk::ChunkTable::read_from(
        &mut cursor,
        header.index_offset,
        header.index_entry_count,
    )
    .map_err(|e| ExportError::ExportFailed {
        reason: format!("Failed to read chunk index: {}", e),
    })?;
    chunk_table
        .validate()
        .map_err(|e| ExportError::ExportFailed {
            reason: format!("Chunk table failed v3.1 validation: {}", e),
        })?;

    // Signature verdict — computed against the RAW on-disk bytes before
    // any chunk is decrypted or parsed, so a tampered file still gets
    // an honest verdict even if its payloads also fail to parse, and an
    // encrypted file verifies without the password.
    let signature = signature_verdict(&cursor, &header, &chunk_table)?;

    let hist_chunk = read_chunk_payload::<HistChunk>(
        &mut cursor,
        &chunk_table,
        ChunkType::HIST,
        key_set.as_ref(),
        header.file_iv,
    )?;

    let prov_chunk = read_chunk_payload::<ProvChunk>(
        &mut cursor,
        &chunk_table,
        ChunkType::PROV,
        key_set.as_ref(),
        header.file_iv,
    )?;

    let snapshot = if chunk_table.find_by_type(ChunkType::GEOM).is_some() {
        Some(read_chunk_payload::<BRepSnapshot>(
            &mut cursor,
            &chunk_table,
            ChunkType::GEOM,
            key_set.as_ref(),
            header.file_iv,
        )?)
    } else {
        None
    };

    let replay_status = read_meta_replay_status(&cursor, &chunk_table)?;

    Ok(RosImport {
        timeline: hist_chunk.events,
        branches: hist_chunk.branches,
        aipr: prov_chunk,
        snapshot,
        signature,
        replay_status,
    })
}

/// Read the file's `replay_status` claim out of META.
///
/// META is a mandatory, always-plaintext JSON chunk (the writer never
/// encrypts it), so this needs no key material. A file written before
/// the field existed made no replay claim; that absence reads back as
/// [`RosReplayStatus::NotAttempted`] — never a pass. A META that is not
/// valid JSON, or a `replay_status` that does not parse, is a typed
/// refusal: a mandatory chunk that cannot be read is a broken file, not
/// a warning.
fn read_meta_replay_status(
    cursor: &Cursor<Vec<u8>>,
    table: &ros_format::chunk::ChunkTable,
) -> Result<RosReplayStatus, ExportError> {
    let entry = table
        .find_by_type(ChunkType::META)
        .ok_or_else(|| ExportError::ExportFailed {
            reason: "Missing chunk: META".to_string(),
        })?;
    let meta_bytes = raw_chunk_bytes(cursor.get_ref(), entry)?;
    let meta: serde_json::Value =
        serde_json::from_slice(meta_bytes).map_err(|e| ExportError::ExportFailed {
            reason: format!("META chunk is not valid JSON: {}", e),
        })?;
    match meta.get("replay_status") {
        None => Ok(RosReplayStatus::NotAttempted),
        Some(value) => {
            serde_json::from_value(value.clone()).map_err(|e| ExportError::ExportFailed {
                reason: format!("META.replay_status failed to parse: {}", e),
            })
        }
    }
}

/// Compute the signature verdict for a file whose header and chunk
/// table have been read.
///
/// - Header claims nothing, no SIGN chunk → `Unsigned`.
/// - Header claims a signature, no SIGN chunk → HARD typed error. This
///   exact state is what every file written by the pre-fix code looks
///   like (flag set, nothing signed): a forged provenance claim, never
///   a warning, never a silent pass.
/// - SIGN chunk present (claimed or not) → recompute the Merkle root
///   over the on-disk bytes of every non-SIGN chunk in chunk-table
///   order and verify the Ed25519 signature against it.
fn signature_verdict(
    cursor: &Cursor<Vec<u8>>,
    header: &ros_format::FileHeader,
    table: &ros_format::chunk::ChunkTable,
) -> Result<RosSignatureVerdict, ExportError> {
    let sign_entry = table.find_by_type(ChunkType::SIGN);
    let header_claims = header.feature_flags.has_signature() || header.signature_algo != 0;

    let entry = match (header_claims, sign_entry) {
        (false, None) => return Ok(RosSignatureVerdict::Unsigned),
        (true, None) => {
            return Err(ExportError::ExportFailed {
                reason: "header claims an Ed25519 signature but the file \
                         contains no SIGN chunk — the signature claim is \
                         forged (or the file was written by a broken signer); \
                         refusing to import"
                    .to_string(),
            })
        }
        // A carried signature is verified whether or not the header
        // remembered to claim it — the chunk is the substantive fact.
        (_, Some(entry)) => entry,
    };

    let file = cursor.get_ref();
    let sign_bytes = raw_chunk_bytes(file, entry)?;
    let sig_chunk = match SignatureChunk::deserialize(sign_bytes) {
        Ok(c) => c,
        Err(e) => {
            return Ok(RosSignatureVerdict::Invalid {
                reason: format!("SIGN chunk failed to deserialize: {}", e),
            })
        }
    };

    // Recompute the Merkle root over exactly the bytes the writer
    // signed: on-disk (post-encryption) chunk bytes, chunk-table order,
    // SIGN excluded.
    let mut leaves: Vec<Vec<u8>> = Vec::new();
    for e in table.iter() {
        if e.chunk_type == ChunkType::SIGN.as_fourcc() {
            continue;
        }
        leaves.push(raw_chunk_bytes(file, e)?.to_vec());
    }
    let tree = MerkleTree::from_leaves(leaves, HashAlgorithm::Sha256).map_err(ros_err)?;
    let root = match tree.root_hash() {
        Some(r) => r.to_vec(),
        None => {
            return Ok(RosSignatureVerdict::Invalid {
                reason: "signed file has no content chunks to verify against".to_string(),
            })
        }
    };

    match SignatureVerifier::verify_chunk(&root, &sig_chunk) {
        Ok(true) => Ok(RosSignatureVerdict::Verified {
            signer_id: to_hex(&sig_chunk.signer.metadata.signer_id),
            public_key: to_hex(&sig_chunk.signer.public_key),
        }),
        Ok(false) => Ok(RosSignatureVerdict::Invalid {
            reason: "Ed25519 signature does not match the Merkle root of the \
                     file's chunk bytes — the file was modified after signing, \
                     or the signature was transplanted from another file"
                .to_string(),
        }),
        Err(e) => Ok(RosSignatureVerdict::Invalid {
            reason: format!("signature verification errored: {}", e),
        }),
    }
}

/// Slice a chunk's raw on-disk bytes out of the whole-file buffer.
fn raw_chunk_bytes<'a>(
    file: &'a [u8],
    entry: &ros_format::chunk::ChunkIndexEntry,
) -> Result<&'a [u8], ExportError> {
    let start = entry.offset as usize;
    let len = entry.size_on_disk() as usize;
    let out_of_bounds = || ExportError::ExportFailed {
        reason: format!(
            "{} chunk at offset {} (+{} bytes) lies outside the {}-byte file",
            ChunkType::from_fourcc(entry.chunk_type).as_str(),
            entry.offset,
            len,
            file.len(),
        ),
    };
    let end = start.checked_add(len).ok_or_else(out_of_bounds)?;
    file.get(start..end).ok_or_else(out_of_bounds)
}

/// Convenience wrapper: import and materialise a `BRepModel`.
///
/// Uses the GEOM cache when present; otherwise rebuilds from HIST
/// events via `timeline_engine::rebuild_model_from_events`.
pub async fn import_ros_to_brep(
    path: &Path,
    password: Option<&str>,
) -> Result<BRepModel, shared_types::ExportError> {
    import_ros(path, password).await?.into_model()
}

/// Read + decrypt + deserialize a single chunk payload.
fn read_chunk_payload<T: serde::de::DeserializeOwned>(
    cursor: &mut Cursor<Vec<u8>>,
    table: &ros_format::chunk::ChunkTable,
    chunk_type: ChunkType,
    key_set: Option<&ros_format::KeySet>,
    file_iv: [u8; 8],
) -> Result<T, ExportError> {
    let entry = table
        .find_by_type(chunk_type)
        .ok_or_else(|| ExportError::ExportFailed {
            reason: format!("Missing chunk: {}", chunk_type.as_str()),
        })?;

    std::io::Seek::seek(cursor, std::io::SeekFrom::Start(entry.offset)).map_err(|e| {
        ExportError::ExportFailed {
            reason: format!("Failed to seek to {} chunk: {}", chunk_type.as_str(), e),
        }
    })?;
    let mut data = vec![0u8; entry.uncompressed_size as usize];
    std::io::Read::read_exact(cursor, &mut data).map_err(|e| ExportError::ExportFailed {
        reason: format!("Failed to read {} chunk: {}", chunk_type.as_str(), e),
    })?;

    if entry.encrypted {
        let keys = key_set.ok_or_else(|| ExportError::ExportFailed {
            reason: format!(
                "{} chunk is encrypted but no key was supplied",
                chunk_type.as_str()
            ),
        })?;
        let algo = ros_format::EncryptionAlgorithm::from_id(entry.enc_algo).map_err(|_| {
            ExportError::ExportFailed {
                reason: format!(
                    "Unknown encryption algorithm id {} on {} chunk",
                    entry.enc_algo,
                    chunk_type.as_str()
                ),
            }
        })?;
        let chunk_index = table
            .iter()
            .position(|e| e.chunk_type == entry.chunk_type)
            .unwrap_or(0) as u32;
        let decryptor = ros_format::ChunkEncryptor::new(algo, keys.clone(), file_iv);
        data = decryptor
            .decrypt_chunk(&entry.chunk_type, &data, chunk_index, None)
            .map_err(|e| ExportError::ExportFailed {
                reason: format!("Decryption of {} failed: {}", chunk_type.as_str(), e),
            })?;
    }

    if chunk_type == ChunkType::META {
        // META is JSON, but this helper is only used for MessagePack
        // chunks; bail loudly if we ever get pointed at META.
        return Err(ExportError::ExportFailed {
            reason: "META is JSON, not MessagePack".to_string(),
        });
    }

    rmp_serde::from_slice(&data).map_err(|e| ExportError::ExportFailed {
        reason: format!("Failed to deserialize {} chunk: {}", chunk_type.as_str(), e),
    })
}
