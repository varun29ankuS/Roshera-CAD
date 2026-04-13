// src/keys.rs

//! Encryption Key Management for .ros v3 (KEYS chunk)
//!
//! Provides secure key derivation, storage, and management with:
//! - Multiple KDF algorithms (PBKDF2, Argon2)
//! - Hierarchical key derivation
//! - Key rotation support
//! - Hardware security module abstraction

use crate::ros_fs::util::{format_uuid, random_16, random_32, secure_zero, sha256};
use crate::ros_fs::{KeyManagementError, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::fmt;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Algorithm IDs for key derivation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KdfAlgo {
    None = 0,
    PBKDF2 = 1,
    Argon2 = 2,
}

impl KdfAlgo {
    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            0 => Ok(KdfAlgo::None),
            1 => Ok(KdfAlgo::PBKDF2),
            2 => Ok(KdfAlgo::Argon2),
            _ => Err(KeyManagementError::InvalidKeyFormat {
                expected: "KDF algorithm 0-2".to_string(),
                actual: format!("Invalid value: {}", value),
            }
            .into()),
        }
    }
}

/// Key algorithms
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyAlgorithm {
    None = 0,
    AES256GCM = 1,
    ChaCha20Poly1305 = 2,
    AES256CTR = 3,
}

impl KeyAlgorithm {
    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            0 => Ok(KeyAlgorithm::None),
            1 => Ok(KeyAlgorithm::AES256GCM),
            2 => Ok(KeyAlgorithm::ChaCha20Poly1305),
            3 => Ok(KeyAlgorithm::AES256CTR),
            _ => Err(KeyManagementError::InvalidKeyFormat {
                expected: "Key algorithm 0-3".to_string(),
                actual: format!("Invalid value: {}", value),
            }
            .into()),
        }
    }

    pub fn key_size_bytes(&self) -> usize {
        match self {
            KeyAlgorithm::None => 0,
            KeyAlgorithm::AES256GCM => 32,
            KeyAlgorithm::ChaCha20Poly1305 => 32,
            KeyAlgorithm::AES256CTR => 32,
        }
    }
}

/// Key types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyType {
    Symmetric = 0,
    Public = 1,
    Derived = 2,
    Escrowed = 3,
}

impl KeyType {
    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            0 => Ok(KeyType::Symmetric),
            1 => Ok(KeyType::Public),
            2 => Ok(KeyType::Derived),
            3 => Ok(KeyType::Escrowed),
            _ => Err(KeyManagementError::InvalidKeyFormat {
                expected: "Key type 0-3".to_string(),
                actual: format!("Invalid value: {}", value),
            }
            .into()),
        }
    }
}

/// .ros v3 KEYS chunk header
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeysHeader {
    pub version: u32,
    pub key_count: u32,
    pub master_key_id: [u8; 16],
    pub flags: u32,
}

impl KeysHeader {
    pub fn new(master_key_id: [u8; 16]) -> Self {
        KeysHeader {
            version: 1,
            key_count: 0,
            master_key_id,
            flags: 0,
        }
    }
}

/// Single key entry in KEYS chunk
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyEntry {
    pub key_id: [u8; 16], // UUID
    pub key_type: KeyType,
    pub algorithm: KeyAlgorithm,
    pub key_size: u16,                    // bits
    pub usage_flags: u32,                 // Chunk types this key can decrypt
    pub parent_key_id: Option<[u8; 16]>,  // For derived keys
    pub derivation_info: Option<Vec<u8>>, // Salt or other derivation data
    pub required_level: u32,              // Access level required
    pub expiration: Option<u64>,          // Unix ms
    pub encrypted_key: Option<Vec<u8>>,   // For key escrow
    pub certificate: Option<Vec<u8>>,     // For public keys
}

impl KeyEntry {
    pub fn new(algorithm: KeyAlgorithm) -> Self {
        KeyEntry {
            key_id: random_16(),
            key_type: KeyType::Symmetric,
            algorithm,
            key_size: (algorithm.key_size_bytes() * 8) as u16,
            usage_flags: 0xFFFFFFFF, // All chunks by default
            parent_key_id: None,
            derivation_info: None,
            required_level: 0,
            expiration: None,
            encrypted_key: None,
            certificate: None,
        }
    }

