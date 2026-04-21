// src/encryption.rs

//! Encryption & Decryption for .ros v3 Chunks
//!
//! - AES-256-GCM, ChaCha20-Poly1305
//! - Streaming support for large files
//! - Per-chunk and file-level encryption
//! - Authenticated encryption with additional data (AEAD)

// Add this line to bring the trait into scope
use crate::ros_fs::keys::{KeySet, SecureKey};
use crate::ros_fs::util::constant_time_eq;
use crate::ros_fs::{EncryptionError, Result};
use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce as AesNonce,
};
use chacha20poly1305::{ChaCha20Poly1305, Nonce as ChaNonce};
use std::io::{Read, Write};

/// Supported encryption algorithms (.ros v3)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionAlgorithm {
    None,
    AES256GCM,
    ChaCha20Poly1305,
}

impl EncryptionAlgorithm {
    pub fn from_id(id: u8) -> Result<Self> {
        match id {
            0 => Ok(EncryptionAlgorithm::None),
            1 => Ok(EncryptionAlgorithm::AES256GCM),
            2 => Ok(EncryptionAlgorithm::ChaCha20Poly1305),
            _ => Err(EncryptionError::UnsupportedAlgorithm {
                algorithm: format!("Unknown ID: {}", id),
                supported: vec![
                    "None(0)".to_string(),
                    "AES256GCM(1)".to_string(),
                    "ChaCha20Poly1305(2)".to_string(),
                ],
            }
            .into()),
        }
    }

    pub fn as_id(&self) -> u8 {
        match self {
            EncryptionAlgorithm::None => 0,
            EncryptionAlgorithm::AES256GCM => 1,
            EncryptionAlgorithm::ChaCha20Poly1305 => 2,
        }
    }

    pub fn nonce_size(&self) -> usize {
        match self {
            EncryptionAlgorithm::None => 0,
            EncryptionAlgorithm::AES256GCM => 12,
            EncryptionAlgorithm::ChaCha20Poly1305 => 12,
        }
    }

    pub fn tag_size(&self) -> usize {
        match self {
            EncryptionAlgorithm::None => 0,
            EncryptionAlgorithm::AES256GCM => 16,
            EncryptionAlgorithm::ChaCha20Poly1305 => 16,
        }
    }
}

/// Encryption context for chunks
pub struct ChunkEncryptor {
    pub algorithm: EncryptionAlgorithm,
    pub key_set: KeySet,
    pub file_iv: [u8; 8], // File-level IV for deterministic chunk IVs
}

impl ChunkEncryptor {
    /// Create a new encryptor
    pub fn new(algorithm: EncryptionAlgorithm, key_set: KeySet, file_iv: [u8; 8]) -> Self {
        ChunkEncryptor {
            algorithm,
            key_set,
            file_iv,
        }
    }

    /// Encrypts a chunk with authenticated encryption
    pub fn encrypt_chunk(
        &self,
        chunk_type: &[u8; 4],
        data: &[u8],
        chunk_index: u32,
        additional_data: Option<&[u8]>,
    ) -> Result<Vec<u8>> {
        if self.algorithm == EncryptionAlgorithm::None {
            return Ok(data.to_vec());
        }

        // Get chunk key
        let key =
            self.key_set
                .get_chunk_key(chunk_type)
                .ok_or_else(|| EncryptionError::MissingKey {
                    key_id: format!("chunk:{}", String::from_utf8_lossy(chunk_type)),
                })?;

        // Generate deterministic IV
        let iv = Self::generate_chunk_iv(&self.file_iv, chunk_type, chunk_index);

        // Encrypt based on algorithm
        match self.algorithm {
            EncryptionAlgorithm::AES256GCM => {
                self.encrypt_aes_gcm(&key.material, &iv, data, additional_data)
            }
            EncryptionAlgorithm::ChaCha20Poly1305 => {
                self.encrypt_chacha20(&key.material, &iv, data, additional_data)
            }
            EncryptionAlgorithm::None => unreachable!(),
        }
    }

