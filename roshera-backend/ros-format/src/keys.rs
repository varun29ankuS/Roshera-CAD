// src/keys.rs

//! Encryption Key Management for .ros v3 (KEYS chunk)
//!
//! Provides secure key derivation, storage, and management with:
//! - Multiple KDF algorithms (PBKDF2, Argon2)
//! - Hierarchical key derivation
//! - Key rotation support

use crate::util::{format_uuid, random_16, secure_zero, sha256};
use crate::{KeyManagementError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Algorithm IDs for key derivation, as stored in
/// [`crate::header::FileHeader::kdf_algo`].
///
/// The id names the whole derivation CHAIN, not just the password hash.
/// [`Argon2`](KdfAlgo::Argon2) and [`Argon2idFileBound`](KdfAlgo::Argon2idFileBound)
/// use the same Argon2id password hash and differ only in what the file
/// key is expanded from — and that difference is the difference between
/// a file that can be reopened and one that cannot, so it must be
/// legible on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KdfAlgo {
    None = 0,
    PBKDF2 = 1,
    /// **Superseded, and unreadable by construction.** Argon2id over the
    /// password, then HKDF-expand of the file key over a `file_id` that
    /// was freshly randomised on every call to `generate_key_set` and
    /// never written into the file. A reader therefore invents a
    /// different `file_id`, derives different chunk keys, and the
    /// AES-256-GCM tag rejects — for the writer's own password as much
    /// as for anyone else's. Files carrying this id are refused by name
    /// on import rather than failing as a generic auth error, because
    /// their key material does not exist anywhere to be recovered.
    Argon2 = 2,
    /// Argon2id over the password, then HKDF-expand of the file key over
    /// the header's `file_uuid` — a value that IS written to disk and,
    /// on a signed file, covered by the signature. Every key in the set
    /// is reproducible from bytes the file actually carries.
    Argon2idFileBound = 3,
}

impl KdfAlgo {
    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            0 => Ok(KdfAlgo::None),
            1 => Ok(KdfAlgo::PBKDF2),
            2 => Ok(KdfAlgo::Argon2),
            3 => Ok(KdfAlgo::Argon2idFileBound),
            _ => Err(KeyManagementError::InvalidKeyFormat {
                expected: "KDF algorithm 0-3".to_string(),
                actual: format!("Invalid value: {}", value),
            }
            .into()),
        }
    }

    pub fn as_u8(&self) -> u8 {
        *self as u8
    }
}

/// The `kdf_algo` id every .ros writer emits for an encrypted file: the
/// file-key derivation is bound to the header's `file_uuid`, so the
/// importer reproduces the writer's keys exactly.
pub const KDF_ALGO_ARGON2ID_FILE_BOUND: u8 = KdfAlgo::Argon2idFileBound as u8;

/// The superseded id. An encrypted file declaring this was written with
/// a random, never-persisted KDF file id; see [`KdfAlgo::Argon2`].
pub const KDF_ALGO_ARGON2ID_UNBOUND: u8 = KdfAlgo::Argon2 as u8;

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
        let material = crate::util::random_bytes(size);
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
///
/// # The reproducibility rule
///
/// Every key this trait produces must be derivable from material the
/// file itself carries: the password, the header's `kdf_salt`, its
/// `kdf_iterations`, its `file_uuid`, and the chunk's own FourCC.
/// Nothing else. A key derived from a value that is minted at write time
/// and never written down encrypts data that no password can ever
/// recover — which is precisely what `generate_key_set` did until
/// 2026-08-04, when it drew `file_id = random_16()` internally.
///
/// The `file_id` is therefore a REQUIRED parameter of both
/// [`generate_key_set`](KeyManager::generate_key_set) and
/// [`rotate_keys`](KeyManager::rotate_keys) rather than an internal
/// detail: a caller cannot derive a key set without having decided
/// where the id is persisted, so the defect is not merely fixed but
/// inexpressible.
pub trait KeyManager: Send + Sync {
    /// Derive master key from password
    fn derive_master_key(&self, password: &str, salt: &[u8], iterations: u32) -> Result<SecureKey>;

    /// Derive file key from master key
    fn derive_file_key(&self, master_key: &SecureKey, file_id: &[u8; 16]) -> Result<SecureKey>;