    pub fn is_expired(&self, now_ms: u64) -> bool {
        if let Some(exp) = self.expiration {
            now_ms > exp
        } else {
            false
        }
    }

    pub fn can_decrypt_chunk(&self, chunk_fourcc: &[u8; 4]) -> bool {
        let chunk_bits = match chunk_fourcc {
            b"GEOM" => 1 << 0,
            b"TOPO" => 1 << 1,
            b"FEAT" => 1 << 2,
            b"AIPR" => 1 << 3,
            b"META" => 1 << 4,
            _ => 1 << 31, // Custom chunks
        };
        (self.usage_flags & chunk_bits) != 0
    }
}

/// Secure key material container
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SecureKey {
    #[zeroize(skip)]
    pub id: [u8; 16],
    pub material: Vec<u8>,
}

impl SecureKey {
    pub fn new(id: [u8; 16], material: Vec<u8>) -> Self {
        SecureKey { id, material }
    }

    pub fn random(algorithm: KeyAlgorithm) -> Result<Self> {
        let size = algorithm.key_size_bytes();
        if size == 0 {
            return Err(KeyManagementError::KeyGenerationFailed {
                algorithm: format!("{:?}", algorithm),
                reason: "Invalid key size".to_string(),
            }
            .into());
        }

        let id = random_16();
        let material = crate::ros_fs::util::random_bytes(size);
        Ok(SecureKey { id, material })
    }
}

impl fmt::Debug for SecureKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SecureKey {{ id: {}, material: [REDACTED] }}",
            format_uuid(&self.id)
        )
    }
}

// Custom serialization that only serializes the ID, not the key material
impl Serialize for SecureKey {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Only serialize the ID for security reasons
        self.id.serialize(serializer)
    }
}

// Custom deserialization that creates a placeholder
impl<'de> Deserialize<'de> for SecureKey {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Deserialize only the ID, material must be loaded separately
        let id = <[u8; 16]>::deserialize(deserializer)?;
        Ok(SecureKey {
            id,
            material: vec![], // Empty placeholder - must be loaded separately
        })
    }
}

/// In-memory representation of all file keys
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeySet {
    pub master: SecureKey,
    pub file_id: [u8; 16],
    pub file_key: SecureKey,
    pub chunk_keys: HashMap<[u8; 4], SecureKey>,
    pub metadata: HashMap<[u8; 16], KeyEntry>,
}

impl KeySet {
    /// Create a new empty key set
    pub fn new(master: SecureKey, file_id: [u8; 16]) -> Self {
        KeySet {
            master,
            file_id,
            file_key: SecureKey::new([0; 16], vec![0; 32]),
            chunk_keys: HashMap::new(),
            metadata: HashMap::new(),
        }
    }

    /// Get key for a specific chunk type
    pub fn get_chunk_key(&self, chunk_fourcc: &[u8; 4]) -> Option<&SecureKey> {
        self.chunk_keys.get(chunk_fourcc)
    }

    /// Add a chunk key
    pub fn add_chunk_key(&mut self, chunk_fourcc: [u8; 4], key: SecureKey) {
        let entry = KeyEntry::new(KeyAlgorithm::AES256GCM);
        self.metadata.insert(key.id, entry);
        self.chunk_keys.insert(chunk_fourcc, key);
    }

    /// Clear all sensitive key material
    pub fn zeroize(&mut self) {
        secure_zero(&mut self.master.material);
        secure_zero(&mut self.file_key.material);
        for (_, key) in self.chunk_keys.iter_mut() {
            secure_zero(&mut key.material);
        }
    }
}

impl Drop for KeySet {
    fn drop(&mut self) {
        self.zeroize();
    }
}

/// Trait for key management implementations
pub trait KeyManager: Send + Sync {
    /// Derive master key from password
    fn derive_master_key(&self, password: &str, salt: &[u8], iterations: u32) -> Result<SecureKey>;

    /// Derive file key from master key
    fn derive_file_key(&self, master_key: &SecureKey, file_id: &[u8; 16]) -> Result<SecureKey>;