    /// Decrypts an encrypted chunk
    pub fn decrypt_chunk(
        &self,
        chunk_type: &[u8; 4],
        encrypted_data: &[u8],
        chunk_index: u32,
        additional_data: Option<&[u8]>,
    ) -> Result<Vec<u8>> {
        if self.algorithm == EncryptionAlgorithm::None {
            return Ok(encrypted_data.to_vec());
        }

        // Verify we have enough data for tag
        let tag_size = self.algorithm.tag_size();
        if encrypted_data.len() < tag_size {
            return Err(EncryptionError::CorruptedData {
                expected_tag: format!("{} bytes minimum", tag_size),
                actual_tag: format!("{} bytes", encrypted_data.len()),
            }
            .into());
        }

        // Get chunk key
        let key =
            self.key_set
                .get_chunk_key(chunk_type)
                .ok_or_else(|| EncryptionError::MissingKey {
                    key_id: format!("chunk:{}", String::from_utf8_lossy(chunk_type)),
                })?;

        // Generate deterministic IV
        let iv = Self::generate_chunk_iv(&self.file_iv, chunk_type, chunk_index);

        // Decrypt based on algorithm
        match self.algorithm {
            EncryptionAlgorithm::AES256GCM => {
                self.decrypt_aes_gcm(&key.material, &iv, encrypted_data, additional_data)
            }
            EncryptionAlgorithm::ChaCha20Poly1305 => {
                self.decrypt_chacha20(&key.material, &iv, encrypted_data, additional_data)
            }
            EncryptionAlgorithm::None => unreachable!(),
        }
    }

    /// Encrypt with AES-256-GCM
    fn encrypt_aes_gcm(
        &self,
        key: &[u8],
        iv: &[u8],
        data: &[u8],
        aad: Option<&[u8]>,
    ) -> Result<Vec<u8>> {
        let cipher =
            Aes256Gcm::new_from_slice(key).map_err(|_| EncryptionError::EncryptionFailed {
                algorithm: "AES-256-GCM".to_string(),
                details: "Invalid key length".to_string(),
            })?;

        let nonce = AesNonce::from_slice(iv);

        let payload = match aad {
            Some(aad_data) => Payload {
                msg: data,
                aad: aad_data,
            },
            None => Payload::from(data),
        };

        cipher.encrypt(nonce, payload).map_err(|_| {
            EncryptionError::EncryptionFailed {
                algorithm: "AES-256-GCM".to_string(),
                details: "Encryption operation failed".to_string(),
            }
            .into()
        })
    }

    /// Decrypt with AES-256-GCM
    fn decrypt_aes_gcm(
        &self,
        key: &[u8],
        iv: &[u8],
        encrypted_data: &[u8],
        aad: Option<&[u8]>,
    ) -> Result<Vec<u8>> {
        let cipher =
            Aes256Gcm::new_from_slice(key).map_err(|_| EncryptionError::DecryptionFailed {
                algorithm: "AES-256-GCM".to_string(),
                details: "Invalid key length".to_string(),
            })?;

        let nonce = AesNonce::from_slice(iv);

        let payload = match aad {
            Some(aad_data) => Payload {
                msg: encrypted_data,
                aad: aad_data,
            },
            None => Payload::from(encrypted_data),
        };

        cipher.decrypt(nonce, payload).map_err(|_| {
            EncryptionError::DecryptionFailed {
                algorithm: "AES-256-GCM".to_string(),
                details: "Decryption failed - invalid tag or corrupted data".to_string(),
            }
            .into()
        })
    }

    /// Encrypt with ChaCha20-Poly1305
    fn encrypt_chacha20(
        &self,
        key: &[u8],
        iv: &[u8],
        data: &[u8],
        aad: Option<&[u8]>,
    ) -> Result<Vec<u8>> {
        let cipher = ChaCha20Poly1305::new_from_slice(key).map_err(|_| {
            EncryptionError::EncryptionFailed {
                algorithm: "ChaCha20-Poly1305".to_string(),
                details: "Invalid key length".to_string(),
            }
        })?;

        let nonce = ChaNonce::from_slice(iv);

        let payload = match aad {
            Some(aad_data) => Payload {
                msg: data,
                aad: aad_data,
            },
            None => Payload::from(data),
        };

        cipher.encrypt(nonce, payload).map_err(|_| {
            EncryptionError::EncryptionFailed {
                algorithm: "ChaCha20-Poly1305".to_string(),
                details: "Encryption operation failed".to_string(),
            }
            .into()
        })
    }

