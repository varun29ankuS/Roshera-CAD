//! Session persistence functionality

use shared_types::*;
use std::path::PathBuf;
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// Persistence manager for sessions
#[derive(Clone)]
pub struct PersistenceManager {
    /// Base directory for persistence
    base_dir: PathBuf,
}

impl PersistenceManager {
    /// Create new persistence manager
    pub fn new(base_dir: String) -> Self {
        Self {
            base_dir: PathBuf::from(base_dir),
        }
    }

    /// Save session to disk
    pub async fn save_session(&self, session: &SessionState) -> Result<(), SessionError> {
        let session_file = self.base_dir.join(format!("{}.json", session.id));

        let json =
            serde_json::to_string_pretty(session).map_err(|e| SessionError::PersistenceError {
                reason: format!("Serialization failed: {}", e),
            })?;

        fs::create_dir_all(&self.base_dir)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to create directory: {}", e),
            })?;

        let mut file =
            fs::File::create(&session_file)
                .await
                .map_err(|e| SessionError::PersistenceError {
                    reason: format!("Failed to create file: {}", e),
                })?;

        file.write_all(json.as_bytes())
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to write file: {}", e),
            })?;

        Ok(())
    }

    /// Load session from disk
    pub async fn load_session(&self, session_id: &ObjectId) -> Result<SessionState, SessionError> {
        let session_file = self.base_dir.join(format!("{}.json", session_id));

        let contents = fs::read_to_string(&session_file).await.map_err(|e| {
            SessionError::PersistenceError {
                reason: format!("Failed to read file: {}", e),
            }
        })?;

        let session =
            serde_json::from_str(&contents).map_err(|e| SessionError::PersistenceError {
                reason: format!("Deserialization failed: {}", e),
            })?;

        Ok(session)
    }

    /// Delete session from disk
    pub async fn delete_session(&self, session_id: &ObjectId) -> Result<(), SessionError> {
        let session_file = self.base_dir.join(format!("{}.json", session_id));

        fs::remove_file(&session_file)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to delete file: {}", e),
            })?;

        Ok(())
    }

    /// List all saved sessions
    pub async fn list_sessions(&self) -> Result<Vec<ObjectId>, SessionError> {
        let mut sessions = Vec::new();

        let mut entries =
            fs::read_dir(&self.base_dir)
                .await
                .map_err(|e| SessionError::PersistenceError {
                    reason: format!("Failed to read directory: {}", e),
                })?;

        while let Some(entry) =
            entries
                .next_entry()
                .await
                .map_err(|e| SessionError::PersistenceError {
                    reason: format!("Failed to read entry: {}", e),
                })?
        {
            if let Some(name) = entry.file_name().to_str() {
                if name.ends_with(".json") {
                    if let Ok(id) = uuid::Uuid::parse_str(&name[..name.len() - 5]) {
                        sessions.push(id);
                    }
                }
            }
        }

        Ok(sessions)
    }
}