    /// Derive chunk key from file key
    fn derive_chunk_key(&self, file_key: &SecureKey, chunk_type: &[u8; 4]) -> Result<SecureKey>;

    /// Generate a complete key set for a file
    fn generate_key_set(&self, password: &str, salt: &[u8]) -> Result<KeySet>;

    /// Rotate keys in a key set
    fn rotate_keys(&self, key_set: &mut KeySet) -> Result<()>;
}

/// Default software-only key manager using Argon2 + HKDF
pub struct SoftwareKeyManager {
    pub kdf_iterations: u32,
}

impl Default for SoftwareKeyManager {
    fn default() -> Self {
        SoftwareKeyManager {
            kdf_iterations: 10_000,
        }
    }
}

impl KeyManager for SoftwareKeyManager {
    fn derive_master_key(&self, password: &str, salt: &[u8], iterations: u32) -> Result<SecureKey> {
        use argon2::{Algorithm, Argon2, Params, Version};

        let params = Params::new(
            64 * 1024, // 64 MB memory
            iterations,
            4,        // parallelism
            Some(32), // output length
        )
        .map_err(|e| KeyManagementError::KeyDerivationFailed {
            reason: format!("Invalid Argon2 params: {}", e),
        })?;

        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

        let mut output = vec![0u8; 32];
        argon2
            .hash_password_into(password.as_bytes(), salt, &mut output)
            .map_err(|e| KeyManagementError::KeyDerivationFailed {
                reason: format!("Argon2 failed: {}", e),
            })?;

        let id = random_16();
        Ok(SecureKey::new(id, output))
    }

    fn derive_file_key(&self, master_key: &SecureKey, file_id: &[u8; 16]) -> Result<SecureKey> {
        use hkdf::Hkdf;
        use sha2::Sha256;

        let hk = Hkdf::<Sha256>::new(Some(b"ROSHERA_FILE_KEY_V3"), &master_key.material);
        let mut output = vec![0u8; 32];

        hk.expand(file_id, &mut output)
            .map_err(|_| KeyManagementError::KeyDerivationFailed {
                reason: "HKDF expand failed".to_string(),
            })?;

        let id = {
            let mut id = [0u8; 16];
            id.copy_from_slice(&sha256(&output)[..16]);
            id
        };

        Ok(SecureKey::new(id, output))
    }

    fn derive_chunk_key(&self, file_key: &SecureKey, chunk_type: &[u8; 4]) -> Result<SecureKey> {
        use hkdf::Hkdf;
        use sha2::Sha256;

        let mut info = Vec::new();
        info.extend_from_slice(b"ROSHERA_CHUNK_V3");
        info.extend_from_slice(chunk_type);

        let hk = Hkdf::<Sha256>::new(Some(&info), &file_key.material);
        let mut output = vec![0u8; 32];

        hk.expand(chunk_type, &mut output).map_err(|_| {
            KeyManagementError::KeyDerivationFailed {
                reason: "HKDF expand failed".to_string(),
            }
        })?;

        let id = {
            let mut id = [0u8; 16];
            id.copy_from_slice(&sha256(&output)[..16]);
            id
        };

        Ok(SecureKey::new(id, output))
    }

    fn generate_key_set(&self, password: &str, salt: &[u8]) -> Result<KeySet> {
        // Derive master key
        let master = self.derive_master_key(password, salt, self.kdf_iterations)?;

        // Generate file ID
        let file_id = random_16();

        // Derive file key
        let file_key = self.derive_file_key(&master, &file_id)?;

        // Create key set
        let mut key_set = KeySet::new(master, file_id);
        key_set.file_key = file_key;

        // Derive chunk keys for standard chunks
        for chunk_type in [b"GEOM", b"TOPO", b"FEAT", b"AIPR", b"META", b"KEYS"].iter() {
            let chunk_key = self.derive_chunk_key(&key_set.file_key, chunk_type)?;
            key_set.add_chunk_key(**chunk_type, chunk_key);
        }

        Ok(key_set)
    }

