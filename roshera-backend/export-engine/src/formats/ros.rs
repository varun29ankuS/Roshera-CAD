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
//! ## Signing (.ros v3.2 integrity scheme)
//!
//! When `RosExportOptions::sign` is true the writer computes a SHA-256
//! Merkle root over the leaf set defined by [`signed_leaf_set`] — the
//! normalized 128-byte header image, every non-SIGN chunk's on-disk
//! PAYLOAD bytes, and every non-SIGN chunk's 96-byte INDEX ENTRY — signs
//! that root with the caller-supplied Ed25519 key, and stores the
//! signature in a SIGN chunk. On import the root is recomputed from the
//! raw file bytes and the signature is verified.
//!
//! v3.1 signed the chunk payloads alone. That left the header (version,
//! `signature_algo`, `feature_flags`, `ai_tracking`, `kdf_*`,
//! `file_uuid`) and the entire chunk index outside the signature, so a
//! header could be rewritten — or two chunks' FourCC labels swapped —
//! with the root unchanged and the file still reading `Verified`. v3.2
//! closes both; see [`ros_format::CURRENT_MINOR_VERSION`] for the
//! version-bump rationale and what happens to a v3.1-signed file.
//!
//! The verdict is [`RosSignatureVerdict`], never a bool: "unsigned",
//! "signature failed", "identity claim forged" and "signed under the
//! superseded scheme" are four different facts and must not collapse
//! into each other. A header that claims a signature over a file with no
//! SIGN chunk is a hard error — that is a forged claim, not a warning.
//!
//! ### What SIGN cannot cover
//!
//! SIGN is excluded from its own leaves (a signature cannot cover
//! itself), so the SIGN record's own metadata is unauthenticated. The
//! one field there that carries meaning is `signer_id`, and the reader
//! re-derives it as `sha256(public_key)[..16]` and refuses on mismatch —
//! see [`RosSignatureVerdict::ForgedSignerId`].

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
/// Deliberately an enum, never a bool, and deliberately more than three
/// states: "carries no signature", "carries a signature that does not
/// verify", "carries a genuine signature under a forged identity" and
/// "carries a signature made under a superseded scheme" are four
/// different facts. Collapsing any pair of them into a shared `reason`
/// string would leave a consumer — the primary one being an agent
/// reading JSON — unable to tell them apart mechanically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RosSignatureVerdict {
    /// The file neither claims nor carries a signature.
    Unsigned,
    /// The SIGN chunk's Ed25519 signature verifies against the Merkle
    /// root of the file's signed leaf set, AND the record's `signer_id`
    /// re-derives from its own public key.
    ///
    /// Both fields are now authenticated facts: the holder of
    /// `public_key` signed exactly these bytes, and `signer_id` is
    /// `sha256(public_key)[..16]`, re-derived by the reader rather than
    /// taken on trust from the record.
    Verified {
        signer_id: String,
        public_key: String,
    },
    /// A signature is present but does not verify — the file was
    /// modified after signing, the signature was transplanted, or the
    /// SIGN chunk is malformed.
    Invalid { reason: String },
    /// The Ed25519 signature VERIFIES over the file's bytes, but the
    /// SIGN record's `signer_id` is not `sha256(public_key)[..16]`.
    ///
    /// This is its own state rather than an [`Invalid`] because the two
    /// say opposite things about the cryptography: `Invalid` means the
    /// bytes do not match the signature, whereas this means the bytes DO
    /// match and only the identity label attached to them is false. On
    /// an IP-attribution artifact that is the field an adversary
    /// targets, so it must be nameable, not buried in prose.
    ///
    /// SIGN is excluded from its own Merkle leaves, so this field is the
    /// one an attacker can rewrite without disturbing the root. The
    /// writer derives it from the public key unconditionally
    /// (`append_sign_chunk`), so a mismatch has no legitimate producer:
    /// there is no compatibility case to preserve, only tampering (or a
    /// broken third-party signer, which is equally not to be trusted).
    ///
    /// [`Invalid`]: RosSignatureVerdict::Invalid
    ForgedSignerId {
        /// The id the record claims (hex).
        declared_signer_id: String,
        /// The id its own public key actually derives to (hex).
        derived_signer_id: String,
        /// The key that genuinely signed the bytes (hex).
        public_key: String,
    },
    /// The file carries a signature made under the .ros ≤3.1 scheme,
    /// which covered the chunk payloads ALONE — not the header, not the
    /// chunk index.
    ///
    /// Reported as its own state rather than as [`Invalid`] because
    /// `Invalid` would be a lie in both directions: the signature is not
    /// broken, and re-checking it under the v3.1 rule would certify
    /// coverage this reader knows to be insufficient (a v3.1 `Verified`
    /// tolerated a rewritten header and swapped FourCC labels). The
    /// honest statement is that the file's signature is not re-checkable
    /// here — re-sign the artifact with a v3.2 writer.
    ///
    /// [`Invalid`]: RosSignatureVerdict::Invalid
    SupersededScheme {
        /// The file's format version, e.g. `"3.1.0"`.
        file_version: String,
        reason: String,
    },
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

    // Header -----------------------------------------------------------
    // Built BEFORE the key set, and that order is load-bearing:
    // `FileHeader::new` is what mints `file_uuid`, and the file key is
    // derived from exactly that field. Deriving keys first and minting
    // the uuid afterwards is what produced the defect this ordering
    // removes — one value with two independent sources, only one of
    // which ever reached the disk.
    //
    // PROV is always present in v3.1, so the AI-provenance flag is
    // unconditionally set. The tracking level is taken from options.
    //
    // NOTE: the header's signature claim is NOT set here. It is set by
    // `append_sign_chunk` and re-checked against the chunk table
    // immediately before the file is written.
    let mut header = ros_format::FileHeader::builder()
        .with_ai_tracking(options.tracking_level as u8)
        .build();

    // Encryption setup -------------------------------------------------
    // Every derived key must be reproducible on import from bytes the
    // file carries: the password, `kdf_salt`, `kdf_iterations`,
    // `file_uuid` and the chunk's own FourCC. `file_uuid` is the KDF
    // file id for that reason — it is already written, and on a signed
    // file it is already inside the signed Merkle leaf set (header bytes
    // 32..48, outside every zeroed range), so binding to it adds no wire
    // bytes and costs no authentication.
    let encrypt = options.password.is_some();
    let key_set = if encrypt {
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
        let key_set = key_manager
            .generate_key_set(password, &salt, &header.file_uuid)
            .map_err(|e| ExportError::ExportFailed {
                reason: format!("Key generation failed: {}", e),
            })?;

        // AES-256-GCM, and KDF id 3 = Argon2id whose file key is bound
        // to `file_uuid`. Id 2 is the superseded, unreadable chain (a
        // random per-export file id that was never persisted); the
        // importer refuses it by name rather than letting it fail as a
        // generic auth error. `kdf_iterations` is read off the key
        // manager itself, not a constant repeated here, so the number in
        // the header is the number that was actually used.
        header.encryption_algo = ros_format::EncryptionAlgorithm::AES256GCM.as_id();
        header.kdf_algo = ros_format::keys::KDF_ALGO_ARGON2ID_FILE_BOUND;
        header.kdf_iterations = key_manager.kdf_iterations;
        header.kdf_salt = salt;
        header.file_iv = file_iv;
        header.feature_flags = header.feature_flags.with_encryption();

        Some(key_set)
    } else {
        None
    };
    let file_iv = header.file_iv;

    // Endianness is PINNED little, not taken from the host.
    // `FileHeader` honours the endianness byte for every multi-byte
    // header field, but `ChunkIndexEntry::read_from`/`write_to` are
    // hard-coded LittleEndian — so a big-endian host would emit a
    // big-endian header over a little-endian chunk table and nothing in
    // the format would notice. Rather than steer around that, the .ros
    // writer makes it inexpressible: every .ros file is little-endian,
    // and `import_ros` refuses a big-endian header by name. It also
    // makes exports byte-reproducible across hosts, which matters for a
    // format whose signature is over its own bytes.
    header.endianness = ros_format::Endianness::Little;

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

    // Layout, pass 1 — content chunks only -----------------------------
    // Their offsets, sizes and CRCs are final at this point and none of
    // them depends on the SIGN chunk, so their index entries can be
    // signed. SIGN is appended LAST for exactly this reason: if it sat
    // anywhere else, every following chunk's offset would depend on the
    // signature's own encoded length and the leaf set would be circular.
    let mut current_offset: u64 = 128;
    for chunk in &mut chunks {
        chunk.index.offset = current_offset;
        current_offset += chunk.data.len() as u64;
    }

    // AI-provenance hints ----------------------------------------------
    // Populated with the truth rather than left at zero. Until v3.2 the
    // header said `ai_command_count = 0` and `ai_chunk_offset = 0` on
    // every file, including files whose PROV chunk carried commands — so
    // a reader trusting the header concluded the file had no provenance
    // at all. These are now filled in AND covered by the signature, so
    // the header's summary of PROV is an authenticated claim; the reader
    // additionally cross-checks both against PROV itself, which turns a
    // field that used to lie by default into a redundancy check.
    let prov_offset = chunks
        .iter()
        .find(|c| c.index.chunk_type == ChunkType::PROV.as_fourcc())
        .map(|c| c.index.offset)
        .ok_or_else(|| ExportError::ExportFailed {
            reason: "PROV is mandatory in .ros v3.1+ but no PROV chunk was laid out".to_string(),
        })?;
    header.ai_command_count = prov_command_count as u64;
    header.ai_chunk_offset = prov_offset;

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

    // Layout, pass 2 — assigns SIGN its offset -------------------------
    // Idempotent for the content chunks: same start (128), same lengths,
    // same order, so every offset the signature already covers is
    // recomputed to the identical value.
    let mut current_offset: u64 = 128;
    for chunk in &mut chunks {
        chunk.index.offset = current_offset;
        current_offset += chunk.data.len() as u64;
    }
    header.index_offset = current_offset;
    header.index_entry_count = chunks.len() as u32;
    header.file_size = current_offset + (chunks.len() * CHUNK_INDEX_ENTRY_SIZE) as u64;

    // The two write-site invariants, enforced at the ONLY write site.
    // Both are named predicates rather than inline `if`s so the failing
    // branch is REACHABLE from a test — see the note on
    // [`check_signature_claim_matches_table`].
    let carries_sign_chunk = chunks
        .iter()
        .any(|c| c.index.chunk_type == ChunkType::SIGN.as_fourcc());
    check_signature_claim_matches_table(header.feature_flags.has_signature(), carries_sign_chunk)?;
    if let Some(keys) = key_set.as_ref() {
        check_key_binding(&keys.file_id, &header.file_uuid)?;
    }

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