    /// Decrypt with ChaCha20-Poly1305
    fn decrypt_chacha20(
        &self,
        key: &[u8],
        iv: &[u8],
        encrypted_data: &[u8],
        aad: Option<&[u8]>,
    ) -> Result<Vec<u8>> {
        let cipher = ChaCha20Poly1305::new_from_slice(key).map_err(|_| {
            EncryptionError::DecryptionFailed {
                algorithm: "ChaCha20-Poly1305".to_string(),
                details: "Invalid key length".to_string(),
            }
        })?;

        let nonce = ChaNonce::from_slice(iv);

        let payload = match aad {
            Some(aad_data) => Payload {
                msg: encrypted_data,
                aad: aad_data,
            },
            None => Payload::from(encrypted_data),
        };

        cipher.decrypt(nonce, payload).map_err(|_| {
            EncryptionError::DecryptionFailed {
                algorithm: "ChaCha20-Poly1305".to_string(),
                details: "Decryption failed - invalid tag or corrupted data".to_string(),
            }
            .into()
        })
    }

    /// Generate deterministic IV for a chunk
    pub fn generate_chunk_iv(
        file_iv: &[u8; 8],
        chunk_type: &[u8; 4],
        chunk_index: u32,
    ) -> [u8; 12] {
        let mut iv = [0u8; 12];

        // File IV (8 bytes)
        iv[..8].copy_from_slice(file_iv);

        // Chunk type (4 bytes) XORed with chunk index
        iv[8] = chunk_type[0] ^ ((chunk_index >> 24) as u8);
        iv[9] = chunk_type[1] ^ ((chunk_index >> 16) as u8);
        iv[10] = chunk_type[2] ^ ((chunk_index >> 8) as u8);
        iv[11] = chunk_type[3] ^ (chunk_index as u8);

        iv
    }
}

/// Streaming encryption/decryption for large chunks
pub struct StreamingEncryptor<W: Write> {
    algorithm: EncryptionAlgorithm,
    writer: W,
    buffer: Vec<u8>,
    buffer_size: usize,
    key: SecureKey,
    iv: [u8; 12],
    chunk_index: u32,
}

impl<W: Write> StreamingEncryptor<W> {
    /// Create a new streaming encryptor
    pub fn new(
        algorithm: EncryptionAlgorithm,
        writer: W,
        key: SecureKey,
        iv: [u8; 12],
        buffer_size: usize,
    ) -> Self {
        StreamingEncryptor {
            algorithm,
            writer,
            buffer: Vec::with_capacity(buffer_size),
            buffer_size,
            key,
            iv,
            chunk_index: 0,
        }
    }

    /// Process a chunk of data
    pub fn update(&mut self, data: &[u8]) -> Result<()> {
        self.buffer.extend_from_slice(data);

        // Process full blocks
        while self.buffer.len() >= self.buffer_size {
            let block = self.buffer.drain(..self.buffer_size).collect::<Vec<_>>();
            self.encrypt_block(&block)?;
        }

        Ok(())
    }

    /// Finalize encryption
    pub fn finalize(mut self) -> Result<W> {
        // Process remaining data
        if !self.buffer.is_empty() {
            let block = self.buffer.clone();
            self.encrypt_block(&block)?;
        }

        Ok(self.writer)
    }

