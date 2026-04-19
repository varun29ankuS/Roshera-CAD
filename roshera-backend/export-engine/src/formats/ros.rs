//! ROS file format support
//!
//! Provides export and import functionality for the Roshera .ros v3 format

use crate::formats::ros_snapshot::BRepSnapshot;
use crate::ros_fs::keys::{KeyManager, SoftwareKeyManager};
use crate::ros_fs::util::current_time_ms;
use crate::ros_fs::{
    self, Chunk, ChunkIndexEntry, ChunkType, EncryptionAlgorithm, FileHeader,
    CHUNK_INDEX_ENTRY_SIZE,
};
use geometry_engine::primitives::topology_builder::BRepModel;
use shared_types::*;
use std::io::{Cursor, Seek};
use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Convert RosFileError to ExportError
impl From<ros_fs::error::RosFileError> for ExportError {
    fn from(err: ros_fs::error::RosFileError) -> Self {
        ExportError::ExportFailed {
            reason: format!("ROS file error: {}", err),
        }
    }
}

/// Export options for ROS format
#[derive(Debug, Clone)]
pub struct RosExportOptions {
    /// Enable encryption (AES-256-GCM)
    pub encrypt: bool,

    /// Password for encryption (required if encrypt is true)
    pub password: Option<String>,

    /// Enable AI provenance tracking
    pub track_ai: bool,

    /// AI tracking level (0=Basic, 1=Detailed, 2=Forensic)
    pub ai_tracking_level: u8,

    /// Enable digital signatures
    pub sign: bool,

    /// Author/creator name
    pub author: String,

    /// Software version
    pub software: String,

    /// Units (e.g., "millimeters", "inches")
    pub units: String,
}

impl Default for RosExportOptions {
    fn default() -> Self {
        Self {
            encrypt: false,
            password: None,
            track_ai: true,
            ai_tracking_level: 1, // Detailed
            sign: false,
            author: "Roshera CAD".to_string(),
            software: "Roshera Export Engine v1.0".to_string(),
            units: "millimeters".to_string(),
        }
    }
}

/// Export B-Rep model to ROS format
pub async fn export_brep_to_ros(
    model: &BRepModel,
    path: &Path,
    options: RosExportOptions,
) -> Result<(), shared_types::ExportError> {
    // Create file
    let mut file = File::create(path)
        .await
        .map_err(|e| ExportError::FileWriteError {
            path: path.to_string_lossy().to_string(),
        })?;

    // Set up encryption if requested
    let (key_set, salt, file_iv): (Option<ros_fs::KeySet>, [u8; 16], [u8; 8]) = if options.encrypt {
        let password = options.password.ok_or_else(|| ExportError::ExportFailed {
            reason: "Password required for encryption".to_string(),
        })?;

        let salt = ros_fs::random_16();
        let file_iv: [u8; 8] =
            ros_fs::random_bytes(8)
                .try_into()
                .map_err(|_| ExportError::ExportFailed {
                    reason: "random_bytes(8) did not return 8 bytes".to_string(),
                })?;
        let key_manager = SoftwareKeyManager::default();
        let key_set = key_manager
            .generate_key_set(&password, &salt)
            .map_err(|e| ExportError::ExportFailed {
                reason: format!("Key generation failed: {}", e),
            })?;

        (Some(key_set), salt, file_iv)
    } else {
        (None, [0u8; 16], [0u8; 8])
    };

    // Create file header
    let mut header = ros_fs::FileHeader::builder();

    if options.encrypt {
        header = header.with_encryption(1, 2, 10_000, salt, file_iv);
    }

    if options.track_ai {
        header = header.with_ai_tracking(options.ai_tracking_level);
    }

    if options.sign {
        header = header.with_signature(1); // Ed25519
    }

    let mut header = header.build();

    // Create chunks
    let mut chunks = Vec::new();

    // META chunk - File metadata
    let meta_data = serde_json::json!({
        "name": "Roshera CAD Model",
        "author": options.author,
        "created": current_time_ms(),
        "software": options.software,
        "units": options.units,
        "vertices": model.vertices.len(),
        "edges": model.edges.len(),
        "faces": model.faces.len(),
        "solids": model.solids.len(),
    })
    .to_string();

    let meta_chunk = Chunk::new(ChunkType::META, meta_data.as_bytes().to_vec());
    chunks.push(meta_chunk);

    // GEOM chunk - Geometry data
    // Convert B-Rep model to serializable snapshot
    let snapshot = BRepSnapshot::from_model(model);

    // Serialize the snapshot
    let geom_data = rmp_serde::to_vec_named(&snapshot).map_err(|e| ExportError::ExportFailed {
        reason: format!("Failed to serialize geometry: {}", e),
    })?;

    let mut geom_chunk = Chunk::new(ChunkType::GEOM, geom_data);

    // Encrypt geometry chunk if encryption is enabled
    if let Some(ref key_set_value) = key_set {
        let encryptor = ros_fs::ChunkEncryptor::new(
            ros_fs::EncryptionAlgorithm::AES256GCM,
            key_set_value.clone(),
            file_iv,
        );

        let encrypted_data = encryptor
            .encrypt_chunk(
                &geom_chunk.index.chunk_type,
                &geom_chunk.data,
                1, // chunk index
                None,
            )
            .map_err(|e| ExportError::ExportFailed {
                reason: format!("Encryption failed: {}", e),
            })?;

        geom_chunk.data = encrypted_data;
        geom_chunk.index.encrypted = true;
        geom_chunk.index.enc_algo = 1;
    }

    geom_chunk.update_crc();
    chunks.push(geom_chunk);

    // AIPR chunk - AI provenance (if tracking is enabled)
    if options.track_ai {
        let tracking_level = ros_fs::TrackingLevel::from_u8(options.ai_tracking_level)
            .ok_or_else(|| ExportError::ExportFailed {
                reason: format!(
                    "Invalid AI tracking level {} (expected 0..=2)",
                    options.ai_tracking_level
                ),
            })?;
        let ai_tracker = ros_fs::AICommandTracker::new(
            tracking_level,
            ros_fs::PrivacySettings::default(),
            None,
        );

        let aipr_chunk = ros_fs::Chunk::new(ros_fs::ChunkType::AIPR, ai_tracker.serialize());
        chunks.push(aipr_chunk);
    }

    // Calculate chunk positions
    let mut current_offset = 128; // After header
    for chunk in &mut chunks {
        chunk.index.offset = current_offset as u64;
        current_offset += chunk.data.len();
    }

    // Update header with index info
    header.index_offset = current_offset as u64;
    header.index_entry_count = chunks.len() as u32;
    header.file_size = (current_offset + chunks.len() * CHUNK_INDEX_ENTRY_SIZE) as u64;

    // Write to buffer
    let mut buffer = Vec::new();

    // Write header
    {
        let mut cursor = Cursor::new(&mut buffer);
        header
            .write_to(&mut cursor)
            .map_err(|e| ExportError::ExportFailed {
                reason: format!("Failed to write header: {}", e),
            })?;
    }

    // Write chunks
    for chunk in &chunks {
        buffer.extend_from_slice(&chunk.data);
    }

    // Write chunk index (append to end of buffer, after header + chunk data)
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

    // Write to file
    file.write_all(&buffer)
        .await
        .map_err(|e| ExportError::FileWriteError {
            path: path.to_string_lossy().to_string(),
        })?;

    Ok(())
}