/// Byte ranges of the 128-byte header image that are ZEROED in the
/// signed header leaf, because their values are not knowable when the
/// signature is computed:
///
/// - `12..16` `header_crc32` — a checksum over the header itself,
///   including over the two ranges below, so it changes when they do;
/// - `16..24` `file_size` — depends on the SIGN chunk's encoded length;
/// - `48..56` `index_offset` — likewise.
///
/// None of the three is thereby left unprotected:
///
/// - `file_size` is checked against the file's ACTUAL length on import
///   — an exact equality, strictly stronger than a signature over a
///   self-declared number;
/// - `header_crc32` is recomputed and verified on every read (v3.2
///   widens it to the whole header, so it catches corruption in exactly
///   the fields the signature covers);
/// - `index_offset` can only be moved somewhere useful to an attacker if
///   the entries it then points at hash into the signed root — and every
///   non-SIGN entry IS a leaf, so relocating the index to fabricated
///   entries breaks verification.
const HEADER_LEAF_ZEROED_RANGES: [std::ops::Range<usize>; 3] = [12..16, 16..24, 48..56];

/// The 128-byte header image with [`HEADER_LEAF_ZEROED_RANGES`] blanked.
///
/// Called by the writer on the image it is about to store and by the
/// reader on the image it just read, so the two cannot drift.
fn normalized_header_leaf(header_image: &[u8]) -> Result<Vec<u8>, ExportError> {
    let mut leaf = header_image
        .get(..ros_format::HEADER_SIZE)
        .ok_or_else(|| ExportError::ExportFailed {
            reason: format!(
                "header image is {} bytes, shorter than the mandatory {}",
                header_image.len(),
                ros_format::HEADER_SIZE
            ),
        })?
        .to_vec();
    for range in HEADER_LEAF_ZEROED_RANGES {
        // In bounds by construction: every range above lies inside
        // 0..128 and `leaf` is exactly 128 bytes. `get_mut` rather than
        // indexing so a future edit to the range table degrades to a
        // no-op instead of a panic — the workspace denies panics.
        if let Some(slice) = leaf.get_mut(range) {
            slice.fill(0);
        }
    }
    Ok(leaf)
}

/// THE definition of the .ros v3.2 signed leaf set.
///
/// Order is part of the format and is fixed:
///
/// 1. the normalized header image — exactly one leaf;
/// 2. every non-SIGN chunk's on-disk PAYLOAD bytes, in chunk-table order;
/// 3. every non-SIGN chunk's 96-byte INDEX ENTRY, in chunk-table order.
///
/// Groups 2 and 3 are appended as blocks rather than interleaved so the
/// leaf count is `1 + 2n` for `n` signed chunks — a shape a verifier can
/// check before hashing anything.
///
/// Including group 3 is what makes chunk-table metadata tamper-evident:
/// under v3.1 the table was outside the leaves, so two chunks' FourCC
/// labels could be swapped, or a declared CRC rewritten, with the root
/// unchanged.
///
/// For encrypted files the payloads are the POST-ENCRYPTION bytes as
/// they land on disk — chosen deliberately so a verifier can check the
/// signature against the raw file WITHOUT the password. The SIGN chunk
/// itself is never encrypted for the same reason.
///
/// Both the writer and the reader call this with their own
/// materialisation of the same three inputs; the independent oracle in
/// `export-engine/tests/ros_independent_oracle.rs` re-derives it a third
/// time from raw file offsets.
fn signed_leaf_set(
    header_leaf: Vec<u8>,
    payloads: Vec<Vec<u8>>,
    index_entries: Vec<Vec<u8>>,
) -> Vec<Vec<u8>> {
    let mut leaves = Vec::with_capacity(1 + payloads.len() + index_entries.len());
    leaves.push(header_leaf);
    leaves.extend(payloads);
    leaves.extend(index_entries);
    leaves
}

/// Sign the header + content chunks and append the SIGN chunk.
///
/// This is the ONLY site that sets the header's signature claim. Under
/// v3.1 the claim was set immediately AFTER the chunk was pushed, so no
/// reachable state had the claim without the chunk. v3.2 must invert
/// that — the claim (`signature_algo`, the feature flag, and
/// `index_entry_count`) is part of what gets signed, so it has to exist
/// before the header image is taken. The invariant it protected is not
/// lost: this function writes no bytes, `export_brep_to_ros` writes
/// nothing until every step here has succeeded, and the write site
/// re-checks claim ⇔ SIGN chunk before emitting a single byte. Any error
/// below aborts the export with no `.ros` content on disk.
///
/// `chunks` must contain the content chunks ONLY, already laid out
/// (offset, size and CRC final), with SIGN appended by this function.
fn append_sign_chunk(
    chunks: &mut Vec<Chunk>,
    header: &mut ros_format::FileHeader,
    key_bytes: &[u8; 32],
) -> Result<RosWriteSignature, ExportError> {
    // The claim, before the header image that carries it is taken.
    header.signature_algo = SignatureAlgorithm::Ed25519 as u8;
    header.feature_flags = header.feature_flags.with_signature();
    // +1 for the SIGN chunk pushed below: the count is signed, so it
    // must already describe the finished table.
    header.index_entry_count = (chunks.len() + 1) as u32;

    let header_image = header.to_bytes().map_err(ros_err)?;
    let header_leaf = normalized_header_leaf(&header_image)?;

    let mut payloads: Vec<Vec<u8>> = Vec::with_capacity(chunks.len());
    let mut index_entries: Vec<Vec<u8>> = Vec::with_capacity(chunks.len());
    for chunk in chunks.iter() {
        payloads.push(chunk.data.clone());
        index_entries.push(chunk.index.to_bytes().map_err(ros_err)?);
    }
    let leaves = signed_leaf_set(header_leaf, payloads, index_entries);

    let tree = MerkleTree::from_leaves(leaves, HashAlgorithm::Sha256).map_err(ros_err)?;
    let root = tree
        .root_hash()
        .ok_or_else(|| ExportError::ExportFailed {
            reason: "cannot sign a file with no content chunks (empty Merkle tree)".to_string(),
        })?
        .to_vec();

    // The signer id is derived from the public key (first 16 bytes of
    // SHA-256 of the verifying key) so the same key always names the
    // same signer, with no separate identity registry to drift. The
    // reader re-derives it and refuses on mismatch, which is only sound
    // because this derivation is unconditional here.
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

    chunks.push(Chunk::new(ChunkType::SIGN, sign_bytes));

    Ok(RosWriteSignature::Signed {
        signer_id: to_hex(&signer_id),
        public_key: to_hex(&public_key),
    })
}