    fn rotate_keys(&self, key_set: &mut KeySet) -> Result<()> {
        // Generate new file ID
        let new_file_id = random_16();

        // Derive new file key
        let new_file_key = self.derive_file_key(&key_set.master, &new_file_id)?;

        // Update key set
        key_set.file_id = new_file_id;
        key_set.file_key = new_file_key;

        // Clear old chunk keys
        key_set.chunk_keys.clear();
        key_set.metadata.clear();

        // Derive new chunk keys
        for chunk_type in [b"GEOM", b"TOPO", b"FEAT", b"AIPR", b"META", b"KEYS"].iter() {
            let chunk_key = self.derive_chunk_key(&key_set.file_key, chunk_type)?;
            key_set.add_chunk_key(**chunk_type, chunk_key);
        }

        Ok(())
    }
}

/// Hardware Security Module key manager trait
pub trait HsmKeyManager: KeyManager {
    /// Get HSM identifier
    fn hsm_id(&self) -> String;

    /// Initialize HSM session
    fn init_session(&self) -> Result<()>;

    /// Close HSM session
    fn close_session(&self) -> Result<()>;

    /// Generate key in HSM
    fn generate_key_in_hsm(&self, algorithm: KeyAlgorithm) -> Result<[u8; 16]>;

    /// Export key from HSM (if allowed)
    fn export_key(&self, key_id: &[u8; 16]) -> Result<SecureKey>;
}

/// Key escrow service for backup/recovery
pub struct KeyEscrowService {
    escrow_key: SecureKey,
}

impl KeyEscrowService {
    pub fn new(escrow_key: SecureKey) -> Self {
        KeyEscrowService { escrow_key }
    }

    /// Escrow a key set
    pub fn escrow_key_set(&self, key_set: &KeySet) -> Result<Vec<u8>> {
        // Serialize key set using MessagePack instead of JSON
        let serialized = rmp_serde::to_vec_named(key_set) // Changed from serde_json::to_vec
            .map_err(|e| KeyManagementError::EscrowError {
                operation: "serialize".to_string(),
                details: e.to_string(),
            })?;

        // Encrypt with escrow key using AES-GCM
        use aes_gcm::aead::Aead;
        use aes_gcm::{Aes256Gcm, KeyInit, Nonce};

        let cipher = Aes256Gcm::new_from_slice(&self.escrow_key.material).map_err(|_| {
            KeyManagementError::EscrowError {
                operation: "cipher init".to_string(),
                details: "Invalid escrow key".to_string(),
            }
        })?;

        let nonce_bytes = crate::ros_fs::util::random_bytes(12);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher.encrypt(nonce, serialized.as_ref()).map_err(|_| {
            KeyManagementError::EscrowError {
                operation: "encrypt".to_string(),
                details: "Encryption failed".to_string(),
            }
        })?;

        // Return nonce + ciphertext
        let mut result = nonce_bytes;
        result.extend_from_slice(&ciphertext);
        Ok(result)
    }

