// src/util.rs

//! Miscellaneous Utilities for Roshera FS
//!
//! - Cryptographic hashing (SHA-256, SHA-512)
//! - Secure memory operations
//! - Time utilities
//! - Random generation with proper entropy
//! - Constant-time comparisons

use lazy_static::lazy_static;
use ring::rand::{SecureRandom, SystemRandom};
use sha2::{Digest, Sha256, Sha512};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

/// Compute SHA-256 hash of data
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&result);
    arr
}

/// Compute SHA-512 hash of data
pub fn sha512(data: &[u8]) -> [u8; 64] {
    let mut hasher = Sha512::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut arr = [0u8; 64];
    arr.copy_from_slice(&result);
    arr
}

/// Compute HMAC-SHA256
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    use ring::hmac;
    let signing_key = hmac::Key::new(hmac::HMAC_SHA256, key);
    let tag = hmac::sign(&signing_key, data);
    let mut arr = [0u8; 32];
    arr.copy_from_slice(tag.as_ref());
    arr
}

/// Get current Unix time in milliseconds
pub fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Get current Unix time in seconds
pub fn current_time_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Convert milliseconds to SystemTime
pub fn ms_to_system_time(ms: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms)
}

/// Securely zero a mutable buffer
pub fn secure_zero(buf: &mut [u8]) {
    buf.zeroize();
}

/// Secure string that zeros on drop
pub struct SecureString(String);

impl SecureString {
    pub fn new(s: String) -> Self {
        SecureString(s)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Drop for SecureString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl Zeroize for SecureString {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

/// Cryptographically secure random number generator
pub struct SecureRng {
    rng: SystemRandom,
}

impl SecureRng {
    pub fn new() -> Self {
        SecureRng {
            rng: SystemRandom::new(),
        }
    }

    /// Generate random bytes
    pub fn random_bytes(&self, len: usize) -> Result<Vec<u8>, ring::error::Unspecified> {
        let mut buf = vec![0u8; len];
        self.rng.fill(&mut buf)?;
        Ok(buf)
    }

    /// Generate a random 16-byte array (e.g., UUID, salt)
    pub fn random_16(&self) -> Result<[u8; 16], ring::error::Unspecified> {
        let mut arr = [0u8; 16];
        self.rng.fill(&mut arr)?;
        Ok(arr)
    }

    /// Generate a random 32-byte array (e.g., keys)
    pub fn random_32(&self) -> Result<[u8; 32], ring::error::Unspecified> {
        let mut arr = [0u8; 32];
        self.rng.fill(&mut arr)?;
        Ok(arr)
    }

    /// Generate a random u64
    pub fn random_u64(&self) -> Result<u64, ring::error::Unspecified> {
        let mut buf = [0u8; 8];
        self.rng.fill(&mut buf)?;
        Ok(u64::from_le_bytes(buf))
    }
}

lazy_static! {
    static ref SECURE_RNG: SecureRng = SecureRng::new();
}

/// Generate random bytes using the global secure RNG
pub fn random_bytes(len: usize) -> Vec<u8> {
    SECURE_RNG
        .random_bytes(len)
        .expect("OS CSPRNG failed; cannot continue safely (see SecureRng::random_bytes)")
}

/// Generate a random 16-byte array using the global secure RNG
pub fn random_16() -> [u8; 16] {
    SECURE_RNG
        .random_16()
        .expect("OS CSPRNG failed; cannot continue safely (see SecureRng::random_bytes)")
}

/// Generate a random 32-byte array using the global secure RNG
pub fn random_32() -> [u8; 32] {
    SECURE_RNG
        .random_32()
        .expect("OS CSPRNG failed; cannot continue safely (see SecureRng::random_bytes)")
}

/// Constant-time comparison for byte slices
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

/// XOR two byte arrays of the same length
pub fn xor_bytes(a: &[u8], b: &[u8]) -> Result<Vec<u8>, &'static str> {
    if a.len() != b.len() {
        return Err("Byte arrays must have the same length");
    }
    Ok(a.iter().zip(b.iter()).map(|(x, y)| x ^ y).collect())
}

/// Encode bytes to hex string
pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Decode hex string to bytes
pub fn from_hex(hex: &str) -> Result<Vec<u8>, &'static str> {
    if hex.len() % 2 != 0 {
        return Err("Hex string must have even length");
    }

    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| "Invalid hex character"))
        .collect()
}

/// Calculate CRC32 using the crc32fast crate
pub fn crc32(data: &[u8]) -> u32 {
    crc32fast::hash(data)
}

/// Validate that a buffer contains only zeros
pub fn is_all_zeros(buf: &[u8]) -> bool {
    buf.iter().all(|&b| b == 0)
}

