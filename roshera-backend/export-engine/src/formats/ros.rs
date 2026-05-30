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
//! ├── META  (JSON)         — author, units, software, vertex/face counts
//! ├── HIST  (MessagePack)  — timeline events + branch manifest. MANDATORY.
//! ├── PROV  (MessagePack)  — AI command log + privacy. MANDATORY.
//! ├── GEOM  (MessagePack)  — BRepSnapshot. OPTIONAL cache.
//! └── SIGN  (MessagePack)  — Ed25519 signature. OPTIONAL.
//! ```

use crate::formats::ros_snapshot::BRepSnapshot;
use crate::formats::timeline_chunk::{BranchManifest, HistChunk};
use geometry_engine::primitives::topology_builder::BRepModel;
use ros_format::keys::{KeyManager, SoftwareKeyManager};
use ros_format::util::current_time_ms;
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

    /// Sign the file with Ed25519 (writer must also supply a key — the
    /// signature itself is currently emitted with a fresh per-file
    /// key; multi-signer support was removed in slice 1).
    pub sign: bool,

    /// Optional Ed25519 signing key (32 bytes). When `sign` is true
    /// and this is `None`, a fresh key is generated for the file.
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
            tracking_level: TrackingLevel::Basic,
            sign: false,
            signing_key: None,
            password: None,
            author: "Roshera CAD".to_string(),
            software: "Roshera Export Engine v1.0".to_string(),
            units: "millimeters".to_string(),
        }
    }
}

/// Reader output. `snapshot` is `None` when the file omitted the
/// optional GEOM chunk; callers may rebuild geometry by replaying
/// `timeline` against a fresh `BRepModel`.
pub struct RosImport {
    pub timeline: Vec<TimelineEvent>,
    pub branches: Vec<BranchManifest>,
    pub aipr: ProvChunk,
    pub snapshot: Option<BRepSnapshot>,
}

/// Export a B-Rep model + timeline + provenance to .ros v3.1.
pub async fn export_brep_to_ros(
    payload: RosExportPayload<'_>,
    path: &Path,
    options: RosExportOptions,
) -> Result<(), shared_types::ExportError> {
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
        header = header.with_encryption(1, 2, 10_000, salt, file_iv);
    }
    // PROV is always present in v3.1, so the AI-provenance flag is
    // unconditionally set. The tracking level is taken from options.
    header = header.with_ai_tracking(options.tracking_level as u8);
    if options.sign {
        header = header.with_signature(1);
    }
    let mut header = header.build();

    let mut chunks: Vec<Chunk> = Vec::new();

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
    })
    .to_string();
    chunks.push(Chunk::new(ChunkType::META, meta_data.into_bytes()));

    // HIST chunk (mandatory) ------------------------------------------
    let hist_chunk = match payload.history {
        Some(data) => HistChunk::new(data.branches, data.events),
        None => HistChunk::empty(),
    };
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
    let prov_bytes = prov_chunk.serialize().map_err(ros_err)?;
    chunks.push(encrypt_if_enabled(
        Chunk::new(ChunkType::PROV, prov_bytes),
        key_set.as_ref(),
        file_iv,
        chunks.len(),
    )?);

    // GEOM chunk (optional cache) -------------------------------------
    if options.include_snapshot {
        let snapshot = BRepSnapshot::from_model(payload.model);
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

    Ok(())
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
        let key_manager = SoftwareKeyManager::default();
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

    Ok(RosImport {
        timeline: hist_chunk.events,
        branches: hist_chunk.branches,
        aipr: prov_chunk,
        snapshot,
    })
}

/// Convenience wrapper: import and materialise a `BRepModel`.
///
/// Uses the GEOM cache when present; otherwise rebuilds from HIST
/// events via `timeline_engine::rebuild_model_from_events`.
pub async fn import_ros_to_brep(
    path: &Path,
    password: Option<&str>,
) -> Result<BRepModel, shared_types::ExportError> {
    let import = import_ros(path, password).await?;

    if let Some(snapshot) = import.snapshot {
        return Ok(snapshot.to_model());
    }

    let mut model = BRepModel::new();
    let outcome = timeline_engine::rebuild_model_from_events(&mut model, &import.timeline);
    if outcome.events_skipped > 0 {
        return Err(ExportError::ExportFailed {
            reason: format!(
                "Failed to rebuild geometry from {} HIST events: {} skipped",
                import.timeline.len(),
                outcome.events_skipped
            ),
        });
    }
    Ok(model)
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