    fn encrypt_block(&mut self, block: &[u8]) -> Result<()> {
        // Create sub-IV for this block
        let mut block_iv = self.iv;
        block_iv[8] ^= (self.chunk_index >> 24) as u8;
        block_iv[9] ^= (self.chunk_index >> 16) as u8;
        block_iv[10] ^= (self.chunk_index >> 8) as u8;
        block_iv[11] ^= self.chunk_index as u8;

        let encrypted = match self.algorithm {
            EncryptionAlgorithm::AES256GCM => {
                let cipher = Aes256Gcm::new_from_slice(&self.key.material).map_err(|_| {
                    EncryptionError::EncryptionFailed {
                        algorithm: "AES-256-GCM".to_string(),
                        details: "Invalid key".to_string(),
                    }
                })?;

                let nonce = AesNonce::from_slice(&block_iv);
                cipher
                    .encrypt(nonce, block)
                    .map_err(|_| EncryptionError::EncryptionFailed {
                        algorithm: "AES-256-GCM".to_string(),
                        details: "Block encryption failed".to_string(),
                    })?
            }
            EncryptionAlgorithm::ChaCha20Poly1305 => {
                let cipher =
                    ChaCha20Poly1305::new_from_slice(&self.key.material).map_err(|_| {
                        EncryptionError::EncryptionFailed {
                            algorithm: "ChaCha20-Poly1305".to_string(),
                            details: "Invalid key".to_string(),
                        }
                    })?;

                let nonce = ChaNonce::from_slice(&block_iv);
                cipher
                    .encrypt(nonce, block)
                    .map_err(|_| EncryptionError::EncryptionFailed {
                        algorithm: "ChaCha20-Poly1305".to_string(),
                        details: "Block encryption failed".to_string(),
                    })?
            }
            EncryptionAlgorithm::None => block.to_vec(),
        };

        // Write block size (4 bytes) + encrypted data
        self.writer
            .write_all(&(encrypted.len() as u32).to_le_bytes())?;
        self.writer.write_all(&encrypted)?;

        self.chunk_index += 1;
        Ok(())
    }
}

/// Streaming decryption for large chunks
pub struct StreamingDecryptor<R: Read> {
    algorithm: EncryptionAlgorithm,
    reader: R,
    key: SecureKey,
    iv: [u8; 12],
    chunk_index: u32,
}

impl<R: Read> StreamingDecryptor<R> {
    /// Create a new streaming decryptor
    pub fn new(algorithm: EncryptionAlgorithm, reader: R, key: SecureKey, iv: [u8; 12]) -> Self {
        StreamingDecryptor {
            algorithm,
            reader,
            key,
            iv,
            chunk_index: 0,
        }
    }

    /// Read and decrypt the next block
    pub fn read_block(&mut self) -> Result<Option<Vec<u8>>> {
        // Read block size
        let mut size_buf = [0u8; 4];
        match self.reader.read_exact(&mut size_buf) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }

        let block_size = u32::from_le_bytes(size_buf) as usize;
        if block_size > 1024 * 1024 {
            // 1MB max block size
            return Err(EncryptionError::CorruptedData {
                expected_tag: "Block size <= 1MB".to_string(),
                actual_tag: format!("{} bytes", block_size),
            }
            .into());
        }

        // Read encrypted block
        let mut encrypted = vec![0u8; block_size];
        self.reader.read_exact(&mut encrypted)?;

        // Create sub-IV for this block
        let mut block_iv = self.iv;
        block_iv[8] ^= (self.chunk_index >> 24) as u8;
        block_iv[9] ^= (self.chunk_index >> 16) as u8;
        block_iv[10] ^= (self.chunk_index >> 8) as u8;
        block_iv[11] ^= self.chunk_index as u8;