    /// Recover a key set from escrow
    pub fn recover_key_set(&self, escrowed_data: &[u8]) -> Result<KeySet> {
        if escrowed_data.len() < 12 {
            return Err(KeyManagementError::EscrowError {
                operation: "recover".to_string(),
                details: "Invalid escrow data".to_string(),
            }
            .into());
        }

        // Extract nonce and ciphertext
        let (nonce_bytes, ciphertext) = escrowed_data.split_at(12);

        // Decrypt
        use aes_gcm::aead::Aead;
        use aes_gcm::{Aes256Gcm, KeyInit, Nonce};

        let cipher = Aes256Gcm::new_from_slice(&self.escrow_key.material).map_err(|_| {
            KeyManagementError::EscrowError {
                operation: "cipher init".to_string(),
                details: "Invalid escrow key".to_string(),
            }
        })?;

        let nonce = Nonce::from_slice(nonce_bytes);

        let plaintext =
            cipher
                .decrypt(nonce, ciphertext)
                .map_err(|_| KeyManagementError::EscrowError {
                    operation: "decrypt".to_string(),
                    details: "Decryption failed".to_string(),
                })?;

        // Deserialize using MessagePack
        rmp_serde::from_slice(&plaintext) // Changed from serde_json::from_slice
            .map_err(|e| {
                KeyManagementError::EscrowError {
                    operation: "deserialize".to_string(),
                    details: e.to_string(),
                }
                .into()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper for fast key generation in tests
    fn test_key_manager() -> SoftwareKeyManager {
        SoftwareKeyManager {
            kdf_iterations: 4, // Much faster for tests
        }
    }

    #[test]
    fn test_secure_key() {
        let key = SecureKey::random(KeyAlgorithm::AES256GCM).unwrap();
        assert_eq!(key.material.len(), 32);

        // Test debug doesn't leak key material
        let debug_str = format!("{:?}", key);
        assert!(debug_str.contains("[REDACTED]"));
    }

    #[test]
    fn test_key_derivation() {
        let manager = test_key_manager();
        let salt = random_16();

        // Derive master key
        let master = manager
            .derive_master_key("test_password", &salt, 4)
            .unwrap();
        assert_eq!(master.material.len(), 32);

        // Derive file key
        let file_id = random_16();
        let file_key = manager.derive_file_key(&master, &file_id).unwrap();
        assert_eq!(file_key.material.len(), 32);

        // Derive chunk key
        let chunk_key = manager.derive_chunk_key(&file_key, b"GEOM").unwrap();
        assert_eq!(chunk_key.material.len(), 32);

        // Different chunk types should produce different keys
        let chunk_key2 = manager.derive_chunk_key(&file_key, b"TOPO").unwrap();
        assert_ne!(chunk_key.material, chunk_key2.material);
    }

    #[test]
    fn test_key_set_generation() {
        let manager = test_key_manager();
        let salt = random_16();

        let key_set = manager.generate_key_set("test_password", &salt).unwrap();

        assert_eq!(key_set.chunk_keys.len(), 6);
        assert!(key_set.get_chunk_key(b"GEOM").is_some());
        assert!(key_set.get_chunk_key(b"XXXX").is_none());
    }

    #[test]
    fn test_key_rotation() {
        let manager = test_key_manager();
        let salt = random_16();

        let mut key_set = manager.generate_key_set("test_password", &salt).unwrap();
        let old_file_id = key_set.file_id;
        let old_geom_key = key_set.get_chunk_key(b"GEOM").unwrap().id;

        manager.rotate_keys(&mut key_set).unwrap();

        assert_ne!(key_set.file_id, old_file_id);
        assert_ne!(key_set.get_chunk_key(b"GEOM").unwrap().id, old_geom_key);
        assert_eq!(key_set.chunk_keys.len(), 6);
    }

    #[test]
    fn test_key_entry_expiration() {
        let mut entry = KeyEntry::new(KeyAlgorithm::AES256GCM);
        assert!(!entry.is_expired(1000));

        entry.expiration = Some(500);
        assert!(entry.is_expired(1000));
        assert!(!entry.is_expired(400));
    }

    #[test]
    fn test_key_entry_chunk_permissions() {
        let mut entry = KeyEntry::new(KeyAlgorithm::AES256GCM);

        // Default: can decrypt all
        assert!(entry.can_decrypt_chunk(b"GEOM"));
        assert!(entry.can_decrypt_chunk(b"TOPO"));

        // Restrict to only GEOM
        entry.usage_flags = 1; // Only bit 0
        assert!(entry.can_decrypt_chunk(b"GEOM"));
        assert!(!entry.can_decrypt_chunk(b"TOPO"));
    }

    #[test]
    fn test_key_escrow() {
        let escrow_key = SecureKey::random(KeyAlgorithm::AES256GCM).unwrap();
        let escrow_service = KeyEscrowService::new(escrow_key);

        let manager = test_key_manager();
        let salt = random_16();
        let key_set = manager.generate_key_set("test_password", &salt).unwrap();

        // Escrow
        let escrowed = escrow_service.escrow_key_set(&key_set).unwrap();
        assert!(escrowed.len() > 12); // At least nonce + some data

        // Recover
        let recovered = escrow_service.recover_key_set(&escrowed).unwrap();
        assert_eq!(recovered.file_id, key_set.file_id);
        assert_eq!(recovered.chunk_keys.len(), key_set.chunk_keys.len());
    }
}
