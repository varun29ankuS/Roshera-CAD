// src/signature.rs

//! Digital Signature & Verification (.ros v3 SIGN chunk)
//!
//! One Ed25519 signature per file, full stop. M-of-N multisig and
//! ECDSA were scaffolding for enterprise-CAD scenarios Roshera does
//! not target; the file format reserves the on-disk SIGN chunk shape
//! but the public API now signs and verifies a single record.

use crate::util::{current_time_ms, sha256, to_hex};
use crate::{Result, SignatureError};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Supported signature algorithms.
///
/// Ed25519 is the only signing algorithm Roshera accepts. The `None`
/// variant exists so an unsigned file can be represented in the on-disk
/// header byte (`signature_algo = 0`) without forcing an `Option<…>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignatureAlgorithm {
    None = 0,
    Ed25519 = 1,
}

/// File signature metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSignatureMetadata {
    pub file_id: [u8; 16],
    pub timestamp: u64,
    pub signer_id: [u8; 16],
    pub signature_version: u32,
}

impl FileSignatureMetadata {
    pub fn new(file_id: [u8; 16], signer_id: [u8; 16]) -> Self {
        FileSignatureMetadata {
            file_id,
            timestamp: current_time_ms(),
            signer_id,
            signature_version: 1,
        }
    }
}

/// A single signature record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureRecord {
    pub algorithm: SignatureAlgorithm,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
    pub metadata: FileSignatureMetadata,
}

/// The .ros v3 SIGN chunk — exactly one Ed25519 signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureChunk {
    pub signer: SignatureRecord,
}

impl SignatureChunk {
    pub fn new(signer: SignatureRecord) -> Self {
        SignatureChunk { signer }
    }

    /// Serialize to bytes
    pub fn serialize(&self) -> Vec<u8> {
        rmp_serde::to_vec_named(self).unwrap_or_default()
    }

    /// Deserialize from bytes
    pub fn deserialize(data: &[u8]) -> Result<Self> {
        rmp_serde::from_slice(data).map_err(|e| {
            SignatureError::InvalidSignature {
                signer: "unknown".to_string(),
                reason: e.to_string(),
            }
            .into()
        })
    }
}

/// File signer
pub struct FileSigner {
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
    signer_id: [u8; 16],
}

impl FileSigner {
    pub fn new(signing_key: SigningKey, signer_id: [u8; 16]) -> Self {
        let verifying_key = signing_key.verifying_key();
        FileSigner {
            signing_key,
            verifying_key,
            signer_id,
        }
    }

    pub fn from_bytes(key_bytes: &[u8; 32], signer_id: [u8; 16]) -> Result<Self> {
        let signing_key = SigningKey::from_bytes(key_bytes);
        Ok(FileSigner::new(signing_key, signer_id))
    }

    /// Sign file data
    pub fn sign_file(&self, file_data: &[u8], file_id: [u8; 16]) -> Result<SignatureRecord> {
        // Sign the hash instead of full data for efficiency
        let data_hash = sha256(file_data);
        let signature = self.signing_key.sign(&data_hash);

        Ok(SignatureRecord {
            algorithm: SignatureAlgorithm::Ed25519,
            public_key: self.verifying_key.as_bytes().to_vec(),
            signature: signature.to_bytes().to_vec(),
            metadata: FileSignatureMetadata::new(file_id, self.signer_id),
        })
    }

    pub fn verifying_key_bytes(&self) -> [u8; 32] {
        self.verifying_key.to_bytes()
    }
}

/// Signature verifier
pub struct SignatureVerifier;

impl SignatureVerifier {
    /// Verify a single signature record against file data.
    pub fn verify_signature(file_data: &[u8], record: &SignatureRecord) -> Result<bool> {
        if record.algorithm != SignatureAlgorithm::Ed25519 {
            return Err(SignatureError::InvalidSignature {
                signer: "unknown".to_string(),
                reason: "Only Ed25519 supported".to_string(),
            }
            .into());
        }

        // Verify key and signature lengths
        if record.public_key.len() != 32 || record.signature.len() != 64 {
            return Ok(false);
        }

        let mut pk_bytes = [0u8; 32];
        pk_bytes.copy_from_slice(&record.public_key);

        let verifying_key =
            VerifyingKey::from_bytes(&pk_bytes).map_err(|_| SignatureError::InvalidSignature {
                signer: to_hex(&record.metadata.signer_id),
                reason: "Invalid public key".to_string(),
            })?;

        let signature = Signature::from_slice(&record.signature).map_err(|_| {
            SignatureError::InvalidSignature {
                signer: to_hex(&record.metadata.signer_id),
                reason: "Invalid signature format".to_string(),
            }
        })?;

        // Verify against hash
        let data_hash = sha256(file_data);
        Ok(verifying_key.verify(&data_hash, &signature).is_ok())
    }

    /// Verify the single Ed25519 signature carried by a SIGN chunk.
    pub fn verify_chunk(file_data: &[u8], chunk: &SignatureChunk) -> Result<bool> {
        Self::verify_signature(file_data, &chunk.signer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    #[test]
    fn test_sign_and_verify() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let signer = FileSigner::new(signing_key, [1u8; 16]);

        let data = b"Test file data";
        let file_id = [42u8; 16];

        let record = signer.sign_file(data, file_id).unwrap();
        assert!(SignatureVerifier::verify_signature(data, &record).unwrap());

        // Wrong data should fail
        assert!(!SignatureVerifier::verify_signature(b"Wrong data", &record).unwrap());
    }

    #[test]
    fn test_chunk_round_trip() {
        let signer = FileSigner::new(SigningKey::generate(&mut OsRng), [7u8; 16]);
        let data = b"chunk payload";
        let file_id = [11u8; 16];

        let record = signer.sign_file(data, file_id).unwrap();
        let chunk = SignatureChunk::new(record);

        let serialized = chunk.serialize();
        let deserialized = SignatureChunk::deserialize(&serialized).unwrap();

        assert!(SignatureVerifier::verify_chunk(data, &deserialized).unwrap());
        assert!(!SignatureVerifier::verify_chunk(b"tampered", &deserialized).unwrap());
    }
}