        let decrypted = match self.algorithm {
            EncryptionAlgorithm::AES256GCM => {
                let cipher = Aes256Gcm::new_from_slice(&self.key.material).map_err(|_| {
                    EncryptionError::DecryptionFailed {
                        algorithm: "AES-256-GCM".to_string(),
                        details: "Invalid key".to_string(),
                    }
                })?;

                let nonce = AesNonce::from_slice(&block_iv);
                cipher.decrypt(nonce, encrypted.as_ref()).map_err(|_| {
                    EncryptionError::DecryptionFailed {
                        algorithm: "AES-256-GCM".to_string(),
                        details: "Block decryption failed".to_string(),
                    }
                })?
            }
            EncryptionAlgorithm::ChaCha20Poly1305 => {
                let cipher =
                    ChaCha20Poly1305::new_from_slice(&self.key.material).map_err(|_| {
                        EncryptionError::DecryptionFailed {
                            algorithm: "ChaCha20-Poly1305".to_string(),
                            details: "Invalid key".to_string(),
                        }
                    })?;

                let nonce = ChaNonce::from_slice(&block_iv);
                cipher.decrypt(nonce, encrypted.as_ref()).map_err(|_| {
                    EncryptionError::DecryptionFailed {
                        algorithm: "ChaCha20-Poly1305".to_string(),
                        details: "Block decryption failed".to_string(),
                    }
                })?
            }
            EncryptionAlgorithm::None => encrypted,
        };

        self.chunk_index += 1;
        Ok(Some(decrypted))
    }

    /// Read all blocks into a single buffer
    pub fn read_all(mut self) -> Result<Vec<u8>> {
        let mut result = Vec::new();

        loop {
            match self.read_block()? {
                Some(block) => {
                    let block_data: Vec<u8> = block;
                    result.extend_from_slice(&block_data);
                }
                None => break,
            }
        }

        Ok(result)
    }
}