/// The claim ⇔ chunk invariant: a file must never assert a signature it
/// does not carry, nor carry one it does not declare.
///
/// v3.1 held it by adjacency (`append_sign_chunk` pushed the chunk and set
/// the claim in two neighbouring infallible statements). v3.2 cannot: the
/// claim is part of what gets signed, so it must be set BEFORE the
/// signature exists. The invariant is therefore enforced at the write
/// site, which is strictly stronger — it holds for every path into the
/// writer, not just for one function's statement order, and no byte
/// reaches the disk until it passes.
///
/// # Why this is a function and not an inline `if`
///
/// At the call site the failing branch is unreachable: `append_sign_chunk`
/// is the only producer of both the claim and the chunk, and it sets them
/// together. A guard whose failing branch can never be entered is a guard
/// nobody has ever seen work — the "built and disconnected" shape this
/// codebase has produced repeatedly. Lifting the condition into a pure
/// predicate makes the refusal REACHABLE from a test without a fault
/// injection hook, a feature flag, or a weakened writer: the unit tests at
/// the bottom of this file call it with both disagreeing combinations and
/// pin the refusal text.
///
/// The predicate is deliberately total over its two inputs — it says
/// nothing about how they were produced — so it remains correct if a
/// future writer path ever does set them independently.
fn check_signature_claim_matches_table(
    header_claims_signature: bool,
    carries_sign_chunk: bool,
) -> Result<(), ExportError> {
    if header_claims_signature == carries_sign_chunk {
        return Ok(());
    }
    Err(ExportError::ExportFailed {
        reason: format!(
            "REFUSED to write a .ros file whose header signature claim ({}) \
             disagrees with its chunk table ({} SIGN chunk) — a file must \
             never assert a signature it does not carry, nor carry one it \
             does not declare",
            header_claims_signature,
            if carries_sign_chunk {
                "has a"
            } else {
                "has no"
            }
        ),
    })
}