/// UUID v4 generation
pub fn generate_uuid_v4() -> [u8; 16] {
    let mut uuid = random_16();
    // Set version (4) and variant bits
    uuid[6] = (uuid[6] & 0x0f) | 0x40; // Version 4
    uuid[8] = (uuid[8] & 0x3f) | 0x80; // Variant 10
    uuid
}

/// Format UUID bytes as string
pub fn format_uuid(uuid: &[u8; 16]) -> String {
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        u32::from_be_bytes([uuid[0], uuid[1], uuid[2], uuid[3]]),
        u16::from_be_bytes([uuid[4], uuid[5]]),
        u16::from_be_bytes([uuid[6], uuid[7]]),
        u16::from_be_bytes([uuid[8], uuid[9]]),
        u64::from_be_bytes([uuid[10], uuid[11], uuid[12], uuid[13], uuid[14], uuid[15], 0, 0])
            >> 16
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256() {
        let data = b"Hello, Roshera!";
        let hash = sha256(data);
        assert_eq!(hash.len(), 32);

        // Test known hash for "test"
        let known_hash = sha256(b"test");
        let expected = [
            0x9f, 0x86, 0xd0, 0x81, 0x88, 0x4c, 0x7d, 0x65, 0x9a, 0x2f, 0xea, 0xa0, 0xc5, 0x5a,
            0xd0, 0x15, 0xa3, 0xbf, 0x4f, 0x1b, 0x2b, 0x0b, 0x82, 0x2c, 0xd1, 0x5d, 0x6c, 0x15,
            0xb0, 0xf0, 0x0a, 0x08,
        ];
        assert_eq!(known_hash, expected);
    }

    #[test]
    fn test_secure_zero() {
        let mut sensitive = vec![1, 2, 3, 4, 5];
        secure_zero(&mut sensitive);
        assert!(is_all_zeros(&sensitive));
    }

    #[test]
    fn test_secure_string() {
        {
            let mut secure = SecureString::new("sensitive data".to_string());
            assert_eq!(secure.as_str(), "sensitive data");
            secure.zeroize();
            assert_eq!(secure.as_str(), "");
        }
        // After drop, memory should be zeroed
    }

    #[test]
    fn test_constant_time_eq() {
        let a = [1, 2, 3, 4];
        let b = [1, 2, 3, 4];
        let c = [1, 2, 3, 5];

        assert!(constant_time_eq(&a, &b));
        assert!(!constant_time_eq(&a, &c));
        assert!(!constant_time_eq(&a, &[1, 2, 3])); // Different lengths
    }

    #[test]
    fn test_hex_conversion() {
        let bytes = vec![0xde, 0xad, 0xbe, 0xef];
        let hex = to_hex(&bytes);
        assert_eq!(hex, "deadbeef");

        let decoded = from_hex(&hex).unwrap();
        assert_eq!(decoded, bytes);

        // Test error cases
        assert!(from_hex("deadbee").is_err()); // Odd length
        assert!(from_hex("deadbeeg").is_err()); // Invalid char
    }

    #[test]
    fn test_xor_bytes() {
        let a = vec![0xFF, 0x00, 0xAA];
        let b = vec![0x00, 0xFF, 0x55];
        let result = xor_bytes(&a, &b).unwrap();
        assert_eq!(result, vec![0xFF, 0xFF, 0xFF]);

        // Test error case
        assert!(xor_bytes(&[1, 2], &[1, 2, 3]).is_err());
    }

    #[test]
    fn test_uuid_generation() {
        let uuid = generate_uuid_v4();

        // Check version bits
        assert_eq!(uuid[6] & 0xf0, 0x40); // Version 4
        assert_eq!(uuid[8] & 0xc0, 0x80); // Variant 10

        let formatted = format_uuid(&uuid);
        assert_eq!(formatted.len(), 36); // 8-4-4-4-12 format
        assert_eq!(formatted.chars().filter(|&c| c == '-').count(), 4);
    }

    #[test]
    fn test_random_generation() {
        let rng = SecureRng::new();

        // Test different sizes
        let bytes = rng.random_bytes(32).unwrap();
        assert_eq!(bytes.len(), 32);

        let arr16 = rng.random_16().unwrap();
        assert_eq!(arr16.len(), 16);

        let arr32 = rng.random_32().unwrap();
        assert_eq!(arr32.len(), 32);

        // Test randomness (very basic)
        let another = rng.random_32().unwrap();
        assert_ne!(arr32, another);
    }

    #[test]
    fn test_time_functions() {
        let ms = current_time_ms();
        let secs = current_time_secs();

        assert!(ms > 0);
        assert!(secs > 0);
        assert!(ms / 1000 >= secs - 1); // Allow 1 second tolerance

        let sys_time = ms_to_system_time(ms);
        let duration = sys_time.duration_since(UNIX_EPOCH).unwrap();
        assert_eq!(duration.as_millis() as u64, ms);
    }
}