    /// Derive chunk key from file key
    fn derive_chunk_key(&self, file_key: &SecureKey, chunk_type: &[u8; 4]) -> Result<SecureKey>;

    /// Generate a complete key set for a file.
    ///
    /// `file_id` MUST be a value the file persists — for .ros that is
    /// [`crate::header::FileHeader::file_uuid`]. Given the same
    /// `(password, salt, file_id)` this is a pure function: identical
    /// key material every time, on any host.
    fn generate_key_set(&self, password: &str, salt: &[u8], file_id: &[u8; 16]) -> Result<KeySet>;

    /// Rotate every derived key in a key set onto a new file id.
    ///
    /// `new_file_id` is supplied by the caller for the same reason it is
    /// supplied to [`generate_key_set`](KeyManager::generate_key_set):
    /// the rotated keys are only recoverable if the id they were
    /// expanded from is written somewhere.
    fn rotate_keys(&self, key_set: &mut KeySet, new_file_id: [u8; 16]) -> Result<()>;
}

/// Argon2id memory cost (KiB) — 64 MiB. Paired with [`ROSHERA_KDF_TIME_COST`]
/// this is an OWASP-aligned configuration that derives in ~100 ms.
pub const ROSHERA_KDF_MEMORY_KIB: u32 = 64 * 1024;

/// Argon2id time cost (number of passes over memory).
///
/// This is Argon2's `t_cost`, **not** a PBKDF2 iteration count. With the
/// 64 MiB memory cost above, each pass is expensive; OWASP recommends a
/// `t_cost` of 1–4 for Argon2id, not the 10k–600k typical of PBKDF2.
/// A value in the thousands here means tens of minutes per derivation —
/// the parameters are not interchangeable across the two KDFs.
pub const ROSHERA_KDF_TIME_COST: u32 = 3;

/// Upper bound enforced when an Argon2 `t_cost` is read back from an
/// untrusted file header. A corrupt or hostile header could otherwise
/// request billions of passes and wedge the importer; clamping to this
/// ceiling turns that into a clean decryption failure (the derived key
/// simply won't match) instead of an unbounded hang.
pub const ROSHERA_KDF_TIME_COST_MAX: u32 = 16;

/// FourCCs of every standard .ros chunk type, kept in lockstep with
/// [`crate::chunk::ChunkType`]. A per-chunk key is derived for each so
/// that any standard chunk written to a file can be encrypted. A type
/// missing from this list surfaces at export time as a
/// `Missing encryption key` error — which is exactly how the v3.1
/// `HIST`/`PROV` chunks failed when this list still named the
/// pre-rename `AIPR` and omitted both mandatory chunks.
pub const STANDARD_CHUNK_FOURCCS: [&[u8; 4]; 11] = [
    b"META", b"HIST", b"PROV", b"GEOM", b"TOPO", b"FEAT", b"CONS", b"KEYS", b"BCHN", b"ACLS",
    b"SIGN",
];

/// Default software-only key manager using Argon2 + HKDF
pub struct SoftwareKeyManager {
    pub kdf_iterations: u32,
}

impl SoftwareKeyManager {
    /// Construct a manager pinned to a specific Argon2 `t_cost`, clamped
    /// to `[1, ROSHERA_KDF_TIME_COST_MAX]`. Used on the import path to
    /// reproduce the derivation recorded in a file header without
    /// trusting the header to be benign.
    pub fn with_clamped_time_cost(time_cost: u32) -> Self {
        SoftwareKeyManager {
            kdf_iterations: time_cost.clamp(1, ROSHERA_KDF_TIME_COST_MAX),
        }
    }
}

impl Default for SoftwareKeyManager {
    fn default() -> Self {
        SoftwareKeyManager {
            kdf_iterations: ROSHERA_KDF_TIME_COST,
        }
    }
}