/// The key-binding invariant: no byte reaches the disk encrypted under a
/// key the header cannot reproduce.
///
/// Sibling of [`check_signature_claim_matches_table`], and unreachable at
/// its call site for the same kind of reason: `export_brep_to_ros` builds
/// the header FIRST and hands `header.file_uuid` straight to
/// `generate_key_set`, so the two values have one source. The property
/// that matters is not "the code currently passes `file_uuid`", it is
/// "every chunk key descends from a file id the importer can re-derive
/// from the header". If the two ever diverge the file is unopenable by
/// anyone, including its author, and — worse — it would say nothing about
/// that.
///
/// As a function the refusal is reachable and pinned; inline it was a
/// branch no test could enter.
fn check_key_binding(
    key_set_file_id: &[u8; 16],
    header_file_uuid: &[u8; 16],
) -> Result<(), ExportError> {
    if key_set_file_id == header_file_uuid {
        return Ok(());
    }
    Err(ExportError::ExportFailed {
        reason: format!(
            "REFUSED to write an encrypted .ros file whose chunk keys were \
             derived from file id {} while its header carries file_uuid {} — \
             the importer re-derives from the header, so this file could \
             never be decrypted by any password",
            to_hex(key_set_file_id),
            to_hex(header_file_uuid)
        ),
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
    let buffer = read_file_bytes(path).await?;
    let RosAudited {
        mut cursor,
        header,
        chunk_table,
        signature,
    } = audit_ros_bytes(buffer)?;

    // Key derivation happens HERE — after the whole password-free audit,
    // not before it. The v3.2 signature covers the post-encryption
    // on-disk bytes precisely so integrity is checkable without the
    // password; deriving first (as this reader used to) demanded the
    // password to reach a verdict that never needed it. See
    // [`verify_ros_file`], which runs the identical audit and stops here.
    let key_set = derive_key_set(&header, password)?;

    // What the file's own signature lets a reader conclude about the
    // header fields the chunk keys descend from. Computed from the verdict
    // the password-free audit ALREADY produced, so an AEAD rejection below
    // can name its cause instead of presenting every failure as a wrong
    // password. See [`KdfInputAuthenticity`].
    let kdf_authenticity = KdfInputAuthenticity::of(&signature);

    let hist_chunk = read_chunk_payload::<HistChunk>(
        &mut cursor,
        &chunk_table,
        ChunkType::HIST,
        key_set.as_ref(),
        header.file_iv,
        &kdf_authenticity,
    )?;

    let prov_chunk = read_chunk_payload::<ProvChunk>(
        &mut cursor,
        &chunk_table,
        ChunkType::PROV,
        key_set.as_ref(),
        header.file_iv,
        &kdf_authenticity,
    )?;

    // AI-provenance hint cross-check (v3.2+ only: v3.0/v3.1 writers left
    // both fields at zero unconditionally, so a mismatch there carries no
    // information). From v3.2 the header's summary of PROV is populated
    // AND signed, so disagreement with PROV itself means one of the two
    // was rewritten. Refuse rather than pick a winner.
    if ros_format::uses_integrity_scheme_v2(header.major_version, header.minor_version) {
        if header.ai_command_count != prov_chunk.commands.len() as u64 {
            return Err(ExportError::ExportFailed {
                reason: format!(
                    "header claims {} AI provenance commands but the PROV chunk \
                     carries {} — the header and its own provenance chunk disagree",
                    header.ai_command_count,
                    prov_chunk.commands.len()
                ),
            });
        }
        let prov_offset = chunk_table
            .find_by_type(ChunkType::PROV)
            .map(|e| e.offset)
            .ok_or_else(|| ExportError::ExportFailed {
                reason: "Missing chunk: PROV".to_string(),
            })?;
        if header.ai_chunk_offset != prov_offset {
            return Err(ExportError::ExportFailed {
                reason: format!(
                    "header points at the PROV chunk at offset {} but the chunk \
                     table places PROV at {}",
                    header.ai_chunk_offset, prov_offset
                ),
            });
        }
    }

    let snapshot = if chunk_table.find_by_type(ChunkType::GEOM).is_some() {
        Some(read_chunk_payload::<BRepSnapshot>(
            &mut cursor,
            &chunk_table,
            ChunkType::GEOM,
            key_set.as_ref(),
            header.file_iv,
            &kdf_authenticity,
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

/// One chunk of a .ros file as the chunk TABLE describes it — no
/// payload, decrypted or otherwise. The inventory a password-free
/// verifier can honestly report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosChunkSummary {
    /// FourCC as text, e.g. `"HIST"`.
    pub chunk_type: String,
    /// Byte offset of the payload within the file.
    pub offset: u64,
    /// Payload length on disk (post-encryption when encrypted).
    pub size_on_disk: u64,
    /// Whether the payload is encrypted.
    pub encrypted: bool,
    /// Encryption algorithm id (0 when not encrypted).
    pub enc_algo: u8,
    /// The declared CRC-32, already verified against the bytes on disk
    /// by the time this is reported.
    pub crc32: u32,
}

/// Whether a file's chunk keys can be re-derived at all — and when they
/// cannot, which of two very different reasons applies.
///
/// An enum, never a bool, for the same reason [`RosSignatureVerdict`] is
/// one: "this reader does not implement that KDF chain" and "the key
/// material for this file was never written down and no longer exists
/// anywhere" are opposite statements. The first says try another reader;
/// the second says stop trying, and re-export from source. A shared
/// `false` would tell a custodian deciding whether to keep an archived
/// artifact exactly the wrong thing half the time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RosKeyRecoverability {
    /// The file is not encrypted; there is no key material to recover.
    NotEncrypted,
    /// Encrypted under the file-uuid-bound Argon2id chain. Every chunk
    /// key re-derives from the password plus bytes the file carries.
    DerivableFromPassword,
    /// Encrypted under the superseded chain (`kdf_algo` 2), whose file
    /// key was expanded from an id randomised at export time and never
    /// written into the file. The material exists nowhere — not in the
    /// file, not on the machine that wrote it. No password opens this
    /// file, and none ever could.
    NeverPersisted,
    /// Encrypted under a KDF chain this reader does not implement. The
    /// key material may well be perfectly recoverable — by something
    /// else. This reader will not guess.
    UnsupportedChain {
        /// The `kdf_algo` id the header declares.
        kdf_algo: u8,
    },
}

/// What a reader can establish about a .ros file with NO key material.
///
/// Produced by [`verify_ros_file`]. Every field here is derived from
/// header bytes and chunk-table metadata that are plaintext on every
/// file, encrypted or not — plus the signature verdict, which the v3.2
/// scheme deliberately computes over the POST-encryption on-disk bytes
/// so exactly this is possible.
///
/// It carries no chunk contents. On an encrypted file the payloads are
/// never decrypted and never parsed, so this says nothing about the
/// model, the timeline, the AI provenance, or the file's replay claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosFileVerification {
    /// Format version, e.g. `"3.2.0"`.
    pub version: String,
    /// The file's own UUID (hex). Also the KDF file id on an encrypted
    /// file written by this engine.
    pub file_uuid: String,
    /// Declared size, already checked to equal the bytes on disk.
    pub file_size: u64,
    /// Creation timestamp (Unix ms) as the header states it.
    pub creation_time: u64,
    /// Whether the file's chunk payloads are encrypted.
    pub encrypted: bool,
    /// Encryption algorithm id from the header (0 when not encrypted).
    pub encryption_algo: u8,
    /// KDF algorithm id from the header. `3` is the file-uuid-bound
    /// chain this engine writes; `2` marks a file whose key material was
    /// never persisted and which no password can open — see
    /// [`ros_format::keys::KdfAlgo`].
    pub kdf_algo: u8,
    /// Argon2 `t_cost` recorded for the KDF (0 when not encrypted).
    pub kdf_iterations: u32,
    /// Whether this file's chunk keys can be re-derived at all, and if
    /// not, why not — see [`RosKeyRecoverability`].
    pub key_recoverability: RosKeyRecoverability,
    /// The header's own count of AI provenance commands. Signed on v3.2+
    /// files, and cross-checked against PROV by [`import_ros`] — which
    /// this verifier cannot do, since that requires reading PROV.
    pub ai_command_count: u64,
    /// The signature verdict, computed over the raw on-disk bytes.
    pub signature: RosSignatureVerdict,
    /// Every chunk the table declares, in table order.
    pub chunks: Vec<RosChunkSummary>,
}

/// Verify a .ros file's integrity WITHOUT its password.
///
/// The v3.2 signature covers the post-encryption bytes as they land on
/// disk, and the SIGN chunk is never encrypted, specifically so a
/// custodian, an archive, or an agent triaging an artifact can establish
/// that a file is intact and who signed it without being able to read
/// it. `import_ros` demanded the password before it would compute a
/// verdict, which made that property real but unreachable; this is the
/// reachable form of it.
///
/// It runs the IDENTICAL audit `import_ros` runs — header CRC, declared
/// `file_size` versus the bytes on disk, endianness, chunk-table
/// validation, the compression refusal, the layout audit that forbids
/// dead space, the signature verdict, and every chunk's declared CRC-32
/// — by calling the same function. There is no softer variant for the
/// password-free path.
///
/// # What a caller learns, and what it does not
///
/// Learns: the format version, the file uuid, creation time, declared
/// size, whether the file is encrypted and under which algorithm and KDF
/// chain, whether that KDF chain is even recoverable, the AI command
/// count the header claims, the full signature verdict (including
/// `ForgedSignerId` and `SupersededScheme`), and the complete chunk
/// inventory — type, offset, on-disk size, encrypted flag, CRC.
///
/// Does NOT learn: any chunk's contents. No payload is decrypted and no
/// payload is deserialized, so the geometry, the timeline events, the AI
/// provenance log and the META replay claim are all out of reach. On an
/// unencrypted file those bytes are of course readable by anyone with
/// the file — this function simply does not read them.
pub async fn verify_ros_file(path: &Path) -> Result<RosFileVerification, ExportError> {
    let buffer = read_file_bytes(path).await?;
    let RosAudited {
        cursor,
        header,
        chunk_table,
        signature,
    } = audit_ros_bytes(buffer)?;
    // `cursor` owns the file bytes; nothing below reads a payload out of
    // it, and that is the whole contract of this function.
    drop(cursor);

    let chunks = chunk_table
        .iter()
        .map(|e| RosChunkSummary {
            chunk_type: ChunkType::from_fourcc(e.chunk_type).as_str(),
            offset: e.offset,
            size_on_disk: e.size_on_disk(),
            encrypted: e.encrypted,
            enc_algo: e.enc_algo,
            crc32: e.crc32,
        })
        .collect();

    Ok(RosFileVerification {
        version: header.version_string(),
        file_uuid: to_hex(&header.file_uuid),
        file_size: header.file_size,
        creation_time: header.creation_time,
        encrypted: header.feature_flags.encrypted(),
        encryption_algo: header.encryption_algo,
        kdf_algo: header.kdf_algo,
        kdf_iterations: header.kdf_iterations,
        key_recoverability: key_recoverability(&header),
        ai_command_count: header.ai_command_count,
        signature,
        chunks,
    })
}

/// What the file itself lets a reader conclude about the header fields
/// every chunk key descends from — `file_uuid`, `kdf_salt`,
/// `kdf_iterations`, `file_iv`, `encryption_algo`, `kdf_algo`.
///
/// # Why this exists
///
/// When AES-256-GCM rejects, the tag says only "these bytes were not
/// produced by this key". At least two very different events produce
/// that: the caller's password is wrong, or the file was altered so the
/// re-derived key is not the writer's. On the wire those are
/// indistinguishable — the derivation is a one-way function of both, and
/// nothing in an encrypted `.ros` file commits to the writer's key
/// material.
///
/// One thing does discriminate, and it is already computed before any key
/// is derived: the v3.2 signature. All six KDF-relevant header fields sit
/// inside the normalized header leaf (bytes 32..48, 64..96 — outside all
/// three of [`HEADER_LEAF_ZEROED_RANGES`]), so on a file whose signature
/// verifies they are authenticated bytes and a tag rejection is the
/// password; on a file whose signature is broken the header was rewritten
/// and the password may well be correct.
///
/// On an UNSIGNED encrypted file no such discriminator exists. `file_uuid`
/// is covered by `header_crc32` alone, and a CRC is not a security control
/// — anyone who can write the file can recompute it. That is a real,
/// unfixable-without-a-format-change gap, and the honest response is to
/// name it rather than to present one of the two causes as certain. See
/// the report on [`KdfInputAuthenticity::Unauthenticated`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum KdfInputAuthenticity {
    /// An Ed25519 signature over this file's bytes VERIFIES, and those
    /// bytes include the header leaf that carries every KDF input.
    ///
    /// Carries the public key because "verifies" alone is weaker than it
    /// looks: it proves the holder of *this* key signed *these* bytes, not
    /// that the author did. An attacker who rewrites `file_uuid` and
    /// re-signs with their own key also lands here — under a different
    /// public key. The refusal therefore states the key rather than
    /// asserting the file is untampered.
    Signed {
        public_key: String,
        /// The signature verifies but the record's `signer_id` does not
        /// re-derive from its own public key
        /// ([`RosSignatureVerdict::ForgedSignerId`]). The KDF inputs are
        /// still covered by a verifying signature; only the identity label
        /// beside it is false, and that must not be silently dropped.
        identity_forged: bool,
    },
    /// A signature is present and does NOT verify. The file was modified
    /// after signing, and the KDF inputs are among what the signature
    /// covers — so a correct password can fail here.
    SignatureBroken,
    /// Nothing in the file authenticates the KDF inputs.
    Unauthenticated {
        /// Why not, in the file's own terms.
        why: &'static str,
    },
}

impl KdfInputAuthenticity {
    /// Classify a file from the signature verdict `audit_ros_bytes`
    /// already computed — before any key is derived, and without the
    /// password.
    fn of(signature: &RosSignatureVerdict) -> Self {
        match signature {
            RosSignatureVerdict::Verified { public_key, .. } => KdfInputAuthenticity::Signed {
                public_key: public_key.clone(),
                identity_forged: false,
            },
            RosSignatureVerdict::ForgedSignerId { public_key, .. } => {
                KdfInputAuthenticity::Signed {
                    public_key: public_key.clone(),
                    identity_forged: true,
                }
            }
            RosSignatureVerdict::Invalid { .. } => KdfInputAuthenticity::SignatureBroken,
            RosSignatureVerdict::Unsigned => KdfInputAuthenticity::Unauthenticated {
                why: "this file carries no signature",
            },
            // A v≤3.1 signature's Merkle leaves were the chunk payloads
            // ALONE — the header was outside it. So a superseded-scheme
            // file discriminates exactly as poorly as an unsigned one, and
            // must not borrow the confidence of the signed case.
            RosSignatureVerdict::SupersededScheme { .. } => KdfInputAuthenticity::Unauthenticated {
                why: "this file's signature was made under the superseded .ros v≤3.1 \
                      scheme, whose Merkle leaves were the chunk payloads alone — the \
                      128-byte header was outside it",
            },
        }
    }

    /// The sentence appended to an AEAD rejection. States what the file
    /// can and cannot rule out; never asserts a cause it cannot establish.
    fn aead_failure_diagnosis(&self) -> String {
        match self {
            KdfInputAuthenticity::Signed {
                public_key,
                identity_forged,
            } => {
                let identity = if *identity_forged {
                    " (note: the SIGN record's signer_id does NOT re-derive from that \
                     key, so the identity label beside the signature is forged even \
                     though the signature itself verifies)"
                } else {
                    ""
                };
                format!(
                    "DIAGNOSIS: the supplied password is wrong. This file's signature \
                     verifies under public key {public_key}{identity}, and the .ros \
                     v3.2 signed header leaf covers every input the chunk keys are \
                     derived from — file_uuid, kdf_salt, kdf_iterations, file_iv, \
                     encryption_algo and kdf_algo — so none of them was altered after \
                     that key signed these bytes. If {public_key} is the key you \
                     expect, the key material is intact and only the password is \
                     wrong."
                )
            }
            KdfInputAuthenticity::SignatureBroken => {
                "DIAGNOSIS: this file was ALTERED after it was signed — its Ed25519 \
                 signature does not match the Merkle root of its own header, chunk \
                 bytes and chunk index. The signed header leaf covers every input the \
                 chunk keys are derived from (file_uuid, kdf_salt, kdf_iterations, \
                 file_iv, encryption_algo, kdf_algo), so this decryption failure is \
                 consistent with a CORRECT password over rewritten key material. Do \
                 not conclude the password is wrong; restore the file from an intact \
                 copy."
                    .to_string()
            }
            KdfInputAuthenticity::Unauthenticated { why } => format!(
                "DIAGNOSIS: UNDETERMINED — two different causes produce this exact \
                 failure and {why}, so this file cannot tell them apart. Either (1) \
                 the password does not match the one this file was written with, or \
                 (2) the file was altered: file_uuid, kdf_salt, file_iv or \
                 kdf_iterations was rewritten and the header CRC-32 recomputed, which \
                 permanently changes every derived chunk key. header_crc32 is not a \
                 security control — anyone who can write the file can recompute it — \
                 so where no v3.2 signature covers the header, the two are \
                 indistinguishable by construction. A file signed under the v3.2 \
                 scheme WOULD distinguish them: the signed header leaf covers all four \
                 fields. Sign encrypted artifacts you intend to archive."
            ),
        }
    }
}

/// Classify a header's KDF declaration. THE single definition, shared by
/// the reader's refusal and the password-free report so the two cannot
/// tell a caller different stories about the same file.
fn key_recoverability(header: &ros_format::FileHeader) -> RosKeyRecoverability {
    if !header.feature_flags.encrypted() {
        return RosKeyRecoverability::NotEncrypted;
    }
    match header.kdf_algo {
        a if a == ros_format::keys::KDF_ALGO_ARGON2ID_FILE_BOUND => {
            RosKeyRecoverability::DerivableFromPassword
        }
        a if a == ros_format::keys::KDF_ALGO_ARGON2ID_UNBOUND => {
            RosKeyRecoverability::NeverPersisted
        }
        kdf_algo => RosKeyRecoverability::UnsupportedChain { kdf_algo },
    }
}

/// Re-derive the file's chunk keys from the header and the password.
///
/// Every input is either the caller's (the password) or on disk
/// (`kdf_salt`, `kdf_iterations`, `file_uuid`), which is what makes the
/// writer's keys reproducible at all.
fn derive_key_set(
    header: &ros_format::FileHeader,
    password: Option<&str>,
) -> Result<Option<ros_format::KeySet>, ExportError> {
    if !header.feature_flags.encrypted() {
        return Ok(None);
    }

    // The KDF-chain gate, BEFORE the password is even looked at and
    // before ~100 ms of Argon2 is spent, because it is a fact about the
    // file rather than about the caller.
    //
    // Every encrypted .ros file written before 2026-08-04 derived its
    // file key — and therefore every chunk key — from a `file_id` that
    // `generate_key_set` randomised on each call and that no writer ever
    // stored. The material is not in the file and is not on the writer's
    // machine; it was never anywhere but one process's memory. Such a
    // file is not "wrong password", it is unopenable, and saying so is
    // the difference between an honest refusal and letting the user try
    // passwords forever against an AEAD tag that can never match.
    //
    // The classification comes from `key_recoverability`, the same
    // function [`verify_ros_file`] reports from, so a caller cannot be
    // told "unrecoverable" by one and "unsupported" by the other.
    let detail = match key_recoverability(header) {
        RosKeyRecoverability::NotEncrypted | RosKeyRecoverability::DerivableFromPassword => None,
        RosKeyRecoverability::NeverPersisted => Some(
            "Files written before 2026-08-04 derived every chunk key from a \
             KDF file id that was randomised at export time and never written \
             into the file. That key material exists nowhere — not in this \
             file, not on the machine that wrote it — so these chunks cannot \
             be decrypted by this password or any other. Re-export the model \
             from its source."
                .to_string(),
        ),
        RosKeyRecoverability::UnsupportedChain { kdf_algo } => Some(format!(
            "This reader implements KDF chain {} only (Argon2id with the file \
             key bound to the header's file_uuid). It will not guess at chain \
             {}.",
            ros_format::keys::KDF_ALGO_ARGON2ID_FILE_BOUND,
            kdf_algo
        )),
    };
    if let Some(detail) = detail {
        return Err(ExportError::ExportFailed {
            reason: format!(
                "REFUSED: encrypted .ros file declares KDF algorithm {}, which is \
                 not the file-bound chain this reader can reproduce. {}",
                header.kdf_algo, detail
            ),
        });
    }

    let password = password.ok_or_else(|| ExportError::ExportFailed {
        reason: "Password required for encrypted file".to_string(),
    })?;

    // Reproduce the derivation recorded in the header. The t_cost is
    // clamped against a hostile/corrupt header so import fails cleanly
    // (key mismatch) rather than hanging on an absurd pass count.
    let key_manager = SoftwareKeyManager::with_clamped_time_cost(header.kdf_iterations);
    let key_set = key_manager
        .generate_key_set(password, &header.kdf_salt, &header.file_uuid)
        .map_err(|e| ExportError::ExportFailed {
            reason: format!("Key derivation failed: {}", e),
        })?;
    Ok(Some(key_set))
}

/// Slurp a file into memory, as a typed error rather than an io panic.
async fn read_file_bytes(path: &Path) -> Result<Vec<u8>, ExportError> {
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
    Ok(buffer)
}

/// The result of the password-free half of reading a .ros file.
struct RosAudited {
    cursor: Cursor<Vec<u8>>,
    header: ros_format::FileHeader,
    chunk_table: ros_format::chunk::ChunkTable,
    signature: RosSignatureVerdict,
}

/// Everything [`import_ros`] does before it needs a key: parse and
/// validate the header, refuse a byte order this format cannot read,
/// check the declared length against the real one, read and validate the
/// chunk table, refuse declared compression, audit the layout for dead
/// space, compute the signature verdict over the raw bytes, and verify
/// every chunk's declared CRC-32.
///
/// Factored out so [`verify_ros_file`] runs the same checks in the same
/// order by construction rather than by maintenance — a password-free
/// verifier that quietly skipped a gate would be reporting a weaker
/// verdict under the same name.
fn audit_ros_bytes(buffer: Vec<u8>) -> Result<RosAudited, ExportError> {
    let file_len = buffer.len() as u64;
    let mut cursor = Cursor::new(buffer);
    // `read_from` verifies the header CRC-32 over the range this file's
    // version defines — the whole header minus the CRC field for v3.2+,
    // bytes 0..12 for v3.0/v3.1.
    let header =
        ros_format::FileHeader::read_from(&mut cursor).map_err(|e| ExportError::ExportFailed {
            reason: format!("Failed to read header: {}", e),
        })?;
    header.validate().map_err(|e| ExportError::ExportFailed {
        reason: format!("Header failed validation: {}", e),
    })?;

    // The chunk index is defined little-endian
    // (`ChunkIndexEntry::read_from`), so a big-endian header would
    // describe a file this format cannot consistently parse. Refuse by
    // name rather than silently misreading every offset.
    if header.endianness != ros_format::Endianness::Little {
        return Err(ExportError::ExportFailed {
            reason: "REFUSED: header declares big-endian byte order, but the .ros \
                     chunk index is defined little-endian — the two halves of the \
                     file would be read under different rules. Every .ros file \
                     written by this engine is little-endian."
                .to_string(),
        });
    }

    // `file_size` is excluded from the signed header leaf (it is not
    // known when the signature is computed), so it is checked exactly
    // instead: it must equal the bytes actually on disk. This also
    // catches truncation and appended trailers, neither of which any
    // checksum in the file would notice.
    if header.file_size != file_len {
        return Err(ExportError::ExportFailed {
            reason: format!(
                "header declares file_size {} but the file is {} bytes — \
                 the file was truncated, extended, or its header was rewritten",
                header.file_size, file_len
            ),
        });
    }

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

    // Compression refusal — pins the ONE-RULE invariant below.
    //
    // The signed byte range is `size_on_disk()` (compressed_size when
    // non-zero) while the parsed byte range used to be
    // `uncompressed_size`: two rules that agreed only because the writer
    // never compresses. This engine has no decompressor at all, so a
    // chunk declaring compression cannot be read correctly by ANY path.
    // Refusing here makes the divergence unreachable — after this loop
    // `size_on_disk() == uncompressed_size` for every entry, and
    // `read_chunk_payload` uses `size_on_disk()` so both callers slice
    // the same bytes by the same rule.
    for entry in chunk_table.iter() {
        if entry.compressed_size != 0 || entry.compression != ros_format::CompressionAlgorithm::None
        {
            return Err(ExportError::ExportFailed {
                reason: format!(
                    "REFUSED: {} chunk declares compression (algorithm {:?}, \
                     compressed_size {}), which this engine cannot decompress. \
                     Reading it would slice a different byte range than the \
                     signature covers, so the file is refused rather than \
                     silently misread.",
                    ChunkType::from_fourcc(entry.chunk_type).as_str(),
                    entry.compression,
                    entry.compressed_size
                ),
            });
        }
    }

    // Layout audit — the file must be exactly what its own header says it
    // is, with no dead space anywhere.
    //
    // This is what makes the two header fields excluded from the signed
    // leaf (`index_offset`, `file_size`) harmless BY CONSTRUCTION rather
    // than by argument. Without it they are jointly exploitable: insert
    // padding between the last payload and the chunk index, bump both
    // fields by the padding size, and recompute the header CRC. Every
    // payload then still sits at its declared offset, every 96-byte index
    // entry is byte-identical (merely relocated), and all three edited
    // header fields are blanked in the leaf — so the Merkle root is
    // UNCHANGED and the signature still verifies. `ChunkTable::validate`
    // does not catch it either: it checks overlaps, not gaps. Trailing
    // padding after the index survives the same way.
    //
    // Both equalities hold exactly for every file this writer produces
    // (layout pass 2 computes `index_offset` as 128 + Σ payload lengths,
    // and `file_size` as `index_offset + count * 96`), so there is no
    // tolerance to tune and no false-positive class.
    //
    // Ported from `OracleFile::audit_layout` in
    // `export-engine/tests/ros_independent_oracle.rs`, which enforced
    // exactly these constraints from the day it was written — the reader
    // simply had no equivalent.
    {
        let layout_err = |detail: String| ExportError::ExportFailed {
            reason: format!(
                "REFUSED: .ros layout is not exactly what the header declares — {}. \
                 Dead space in a signed file is how padding is smuggled past a \
                 signature that cannot cover its own offsets.",
                detail
            ),
        };
        let mut payload_total: u64 = 0;
        for entry in chunk_table.iter() {
            let size = entry.size_on_disk();
            let end = entry
                .offset
                .checked_add(size)
                .ok_or_else(|| layout_err("a chunk's offset + size overflows".to_string()))?;
            if entry.offset < ros_format::HEADER_SIZE as u64 || end > header.index_offset {
                return Err(layout_err(format!(
                    "{} chunk occupies [{}..{}), outside the payload region [{}..{})",
                    ChunkType::from_fourcc(entry.chunk_type).as_str(),
                    entry.offset,
                    end,
                    ros_format::HEADER_SIZE,
                    header.index_offset
                )));
            }
            payload_total = payload_total
                .checked_add(size)
                .ok_or_else(|| layout_err("total payload size overflows".to_string()))?;
        }
        // Sum equality + the chunk table's own overlap check + every
        // chunk being inside the region together forbid gaps as well as
        // overlaps: the chunks must tile [128, index_offset) exactly.
        let expected_index_offset = ros_format::HEADER_SIZE as u64 + payload_total;
        if header.index_offset != expected_index_offset {
            return Err(layout_err(format!(
                "the chunk payloads total {} bytes, so the index must begin at {}, \
                 but the header places it at {} ({} bytes of dead space)",
                payload_total,
                expected_index_offset,
                header.index_offset,
                header.index_offset as i128 - expected_index_offset as i128
            )));
        }
        let index_end = header
            .index_offset
            .checked_add(chunk_table.len() as u64 * CHUNK_INDEX_ENTRY_SIZE as u64)
            .ok_or_else(|| layout_err("the chunk index end overflows".to_string()))?;
        if index_end != file_len {
            return Err(layout_err(format!(
                "the chunk index ends at {} but the file is {} bytes",
                index_end, file_len
            )));
        }
    }

    // Signature verdict — computed against the RAW on-disk bytes before
    // any chunk is decrypted or parsed, so a tampered file still gets
    // an honest verdict even if its payloads also fail to parse, and an
    // encrypted file verifies without the password.
    let signature = signature_verdict(&cursor, &header, &chunk_table)?;

    // Declared chunk CRC-32 — written on every chunk, and until v3.2
    // validated on none, which is worse than absent because it invites
    // trust. Checked here, after the signature verdict so that ordering
    // contract is preserved, and over every chunk in the table rather
    // than only the ones this reader goes on to parse.
    for entry in chunk_table.iter() {
        let bytes = raw_chunk_bytes(cursor.get_ref(), entry)?;
        if !entry.verify_crc(bytes) {
            return Err(ExportError::ExportFailed {
                reason: format!(
                    "{} chunk fails its declared CRC-32: index says {:#010x}, \
                     the {} bytes on disk hash to {:#010x} — the file is \
                     corrupt or was modified after writing",
                    ChunkType::from_fourcc(entry.chunk_type).as_str(),
                    entry.crc32,
                    bytes.len(),
                    ros_format::util::crc32(bytes),
                ),
            });
        }
    }

    Ok(RosAudited {
        cursor,
        header,
        chunk_table,
        signature,
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
/// - SIGN chunk present on a v3.0/v3.1 file → `SupersededScheme`: that
///   signature was made over the chunk payloads alone and is not
///   re-checkable under the v3.2 leaf set.
/// - SIGN chunk present (claimed or not) on a v3.2+ file → recompute the
///   Merkle root over [`signed_leaf_set`] and verify the Ed25519
///   signature against it, then re-derive `signer_id` from the record's
///   own public key.
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

    // Scheme gate. A file signed before the v3.2 integrity change was
    // signed over its chunk payloads ONLY. Re-checking it under the v3.2
    // leaf set would always fail (the header and index leaves did not
    // exist), and reporting that as `Invalid` would accuse an untampered
    // file of tampering. Re-checking it under the v3.1 rule instead
    // would print `Verified` on a file this reader knows tolerates a
    // rewritten header and swapped FourCC labels. Neither is honest, so
    // the state gets its own name.
    if !ros_format::uses_integrity_scheme_v2(header.major_version, header.minor_version) {
        return Ok(RosSignatureVerdict::SupersededScheme {
            file_version: header.version_string(),
            reason: format!(
                "this file was signed under the .ros v{} scheme, whose Merkle \
                 leaves were the chunk payloads alone — the 128-byte header and \
                 the {}-byte chunk index were outside the signature, so a \
                 `Verified` result there did not cover file_size, index_offset, \
                 signature_algo, feature_flags, ai_tracking, kdf_*, file_uuid, \
                 or the chunk FourCC labels. This reader implements the v3.2 \
                 scheme and will not certify the older, narrower one. Re-sign \
                 the artifact with a v3.2 writer.",
                header.version_string(),
                table.len() * CHUNK_INDEX_ENTRY_SIZE
            ),
        });
    }

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

    // Recompute the root over exactly the bytes the writer signed. The
    // index entries are taken RAW from the file — not re-serialized from
    // the parsed `ChunkIndexEntry` — because raw bytes are what was
    // signed, and re-serializing would launder any field this reader's
    // parser normalises.
    let header_leaf = normalized_header_leaf(file)?;
    let index_start = header.index_offset as usize;
    let mut payloads: Vec<Vec<u8>> = Vec::new();
    let mut index_entries: Vec<Vec<u8>> = Vec::new();
    for (i, e) in table.iter().enumerate() {
        if e.chunk_type == ChunkType::SIGN.as_fourcc() {
            continue;
        }
        payloads.push(raw_chunk_bytes(file, e)?.to_vec());

        let entry_oob = || ExportError::ExportFailed {
            reason: format!(
                "chunk index entry {} lies outside the {}-byte file (index_offset {})",
                i,
                file.len(),
                header.index_offset
            ),
        };
        let start = index_start
            .checked_add(i * CHUNK_INDEX_ENTRY_SIZE)
            .ok_or_else(entry_oob)?;
        let end = start
            .checked_add(CHUNK_INDEX_ENTRY_SIZE)
            .ok_or_else(entry_oob)?;
        index_entries.push(file.get(start..end).ok_or_else(entry_oob)?.to_vec());
    }
    let leaves = signed_leaf_set(header_leaf, payloads, index_entries);

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
        Ok(true) => {
            // The signature is genuine over these bytes. Only now does
            // the identity label attached to it matter — and it is the
            // one field an attacker can rewrite freely, because SIGN is
            // excluded from its own leaves. The writer derives it as
            // sha256(public_key)[..16] unconditionally, so a mismatch
            // has no legitimate producer: there is no older shape and no
            // caller-chosen id to preserve. Re-derive and refuse.
            let derived = sha256(&sig_chunk.signer.public_key);
            let derived_prefix = derived.get(..16).ok_or_else(|| ExportError::ExportFailed {
                reason: "SHA-256 produced fewer than 16 bytes".to_string(),
            })?;
            if derived_prefix != &sig_chunk.signer.metadata.signer_id[..] {
                return Ok(RosSignatureVerdict::ForgedSignerId {
                    declared_signer_id: to_hex(&sig_chunk.signer.metadata.signer_id),
                    derived_signer_id: to_hex(derived_prefix),
                    public_key: to_hex(&sig_chunk.signer.public_key),
                });
            }
            Ok(RosSignatureVerdict::Verified {
                signer_id: to_hex(&sig_chunk.signer.metadata.signer_id),
                public_key: to_hex(&sig_chunk.signer.public_key),
            })
        }
        Ok(false) => Ok(RosSignatureVerdict::Invalid {
            reason: "Ed25519 signature does not match the Merkle root of the \
                     file's header, chunk bytes and chunk index — the file was \
                     modified after signing, or the signature was transplanted \
                     from another file"
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
    kdf_authenticity: &KdfInputAuthenticity,
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
    // `size_on_disk()`, matching `raw_chunk_bytes` and the signature's
    // leaf slicing — ONE rule for "how many bytes is this chunk on
    // disk". `import_ros` refuses any entry where `size_on_disk()` and
    // `uncompressed_size` could differ, so this is also exact.
    let mut data = vec![0u8; entry.size_on_disk() as usize];
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
        // On an AEAD rejection the tag alone says only "not produced by
        // this key", which is true of a wrong password AND of altered key
        // material. The diagnosis says which — or says outright that this
        // file cannot tell, rather than letting the user read "wrong
        // password" into an unsigned file that was bricked by tampering.
        data = decryptor
            .decrypt_chunk(&entry.chunk_type, &data, chunk_index, None)
            .map_err(|e| ExportError::ExportFailed {
                reason: format!(
                    "Decryption of {} failed: {}. {}",
                    chunk_type.as_str(),
                    e,
                    kdf_authenticity.aead_failure_diagnosis()
                ),
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

/// The write-site invariants, exercised on BOTH branches.
///
/// `export_brep_to_ros` calls both predicates on values it produced
/// itself, so at the call site the refusing branch is unreachable — the
/// header is built before the key set and `append_sign_chunk` sets the
/// claim and pushes the chunk together. Left inline, those two refusals
/// were code shaped like a check that nothing had ever entered. As pure
/// predicates they are reachable here, so the refusal is a tested
/// behaviour rather than a comment about one.
#[cfg(test)]
mod write_site_invariant_tests {
    use super::{check_key_binding, check_signature_claim_matches_table};
    use shared_types::ExportError;

    fn refusal(err: ExportError) -> String {
        match err {
            ExportError::ExportFailed { reason } => reason,
            other => panic!("expected ExportFailed, got {other:?}"),
        }
    }

    /// The two agreeing states are the only ones the writer may proceed
    /// from: signed file with a SIGN chunk, unsigned file without one.
    #[test]
    fn agreeing_claim_and_table_are_accepted() {
        check_signature_claim_matches_table(true, true).expect("signed file carrying SIGN");
        check_signature_claim_matches_table(false, false).expect("unsigned file with no SIGN");
    }

    /// A header asserting a signature the file does not carry is a forged
    /// provenance claim. This is the exact state every file written by the
    /// pre-fix signer had, and the reader treats it as a hard error — so
    /// the writer must never emit it.
    #[test]
    fn a_claim_with_no_chunk_is_refused_and_names_both_sides() {
        let reason = refusal(
            check_signature_claim_matches_table(true, false)
                .expect_err("a signature claim with no SIGN chunk must be refused"),
        );
        assert!(
            reason.contains("REFUSED"),
            "refusal must be named: {reason}"
        );
        assert!(
            reason.contains("claim (true)") && reason.contains("has no SIGN chunk"),
            "the refusal must state BOTH sides of the disagreement so the writer's \
             own report is diagnosable: {reason}"
        );
    }

    /// The mirror image: a SIGN chunk the header does not declare. Also
    /// refused — a reader that trusts the header would silently ignore a
    /// real signature.
    #[test]
    fn a_chunk_with_no_claim_is_refused_and_names_both_sides() {
        let reason = refusal(
            check_signature_claim_matches_table(false, true)
                .expect_err("a SIGN chunk with no header claim must be refused"),
        );
        assert!(
            reason.contains("REFUSED"),
            "refusal must be named: {reason}"
        );
        assert!(
            reason.contains("claim (false)") && reason.contains("has a SIGN chunk"),
            "the refusal must state BOTH sides of the disagreement: {reason}"
        );
    }

    /// The binding that makes an encrypted file reopenable at all.
    #[test]
    fn a_key_set_bound_to_the_header_uuid_is_accepted() {
        let uuid = [0x5au8; 16];
        check_key_binding(&uuid, &uuid).expect("keys derived from the header's own uuid");
    }

    /// A one-BIT divergence must refuse. This is the defect the 2026-08-04
    /// fix removed, in its general form: chunk keys derived from an id the
    /// header does not carry produce a file no password can ever open, and
    /// — before this guard — the writer would have reported success.
    #[test]
    fn a_key_set_bound_to_any_other_id_is_refused_and_names_both_ids() {
        let header_uuid = [0x5au8; 16];
        let mut key_file_id = header_uuid;
        key_file_id[15] ^= 0x01;

        let reason = refusal(
            check_key_binding(&key_file_id, &header_uuid)
                .expect_err("keys bound to a different id must be refused"),
        );
        assert!(
            reason.contains("REFUSED"),
            "refusal must be named: {reason}"
        );
        assert!(
            reason.contains("5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5b")
                && reason.contains("5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a"),
            "the refusal must print BOTH ids in hex — a custodian holding the file \
             needs to see which id the keys came from: {reason}"
        );
        assert!(
            reason.contains("never be decrypted by any password"),
            "the refusal must state the consequence, not just the mismatch: {reason}"
        );
    }

    /// The all-zero id is not a special case: it must be compared, not
    /// treated as "unset". A key set that never got a real id would
    /// otherwise slip through against a real header uuid.
    #[test]
    fn an_unset_looking_zero_id_is_still_refused_against_a_real_uuid() {
        let reason = refusal(
            check_key_binding(&[0u8; 16], &[0x11u8; 16])
                .expect_err("an all-zero key file id must not be waved through"),
        );
        assert!(reason.contains("REFUSED"), "{reason}");
        assert!(
            reason.contains("00000000000000000000000000000000"),
            "the zero id must be printed, not elided: {reason}"
        );
    }
}

/// The AEAD-failure diagnosis, over every signature verdict.
///
/// The classification is the whole content of the fix for the
/// unsigned-encrypted brick vector: it decides whether the reader may
/// state a cause or must say it cannot tell. Each verdict is pinned
/// individually because collapsing any two of them is exactly the failure
/// mode — telling a user "wrong password" about a file whose key material
/// was rewritten, or telling them "the file was altered" about a typo.
#[cfg(test)]
mod kdf_authenticity_tests {
    use super::{KdfInputAuthenticity, RosSignatureVerdict};

    fn diagnosis(verdict: RosSignatureVerdict) -> String {
        KdfInputAuthenticity::of(&verdict).aead_failure_diagnosis()
    }

    /// The ONE phrase that asserts the password as a definite cause. Only
    /// the signed-and-intact case may emit it. Matched with its `DIAGNOSIS:`
    /// prefix so it cannot be confused with the undetermined case, which
    /// legitimately offers a wrong password as one of two POSSIBILITIES —
    /// a substring check without the prefix flagged that enumeration as a
    /// definite claim, which is the opposite of what it is.
    const DEFINITE_PASSWORD_VERDICT: &str = "DIAGNOSIS: the supplied password is wrong";

    /// A verifying signature covers the header leaf, and every KDF input
    /// lives there — so the password is the only remaining variable.
    /// The verdict must state the key rather than assert "untampered":
    /// an attacker who rewrites the header CAN re-sign with their own key.
    #[test]
    fn a_verified_signature_names_the_password_and_states_the_key() {
        let d = diagnosis(RosSignatureVerdict::Verified {
            signer_id: "aabb".to_string(),
            public_key: "deadbeef".to_string(),
        });
        assert!(
            d.contains(DEFINITE_PASSWORD_VERDICT),
            "a verified signature must let the reader name the cause: {d}"
        );
        assert!(
            d.contains("deadbeef") && d.contains("is the key you expect"),
            "the claim must be conditioned on the key, because `Verified` proves \
             only that the holder of THAT key signed THESE bytes: {d}"
        );
        assert!(
            d.contains("file_uuid") && d.contains("kdf_salt") && d.contains("file_iv"),
            "the diagnosis must name the fields the signature covers: {d}"
        );
    }

    /// The signature verifies but the identity label beside it is forged.
    /// The KDF inputs are still authenticated, so the password verdict
    /// stands — but the forged identity must not be silently dropped.
    #[test]
    fn a_forged_signer_id_keeps_the_password_verdict_and_flags_the_identity() {
        let d = diagnosis(RosSignatureVerdict::ForgedSignerId {
            declared_signer_id: "1111".to_string(),
            derived_signer_id: "2222".to_string(),
            public_key: "cafe".to_string(),
        });
        assert!(
            d.contains(DEFINITE_PASSWORD_VERDICT),
            "the bytes ARE covered by a verifying signature: {d}"
        );
        assert!(
            d.contains("signer_id does NOT re-derive"),
            "the forged identity must travel with the diagnosis: {d}"
        );
    }

    /// A broken signature is the OPPOSITE conclusion: the header was
    /// rewritten, so a correct password can fail. Reporting this as a
    /// wrong password is the specific lie this whole classification exists
    /// to prevent.
    #[test]
    fn a_broken_signature_says_the_file_was_altered_not_the_password() {
        let d = diagnosis(RosSignatureVerdict::Invalid {
            reason: "root mismatch".to_string(),
        });
        assert!(
            d.contains("ALTERED after it was signed"),
            "a broken signature must name tampering: {d}"
        );
        assert!(
            d.contains("Do not conclude the password is wrong"),
            "the reader must be told NOT to blame the password: {d}"
        );
        assert!(
            !d.contains(DEFINITE_PASSWORD_VERDICT),
            "a broken signature must never assert the password is wrong: {d}"
        );
    }

    /// The honest refusal: on an unsigned file the two causes are
    /// indistinguishable by construction, and the file says so — including
    /// that `header_crc32` is not a security control and that signing
    /// would have discriminated.
    #[test]
    fn an_unsigned_file_refuses_to_choose_between_the_two_causes() {
        let d = diagnosis(RosSignatureVerdict::Unsigned);
        assert!(d.contains("UNDETERMINED"), "{d}");
        assert!(
            d.contains("carries no signature"),
            "the reason it cannot tell must be stated: {d}"
        );
        assert!(
            d.contains("header CRC-32") && d.contains("not a security control"),
            "the refusal must explain WHY the CRC does not close the gap: {d}"
        );
        assert!(
            d.contains("v3.2") && d.contains("WOULD distinguish"),
            "the refusal must name the remedy: {d}"
        );
        assert!(
            !d.contains(DEFINITE_PASSWORD_VERDICT),
            "an undetermined cause must never be reported as a definite one: {d}"
        );
        assert!(
            d.contains("Either (1) the password does not match"),
            "it must still ENUMERATE the possibilities rather than say nothing: {d}"
        );
    }

    /// A v≤3.1 signature's leaves were the payloads alone, so it covers
    /// nothing in the header. It must land with the unsigned case and must
    /// NOT borrow the confidence of a v3.2 signature.
    #[test]
    fn a_superseded_scheme_signature_discriminates_no_better_than_none() {
        let d = diagnosis(RosSignatureVerdict::SupersededScheme {
            file_version: "3.1.0".to_string(),
            reason: "payload-only leaves".to_string(),
        });
        assert!(d.contains("UNDETERMINED"), "{d}");
        assert!(
            d.contains("superseded") && d.contains("chunk payloads alone"),
            "the reason must name the scheme, not just say `unsigned`: {d}"
        );
        assert!(
            !d.contains(DEFINITE_PASSWORD_VERDICT),
            "a payload-only signature authenticates no header byte: {d}"
        );
    }
}