/// Verify encryption tag without decrypting (for access control)
pub fn verify_auth_tag(
    algorithm: EncryptionAlgorithm,
    _key: &[u8],
    _iv: &[u8],
    ciphertext: &[u8],
    expected_tag: &[u8],
    _aad: Option<&[u8]>,
) -> Result<bool> {
    if algorithm == EncryptionAlgorithm::None {
        return Ok(true);
    }

    let tag_size = algorithm.tag_size();
    if ciphertext.len() < tag_size {
        return Ok(false);
    }

    // Extract actual tag from ciphertext
    let actual_tag = &ciphertext[ciphertext.len() - tag_size..];

    // Constant-time comparison
    Ok(constant_time_eq(actual_tag, expected_tag))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ros_fs::keys::SoftwareKeyManager;
    use crate::ros_fs::util::random_16;

    // Helper for fast key generation in tests
    fn test_key_manager() -> SoftwareKeyManager {
        SoftwareKeyManager {
            kdf_iterations: 4, // Much faster for tests
        }
    }

    #[test]
    fn test_chunk_encryption_roundtrip() {
        let manager = test_key_manager();
        let key_set = manager
            .generate_key_set("test_password", &random_16())
            .unwrap();
        let file_iv = [1, 2, 3, 4, 5, 6, 7, 8];

        let encryptor = ChunkEncryptor::new(EncryptionAlgorithm::AES256GCM, key_set, file_iv);

        let data = b"Secret geometry data";
        let chunk_type = b"GEOM";
        let aad = b"chunk_metadata";

        // Encrypt
        let encrypted = encryptor
            .encrypt_chunk(chunk_type, data, 1, Some(aad))
            .unwrap();
        assert_ne!(&encrypted, data);
        assert!(encrypted.len() > data.len()); // Should have auth tag

        // Decrypt
        let decrypted = encryptor
            .decrypt_chunk(chunk_type, &encrypted, 1, Some(aad))
            .unwrap();
        assert_eq!(&decrypted, data);

        // Wrong AAD should fail
        let result = encryptor.decrypt_chunk(chunk_type, &encrypted, 1, Some(b"wrong_aad"));
        assert!(result.is_err());
    }

    #[test]
    fn test_streaming_encryption() {
        let manager = test_key_manager();
        let key_set = manager
            .generate_key_set("test_password", &random_16())
            .unwrap();
        let key = key_set.get_chunk_key(b"GEOM").unwrap().clone();
        let iv = [1u8; 12];

        // Encrypt streaming
        let mut encrypted_buffer = Vec::new();
        {
            let mut encryptor = StreamingEncryptor::new(
                EncryptionAlgorithm::AES256GCM,
                &mut encrypted_buffer,
                key.clone(),
                iv,
                1024, // 1KB blocks
            );

            // Write data in chunks
            encryptor.update(b"First chunk of data").unwrap();
            encryptor.update(b" Second chunk").unwrap();
            encryptor.update(b" Final chunk!").unwrap();
            encryptor.finalize().unwrap();
        }

        // Decrypt streaming
        let cursor = Cursor::new(encrypted_buffer);
        let decryptor = StreamingDecryptor::new(EncryptionAlgorithm::AES256GCM, cursor, key, iv);

        let decrypted = decryptor.read_all().unwrap();
        assert_eq!(&decrypted, b"First chunk of data Second chunk Final chunk!");
    }

    #[test]
    fn test_deterministic_iv_generation() {
        let file_iv = [1, 2, 3, 4, 5, 6, 7, 8];
        let chunk_type = b"GEOM";

        let iv1 = ChunkEncryptor::generate_chunk_iv(&file_iv, chunk_type, 0);
        let iv2 = ChunkEncryptor::generate_chunk_iv(&file_iv, chunk_type, 0);
        assert_eq!(iv1, iv2); // Same inputs = same IV

        let iv3 = ChunkEncryptor::generate_chunk_iv(&file_iv, chunk_type, 1);
        assert_ne!(iv1, iv3); // Different index = different IV

        let iv4 = ChunkEncryptor::generate_chunk_iv(&file_iv, b"TOPO", 0);
        assert_ne!(iv1, iv4); // Different chunk type = different IV
    }

    #[test]
    fn test_algorithm_properties() {
        assert_eq!(EncryptionAlgorithm::AES256GCM.nonce_size(), 12);
        assert_eq!(EncryptionAlgorithm::AES256GCM.tag_size(), 16);
        assert_eq!(EncryptionAlgorithm::ChaCha20Poly1305.nonce_size(), 12);
        assert_eq!(EncryptionAlgorithm::ChaCha20Poly1305.tag_size(), 16);
        assert_eq!(EncryptionAlgorithm::None.nonce_size(), 0);
        assert_eq!(EncryptionAlgorithm::None.tag_size(), 0);
    }

    #[test]
    fn test_verify_auth_tag() {
        let key = random_bytes(32);
        let iv = random_bytes(12);
        let data = b"test data";

        // Encrypt to get ciphertext with tag
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let nonce = AesNonce::from_slice(&iv);
        let ciphertext = cipher.encrypt(nonce, data.as_ref()).unwrap();

        // Extract tag (last 16 bytes)
        let tag = &ciphertext[ciphertext.len() - 16..];

        // Verify should succeed
        assert!(verify_auth_tag(
            EncryptionAlgorithm::AES256GCM,
            &key,
            &iv,
            &ciphertext,
            tag,
            None
        )
        .unwrap());

        // Wrong tag should fail
        let mut wrong_tag = tag.to_vec();
        wrong_tag[0] ^= 1;
        assert!(!verify_auth_tag(
            EncryptionAlgorithm::AES256GCM,
            &key,
            &iv,
            &ciphertext,
            &wrong_tag,
            None
        )
        .unwrap());
    }

    #[test]
    fn test_error_handling() {
        let manager = test_key_manager();
        let key_set = manager
            .generate_key_set("test_password", &random_16())
            .unwrap();
        let file_iv = [1, 2, 3, 4, 5, 6, 7, 8];

        let encryptor = ChunkEncryptor::new(EncryptionAlgorithm::AES256GCM, key_set, file_iv);

        // Try to encrypt chunk without key
        let result = encryptor.encrypt_chunk(b"XXXX", b"data", 0, None);
        assert!(matches!(
            result,
            Err(crate::ros_fs::RosFileError::Encryption(
                EncryptionError::MissingKey { .. }
            ))
        ));

        // Try to decrypt too-short data
        let result = encryptor.decrypt_chunk(b"GEOM", b"short", 0, None);
        assert!(matches!(
            result,
            Err(crate::ros_fs::RosFileError::Encryption(
                EncryptionError::CorruptedData { .. }
            ))
        ));
    }
}