/// Import B-Rep model from ROS format
pub async fn import_ros_to_brep(
    path: &Path,
    password: Option<&str>,
) -> Result<BRepModel, shared_types::ExportError> {
    // Read file
    let mut file =
        File::open(path)
            .await
            .map_err(|_e| shared_types::ExportError::ExportFailed {
                reason: format!("Failed to read file: {}", path.to_string_lossy()),
            })?;

    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .await
        .map_err(|_e| shared_types::ExportError::ExportFailed {
            reason: format!("Failed to read file: {}", path.to_string_lossy()),
        })?;

    let mut cursor = std::io::Cursor::new(buffer);

    // Read header
    let header =
        ros_fs::FileHeader::read_from(&mut cursor).map_err(|e| ExportError::ExportFailed {
            reason: format!("Failed to read header: {}", e),
        })?;

    // Check if file is encrypted
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

    // Read chunk index
    std::io::Seek::seek(&mut cursor, std::io::SeekFrom::Start(header.index_offset)).map_err(
        |e| ExportError::ExportFailed {
            reason: format!("Failed to seek to index: {}", e),
        },
    )?;

    let chunk_table = crate::ros_fs::chunk::ChunkTable::read_from(
        &mut cursor,
        header.index_offset,
        header.index_entry_count,
    )
    .map_err(|e| ExportError::ExportFailed {
        reason: format!("Failed to read chunk index: {}", e),
    })?;

    // Find and read GEOM chunk
    let geom_entry = chunk_table
        .find_by_type(ros_fs::ChunkType::GEOM)
        .ok_or_else(|| ExportError::ExportFailed {
            reason: "No geometry chunk found in file".to_string(),
        })?;

    std::io::Seek::seek(&mut cursor, std::io::SeekFrom::Start(geom_entry.offset)).map_err(|e| {
        ExportError::ExportFailed {
            reason: format!("Failed to seek to geometry chunk: {}", e),
        }
    })?;

    let mut geom_data = vec![0u8; geom_entry.compressed_size as usize];
    std::io::Read::read_exact(&mut cursor, &mut geom_data).map_err(|e| {
        ExportError::ExportFailed {
            reason: format!("Failed to read geometry chunk: {}", e),
        }
    })?;

    // Decrypt if necessary
    if geom_entry.encrypted {
        let key_set = key_set.as_ref().ok_or_else(|| ExportError::ExportFailed {
            reason: "Geometry is encrypted but no key available".to_string(),
        })?;

        let enc_algo = ros_fs::EncryptionAlgorithm::from_id(geom_entry.enc_algo)
            .ok_or_else(|| ExportError::ExportFailed {
                reason: format!(
                    "Unknown encryption algorithm id {} in chunk index",
                    geom_entry.enc_algo
                ),
            })?;
        let decryptor = ros_fs::ChunkEncryptor::new(enc_algo, key_set.clone(), header.file_iv);

        geom_data = decryptor
            .decrypt_chunk(
                &geom_entry.chunk_type,
                &geom_data,
                1, // chunk index
                None,
            )
            .map_err(|e| ExportError::ExportFailed {
                reason: format!("Decryption failed: {}", e),
            })?;
    }

    // Deserialize B-Rep snapshot
    let snapshot: BRepSnapshot =
        rmp_serde::from_slice(&geom_data).map_err(|e| ExportError::ExportFailed {
            reason: format!("Failed to deserialize geometry: {}", e),
        })?;

    // Convert snapshot back to B-Rep model
    let model = snapshot.to_model();

    Ok(model)
}
