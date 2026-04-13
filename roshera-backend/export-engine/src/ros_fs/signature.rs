// src/signature.rs

//! Digital Signature & Verification (.ros v3 SIGN chunk)
//!
//! Core functionality for file signing with Ed25519 and multi-signature support

use crate::ros_fs::util::{current_time_ms, sha256, to_hex};
use crate::ros_fs::{Result, SignatureError};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Supported signature algorithms
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignatureAlgorithm {
    None = 0,
    Ed25519 = 1,
    Ecdsa = 2,
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
    pub certificate_hash: Option<[u8; 32]>, // SHA-256 of certificate if present
}

/// The .ros v3 SIGN chunk
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureChunk {
    pub signers: Vec<SignatureRecord>,
    pub multisig_threshold: Option<u8>, // M-of-N threshold
}

impl SignatureChunk {
    pub fn new() -> Self {
        SignatureChunk {
            signers: Vec::new(),
            multisig_threshold: None,
        }
    }

    pub fn add_signature(&mut self, record: SignatureRecord) {
        self.signers.push(record);
    }

    pub fn set_threshold(&mut self, threshold: u8) {
        self.multisig_threshold = Some(threshold);
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
            certificate_hash: None,
        })
    }

    pub fn verifying_key_bytes(&self) -> [u8; 32] {
        self.verifying_key.to_bytes()
    }
}

/// Signature verifier
pub struct SignatureVerifier;

impl SignatureVerifier {
    /// Verify a single signature
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

    /// Verify all signatures in a chunk
    pub fn verify_chunk(file_data: &[u8], chunk: &SignatureChunk) -> Result<(usize, bool)> {
        let mut valid_count = 0;

        for record in &chunk.signers {
            if Self::verify_signature(file_data, record)? {
                valid_count += 1;
            }
        }

        let threshold_met = match chunk.multisig_threshold {
            Some(threshold) => valid_count >= threshold as usize,
            None => valid_count == chunk.signers.len(),
        };

        Ok((valid_count, threshold_met))
    }
}

/// Multi-signature builder
pub struct MultiSigBuilder {
    chunk: SignatureChunk,
}

impl MultiSigBuilder {
    pub fn new(threshold: Option<u8>) -> Self {
        let mut chunk = SignatureChunk::new();
        chunk.multisig_threshold = threshold;
        MultiSigBuilder { chunk }
    }

    pub fn add_signer(
        mut self,
        signer: &FileSigner,
        file_data: &[u8],
        file_id: [u8; 16],
    ) -> Result<Self> {
        let record = signer.sign_file(file_data, file_id)?;
        self.chunk.add_signature(record);
        Ok(self)
    }

    pub fn build(self) -> SignatureChunk {
        self.chunk
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
    fn test_multisig() {
        let signer1 = FileSigner::new(SigningKey::generate(&mut OsRng), [1u8; 16]);
        let signer2 = FileSigner::new(SigningKey::generate(&mut OsRng), [2u8; 16]);

        let data = b"Multi-signed file";
        let file_id = [99u8; 16];

        // Build 2-of-2 multisig
        let chunk = MultiSigBuilder::new(Some(2))
            .add_signer(&signer1, data, file_id)
            .unwrap()
            .add_signer(&signer2, data, file_id)
            .unwrap()
            .build();

        let (valid, threshold_met) = SignatureVerifier::verify_chunk(data, &chunk).unwrap();
        assert_eq!(valid, 2);
        assert!(threshold_met);
    }

    #[test]
    fn test_serialization() {
        let chunk = SignatureChunk {
            signers: vec![],
            multisig_threshold: Some(3),
        };

        let serialized = chunk.serialize();
        let deserialized = SignatureChunk::deserialize(&serialized).unwrap();

        assert_eq!(deserialized.multisig_threshold, Some(3));
    }
}