impl KeyManager for SoftwareKeyManager {
    fn derive_master_key(&self, password: &str, salt: &[u8], iterations: u32) -> Result<SecureKey> {
        use argon2::{Algorithm, Argon2, Params, Version};

        let params = Params::new(
            ROSHERA_KDF_MEMORY_KIB, // 64 MiB memory cost
            iterations,             // Argon2 t_cost (passes), NOT a PBKDF2 count
            4,                      // parallelism
            Some(32),               // output length
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

    /// Derive the complete key set from `(password, salt, file_id)`.
    ///
    /// Pure: no randomness enters here. `file_id` is the caller's —
    /// `.ros` passes the header's `file_uuid`, which is on disk and, on
    /// a signed file, inside the signed Merkle leaf set. Until
    /// 2026-08-04 this function drew `file_id = random_16()` itself and
    /// no writer persisted it, so every encrypted `.ros` file ever
    /// written was undecryptable — the importer re-derived from a
    /// different random id and the AES-256-GCM tag rejected.
    fn generate_key_set(&self, password: &str, salt: &[u8], file_id: &[u8; 16]) -> Result<KeySet> {
        // Derive master key
        let master = self.derive_master_key(password, salt, self.kdf_iterations)?;

        // Derive file key from the file's OWN persisted id
        let file_key = self.derive_file_key(&master, file_id)?;

        // Create key set
        let mut key_set = KeySet::new(master, *file_id);
        key_set.file_key = file_key;

        // Derive chunk keys for every standard chunk type.
        for chunk_type in STANDARD_CHUNK_FOURCCS.iter() {
            let chunk_key = self.derive_chunk_key(&key_set.file_key, chunk_type)?;
            key_set.add_chunk_key(**chunk_type, chunk_key);
        }

        Ok(key_set)
    }

    fn rotate_keys(&self, key_set: &mut KeySet, new_file_id: [u8; 16]) -> Result<()> {
        // Derive new file key from the caller's new (persistable) id
        let new_file_key = self.derive_file_key(&key_set.master, &new_file_id)?;

        // Update key set
        key_set.file_id = new_file_id;
        key_set.file_key = new_file_key;

        // Clear old chunk keys
        key_set.chunk_keys.clear();
        key_set.metadata.clear();

        // Derive new chunk keys
        for chunk_type in STANDARD_CHUNK_FOURCCS.iter() {
            let chunk_key = self.derive_chunk_key(&key_set.file_key, chunk_type)?;
            key_set.add_chunk_key(**chunk_type, chunk_key);
        }

        Ok(())
    }
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

        let nonce_bytes = crate::util::random_bytes(12);
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

        let key_set = manager
            .generate_key_set("test_password", &salt, &random_16())
            .unwrap();

        assert_eq!(key_set.chunk_keys.len(), STANDARD_CHUNK_FOURCCS.len());
        assert!(key_set.get_chunk_key(b"GEOM").is_some());
        // v3.1 mandatory chunks must have derived keys (regression: these
        // were absent, so encrypting HIST/PROV failed at export time).
        assert!(key_set.get_chunk_key(b"HIST").is_some());
        assert!(key_set.get_chunk_key(b"PROV").is_some());
        assert!(key_set.get_chunk_key(b"XXXX").is_none());
    }

    #[test]
    fn test_key_rotation() {
        let manager = test_key_manager();
        let salt = random_16();

        let mut key_set = manager
            .generate_key_set("test_password", &salt, &random_16())
            .unwrap();
        let old_file_id = key_set.file_id;
        let old_geom_key = key_set.get_chunk_key(b"GEOM").unwrap().id;

        let new_file_id = random_16();
        manager.rotate_keys(&mut key_set, new_file_id).unwrap();

        assert_eq!(key_set.file_id, new_file_id);
        assert_ne!(key_set.file_id, old_file_id);
        assert_ne!(key_set.get_chunk_key(b"GEOM").unwrap().id, old_geom_key);
        assert_eq!(key_set.chunk_keys.len(), STANDARD_CHUNK_FOURCCS.len());
    }

    /// The whole key set must be a pure function of
    /// `(password, salt, file_id)` — the property the importer's ability
    /// to reopen a file rests on entirely.
    ///
    /// RED before the 2026-08-04 fix: `generate_key_set` drew its own
    /// `file_id = random_16()`, so two calls with identical arguments
    /// produced different file keys and different chunk keys.
    #[test]
    fn key_set_is_reproducible_from_password_salt_and_file_id() {
        let manager = test_key_manager();
        let salt = random_16();
        let file_id = random_16();

        let a = manager
            .generate_key_set("test_password", &salt, &file_id)
            .unwrap();
        let b = manager
            .generate_key_set("test_password", &salt, &file_id)
            .unwrap();

        assert_eq!(a.file_id, file_id, "the key set must adopt the caller's id");
        assert_eq!(a.file_key.material, b.file_key.material);
        for fourcc in STANDARD_CHUNK_FOURCCS.iter() {
            let ka = a.get_chunk_key(fourcc).unwrap();
            let kb = b.get_chunk_key(fourcc).unwrap();
            assert_eq!(
                ka.material,
                kb.material,
                "chunk key {} must reproduce",
                String::from_utf8_lossy(*fourcc)
            );
        }
    }

    /// Key separation, both ways: neither the salt nor the file id may be
    /// dropped from the chain. Same password + same salt but a different
    /// file id must still yield different chunk keys, and so must same
    /// password + same file id with a different salt.
    #[test]
    fn key_separation_holds_across_both_file_id_and_salt() {
        let manager = test_key_manager();
        let salt = random_16();
        let other_salt = random_16();
        let file_id = random_16();
        let other_file_id = random_16();

        let base = manager
            .generate_key_set("test_password", &salt, &file_id)
            .unwrap();
        let other_file = manager
            .generate_key_set("test_password", &salt, &other_file_id)
            .unwrap();
        let other_salt_set = manager
            .generate_key_set("test_password", &other_salt, &file_id)
            .unwrap();

        assert_ne!(base.file_key.material, other_file.file_key.material);
        assert_ne!(base.file_key.material, other_salt_set.file_key.material);
        for fourcc in STANDARD_CHUNK_FOURCCS.iter() {
            let k = base.get_chunk_key(fourcc).unwrap();
            assert_ne!(
                k.material,
                other_file.get_chunk_key(fourcc).unwrap().material,
                "file_id must separate chunk key {}",
                String::from_utf8_lossy(*fourcc)
            );
            assert_ne!(
                k.material,
                other_salt_set.get_chunk_key(fourcc).unwrap().material,
                "kdf_salt must separate chunk key {}",
                String::from_utf8_lossy(*fourcc)
            );
        }
    }

    /// A wrong password must not merely differ "somewhere" — every chunk
    /// key must differ, so no chunk of a file decrypts under it.
    #[test]
    fn a_different_password_changes_every_chunk_key() {
        let manager = test_key_manager();
        let salt = random_16();
        let file_id = random_16();

        let right = manager
            .generate_key_set("correct-horse-battery-staple", &salt, &file_id)
            .unwrap();
        let wrong = manager
            .generate_key_set("correct-horse-battery-stapl3", &salt, &file_id)
            .unwrap();

        assert_ne!(right.master.material, wrong.master.material);
        for fourcc in STANDARD_CHUNK_FOURCCS.iter() {
            assert_ne!(
                right.get_chunk_key(fourcc).unwrap().material,
                wrong.get_chunk_key(fourcc).unwrap().material
            );
        }
    }

    #[test]
    fn kdf_algo_ids_name_the_binding_not_just_the_hash() {
        assert_eq!(KdfAlgo::from_u8(2).unwrap(), KdfAlgo::Argon2);
        assert_eq!(KdfAlgo::from_u8(3).unwrap(), KdfAlgo::Argon2idFileBound);
        assert!(KdfAlgo::from_u8(4).is_err());
        assert_eq!(KDF_ALGO_ARGON2ID_FILE_BOUND, 3);
        assert_eq!(KDF_ALGO_ARGON2ID_UNBOUND, 2);
        assert_eq!(KdfAlgo::Argon2idFileBound.as_u8(), 3);
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
        let key_set = manager
            .generate_key_set("test_password", &salt, &random_16())
            .unwrap();

        // Escrow
        let escrowed = escrow_service.escrow_key_set(&key_set).unwrap();
        assert!(escrowed.len() > 12); // At least nonce + some data

        // Recover
        let recovered = escrow_service.recover_key_set(&escrowed).unwrap();
        assert_eq!(recovered.file_id, key_set.file_id);
        assert_eq!(recovered.chunk_keys.len(), key_set.chunk_keys.len());
    }
}
