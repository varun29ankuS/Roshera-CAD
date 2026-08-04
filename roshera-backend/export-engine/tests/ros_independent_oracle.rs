// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! THIRD METHOD: an INDEPENDENT oracle for a real `.ros` v3.2 file.
//!
//! (Written against v3.1, where it found four defects the format's own
//! tests structurally could not. Those defects are fixed and the format
//! is now v3.2; the probes below have been re-pointed from asserting the
//! DISAGREEMENT to asserting the AGREEMENT, and made adversarially
//! competent — each one now repairs the checksums a real tamperer would
//! repair, so it proves the SIGNATURE bites rather than a CRC.)
//!
//! Every existing `.ros` claim — HIST carries the timeline, PROV carries
//! provenance derived from recorded intent, SIGN carries a real Ed25519
//! signature over a Merkle root of the chunk bytes, META carries the file's
//! own replay verdict — is verified today by tests that read the file back
//! through `export_engine::formats::ros::import_ros`. That is the format's
//! own reader checking the format's own writer: a wrapper checking itself.
//! This project's rule (`verification-comprehensiveness-gap`) is that
//! independent verification means a DIFFERENT METHOD.
//!
//! This file is that different method. It:
//!
//! 1. drives REAL kernel operations through the REAL recorder bridge (box,
//!    cylinder, boolean difference, and a FILLET whose recorded operation
//!    references its edges by persistent id), with a `roshera.intent` scope
//!    open over the fillet ONLY — so the same document contains both an
//!    operation that stated a reason and operations that did not;
//! 2. exports a SIGNED `.ros` with a caller-supplied Ed25519 key to a
//!    durable path;
//! 3. parses the resulting bytes FROM FIRST PRINCIPLES — its own 128-byte
//!    header decode, its own 96-byte chunk-index decode, its own MessagePack
//!    walker, its own CRC-32, its own SHA-256, its own Merkle root — using
//!    `ros-format`'s `header.rs` / `chunk.rs` / `merkle.rs` as a SPEC to
//!    read, never as a library to call for the parse;
//! 4. proves the oracle BITES by feeding it four known-bad inputs.
//!
//! Test 3 does all of the above for a SIGNED **and ENCRYPTED** file: it
//! derives the whole key schedule itself (own HMAC-SHA256, own HKDF), rebuilds
//! every AES-GCM nonce itself, opens every payload itself, confirms the
//! signature covers the CIPHERTEXT and not the plaintext, and then feeds
//! itself four known-bad inputs. Until it existed, the encrypted path was
//! verified only through `import_ros` / `signed_leaf_set` — the format's own
//! reader checking the format's own writer.
//!
//! # What is borrowed, and why
//!
//! Three raw primitives are borrowed, and nothing else:
//!
//! 1. the Ed25519 curve operation, reached through
//!    `SignatureVerifier::verify_signature(&my_root, &my_record)`;
//! 2. Argon2id, reached through the `argon2` crate directly;
//! 3. AES-256-GCM, reached through the `aes-gcm` crate directly.
//!
//! For all three, the message, the key, the salt, the info, the nonce, the
//! leaf set and the byte ranges are extracted and computed by this file; only
//! the primitive itself is someone else's. `SignatureVerifier::verify_chunk`,
//! `SignatureChunk::deserialize`, `SoftwareKeyManager::generate_key_set` and
//! `ChunkEncryptor` — the format's own paths — are never called.
//!
//! Rebuilding curve25519, a memory-hard password hash or an AEAD here would
//! trade widely-audited primitives for unaudited ones and would not make the
//! *binding* — which bytes go into which primitive under which key — any more
//! independent. That binding is entirely this file's own work, and it is where
//! the KDF defect lived: `generate_key_set` used the right Argon2 and the
//! right HKDF, and expanded them over a `file_id` the file did not carry.
//! HMAC-SHA256 and HKDF ARE reimplemented here, on this file's own SHA-256,
//! because the salt/info wiring is exactly the part that is easy to read
//! backwards off `keys.rs`.
//!
//! Run:
//! `cargo test -p export-engine --test ros_independent_oracle -- --nocapture`

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use export_engine::formats::ros::{
    export_brep_to_ros, import_ros, verify_ros_file, HistData, RosExportOptions, RosExportPayload,
    RosSignatureVerdict, RosWriteSignature,
};
use export_engine::formats::ros_provenance::ai_tracker_from_timeline;
use export_engine::formats::timeline_chunk::BranchManifest;

use geometry_engine::math::{Point3, Vector3, NORMAL_TOLERANCE};
use geometry_engine::operations::fillet::{fillet_edges, FilletOptions, FilletType};
use geometry_engine::operations::recorder::OperationRecorder;
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

use ros_format::signature::{
    FileSignatureMetadata, FileSigner, SignatureAlgorithm, SignatureRecord, SignatureVerifier,
};
use ros_format::TrackingLevel;

use timeline_engine::recorder_bridge::{IntentContext, INTENT_OVERRIDE};
use timeline_engine::{
    Author, BranchId, BranchMetadata, BranchPurpose, BranchState, Operation, SharedTimeline,
    Timeline, TimelineConfig, TimelineEvent, TimelineRecorder,
};
use tokio::sync::RwLock;

/// The author's key. Caller-supplied, fixed, so the oracle can re-derive the
/// public key and the signer id independently of anything the file says.
const SIGNING_KEY_SEED: [u8; 32] = [0x5au8; 32];

/// The design intent scoped over the FILLET only.
const INTENT_TEXT: &str = "break the sharp corner so the bracket does not cut the harness";
const INTENT_TURN: &str = "turn-oracle-01";

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 1 — Independent primitives (own implementations)
// ═══════════════════════════════════════════════════════════════════════════

/// FIPS 180-4 SHA-256, implemented here. Validated below against published
/// vectors AND cross-checked against the `sha2` crate at runtime — the
/// cross-check proves the primitive, the Merkle construction on top of it is
/// this file's own.
fn my_sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for block in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[4 * i],
                block[4 * i + 1],
                block[4 * i + 2],
                block[4 * i + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// CRC-32/ISO-HDLC (the "IEEE" CRC-32 that `crc32fast` computes), implemented
/// here: reflected, poly 0xEDB88320, init/final 0xFFFFFFFF.
fn my_crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// The Merkle root as `ros-format/src/merkle.rs` SPECIFIES it (not as it
/// implements it — this is a re-derivation from the documented construction):
/// leaf = SHA256(b"leaf:" || data), internal = SHA256(b"node:" || l || r),
/// bottom-up pairing, an odd node paired with ITSELF.
fn my_merkle_root(leaves: &[Vec<u8>]) -> Option<[u8; 32]> {
    if leaves.is_empty() {
        return None;
    }
    let mut level: Vec<[u8; 32]> = leaves
        .iter()
        .map(|d| {
            let mut buf = Vec::with_capacity(5 + d.len());
            buf.extend_from_slice(b"leaf:");
            buf.extend_from_slice(d);
            my_sha256(&buf)
        })
        .collect();

    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        for pair in level.chunks(2) {
            let (l, r) = if pair.len() == 2 {
                (pair[0], pair[1])
            } else {
                (pair[0], pair[0])
            };
            let mut buf = Vec::with_capacity(5 + 64);
            buf.extend_from_slice(b"node:");
            buf.extend_from_slice(&l);
            buf.extend_from_slice(&r);
            next.push(my_sha256(&buf));
        }
        level = next;
    }
    Some(level[0])
}

/// HMAC-SHA256 (RFC 2104) on top of this file's own SHA-256. Validated
/// below against the RFC 4231 vectors.
fn my_hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = if key.len() > BLOCK {
        my_sha256(key).to_vec()
    } else {
        key.to_vec()
    };
    k.resize(BLOCK, 0);

    let mut inner = Vec::with_capacity(BLOCK + msg.len());
    inner.extend(k.iter().map(|b| b ^ 0x36));
    inner.extend_from_slice(msg);
    let inner_hash = my_sha256(&inner);

    let mut outer = Vec::with_capacity(BLOCK + 32);
    outer.extend(k.iter().map(|b| b ^ 0x5c));
    outer.extend_from_slice(&inner_hash);
    my_sha256(&outer)
}

/// HKDF-SHA256 (RFC 5869) restricted to a 32-byte output — the only
/// length `ros-format` ever asks for. Extract then one expand round:
/// `PRK = HMAC(salt, ikm)`, `OKM = HMAC(PRK, info ‖ 0x01)`.
///
/// Reimplemented rather than borrowed BECAUSE this is where the .ros
/// chain is easy to get backwards. `keys.rs::derive_chunk_key` passes
/// `b"ROSHERA_CHUNK_V3" ‖ FourCC` as the HKDF **salt** and the bare FourCC
/// as the **info** — the opposite of what the parameter names in that
/// function suggest at a glance. An oracle that called `hkdf::Hkdf` with
/// the two swapped would produce a different key and blame the file; one
/// that re-derives the construction from the spec cannot silently inherit
/// the format's own reading of it.
fn my_hkdf_sha256_32(salt: &[u8], ikm: &[u8], info: &[u8]) -> [u8; 32] {
    let prk = my_hmac_sha256(salt, ikm);
    let mut t = info.to_vec();
    t.push(0x01);
    my_hmac_sha256(&prk, &t)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    assert!(s.len() % 2 == 0, "hex string must have even length");
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("hex digit"))
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 2 — Independent MessagePack walker
//
// Not a value tree: a skipper plus map/array enumeration. A bug in the
// skipper desynchronises immediately (it lands on a byte that is not a valid
// type marker, or the top-level walk fails to consume exactly the chunk's
// declared length) rather than silently returning a wrong count. Every chunk
// walk below asserts the exact-consumption property.
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq, Eq)]
enum MpKey {
    Str(String),
    Int(i64),
    Other,
}

struct Mp<'a> {
    buf: &'a [u8],
    label: &'a str,
}

impl<'a> Mp<'a> {
    fn new(buf: &'a [u8], label: &'a str) -> Self {
        Mp { buf, label }
    }

    fn byte(&self, p: usize) -> u8 {
        assert!(
            p < self.buf.len(),
            "{}: MessagePack read past end of chunk at {} (len {})",
            self.label,
            p,
            self.buf.len()
        );
        self.buf[p]
    }

    fn be16(&self, p: usize) -> usize {
        u16::from_be_bytes([self.byte(p), self.byte(p + 1)]) as usize
    }

    fn be32(&self, p: usize) -> usize {
        u32::from_be_bytes([
            self.byte(p),
            self.byte(p + 1),
            self.byte(p + 2),
            self.byte(p + 3),
        ]) as usize
    }

    /// Advance past exactly one MessagePack value, returning the position of
    /// the next value.
    fn skip(&self, p: usize) -> usize {
        let b = self.byte(p);
        let p = p + 1;
        match b {
            0x00..=0x7f => p,
            0x80..=0x8f => {
                let n = (b & 0x0f) as usize;
                let mut q = p;
                for _ in 0..2 * n {
                    q = self.skip(q);
                }
                q
            }
            0x90..=0x9f => {
                let n = (b & 0x0f) as usize;
                let mut q = p;
                for _ in 0..n {
                    q = self.skip(q);
                }
                q
            }
            0xa0..=0xbf => p + (b & 0x1f) as usize,
            0xc0 | 0xc2 | 0xc3 => p,
            0xc1 => panic!("{}: 0xc1 is a never-used marker (offset {})", self.label, p),
            0xc4 => p + 1 + self.byte(p) as usize,
            0xc5 => p + 2 + self.be16(p),
            0xc6 => p + 4 + self.be32(p),
            0xc7 => p + 1 + 1 + self.byte(p) as usize,
            0xc8 => p + 2 + 1 + self.be16(p),
            0xc9 => p + 4 + 1 + self.be32(p),
            0xca => p + 4,
            0xcb => p + 8,
            0xcc | 0xd0 => p + 1,
            0xcd | 0xd1 => p + 2,
            0xce | 0xd2 => p + 4,
            0xcf | 0xd3 => p + 8,
            0xd4 => p + 2,
            0xd5 => p + 3,
            0xd6 => p + 5,
            0xd7 => p + 9,
            0xd8 => p + 17,
            0xd9 => p + 1 + self.byte(p) as usize,
            0xda => p + 2 + self.be16(p),
            0xdb => p + 4 + self.be32(p),
            0xdc => {
                let n = self.be16(p);
                let mut q = p + 2;
                for _ in 0..n {
                    q = self.skip(q);
                }
                q
            }
            0xdd => {
                let n = self.be32(p);
                let mut q = p + 4;
                for _ in 0..n {
                    q = self.skip(q);
                }
                q
            }
            0xde => {
                let n = self.be16(p);
                let mut q = p + 2;
                for _ in 0..2 * n {
                    q = self.skip(q);
                }
                q
            }
            0xdf => {
                let n = self.be32(p);
                let mut q = p + 4;
                for _ in 0..2 * n {
                    q = self.skip(q);
                }
                q
            }
            0xe0..=0xff => p,
        }
    }

    /// Enumerate a map's `(key, value-position)` pairs.
    fn map(&self, p: usize) -> Vec<(MpKey, usize)> {
        let b = self.byte(p);
        let (n, mut q) = match b {
            0x80..=0x8f => ((b & 0x0f) as usize, p + 1),
            0xde => (self.be16(p + 1), p + 3),
            0xdf => (self.be32(p + 1), p + 5),
            other => panic!(
                "{}: expected a map at offset {}, found marker 0x{:02x}",
                self.label, p, other
            ),
        };
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let key_pos = q;
            let key = match self.str_at(key_pos) {
                Some(s) => MpKey::Str(s),
                None => match self.int_at(key_pos) {
                    Some(i) => MpKey::Int(i),
                    None => MpKey::Other,
                },
            };
            q = self.skip(key_pos);
            let value_pos = q;
            q = self.skip(value_pos);
            out.push((key, value_pos));
        }
        out
    }

    /// Enumerate an array's element positions.
    fn array(&self, p: usize) -> Vec<usize> {
        let b = self.byte(p);
        let (n, mut q) = match b {
            0x90..=0x9f => ((b & 0x0f) as usize, p + 1),
            0xdc => (self.be16(p + 1), p + 3),
            0xdd => (self.be32(p + 1), p + 5),
            other => panic!(
                "{}: expected an array at offset {}, found marker 0x{:02x}",
                self.label, p, other
            ),
        };
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(q);
            q = self.skip(q);
        }
        out
    }

    fn get(&self, map_pos: usize, key: &str) -> Option<usize> {
        self.map(map_pos)
            .into_iter()
            .find(|(k, _)| matches!(k, MpKey::Str(s) if s == key))
            .map(|(_, v)| v)
    }

    fn expect_get(&self, map_pos: usize, key: &str) -> usize {
        self.get(map_pos, key)
            .unwrap_or_else(|| panic!("{}: map at {} has no key `{}`", self.label, map_pos, key))
    }

    fn str_at(&self, p: usize) -> Option<String> {
        let b = self.byte(p);
        let (start, len) = match b {
            0xa0..=0xbf => (p + 1, (b & 0x1f) as usize),
            0xd9 => (p + 2, self.byte(p + 1) as usize),
            0xda => (p + 3, self.be16(p + 1)),
            0xdb => (p + 5, self.be32(p + 1)),
            _ => return None,
        };
        Some(String::from_utf8_lossy(&self.buf[start..start + len]).to_string())
    }

    fn int_at(&self, p: usize) -> Option<i64> {
        let b = self.byte(p);
        match b {
            0x00..=0x7f => Some(b as i64),
            0xe0..=0xff => Some(b as i8 as i64),
            0xcc => Some(self.byte(p + 1) as i64),
            0xcd => Some(self.be16(p + 1) as i64),
            0xce => Some(self.be32(p + 1) as i64),
            0xcf => {
                let mut v: u64 = 0;
                for i in 0..8 {
                    v = (v << 8) | self.byte(p + 1 + i) as u64;
                }
                Some(v as i64)
            }
            0xd0 => Some(self.byte(p + 1) as i8 as i64),
            0xd1 => Some(self.be16(p + 1) as u16 as i16 as i64),
            0xd2 => Some(self.be32(p + 1) as u32 as i32 as i64),
            0xd3 => {
                let mut v: u64 = 0;
                for i in 0..8 {
                    v = (v << 8) | self.byte(p + 1 + i) as u64;
                }
                Some(v as i64)
            }
            _ => None,
        }
    }

    fn is_nil(&self, p: usize) -> bool {
        self.byte(p) == 0xc0
    }

    /// Read a byte string, accepting BOTH the `bin` family and the
    /// array-of-integers shape that serde's default `Vec<u8>` / `[u8; N]`
    /// impls produce through rmp-serde.
    fn bytes_at(&self, p: usize) -> Vec<u8> {
        let b = self.byte(p);
        match b {
            0xc4 => {
                let n = self.byte(p + 1) as usize;
                self.buf[p + 2..p + 2 + n].to_vec()
            }
            0xc5 => {
                let n = self.be16(p + 1);
                self.buf[p + 3..p + 3 + n].to_vec()
            }
            0xc6 => {
                let n = self.be32(p + 1);
                self.buf[p + 5..p + 5 + n].to_vec()
            }
            _ => self
                .array(p)
                .into_iter()
                .map(|q| {
                    self.int_at(q).unwrap_or_else(|| {
                        panic!(
                            "{}: byte-array element at {} is not an integer",
                            self.label, q
                        )
                    }) as u8
                })
                .collect(),
        }
    }

    /// The self-check: a well-formed chunk body is exactly one value that
    /// consumes the whole declared payload.
    fn assert_exact_consumption(&self) {
        let end = self.skip(0);
        assert_eq!(
            end,
            self.buf.len(),
            "{}: walking the top-level MessagePack value consumed {} of {} bytes — \
             the walker desynchronised or the chunk's declared size is wrong",
            self.label,
            end,
            self.buf.len()
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 3 — Independent .ros structural decode
//
// Field offsets transcribed from `ros-format/src/header.rs`
// (`serialize_with_endianness`) and `ros-format/src/chunk.rs`
// (`ChunkIndexEntry::write_to`) read as a SPEC. No ros-format parse call.
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
struct OracleHeader {
    magic: [u8; 8],
    major: u8,
    minor: u8,
    patch: u8,
    endianness: u8,
    stored_crc32: u32,
    file_size: u64,
    creation_time: u64,
    file_uuid: [u8; 16],
    index_offset: u64,
    index_entry_count: u32,
    index_entry_size: u32,
    encryption_algo: u8,
    kdf_algo: u8,
    signature_algo: u8,
    ai_tracking: u8,
    kdf_iterations: u32,
    kdf_salt: [u8; 16],
    file_iv: [u8; 8],
    feature_flags: u64,
    reserved: [u8; 8],
    ai_command_count: u64,
    ai_chunk_offset: u64,
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([
        b[o],
        b[o + 1],
        b[o + 2],
        b[o + 3],
        b[o + 4],
        b[o + 5],
        b[o + 6],
        b[o + 7],
    ])
}

impl OracleHeader {
    fn parse(file: &[u8]) -> Self {
        assert!(
            file.len() >= 128,
            "file is {} bytes — shorter than the mandatory 128-byte header",
            file.len()
        );
        let endianness = file[11];
        assert_eq!(
            endianness, 1,
            "this oracle decodes the little-endian header layout; byte 11 says {}",
            endianness
        );
        let mut magic = [0u8; 8];
        magic.copy_from_slice(&file[0..8]);
        let mut file_uuid = [0u8; 16];
        file_uuid.copy_from_slice(&file[32..48]);
        let mut kdf_salt = [0u8; 16];
        kdf_salt.copy_from_slice(&file[72..88]);
        let mut file_iv = [0u8; 8];
        file_iv.copy_from_slice(&file[88..96]);
        let mut reserved = [0u8; 8];
        reserved.copy_from_slice(&file[104..112]);

        OracleHeader {
            magic,
            major: file[8],
            minor: file[9],
            patch: file[10],
            endianness,
            stored_crc32: u32le(file, 12),
            file_size: u64le(file, 16),
            creation_time: u64le(file, 24),
            file_uuid,
            index_offset: u64le(file, 48),
            index_entry_count: u32le(file, 56),
            index_entry_size: u32le(file, 60),
            encryption_algo: file[64],
            kdf_algo: file[65],
            signature_algo: file[66],
            ai_tracking: file[67],
            kdf_iterations: u32le(file, 68),
            kdf_salt,
            file_iv,
            feature_flags: u64le(file, 96),
            reserved,
            ai_command_count: u64le(file, 112),
            ai_chunk_offset: u64le(file, 120),
        }
    }
}

#[derive(Debug, Clone)]
struct OracleEntry {
    fourcc: [u8; 4],
    version: u32,
    offset: u64,
    uncompressed_size: u64,
    compressed_size: u64,
    declared_crc32: u32,
    flags: u32,
    compression: u8,
    comp_level: u8,
    reserved_comp: [u8; 6],
    encrypted: bool,
    enc_algo: u8,
    key_id: [u8; 16],
    chunk_iv: [u8; 12],
    auth_tag: [u8; 2],
    access_level: u32,
    owner_id: u32,
    reserved: [u8; 8],
    /// Byte offset of this entry inside the file's chunk index.
    entry_file_offset: usize,
}

impl OracleEntry {
    fn name(&self) -> String {
        String::from_utf8_lossy(&self.fourcc).to_string()
    }

    /// `ChunkIndexEntry::size_on_disk()` re-derived: compressed_size when
    /// non-zero, else uncompressed_size.
    fn size_on_disk(&self) -> u64 {
        if self.compressed_size > 0 {
            self.compressed_size
        } else {
            self.uncompressed_size
        }
    }

    fn parse(file: &[u8], at: usize) -> Self {
        // Layout, transcribed from `ChunkIndexEntry::write_to`:
        //  0..4 fourcc | 4..8 version | 8..16 offset | 16..24 uncompressed
        // 24..32 compressed | 32..36 crc32 | 36..40 flags | 40 compression
        // 41 comp_level | 42..48 reserved_comp | 48 encrypted | 49 enc_algo
        // 50..66 key_id | 66..78 chunk_iv | 78..80 auth_tag
        // 80..84 access_level | 84..88 owner_id | 88..96 reserved
        let b = &file[at..at + 96];
        let mut fourcc = [0u8; 4];
        fourcc.copy_from_slice(&b[0..4]);
        let mut reserved_comp = [0u8; 6];
        reserved_comp.copy_from_slice(&b[42..48]);
        let mut key_id = [0u8; 16];
        key_id.copy_from_slice(&b[50..66]);
        let mut chunk_iv = [0u8; 12];
        chunk_iv.copy_from_slice(&b[66..78]);
        let mut auth_tag = [0u8; 2];
        auth_tag.copy_from_slice(&b[78..80]);
        let mut reserved = [0u8; 8];
        reserved.copy_from_slice(&b[88..96]);

        OracleEntry {
            fourcc,
            version: u32le(b, 4),
            offset: u64le(b, 8),
            uncompressed_size: u64le(b, 16),
            compressed_size: u64le(b, 24),
            declared_crc32: u32le(b, 32),
            flags: u32le(b, 36),
            compression: b[40],
            comp_level: b[41],
            reserved_comp,
            encrypted: b[48] != 0,
            enc_algo: b[49],
            key_id,
            chunk_iv,
            auth_tag,
            access_level: u32le(b, 80),
            owner_id: u32le(b, 84),
            reserved,
            entry_file_offset: at,
        }
    }
}

struct OracleFile {
    bytes: Vec<u8>,
    header: OracleHeader,
    entries: Vec<OracleEntry>,
}

impl OracleFile {
    fn open(path: &Path) -> Self {
        let bytes = std::fs::read(path).expect("read the .ros file");
        let header = OracleHeader::parse(&bytes);
        let index_start = header.index_offset as usize;
        let count = header.index_entry_count as usize;
        let entry_size = header.index_entry_size as usize;
        assert_eq!(
            entry_size, 96,
            "the v3 spec fixes the chunk index entry at 96 bytes; header says {}",
            entry_size
        );
        assert!(
            index_start + count * entry_size <= bytes.len(),
            "chunk index [{}..{}) lies outside the {}-byte file",
            index_start,
            index_start + count * entry_size,
            bytes.len()
        );
        let entries = (0..count)
            .map(|i| OracleEntry::parse(&bytes, index_start + i * entry_size))
            .collect();
        OracleFile {
            bytes,
            header,
            entries,
        }
    }

    fn chunk_bytes(&self, e: &OracleEntry) -> &[u8] {
        let start = e.offset as usize;
        let end = start + e.size_on_disk() as usize;
        assert!(
            end <= self.bytes.len(),
            "{} chunk [{}..{}) lies outside the {}-byte file",
            e.name(),
            start,
            end,
            self.bytes.len()
        );
        &self.bytes[start..end]
    }

    fn find(&self, fourcc: &[u8; 4]) -> Option<&OracleEntry> {
        self.entries.iter().find(|e| &e.fourcc == fourcc)
    }

    fn expect(&self, fourcc: &[u8; 4]) -> &OracleEntry {
        self.find(fourcc).unwrap_or_else(|| {
            panic!(
                "chunk {} is absent from the chunk table",
                String::from_utf8_lossy(fourcc)
            )
        })
    }

    /// Independent layout audit: every declared chunk region must start
    /// exactly where the previous one ended (no gap, no overlap), the payload
    /// region must end exactly where the index begins, the index must end
    /// exactly at EOF, and the header's `file_size` must equal the bytes on
    /// disk. Returns the end of the payload region on success.
    ///
    /// Returns `Err` rather than panicking so the audit can itself be tested
    /// against a deliberately broken layout.
    fn audit_layout(&self) -> Result<u64, String> {
        let mut sorted: Vec<&OracleEntry> = self.entries.iter().collect();
        sorted.sort_by_key(|e| e.offset);
        let mut cursor: u64 = 128;
        for e in &sorted {
            if e.offset != cursor {
                return Err(format!(
                    "chunk {} starts at {} but the previous region ended at {} — \
                     the file has a {} of {} bytes",
                    e.name(),
                    e.offset,
                    cursor,
                    if e.offset > cursor { "gap" } else { "overlap" },
                    (e.offset as i128 - cursor as i128).abs()
                ));
            }
            if e.offset + e.size_on_disk() > self.bytes.len() as u64 {
                return Err(format!(
                    "chunk {} runs past end of file ({} + {} > {})",
                    e.name(),
                    e.offset,
                    e.size_on_disk(),
                    self.bytes.len()
                ));
            }
            cursor += e.size_on_disk();
        }
        if cursor != self.header.index_offset {
            return Err(format!(
                "the payload region ends at {} but the chunk index starts at {}",
                cursor, self.header.index_offset
            ));
        }
        let index_end = self.header.index_offset + self.entries.len() as u64 * 96;
        if index_end != self.bytes.len() as u64 {
            return Err(format!(
                "the chunk index ends at {} but the file is {} bytes",
                index_end,
                self.bytes.len()
            ));
        }
        if self.header.file_size != self.bytes.len() as u64 {
            return Err(format!(
                "header file_size says {} but the file is {} bytes",
                self.header.file_size,
                self.bytes.len()
            ));
        }
        Ok(cursor)
    }

    /// The normalized header leaf as the v3.2 spec DEFINES it: the
    /// 128-byte header image with the three ranges that are not knowable
    /// at signing time blanked — `header_crc32` (12..16), `file_size`
    /// (16..24), `index_offset` (48..56). Transcribed from
    /// `export-engine/src/formats/ros.rs`'s `HEADER_LEAF_ZEROED_RANGES`
    /// read as a spec; `normalized_header_leaf` is never called.
    fn header_leaf(&self) -> Vec<u8> {
        let mut leaf = self.bytes[0..128].to_vec();
        for range in [12..16usize, 16..24, 48..56] {
            leaf[range].fill(0);
        }
        leaf
    }

    /// This entry's raw 96 bytes as they sit inside the file's chunk index.
    fn entry_bytes(&self, e: &OracleEntry) -> &[u8] {
        &self.bytes[e.entry_file_offset..e.entry_file_offset + 96]
    }

    /// The header CRC-32 input the v3.2 spec defines: the whole 128-byte
    /// header EXCEPT the CRC field itself. (v3.1 covered bytes 0..12 and
    /// nothing else — see the coverage assertions in the main test.)
    fn header_crc_input(&self) -> Vec<u8> {
        let mut input = self.bytes[0..12].to_vec();
        input.extend_from_slice(&self.bytes[16..128]);
        input
    }

    /// Merkle leaves EXACTLY as the v3.2 writer's contract states:
    ///
    /// 1. the normalized header image (one leaf);
    /// 2. the on-disk PAYLOAD bytes of every non-SIGN chunk, table order;
    /// 3. the 96-byte INDEX ENTRY of every non-SIGN chunk, table order.
    ///
    /// `1 + 2n` leaves for `n` signed chunks. Under v3.1 this was group 2
    /// alone, which is why a rewritten header or a swapped FourCC label
    /// left the root untouched.
    fn signed_leaves(&self) -> Vec<Vec<u8>> {
        self.signed_leaves_with(|f, e| f.chunk_bytes(e).to_vec())
    }

    /// [`signed_leaves`](Self::signed_leaves) with the payload leaf of each
    /// non-SIGN chunk supplied by the caller.
    ///
    /// Exists for exactly one question, and it is the question that makes
    /// password-free verification of an encrypted file possible or not:
    /// does the signature cover the CIPHERTEXT as it lands on disk, or the
    /// plaintext behind it? "The root over the on-disk bytes verifies" is
    /// consistent with both readings only until the plaintext root is
    /// computed too and shown NOT to verify. This builds that second root.
    fn signed_leaves_with<F>(&self, payload_of: F) -> Vec<Vec<u8>>
    where
        F: Fn(&OracleFile, &OracleEntry) -> Vec<u8>,
    {
        let non_sign: Vec<&OracleEntry> = self
            .entries
            .iter()
            .filter(|e| &e.fourcc != b"SIGN")
            .collect();
        let mut leaves = vec![self.header_leaf()];
        for e in &non_sign {
            leaves.push(payload_of(self, e));
        }
        for e in &non_sign {
            leaves.push(self.entry_bytes(e).to_vec());
        }
        leaves
    }

    /// This entry's position in the chunk table — the value the .ros
    /// per-chunk AES-GCM nonce is built from.
    ///
    /// # A rule the format states twice, and differently
    ///
    /// The WRITER uses `chunks.len()` at push time
    /// (`export_brep_to_ros` -> `encrypt_if_enabled`), which is the
    /// entry's positional index in the finished table. The READER uses
    /// `table.iter().position(|e| e.chunk_type == entry.chunk_type)`
    /// (`read_chunk_payload`) — the index of the FIRST entry carrying that
    /// FourCC. Those are two different functions, and they agree only
    /// while every FourCC in the table is unique.
    ///
    /// Nothing enforces that uniqueness. `ChunkTable::validate` requires
    /// META/HIST/PROV to be PRESENT and forbids byte-range overlaps; it
    /// says nothing about duplicate FourCCs, and neither does the layout
    /// audit. A file carrying two chunks of one type — which this writer
    /// never emits, but a third-party one could — would have its second
    /// such chunk sealed under nonce `i` and opened under nonce `j`.
    ///
    /// It is unobservable today only because `find_by_type` also returns
    /// the first match, so the reader never reaches the second chunk at
    /// all: it silently ignores a payload that IS inside the signature.
    /// This is the same shape as the "signed byte range vs parsed byte
    /// range" divergence the oracle found before — two rules that agree
    /// by coincidence rather than by construction.
    ///
    /// The oracle implements the POSITIONAL rule (the writer's) and lets
    /// the AEAD tag be the judge: a wrong index yields a wrong nonce and
    /// the tag rejects. The uniqueness the coincidence rests on is
    /// asserted explicitly rather than assumed — see
    /// `independent_oracle_verifies_a_real_signed_encrypted_ros_file`.
    fn table_index(&self, e: &OracleEntry) -> u32 {
        self.entries
            .iter()
            .position(|x| x.entry_file_offset == e.entry_file_offset)
            .unwrap_or_else(|| panic!("{} is not in this file's chunk table", e.name()))
            as u32
    }
}

/// Rewrite a file image's header CRC-32 in place, the way a COMPETENT
/// tamperer would after editing a header field. Used so the adversarial
/// probes below prove what they claim: without this, every header edit is
/// caught by the widened CRC and nothing is learned about whether the
/// SIGNATURE covers the header.
fn repair_header_crc(bytes: &mut [u8]) {
    let mut input = bytes[0..12].to_vec();
    input.extend_from_slice(&bytes[16..128]);
    let crc = my_crc32(&input);
    bytes[12..16].copy_from_slice(&crc.to_le_bytes());
}

/// Rewrite a chunk's DECLARED crc32 in its index entry to match the bytes
/// actually on disk — again, what a competent tamperer does. The declared
/// CRC is unauthenticated for SIGN (whose index entry is not a leaf), so
/// this is a legitimate move for an attacker and the probes must survive
/// it.
fn repair_declared_crc(
    bytes: &mut [u8],
    entry_file_offset: usize,
    chunk_span: std::ops::Range<usize>,
) {
    let crc = my_crc32(&bytes[chunk_span]);
    bytes[entry_file_offset + 32..entry_file_offset + 36].copy_from_slice(&crc.to_le_bytes());
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 3b — Independent .ros ENCRYPTED-CHAIN re-derivation
//
// The whole key schedule, rebuilt from `ros-format/src/keys.rs` and
// `ros-format/src/encryption.rs` read as a SPEC. `generate_key_set`,
// `derive_master_key`, `derive_file_key`, `derive_chunk_key`,
// `ChunkEncryptor` and `generate_chunk_iv` are NEVER called.
//
// Borrowed: Argon2id (the `argon2` crate) and AES-256-GCM (the `aes-gcm`
// crate) as raw primitives — a memory-hard hash and an AEAD are not
// things to hand-roll, and rebuilding them would trade audited code for
// unaudited without making the BINDING any more independent. Everything
// that binds them — which salt, which ikm, which info, which nonce, which
// chunk index, in which order — is this file's own.
//
// Every constant below is transcribed, not imported, for the same reason
// `ros_integrity_coverage.rs` writes `kdf_algo = 3` literally: these are
// WIRE values. Importing `ROSHERA_KDF_MEMORY_KIB` would mean a change to
// the constant silently redefined what the oracle is checking.
// ═══════════════════════════════════════════════════════════════════════════

/// Argon2id memory cost in KiB — 64 MiB. From `keys.rs`
/// (`ROSHERA_KDF_MEMORY_KIB`), transcribed.
const ORACLE_ARGON2_MEMORY_KIB: u32 = 64 * 1024;

/// Argon2id lanes. From the `Params::new(.., .., 4, ..)` literal in
/// `SoftwareKeyManager::derive_master_key`.
const ORACLE_ARGON2_LANES: u32 = 4;

/// HKDF salt for the file-key expansion (`derive_file_key`).
const ORACLE_FILE_KEY_SALT: &[u8] = b"ROSHERA_FILE_KEY_V3";

/// HKDF salt PREFIX for the chunk-key expansion (`derive_chunk_key`); the
/// chunk's FourCC is appended to it, and the bare FourCC is the info.
const ORACLE_CHUNK_KEY_SALT_PREFIX: &[u8] = b"ROSHERA_CHUNK_V3";

/// The `kdf_algo` an encrypted .ros file must declare, on the wire.
const ORACLE_KDF_ALGO_FILE_BOUND: u8 = 3;

/// The `encryption_algo` an encrypted .ros file must declare (AES-256-GCM).
const ORACLE_ENC_ALGO_AES256GCM: u8 = 1;

/// AES-256-GCM tag length, appended to every ciphertext by the AEAD.
const ORACLE_GCM_TAG_LEN: usize = 16;

/// The .ros key schedule, re-derived here from the password plus bytes the
/// file itself carries — which is the entire property the 2026-08-04 fix
/// established, and the one nothing had independently checked.
struct OracleKeys {
    master: [u8; 32],
    file_key: [u8; 32],
}

impl OracleKeys {
    /// `Argon2id(password, salt = header.kdf_salt, t = header.kdf_iterations,
    /// m = 64 MiB, p = 4) -> 32 bytes`, then
    /// `HKDF-SHA256(ikm = master, salt = "ROSHERA_FILE_KEY_V3",
    /// info = header.file_uuid) -> 32 bytes`.
    ///
    /// `t_cost` comes from the HEADER, not from a constant: reproducing
    /// the derivation the file records is the property under test. A
    /// hard-coded 3 would pass even on a file that recorded something
    /// else.
    fn derive(password: &str, kdf_salt: &[u8; 16], t_cost: u32, file_uuid: &[u8; 16]) -> Self {
        use argon2::{Algorithm, Argon2, Params, Version};

        let params = Params::new(
            ORACLE_ARGON2_MEMORY_KIB,
            t_cost,
            ORACLE_ARGON2_LANES,
            Some(32),
        )
        .expect("Argon2 params");
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut master = [0u8; 32];
        argon2
            .hash_password_into(password.as_bytes(), kdf_salt, &mut master)
            .expect("Argon2id derivation");

        let file_key = my_hkdf_sha256_32(ORACLE_FILE_KEY_SALT, &master, file_uuid);
        OracleKeys { master, file_key }
    }

    /// `HKDF-SHA256(ikm = file_key, salt = "ROSHERA_CHUNK_V3" ‖ FourCC,
    /// info = FourCC) -> 32 bytes`.
    fn chunk_key(&self, fourcc: &[u8; 4]) -> [u8; 32] {
        let mut salt = ORACLE_CHUNK_KEY_SALT_PREFIX.to_vec();
        salt.extend_from_slice(fourcc);
        my_hkdf_sha256_32(&salt, &self.file_key, fourcc)
    }
}

/// The 12-byte AES-GCM nonce, rebuilt from `encryption.rs`
/// (`ChunkEncryptor::generate_chunk_iv`) read as a spec: the header's
/// 8-byte `file_iv`, then the FourCC XORed byte-for-byte with the
/// big-endian chunk index.
fn oracle_chunk_iv(file_iv: &[u8; 8], fourcc: &[u8; 4], chunk_index: u32) -> [u8; 12] {
    let mut iv = [0u8; 12];
    iv[..8].copy_from_slice(file_iv);
    let idx = chunk_index.to_be_bytes();
    for i in 0..4 {
        iv[8 + i] = fourcc[i] ^ idx[i];
    }
    iv
}

/// AES-256-GCM open, with NO additional authenticated data — the .ros
/// writer passes `None` (`encrypt_if_enabled` in
/// `export-engine/src/formats/ros.rs`).
///
/// Returns `None` when the tag rejects, so a caller can assert a
/// rejection instead of unwinding on one.
fn oracle_aes_gcm_open(key: &[u8; 32], iv: &[u8; 12], ciphertext: &[u8]) -> Option<Vec<u8>> {
    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce};

    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte AES key");
    cipher.decrypt(Nonce::from_slice(iv), ciphertext).ok()
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 4 — The recorded document (real kernel ops, real recorder bridge)
// ═══════════════════════════════════════════════════════════════════════════

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(id) => id,
        other => panic!("expected a Solid geometry id, got {other:?}"),
    }
}

/// A vertical corner edge of the boolean result, well outside the r=8 bore.
fn pick_vertical_box_edge(m: &BRepModel) -> EdgeId {
    let eps = NORMAL_TOLERANCE.distance();
    for (id, e) in m.edges.iter() {
        let Some(a) = m.vertices.get(e.start_vertex).map(|v| v.position) else {
            continue;
        };
        let Some(b) = m.vertices.get(e.end_vertex).map(|v| v.position) else {
            continue;
        };
        let same_xy = (a[0] - b[0]).abs() <= eps && (a[1] - b[1]).abs() <= eps;
        let vertical = (a[2] - b[2]).abs() > eps;
        let radius = (a[0] * a[0] + a[1] * a[1]).sqrt();
        if same_xy && vertical && radius > 12.0 {
            return id;
        }
    }
    panic!("no vertical box corner edge on the boolean result — harness precondition broken");
}

fn main_branch_manifest() -> BranchManifest {
    let id = BranchId::main();
    BranchManifest {
        id,
        name: "main".to_string(),
        parent: None,
        fork_point: timeline_engine::ForkPoint {
            branch_id: id,
            event_index: 0,
            timestamp: chrono::Utc::now(),
        },
        state: BranchState::Active,
        metadata: BranchMetadata {
            created_by: Author::System,
            created_at: chrono::Utc::now(),
            purpose: BranchPurpose::UserExploration {
                description: "independent .ros oracle".to_string(),
            },
            ai_context: None,
            checkpoints: vec![],
        },
        protected: true,
        hidden: false,
    }
}

fn event_kind(event: &TimelineEvent) -> String {
    match &event.operation {
        Operation::Generic { command_type, .. } => command_type.clone(),
        other => format!("{other:?}"),
    }
}

struct RecordedDoc {
    model: BRepModel,
    events: Vec<TimelineEvent>,
}

/// box + cylinder + boolean difference (all with NO intent scope) followed by
/// a fillet on the boolean result recorded INSIDE an `INTENT_OVERRIDE` scope.
/// One document, both provenance states.
async fn build_recorded_document() -> RecordedDoc {
    let timeline: SharedTimeline = Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
    let recorder = TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());

    let mut model = BRepModel::new();
    let bridged: Arc<dyn OperationRecorder> = Arc::new(recorder.clone());
    model.attach_recorder(Some(bridged));

    // No intent scope: these three ops state no reason.
    let box_s = sid(TopologyBuilder::new(&mut model)
        .create_box_3d(40.0, 40.0, 20.0)
        .expect("create_box_3d"));
    let cyl_s = sid(TopologyBuilder::new(&mut model)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -15.0), Vector3::Z, 8.0, 30.0)
        .expect("create_cylinder_3d"));
    let result_solid = boolean_operation(
        &mut model,
        box_s,
        cyl_s,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("box - cylinder difference");

    // The blend, recorded INSIDE an intent scope. `fillet_edges` records the
    // selected edges' persistent ids (`edge_pids`) alongside the numeric ids.
    let corner_edge = pick_vertical_box_edge(&model);
    let fillet_result = INTENT_OVERRIDE
        .scope(
            IntentContext {
                text: INTENT_TEXT.to_string(),
                turn_id: Some(INTENT_TURN.to_string()),
            },
            async {
                fillet_edges(
                    &mut model,
                    result_solid,
                    vec![corner_edge],
                    FilletOptions {
                        fillet_type: FilletType::Constant(2.0),
                        radius: 2.0,
                        ..Default::default()
                    },
                )
            },
        )
        .await;
    fillet_result.expect("fillet of a box corner edge on the boolean result");

    model.attach_recorder(None);
    drop(recorder);

    let main = BranchId::main();
    let mut events: Vec<TimelineEvent> = Vec::new();
    let mut stable_reads = 0;
    for _ in 0..500 {
        let now = timeline
            .read()
            .await
            .get_branch_events(&main, None, None)
            .unwrap_or_default();
        if now.len() >= 4 && now.len() == events.len() {
            stable_reads += 1;
            if stable_reads >= 5 {
                events = now;
                break;
            }
        } else {
            stable_reads = 0;
        }
        events = now;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let kinds: Vec<String> = events.iter().map(event_kind).collect();
    assert!(
        events.len() >= 4,
        "harness precondition: expected >= 4 recorded events, got {}: {kinds:?}",
        events.len()
    );
    assert!(
        kinds.iter().any(|k| k == "fillet_edges"),
        "harness precondition: no fillet_edges event recorded; kinds = {kinds:?}"
    );

    RecordedDoc { model, events }
}

fn out_dir() -> PathBuf {
    let dir = std::env::var("ROSHERA_ORACLE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("roshera_ros_oracle"));
    std::fs::create_dir_all(&dir).expect("create the oracle output directory");
    dir
}

// ═══════════════════════════════════════════════════════════════════════════
// TEST 1 — the oracle's own primitives, against known-good AND known-bad
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn oracle_primitives_bite_on_known_vectors() {
    // SHA-256, FIPS 180-4 / RFC 6234 published vectors.
    assert_eq!(
        hex(&my_sha256(b"")),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        hex(&my_sha256(b"abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        hex(&my_sha256(
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
        )),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
    // KNOWN-BAD: one bit different must not collide.
    assert_ne!(
        hex(&my_sha256(b"abd")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        "SHA-256 must not be insensitive to input"
    );
    // Cross-check the primitive against the `sha2` crate (a different
    // implementation; not part of the .ros reader) over a long input.
    let long: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
    assert_eq!(
        my_sha256(&long),
        ros_format::util::sha256(&long),
        "own SHA-256 disagrees with the sha2 crate"
    );

    // HMAC-SHA256, RFC 4231 published vectors.
    assert_eq!(
        hex(&my_hmac_sha256(&[0x0b; 20], b"Hi There")),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
    assert_eq!(
        hex(&my_hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
    // A key LONGER than the 64-byte block must be hashed first (RFC 4231
    // test case 6). Getting this wrong would produce plausible-looking
    // wrong keys for any long HKDF salt.
    assert_eq!(
        hex(&my_hmac_sha256(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First"
        )),
        "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
    );
    // KNOWN-BAD: one bit of key, and one bit of message, must each move it.
    let mut k = [0x0bu8; 20];
    k[0] ^= 1;
    assert_ne!(
        hex(&my_hmac_sha256(&k, b"Hi There")),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
    assert_ne!(
        my_hmac_sha256(&[0x0b; 20], b"Hi There"),
        my_hmac_sha256(&[0x0b; 20], b"Hi There!")
    );

    // HKDF-SHA256, RFC 5869 test case 1 (first 32 bytes of the 42-byte OKM).
    assert_eq!(
        hex(&my_hkdf_sha256_32(
            &unhex("000102030405060708090a0b0c"),
            &[0x0b; 22],
            &unhex("f0f1f2f3f4f5f6f7f8f9"),
        )),
        "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf"
    );
    // The PRK step, checked separately so a compensating pair of errors in
    // extract and expand cannot cancel out.
    assert_eq!(
        hex(&my_hmac_sha256(
            &unhex("000102030405060708090a0b0c"),
            &[0x0b; 22]
        )),
        "077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5"
    );
    // KNOWN-BAD: salt and info are NOT interchangeable. This is the exact
    // confusion `derive_chunk_key` invites (its HKDF salt is
    // "ROSHERA_CHUNK_V3"‖FourCC while its info is the bare FourCC), so an
    // oracle that could not tell the two apart would be worthless here.
    assert_ne!(
        my_hkdf_sha256_32(b"salt-a", b"ikm", b"info-b"),
        my_hkdf_sha256_32(b"info-b", b"ikm", b"salt-a"),
        "HKDF must not be symmetric in salt and info"
    );
    assert_ne!(
        my_hkdf_sha256_32(b"salt", b"ikm", b"info"),
        my_hkdf_sha256_32(b"salt", b"ikn", b"info"),
        "HKDF must depend on the input keying material"
    );

    // The .ros chunk-key separation property, in the oracle's OWN chain:
    // two chunk types under one file key must never collide, and two file
    // uuids under one master must never collide. Both are what makes the
    // per-chunk IV reuse safe and what the 2026-08-04 fix bound.
    let master = [0x77u8; 32];
    let fk_a = my_hkdf_sha256_32(ORACLE_FILE_KEY_SALT, &master, &[0x01; 16]);
    let fk_b = my_hkdf_sha256_32(ORACLE_FILE_KEY_SALT, &master, &[0x02; 16]);
    assert_ne!(fk_a, fk_b, "the file key must depend on the file uuid");
    let keys_a = OracleKeys {
        master,
        file_key: fk_a,
    };
    assert_ne!(
        keys_a.chunk_key(b"HIST"),
        keys_a.chunk_key(b"PROV"),
        "chunk keys must be separated by FourCC"
    );

    // The chunk IV construction, and that it separates on both axes.
    let iv = oracle_chunk_iv(&[1, 2, 3, 4, 5, 6, 7, 8], b"HIST", 1);
    assert_eq!(&iv[..8], &[1, 2, 3, 4, 5, 6, 7, 8], "file_iv prefix");
    assert_eq!(
        &iv[8..],
        &[b'H' ^ 0, b'I' ^ 0, b'S' ^ 0, b'T' ^ 1],
        "FourCC XOR big-endian chunk index"
    );
    assert_ne!(
        oracle_chunk_iv(&[1, 2, 3, 4, 5, 6, 7, 8], b"HIST", 1),
        oracle_chunk_iv(&[1, 2, 3, 4, 5, 6, 7, 8], b"HIST", 2),
        "the IV must separate chunk indices"
    );
    assert_ne!(
        oracle_chunk_iv(&[1, 2, 3, 4, 5, 6, 7, 8], b"HIST", 1),
        oracle_chunk_iv(&[1, 2, 3, 4, 5, 6, 7, 8], b"PROV", 1),
        "the IV must separate chunk types"
    );

    // AES-256-GCM open, against a locally-sealed message, plus the two
    // rejections the encrypted probes below depend on.
    {
        use aes_gcm::aead::Aead;
        use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
        let key = [0x42u8; 32];
        let nonce = [0x24u8; 12];
        let sealed = Aes256Gcm::new_from_slice(&key)
            .expect("key")
            .encrypt(Nonce::from_slice(&nonce), b"oracle self-test".as_ref())
            .expect("seal");
        assert_eq!(
            sealed.len(),
            b"oracle self-test".len() + ORACLE_GCM_TAG_LEN,
            "AES-GCM appends a 16-byte tag and is otherwise length-preserving"
        );
        assert_eq!(
            oracle_aes_gcm_open(&key, &nonce, &sealed).as_deref(),
            Some(&b"oracle self-test"[..])
        );
        // KNOWN-BAD: a flipped ciphertext byte, a wrong key and a wrong
        // nonce must each reject rather than return garbage.
        let mut broken = sealed.clone();
        broken[0] ^= 0x01;
        assert!(oracle_aes_gcm_open(&key, &nonce, &broken).is_none());
        let mut other_key = key;
        other_key[31] ^= 0x01;
        assert!(oracle_aes_gcm_open(&other_key, &nonce, &sealed).is_none());
        let mut other_nonce = nonce;
        other_nonce[11] ^= 0x01;
        assert!(oracle_aes_gcm_open(&key, &other_nonce, &sealed).is_none());
    }

    // CRC-32/ISO-HDLC check value.
    assert_eq!(my_crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(my_crc32(b""), 0);
    assert_ne!(my_crc32(b"123456780"), 0xCBF4_3926);
    assert_eq!(
        my_crc32(&long),
        ros_format::util::crc32(&long),
        "own CRC-32 disagrees with crc32fast"
    );

    // Merkle: a single leaf's root is its leaf hash; a flipped byte in any
    // leaf changes the root; leaf order matters.
    let one = vec![b"only".to_vec()];
    let expected_single = {
        let mut b = b"leaf:".to_vec();
        b.extend_from_slice(b"only");
        my_sha256(&b)
    };
    assert_eq!(my_merkle_root(&one), Some(expected_single));
    let a = vec![b"aa".to_vec(), b"bb".to_vec(), b"cc".to_vec()];
    let mut b = a.clone();
    b[1][0] ^= 0x01;
    assert_ne!(my_merkle_root(&a), my_merkle_root(&b), "Merkle must bite");
    let swapped = vec![b"bb".to_vec(), b"aa".to_vec(), b"cc".to_vec()];
    assert_ne!(
        my_merkle_root(&a),
        my_merkle_root(&swapped),
        "Merkle must be order-sensitive"
    );
    assert_eq!(my_merkle_root(&[]), None);

    // MessagePack walker: hand-built bytes, known structure.
    // {"a": [1, 2, 3], "b": nil, "c": "hi", 7: 0xff}
    let mp_bytes: Vec<u8> = vec![
        0x84, // fixmap(4)
        0xa1, b'a', 0x93, 0x01, 0x02, 0x03, // "a" -> [1,2,3]
        0xa1, b'b', 0xc0, // "b" -> nil
        0xa1, b'c', 0xa2, b'h', b'i', // "c" -> "hi"
        0x07, 0xcc, 0xff, // 7 -> 255
    ];
    let mp = Mp::new(&mp_bytes, "handbuilt");
    mp.assert_exact_consumption();
    let entries = mp.map(0);
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[3].0, MpKey::Int(7), "integer map keys must decode");
    let arr = mp.array(mp.expect_get(0, "a"));
    assert_eq!(arr.len(), 3);
    assert_eq!(mp.int_at(arr[2]), Some(3));
    assert!(mp.is_nil(mp.expect_get(0, "b")));
    assert_eq!(mp.str_at(mp.expect_get(0, "c")).as_deref(), Some("hi"));
    assert_eq!(mp.int_at(entries[3].1), Some(255));
    assert!(mp.get(0, "nope").is_none());
    // KNOWN-BAD 1: drop the last byte. The trailing `0xcc` marker still
    // parses as a 1-byte-payload integer, so this is caught by the
    // exact-consumption invariant (walk ends at 18, buffer is 17) — the same
    // invariant asserted on every real chunk below.
    let short_one = &mp_bytes[..mp_bytes.len() - 1];
    let bad = std::panic::catch_unwind(|| {
        Mp::new(short_one, "truncated-1").assert_exact_consumption();
    });
    assert!(
        bad.is_err(),
        "the walker accepted a buffer it over-ran — it cannot detect desync"
    );
    // KNOWN-BAD 2: drop the last VALUE entirely. The map header still claims
    // four entries, so the walk runs off the end and the bounds check fires.
    let short_two = &mp_bytes[..mp_bytes.len() - 2];
    let bad2 = std::panic::catch_unwind(|| {
        Mp::new(short_two, "truncated-2").map(0);
    });
    assert!(
        bad2.is_err(),
        "the walker read past the end of its buffer instead of refusing"
    );

    println!("[oracle] primitives: SHA-256, CRC-32, Merkle and the MessagePack walker all bite.");
}

// ═══════════════════════════════════════════════════════════════════════════
// TEST 2 — the whole oracle, against a real signed .ros file
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread")]
async fn independent_oracle_verifies_a_real_signed_ros_file() {
    let mut findings: Vec<String> = Vec::new();
    let mut note = |s: String| {
        println!("[oracle] {s}");
        findings.push(s);
    };

    // ── Produce the artifact ────────────────────────────────────────────
    let RecordedDoc { model, events } = build_recorded_document().await;
    let dir = out_dir();
    let path = dir.join("oracle_signed.ros");
    let history = HistData::new(vec![main_branch_manifest()], events.clone());
    let tracker = ai_tracker_from_timeline(&events, TrackingLevel::Detailed);

    let summary = export_brep_to_ros(
        RosExportPayload {
            model: &model,
            history: Some(history),
            aipr: Some(tracker),
        },
        &path,
        RosExportOptions {
            sign: true,
            signing_key: Some(SIGNING_KEY_SEED),
            ..RosExportOptions::default()
        },
    )
    .await
    .expect("signed export");

    println!("[oracle] artifact written: {}", path.display());
    println!(
        "[oracle] writer reports: {} HIST events, {} PROV commands, replay {:?}",
        summary.hist_event_count, summary.prov_command_count, summary.replay_status
    );

    // ── 2a. Structural decode, entirely our own ─────────────────────────
    let f = OracleFile::open(&path);
    let h = &f.header;
    assert_eq!(&h.magic, b"ROSHERA\0", "magic");
    assert_eq!(
        (h.major, h.minor),
        (3, 2),
        "version must be v3.2 — the integrity scheme (wide header CRC, header + \
         chunk index inside the signature, populated AI hints) is version-gated, \
         so a file written under it must SAY 3.2"
    );
    assert_eq!(h.patch, 0);
    assert_eq!(h.endianness, 1);
    assert!(
        h.reserved.iter().all(|&b| b == 0),
        "header reserved bytes must be zero"
    );
    assert_eq!(h.encryption_algo, 0, "this file is written unencrypted");
    assert_eq!(h.kdf_algo, 0);
    assert_eq!(h.kdf_iterations, 0);
    assert!(h.kdf_salt.iter().all(|&b| b == 0));
    assert!(h.file_iv.iter().all(|&b| b == 0));
    assert!(h.creation_time > 0, "creation_time must be a real clock");

    // Header CRC — recomputed with our own CRC-32, over the range the v3.2
    // SPEC covers: the whole 128-byte header EXCEPT the CRC field itself.
    assert_eq!(
        my_crc32(&f.header_crc_input()),
        h.stored_crc32,
        "header CRC-32 over bytes 0..12 ++ 16..128 must match the stored value"
    );
    // …and it must NOT be the v3.1 range any more. This is the assertion
    // that would go red if the widening were reverted.
    assert_ne!(
        my_crc32(&f.bytes[0..12]),
        h.stored_crc32,
        "the stored header CRC still equals the v3.1 range (bytes 0..12 only) — \
         file_size, index_offset, signature_algo, feature_flags, ai_tracking, \
         kdf_* and file_uuid would again be under no checksum"
    );
    note(format!(
        "CLOSED (was: header_crc32 covers ONLY bytes 0..12): the header CRC-32 now covers \
         bytes 0..12 ++ 16..128 — every header field except the CRC slot itself. \
         stored={:#010x}, recomputed independently over 124 bytes.",
        h.stored_crc32
    ));

    // ── 2b. The chunk table's offsets/sizes must land where it says, and
    //        the file must be fully covered with no gaps or overlaps ─────
    assert_eq!(
        h.index_entry_count as usize,
        f.entries.len(),
        "declared entry count vs decoded entries"
    );
    let names: Vec<String> = f.entries.iter().map(|e| e.name()).collect();
    println!("[oracle] chunk table order: {names:?}");
    for required in [b"META", b"HIST", b"PROV"] {
        assert!(
            f.find(required).is_some(),
            "v3.1 requires the {} chunk",
            String::from_utf8_lossy(required)
        );
    }

    let cursor = f
        .audit_layout()
        .unwrap_or_else(|e| panic!("layout audit failed: {e}"));
    for e in &f.entries {
        assert!(e.uncompressed_size > 0, "{} chunk is empty", e.name());
        assert_eq!(
            e.compressed_size,
            0,
            "{}: writer never compresses; compressed_size must be 0",
            e.name()
        );
        assert_eq!(e.compression, 0, "{}: compression algorithm none", e.name());
        assert!(!e.encrypted, "{}: unencrypted export", e.name());
        assert_eq!(e.version, 1, "{}: chunk entry version", e.name());
        assert!(
            e.reserved.iter().all(|&b| b == 0) && e.reserved_comp.iter().all(|&b| b == 0),
            "{}: reserved bytes must be zero",
            e.name()
        );
        // The remaining index fields. Every one of these is written by the
        // format and read back by no integrity check — see the coverage
        // finding below. Pinning them here documents their real values.
        assert_eq!(e.flags, 0, "{}: chunk flags", e.name());
        assert_eq!(e.comp_level, 0, "{}: compression level", e.name());
        assert_eq!(e.enc_algo, 0, "{}: encryption algorithm id", e.name());
        assert!(
            e.key_id.iter().all(|&b| b == 0),
            "{}: key_id must be zero on an unencrypted chunk",
            e.name()
        );
        assert!(
            e.chunk_iv.iter().all(|&b| b == 0),
            "{}: chunk_iv must be zero on an unencrypted chunk",
            e.name()
        );
        assert!(
            e.auth_tag.iter().all(|&b| b == 0),
            "{}: auth_tag must be zero on an unencrypted chunk",
            e.name()
        );
        assert_eq!(e.access_level, 0, "{}: access level", e.name());
        assert_eq!(e.owner_id, 0, "{}: owner id", e.name());
    }
    note(format!(
        "COVERAGE: the file is {} bytes = 128 header + {} chunk payload + {} index ({} entries). \
         No unreferenced region exists. Under v3.2 the header (minus 20 bytes: header_crc32, \
         file_size, index_offset) and every NON-SIGN index entry are inside the signature's \
         Merkle leaves. What remains outside: the 20 excluded header bytes (file_size is \
         checked against the real file length; index_offset only resolves to signed entries) \
         and SIGN's own 96-byte index entry + payload, which no signature can cover.",
        f.bytes.len(),
        cursor - 128,
        f.entries.len() * 96,
        f.entries.len(),
    ));

    // ── 2c. Declared CRC-32 per chunk vs the actual bytes ───────────────
    for e in &f.entries {
        let actual = my_crc32(f.chunk_bytes(e));
        assert_eq!(
            actual,
            e.declared_crc32,
            "{}: declared crc32 {:#010x} != actual {:#010x} over its on-disk bytes",
            e.name(),
            e.declared_crc32,
            actual
        );
    }
    println!(
        "[oracle] all {} declared chunk CRC-32s match the bytes.",
        f.entries.len()
    );

    // ── 2d. Header claims that no chunk backs ───────────────────────────
    assert_ne!(h.feature_flags & (1 << 5), 0, "ai_provenance flag");
    assert_eq!(
        h.ai_tracking,
        TrackingLevel::Detailed as u8,
        "header ai_tracking must record the writer's level"
    );
    let prov = f.expect(b"PROV");
    // The header's AI-provenance hints used to be dead (both zero on every
    // file, including files whose PROV carried commands). They are now
    // populated — and, because the header is a signed leaf, they are
    // AUTHENTICATED claims rather than decoration.
    assert!(
        summary.prov_command_count > 0,
        "harness precondition: this document must produce PROV commands"
    );
    assert_eq!(
        h.ai_command_count, summary.prov_command_count as u64,
        "header ai_command_count must state the number of commands PROV actually carries, \
         not zero — a hint that reads 'no provenance' on a provenance-bearing file is a \
         field that lies by default"
    );
    assert_eq!(
        h.ai_chunk_offset, prov.offset,
        "header ai_chunk_offset must point at the PROV chunk's real offset"
    );
    note(format!(
        "CLOSED (was: the header's AI-provenance hint fields are dead): ai_command_count = {} \
         matches PROV's {} commands and ai_chunk_offset = {} matches PROV's real offset {}. \
         Both fields sit inside the signed header leaf, so they are now signed statements.",
        h.ai_command_count, summary.prov_command_count, h.ai_chunk_offset, prov.offset
    ));

    // ── 2e. HIST: present, non-empty, matches the exported timeline ─────
    let hist_entry = f.expect(b"HIST");
    let hist_bytes = f.chunk_bytes(hist_entry).to_vec();
    let hist = Mp::new(&hist_bytes, "HIST");
    hist.assert_exact_consumption();
    assert_eq!(
        hist.int_at(hist.expect_get(0, "schema_version")),
        Some(1),
        "HIST schema version"
    );
    let hist_events = hist.array(hist.expect_get(0, "events"));
    assert!(!hist_events.is_empty(), "HIST must be non-empty");
    assert_eq!(
        hist_events.len(),
        events.len(),
        "HIST event count read from raw bytes must equal the exported timeline"
    );
    let hist_seqs: Vec<i64> = hist_events
        .iter()
        .map(|&p| {
            hist.int_at(hist.expect_get(p, "sequence_number"))
                .expect("sequence_number is an integer")
        })
        .collect();
    let expected_seqs: Vec<i64> = events.iter().map(|e| e.sequence_number as i64).collect();
    assert_eq!(
        hist_seqs, expected_seqs,
        "HIST sequence numbers, read byte-for-byte, must equal the exported timeline's"
    );
    let hist_branches = hist.array(hist.expect_get(0, "branches"));
    assert_eq!(hist_branches.len(), 1, "one branch manifest");
    println!(
        "[oracle] HIST: {} events, seqs {hist_seqs:?}",
        hist_events.len()
    );

    // Per-event kinds and the intent facet, read from the file's own bytes.
    // `Operation` is INTERNALLY tagged (`#[serde(tag = "type")]`, types.rs:220),
    // so a Generic op is a flat map:
    // {"type": "Generic", "command_type": .., "parameters": {..}}.
    let mut kinds_on_disk: Vec<String> = Vec::new();
    let mut intent_on_disk: Vec<Option<String>> = Vec::new();
    let mut fillet_index: Option<usize> = None;
    for (i, &ev) in hist_events.iter().enumerate() {
        let op = hist.expect_get(ev, "operation");
        assert_eq!(
            hist.str_at(hist.expect_get(op, "type")).as_deref(),
            Some("Generic"),
            "kernel-recorded events arrive as Operation::Generic"
        );
        let kind = hist
            .str_at(hist.expect_get(op, "command_type"))
            .expect("command_type is a string");
        let params = hist.expect_get(op, "parameters");
        let intent = hist
            .get(params, "facets")
            .and_then(|fa| hist.get(fa, "roshera.intent"))
            .map(|it| {
                hist.str_at(hist.expect_get(it, "text"))
                    .expect("intent text is a string")
            });
        if kind == "fillet_edges" {
            fillet_index = Some(i);
            // The blend must reference its edges by persistent id.
            let inner = hist.expect_get(params, "params");
            let pids = hist.array(hist.expect_get(inner, "edge_pids"));
            assert!(
                !pids.is_empty(),
                "the recorded fillet must carry an edge_pids entry per selected edge"
            );
            let concrete: Vec<String> = pids.iter().filter_map(|&p| hist.str_at(p)).collect();
            assert!(
                !concrete.is_empty(),
                "every recorded edge_pid was null — the blend does not actually reference \
                 its edges by persistent id"
            );
            println!("[oracle] fillet edge_pids on disk: {concrete:?}");
        }
        kinds_on_disk.push(kind);
        intent_on_disk.push(intent);
    }
    let fillet_index = fillet_index.expect("a fillet_edges event exists on disk");
    println!("[oracle] HIST kinds on disk: {kinds_on_disk:?}");
    assert_eq!(
        intent_on_disk[fillet_index].as_deref(),
        Some(INTENT_TEXT),
        "the fillet was recorded inside an intent scope; HIST must carry the text verbatim"
    );
    for (i, intent) in intent_on_disk.iter().enumerate() {
        if i != fillet_index {
            assert!(
                intent.is_none(),
                "event {} ({}) recorded no intent, yet HIST carries one: {:?}",
                i,
                kinds_on_disk[i],
                intent
            );
        }
    }

    // ── 2f. PROV: one command per operation; no intent ⇒ no prompt AND
    //        no prompt hash ────────────────────────────────────────────
    let prov_bytes = f.chunk_bytes(prov).to_vec();
    let mp_prov = Mp::new(&prov_bytes, "PROV");
    mp_prov.assert_exact_consumption();
    assert_eq!(
        mp_prov.int_at(mp_prov.expect_get(0, "schema_version")),
        Some(1)
    );
    let commands = mp_prov.array(mp_prov.expect_get(0, "commands"));
    assert_eq!(
        commands.len(),
        hist_events.len(),
        "PROV command count must equal the HIST event count (one command per recorded \
         operation) — got {} commands for {} events",
        commands.len(),
        hist_events.len()
    );
    let prov_seqs: Vec<i64> = commands
        .iter()
        .map(|&c| {
            mp_prov
                .int_at(mp_prov.expect_get(c, "sequence_num"))
                .expect("sequence_num is an integer")
        })
        .collect();
    assert_eq!(
        prov_seqs, expected_seqs,
        "PROV sequence numbers must mirror the timeline's"
    );

    let expected_prompt_hash = my_sha256(INTENT_TEXT.as_bytes());
    for (i, &c) in commands.iter().enumerate() {
        let prompt_pos = mp_prov.expect_get(c, "prompt");
        let hash = mp_prov.bytes_at(mp_prov.expect_get(c, "prompt_hash"));
        assert_eq!(hash.len(), 32, "prompt_hash is a 32-byte commitment");
        if i == fillet_index {
            assert_eq!(
                mp_prov.str_at(prompt_pos).as_deref(),
                Some(INTENT_TEXT),
                "the operation that stated a reason must carry it as its prompt"
            );
            assert_eq!(
                hash.as_slice(),
                expected_prompt_hash.as_slice(),
                "the prompt hash must be our own SHA-256 of the recorded intent text"
            );
        } else {
            // THE honesty property, checked from raw bytes.
            assert!(
                mp_prov.is_nil(prompt_pos),
                "command {} ({}) recorded no intent but carries a prompt: {:?}",
                i,
                kinds_on_disk[i],
                mp_prov.str_at(prompt_pos)
            );
            assert!(
                hash.iter().all(|&b| b == 0),
                "command {} ({}) recorded no intent but carries a non-zero prompt hash {} — \
                 a commitment to text that was never stated is fabricated provenance",
                i,
                kinds_on_disk[i],
                hex(&hash)
            );
        }
    }
    println!(
        "[oracle] PROV: {} commands; prompt present on index {fillet_index} only, all others \
         nil prompt + all-zero prompt_hash.",
        commands.len()
    );

    // ── 2g. META: replay_status matches an actual replay ────────────────
    let meta_entry = f.expect(b"META");
    let meta_bytes = f.chunk_bytes(meta_entry);
    let meta: serde_json::Value =
        serde_json::from_slice(meta_bytes).expect("META is JSON (parsed off our own byte slice)");
    let claim = meta
        .get("replay_status")
        .expect("META must carry a replay_status; absence is not a pass");
    println!("[oracle] META replay_status claim: {claim}");
    assert_eq!(
        meta.get("vertices").and_then(|v| v.as_u64()),
        Some(model.vertices.len() as u64),
        "META vertex count"
    );

    // Do the replay ourselves and compare against the file's claim. We
    // replay the in-memory event list, having just proven byte-for-byte that
    // HIST carries exactly those events (count + every sequence number).
    let mut replica = BRepModel::new();
    let outcome = timeline_engine::rebuild_model_from_events(&mut replica, &events);
    println!(
        "[oracle] independent replay: applied {}, skipped {}",
        outcome.events_applied, outcome.events_skipped
    );
    let verdict = claim
        .get("verdict")
        .and_then(|v| v.as_str())
        .expect("replay_status carries a verdict tag");
    if outcome.events_skipped == 0 {
        assert_eq!(
            verdict, "verified",
            "our replay applied every event cleanly, so the file must claim `verified`"
        );
        assert_eq!(
            claim.get("events_applied").and_then(|v| v.as_u64()),
            Some(outcome.events_applied as u64),
            "the file's events_applied must equal what actually replays"
        );
        assert_eq!(
            outcome.events_applied,
            events.len(),
            "a clean replay applies every event"
        );
    } else {
        assert_eq!(
            verdict, "incomplete",
            "our replay skipped {} events, so the file must claim `incomplete`, not `{}`",
            outcome.events_skipped, verdict
        );
        assert_eq!(
            claim.get("events_skipped").and_then(|v| v.as_u64()),
            Some(outcome.events_skipped as u64)
        );
        note(format!(
            "NOTE: this document's HIST does not fully replay ({} of {} events skipped); \
             the file states that honestly as `incomplete`.",
            outcome.events_skipped,
            events.len()
        ));
    }

    // ── 2h. SIGN: the chunk genuinely exists, and the signature binds the
    //        chunk bytes — verified through OUR root, OUR leaves ────────
    let sign_entry = f.expect(b"SIGN");
    assert_ne!(
        h.feature_flags & 1,
        0,
        "header signature flag set alongside the chunk"
    );
    assert_eq!(h.signature_algo, 1, "Ed25519");
    let sign_bytes = f.chunk_bytes(sign_entry).to_vec();
    let mp_sign = Mp::new(&sign_bytes, "SIGN");
    mp_sign.assert_exact_consumption();
    let signer = mp_sign.expect_get(0, "signer");
    let public_key = mp_sign.bytes_at(mp_sign.expect_get(signer, "public_key"));
    let signature = mp_sign.bytes_at(mp_sign.expect_get(signer, "signature"));
    let sig_meta = mp_sign.expect_get(signer, "metadata");
    let declared_signer_id = mp_sign.bytes_at(mp_sign.expect_get(sig_meta, "signer_id"));
    let declared_file_id = mp_sign.bytes_at(mp_sign.expect_get(sig_meta, "file_id"));
    assert_eq!(public_key.len(), 32, "Ed25519 public key length");
    assert_eq!(signature.len(), 64, "Ed25519 signature length");

    // The key in the file must be the key the caller supplied.
    let expected_pk = FileSigner::from_bytes(&SIGNING_KEY_SEED, [0u8; 16])
        .expect("derive the caller's verifying key")
        .verifying_key_bytes();
    assert_eq!(
        public_key.as_slice(),
        expected_pk.as_slice(),
        "the file's public key must be the caller-supplied key's, not a minted one"
    );
    // The signer id must be our own SHA-256 of the public key, truncated.
    assert_eq!(
        declared_signer_id.as_slice(),
        &my_sha256(&public_key)[..16],
        "signer_id must be sha256(public_key)[..16]"
    );
    assert_eq!(
        declared_file_id.as_slice(),
        h.file_uuid.as_slice(),
        "the SIGN record's file_id must name this file's UUID"
    );

    // Our leaves, our root.
    let leaves = f.signed_leaves();
    let signed_chunks = f.entries.len() - 1; // every chunk but SIGN
    assert_eq!(
        leaves.len(),
        1 + 2 * signed_chunks,
        "v3.2 leaf shape: 1 normalized header + {signed_chunks} payloads + \
         {signed_chunks} index entries"
    );
    assert_eq!(leaves[0].len(), 128, "the header leaf is the header image");
    assert!(
        leaves[0][12..24].iter().all(|&b| b == 0) && leaves[0][48..56].iter().all(|&b| b == 0),
        "the header leaf must blank header_crc32, file_size and index_offset"
    );
    assert_ne!(
        &leaves[0][56..128],
        &[0u8; 72][..],
        "the rest of the header must NOT be blanked — signature_algo, feature_flags, \
         ai_tracking and the AI hints live there and are the point of the header leaf"
    );
    let root = my_merkle_root(&leaves).expect("non-empty leaf set");
    println!("[oracle] independent Merkle root: {}", hex(&root));

    // Our message, our key bytes, our signature bytes — the metadata we
    // hand in is DELIBERATELY invented, which proves the metadata plays no
    // part in the cryptographic check.
    let invented_metadata = FileSignatureMetadata {
        file_id: [0xAB; 16],
        timestamp: 1,
        signer_id: [0xCD; 16],
        signature_version: 999,
    };
    let mine = SignatureRecord {
        algorithm: SignatureAlgorithm::Ed25519,
        public_key: public_key.clone(),
        signature: signature.clone(),
        metadata: invented_metadata,
    };
    assert!(
        SignatureVerifier::verify_signature(&root, &mine).expect("verification runs"),
        "the Ed25519 signature must verify against OUR Merkle root over OUR leaf set"
    );
    note(
        "FINDING (unauthenticated metadata): the same signature verifies with a SignatureRecord \
         whose file_id / signer_id / timestamp / signature_version were invented by this test. \
         Nothing in the SIGN record's metadata is covered by the signature."
            .to_string(),
    );

    // KNOWN-BAD #1 — a root that differs by one leaf byte must be rejected.
    let mut bad_leaves = leaves.clone();
    bad_leaves[0][0] ^= 0x01;
    let bad_root = my_merkle_root(&bad_leaves).expect("non-empty");
    assert_ne!(root, bad_root);
    assert!(
        !SignatureVerifier::verify_signature(&bad_root, &mine).expect("verification runs"),
        "our verifier accepted a root it should have rejected — the oracle does not bite"
    );

    // KNOWN-BAD #2 — leaf ORDER reversed must be rejected.
    let mut reordered = leaves.clone();
    reordered.reverse();
    if reordered != leaves {
        let reordered_root = my_merkle_root(&reordered).expect("non-empty");
        assert!(
            !SignatureVerifier::verify_signature(&reordered_root, &mine).expect("runs"),
            "leaf order is not bound by the signature"
        );
    }

    // The format's own reader must agree that the file is Verified.
    let imported = import_ros(&path, None)
        .await
        .expect("import the pristine file");
    match &imported.signature {
        RosSignatureVerdict::Verified { public_key: pk, .. } => {
            assert_eq!(pk, &hex(&public_key), "reader's key vs ours");
        }
        other => panic!("the format's reader disagrees with our oracle: {other:?}"),
    }
    match &summary.signature {
        RosWriteSignature::Signed { public_key: pk, .. } => {
            assert_eq!(pk, &hex(&public_key))
        }
        RosWriteSignature::Unsigned => panic!("writer reported Unsigned for a signed export"),
    }
    assert_eq!(
        imported.timeline.len(),
        hist_events.len(),
        "reader's event count vs our byte-level count"
    );
    assert_eq!(
        imported.aipr.commands.len(),
        commands.len(),
        "reader's command count vs our byte-level count"
    );

    // ── 2i. TAMPER A: flip a byte inside a signed chunk ─────────────────
    let tampered_hist = dir.join("oracle_tampered_hist.ros");
    {
        let mut bytes = f.bytes.clone();
        let target = hist_entry.offset as usize + hist_entry.uncompressed_size as usize / 2;
        bytes[target] ^= 0x01;
        std::fs::write(&tampered_hist, &bytes).expect("write tampered file");
    }
    {
        let t = OracleFile::open(&tampered_hist);
        let t_root = my_merkle_root(&t.signed_leaves()).expect("non-empty");
        assert_ne!(t_root, root, "a flipped HIST byte must change our root");
        let t_sign = t.expect(b"SIGN");
        let t_sign_bytes = t.chunk_bytes(t_sign).to_vec();
        let ms = Mp::new(&t_sign_bytes, "SIGN(tampered)");
        let s = ms.expect_get(0, "signer");
        let rec = SignatureRecord {
            algorithm: SignatureAlgorithm::Ed25519,
            public_key: ms.bytes_at(ms.expect_get(s, "public_key")),
            signature: ms.bytes_at(ms.expect_get(s, "signature")),
            metadata: FileSignatureMetadata {
                file_id: [0; 16],
                timestamp: 0,
                signer_id: [0; 16],
                signature_version: 1,
            },
        };
        assert!(
            !SignatureVerifier::verify_signature(&t_root, &rec).expect("runs"),
            "KNOWN-BAD: our oracle accepted a file with a flipped byte inside a signed chunk"
        );
        // Our CRC audit must also fire.
        let t_hist = t.expect(b"HIST");
        assert_ne!(
            my_crc32(t.chunk_bytes(t_hist)),
            t_hist.declared_crc32,
            "the flipped byte must break HIST's declared CRC-32"
        );
        println!("[oracle] TAMPER A (HIST byte flip): signature REJECTED, CRC mismatch detected.");
    }

    // ── 2j. TAMPER B: corrupt a DECLARED CRC in the chunk index ─────────
    // Changes no chunk payload byte. Under v3.1 this left the Merkle root
    // untouched (index entries were outside the leaves) AND passed the
    // reader unchallenged (no CRC was ever validated). Under v3.2 it must
    // trip BOTH.
    let tampered_crc = dir.join("oracle_tampered_declared_crc.ros");
    {
        let mut bytes = f.bytes.clone();
        let geom = f.expect(b"GEOM");
        let crc_field = geom.entry_file_offset + 32;
        bytes[crc_field] ^= 0xFF;
        std::fs::write(&tampered_crc, &bytes).expect("write file");
    }
    {
        let t = OracleFile::open(&tampered_crc);
        let t_root = my_merkle_root(&t.signed_leaves()).expect("non-empty");
        assert_ne!(
            t_root, root,
            "GEOM's index entry is a signed leaf under v3.2, so editing its declared crc32 \
             MUST move the Merkle root — if it does not, the chunk index is outside the \
             signature again"
        );
        let geom = t.expect(b"GEOM");
        let actual = my_crc32(t.chunk_bytes(geom));
        assert_ne!(
            actual, geom.declared_crc32,
            "KNOWN-BAD: our CRC audit failed to notice a corrupted declared CRC"
        );
        // …and the format's own reader must now refuse the file outright.
        match import_ros(&tampered_crc, None).await {
            Ok(imp) => panic!(
                "the reader accepted a file whose GEOM declared crc32 ({:#010x}) does not \
                 match its bytes ({:#010x}); verdict was {:?}",
                geom.declared_crc32, actual, imp.signature
            ),
            Err(e) => {
                let msg = format!("{e:?}");
                assert!(
                    msg.contains("CRC-32"),
                    "the refusal must name the failing CRC, got: {msg}"
                );
                note(format!(
                    "AGREEMENT (was DISAGREEMENT): with GEOM's declared crc32 corrupted \
                     ({:#010x} on disk vs {:#010x} actual), our oracle rejects the file and \
                     `import_ros` now refuses it too — `ChunkIndexEntry::verify_crc` is called \
                     over every chunk in the table. The same edit also moves the Merkle root, \
                     because index entries are signed leaves. Reader refusal: {msg}",
                    geom.declared_crc32, actual
                ));
            }
        }
    }

    // ── 2k. TAMPER C: rewrite the SIGN record's signer_id ───────────────
    // SIGN is excluded from the Merkle leaves, so its own contents are
    // unauthenticated. The reader reports signer_id from this record.
    let tampered_signer = dir.join("oracle_forged_signer_id.ros");
    let forged_signer_hex = {
        let mut bytes = f.bytes.clone();
        // Element positions are chunk-relative; add the chunk's file offset.
        let elems = mp_sign.array(mp_sign.expect_get(sig_meta, "signer_id"));
        assert_eq!(elems.len(), 16, "signer_id is 16 bytes");
        // Mutate the first element encoded as a single-byte fixint, so the
        // encoded length — and therefore every downstream offset — is
        // unchanged and the file stays structurally valid.
        let mut forged: Option<String> = None;
        for (idx, &el) in elems.iter().enumerate() {
            let abs = sign_entry.offset as usize + el;
            if bytes[abs] < 0x80 {
                let new_byte = if bytes[abs] == 0x11 { 0x22 } else { 0x11 };
                bytes[abs] = new_byte;
                let mut v = declared_signer_id.clone();
                v[idx] = new_byte;
                forged = Some(hex(&v));
                break;
            }
        }
        // A COMPETENT tamperer repairs SIGN's declared CRC-32 afterwards.
        // SIGN's index entry is not a Merkle leaf (a signature cannot
        // cover itself), so this repair is free to the attacker and the
        // probe must survive it — otherwise the CRC gate would catch the
        // forgery and we would learn nothing about the derivation check.
        let span = sign_entry.offset as usize
            ..sign_entry.offset as usize + sign_entry.size_on_disk() as usize;
        repair_declared_crc(&mut bytes, sign_entry.entry_file_offset, span);
        std::fs::write(&tampered_signer, &bytes).expect("write file");
        forged
    };
    assert!(
        forged_signer_hex.is_some(),
        "no signer_id byte was encodable in place — cannot run the forgery probe"
    );
    if let Some(forged_hex) = forged_signer_hex {
        let t = OracleFile::open(&tampered_signer);
        let t_root = my_merkle_root(&t.signed_leaves()).expect("non-empty");
        assert_eq!(t_root, root, "SIGN is excluded from the leaves");
        // Our oracle rejects: signer_id must be sha256(pubkey)[..16].
        let t_sign = t.expect(b"SIGN");
        let t_bytes = t.chunk_bytes(t_sign).to_vec();
        let ms = Mp::new(&t_bytes, "SIGN(forged)");
        let s = ms.expect_get(0, "signer");
        let pk = ms.bytes_at(ms.expect_get(s, "public_key"));
        let sid_now = ms.bytes_at(ms.expect_get(ms.expect_get(s, "metadata"), "signer_id"));
        assert_ne!(
            sid_now.as_slice(),
            &my_sha256(&pk)[..16],
            "KNOWN-BAD: our signer-id derivation check failed to notice the forgery"
        );
        // The format's reader must now REFUSE the forged identity — and
        // must do so with the signature itself still verifying, which is
        // exactly why this needs a state of its own.
        match import_ros(&tampered_signer, None).await {
            Ok(imp) => match imp.signature {
                RosSignatureVerdict::ForgedSignerId {
                    declared_signer_id,
                    derived_signer_id,
                    public_key: reported_pk,
                } => {
                    assert_eq!(
                        declared_signer_id, forged_hex,
                        "the verdict must report the id the file actually claims"
                    );
                    assert_eq!(
                        derived_signer_id,
                        hex(&my_sha256(&public_key)[..16]),
                        "the derived id must be OUR sha256(public_key)[..16]"
                    );
                    assert_eq!(reported_pk, hex(&public_key));
                    assert_ne!(
                        declared_signer_id, derived_signer_id,
                        "the two ids must differ — that IS the forgery"
                    );
                    note(format!(
                        "AGREEMENT (was DISAGREEMENT): after rewriting one byte of the SIGN \
                         record's signer_id (SIGN is excluded from the Merkle leaves, so the \
                         root is unchanged and the Ed25519 signature STILL VERIFIES), \
                         `import_ros` now returns ForgedSignerId {{ declared: {declared_signer_id}, \
                         derived: {derived_signer_id} }} instead of Verified. The reader \
                         re-derives signer_id = sha256(public_key)[..16], the same derivation \
                         our oracle performs and the same one the writer performs \
                         unconditionally — so a mismatch has no legitimate producer."
                    ));
                }
                other => panic!(
                    "expected ForgedSignerId for a rewritten signer_id, got {other:?} — \
                     an attacker-chosen identity is being presented beside a genuine signature"
                ),
            },
            Err(e) => panic!("unexpected import failure on the forged-signer file: {e:?}"),
        }
    }

    // ── 2l. TAMPER D: edit header fields the v3.1 CRC did not reach ─────
    // Two probes, because they prove different things.
    //
    // D1 — the NAIVE edit: flip ai_tracking to Forensic and zero
    // creation_time, leaving the stored header CRC alone. v3.2's widened
    // CRC must catch this before anything else runs.
    let tampered_header_naive = dir.join("oracle_tampered_header_naive.ros");
    {
        let mut bytes = f.bytes.clone();
        bytes[67] = 2; // claim Forensic tracking
        bytes[24..32].copy_from_slice(&0u64.to_le_bytes());
        std::fs::write(&tampered_header_naive, &bytes).expect("write file");

        let t = OracleFile::open(&tampered_header_naive);
        assert_ne!(
            my_crc32(&t.header_crc_input()),
            t.header.stored_crc32,
            "the widened header CRC must no longer validate after a byte-67 edit"
        );
        assert_eq!(
            my_crc32(&t.bytes[0..12]),
            my_crc32(&f.bytes[0..12]),
            "sanity: bytes 0..12 are untouched, so the v3.1 CRC range would still have passed"
        );
        match import_ros(&tampered_header_naive, None).await {
            Ok(imp) => panic!(
                "the reader accepted a header-edited file; verdict was {:?}",
                imp.signature
            ),
            Err(e) => println!(
                "[oracle] TAMPER D1 (ai_tracking + creation_time, CRC left stale): \
                 reader REFUSED — {e:?}"
            ),
        }
    }

    // D2 — the COMPETENT edit: the same header rewrite, with the header
    // CRC repaired. A CRC is not a security control; only the signature
    // can catch this. Under v3.1 it did not, because the header was not a
    // leaf. Under v3.2 it must.
    let tampered_header = dir.join("oracle_tampered_header.ros");
    {
        let mut bytes = f.bytes.clone();
        bytes[67] = 2;
        bytes[24..32].copy_from_slice(&0u64.to_le_bytes());
        repair_header_crc(&mut bytes);
        std::fs::write(&tampered_header, &bytes).expect("write file");
    }
    {
        let t = OracleFile::open(&tampered_header);
        assert_eq!(
            my_crc32(&t.header_crc_input()),
            t.header.stored_crc32,
            "the repaired header CRC must validate — a checksum an attacker can recompute \
             is not what protects the header"
        );
        assert_ne!(
            my_merkle_root(&t.signed_leaves()).expect("non-empty"),
            root,
            "a header edit MUST move the Merkle root under v3.2 — the header is a leaf"
        );
        let imported = import_ros(&tampered_header, None)
            .await
            .expect("the file is still structurally readable");
        match &imported.signature {
            RosSignatureVerdict::Invalid { reason } => {
                assert!(
                    reason.contains("does not match"),
                    "the refusal must state the mismatch, got: {reason}"
                );
            }
            other => panic!(
                "a CRC-repaired header rewrite must report Invalid, got {other:?} — \
                 the header is outside the signature again"
            ),
        }
        // The cross-chunk contradiction the header edit creates.
        let prov_now = t.expect(b"PROV");
        let pb = t.chunk_bytes(prov_now).to_vec();
        let mpn = Mp::new(&pb, "PROV");
        let level_pos = mpn.expect_get(0, "tracking_level");
        note(format!(
            "CLOSED (was: the header is outside all integrity coverage): setting header byte 67 \
             (ai_tracking) to Forensic and zeroing creation_time now (a) breaks the widened \
             header CRC if left unrepaired, and (b) breaks the SIGNATURE even when the CRC is \
             repaired, because the normalized header image is a Merkle leaf. The header claims \
             Forensic tracking while PROV's own tracking_level says {:?} — a contradiction the \
             signature now refuses outright instead of leaving to a cross-chunk comparison.",
            mpn.str_at(level_pos)
                .unwrap_or_else(|| format!("<non-string at {level_pos}>"))
        ));
    }

    // ── 2l-bis. TAMPER F: swap two chunks' FourCC labels ────────────────
    // Table ORDER is preserved and no payload byte moves, so under v3.1
    // (payload-only leaves) the root was unchanged and the swap was
    // invisible. Under v3.2 the index entries are leaves, so it must bite.
    let tampered_fourcc = dir.join("oracle_swapped_fourcc.ros");
    {
        let mut bytes = f.bytes.clone();
        let meta_entry = f.expect(b"META");
        let geom_entry = f.expect(b"GEOM");
        bytes[meta_entry.entry_file_offset..meta_entry.entry_file_offset + 4]
            .copy_from_slice(b"GEOM");
        bytes[geom_entry.entry_file_offset..geom_entry.entry_file_offset + 4]
            .copy_from_slice(b"META");
        std::fs::write(&tampered_fourcc, &bytes).expect("write file");

        let t = OracleFile::open(&tampered_fourcc);
        // Every payload byte is exactly where it was.
        assert_eq!(
            t.bytes[128..f.header.index_offset as usize],
            f.bytes[128..f.header.index_offset as usize],
            "the swap must not move a single payload byte — that is what made it invisible"
        );
        assert_ne!(
            my_merkle_root(&t.signed_leaves()).expect("non-empty"),
            root,
            "swapping two FourCC labels MUST move the Merkle root under v3.2"
        );
        match import_ros(&tampered_fourcc, None).await {
            Ok(imp) => assert!(
                !matches!(imp.signature, RosSignatureVerdict::Verified { .. }),
                "a file with swapped chunk labels must not read as Verified: {:?}",
                imp.signature
            ),
            Err(e) => println!("[oracle] TAMPER F (META/GEOM FourCC swap): reader REFUSED — {e:?}"),
        }
        note(
            "CLOSED (was: FourCC labels can be swapped preserving table order and therefore \
             the root): the 96-byte index entries are Merkle leaves under v3.2, so relabelling \
             META as GEOM (and vice versa) moves the root even though no payload byte changes."
                .to_string(),
        );
    }

    // ── 2m. TAMPER E: shift a declared chunk offset by one byte ─────────
    // Proves the layout auditor itself bites: a one-byte shift in GEOM's
    // declared offset makes the chunk regions non-contiguous.
    let tampered_layout = dir.join("oracle_tampered_layout.ros");
    {
        let mut bytes = f.bytes.clone();
        let geom = f.expect(b"GEOM");
        let off_field = geom.entry_file_offset + 8;
        let shifted = geom.offset + 1;
        bytes[off_field..off_field + 8].copy_from_slice(&shifted.to_le_bytes());
        std::fs::write(&tampered_layout, &bytes).expect("write file");

        let t = OracleFile::open(&tampered_layout);
        let audit = t.audit_layout();
        assert!(
            audit.is_err(),
            "KNOWN-BAD: the layout auditor accepted a chunk table with a one-byte hole"
        );
        println!(
            "[oracle] TAMPER E (GEOM offset +1): layout audit REJECTED — {}",
            audit.unwrap_err()
        );
    }

    // ── 2n. The two latent divergences, now pinned by construction ──────
    for e in &f.entries {
        assert_eq!(
            e.size_on_disk(),
            e.uncompressed_size,
            "{}: the SIGNED byte range (size_on_disk) and the PARSED byte range \
             (uncompressed_size) must be the same range",
            e.name()
        );
    }
    note(
        "CLOSED (was: the signed byte range and the parsed byte range are computed by two \
         different rules): `read_chunk_payload` now uses `size_on_disk()` like the signature \
         and `raw_chunk_bytes` do, and `import_ros` REFUSES any chunk declaring compression \
         (compressed_size != 0 or compression != None) because this engine has no \
         decompressor. The state in which the two rules could disagree is unreachable rather \
         than merely unexercised."
            .to_string(),
    );
    assert_eq!(
        h.endianness, 1,
        "the .ros writer pins little-endian; import refuses anything else"
    );
    note(
        "CLOSED (was: FileHeader honours the endianness byte while ChunkIndexEntry hard-codes \
         LittleEndian): the .ros writer now pins `Endianness::Little` explicitly rather than \
         taking the host's, and `import_ros` refuses a big-endian header by name. The format \
         has ONE byte order, so the header and the chunk table cannot be read under different \
         rules."
            .to_string(),
    );

    println!("\n══════ ORACLE FINDINGS ({}) ══════", findings.len());
    for (i, fnd) in findings.iter().enumerate() {
        println!("{:>2}. {fnd}\n", i + 1);
    }
    println!("artifacts:");
    for p in [
        &path,
        &tampered_hist,
        &tampered_crc,
        &tampered_signer,
        &tampered_header_naive,
        &tampered_header,
        &tampered_fourcc,
        &tampered_layout,
    ] {
        println!("  {}", p.display());
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// TEST 3 — the whole oracle, against a real SIGNED + ENCRYPTED .ros file
//
// Until now the oracle asserted `!encrypted` and `compressed_size == 0` as
// PRECONDITIONS and checked only `kdf_algo == 0`. The entire encrypted path —
// the KDF chain, the per-chunk nonces, the AEAD, and the question of what the
// signature actually covers on an encrypted file — was therefore verified by
// `ros_integrity_coverage.rs` alone, which reaches it through `import_ros` and
// `signed_leaf_set`: the format's own reader checking the format's own writer.
// That is precisely the shape that hid a forgeable `signer_id` and a
// padding-smuggling layout hole.
//
// This test is the different method for the encrypted path. It derives every
// key itself, rebuilds every nonce itself, opens every payload itself, and then
// feeds itself four known-bad inputs to prove it bites.
// ═══════════════════════════════════════════════════════════════════════════

/// The password for the encrypted artifact. Fixed, so the derivation is
/// reproducible across runs.
const ENC_PASSWORD: &str = "oracle-encrypted-artifact-password";

/// One character off — enough to change the Argon2id master key and therefore
/// every key below it.
const ENC_WRONG_PASSWORD: &str = "oracle-encrypted-artifact-passworb";

#[tokio::test(flavor = "multi_thread")]
async fn independent_oracle_verifies_a_real_signed_encrypted_ros_file() {
    let mut findings: Vec<String> = Vec::new();
    let mut note = |s: String| {
        println!("[oracle-enc] {s}");
        findings.push(s);
    };

    // ── Produce the artifact ────────────────────────────────────────────
    let RecordedDoc { model, events } = build_recorded_document().await;
    let dir = out_dir();
    let path = dir.join("oracle_signed_encrypted.ros");
    let history = HistData::new(vec![main_branch_manifest()], events.clone());
    let tracker = ai_tracker_from_timeline(&events, TrackingLevel::Detailed);

    let summary = export_brep_to_ros(
        RosExportPayload {
            model: &model,
            history: Some(history),
            aipr: Some(tracker),
        },
        &path,
        RosExportOptions {
            sign: true,
            signing_key: Some(SIGNING_KEY_SEED),
            password: Some(ENC_PASSWORD.to_string()),
            ..RosExportOptions::default()
        },
    )
    .await
    .expect("signed + encrypted export");

    println!("[oracle-enc] artifact written: {}", path.display());

    // ── 3a. Structural decode: the header's KDF declaration ─────────────
    let f = OracleFile::open(&path);
    let h = &f.header;
    assert_eq!(&h.magic, b"ROSHERA\0", "magic");
    assert_eq!((h.major, h.minor), (3, 2), "v3.2 integrity scheme");
    assert_eq!(h.endianness, 1);
    assert_ne!(h.feature_flags & (1 << 4), 0, "encryption feature flag");
    assert_ne!(h.feature_flags & 1, 0, "signature feature flag");
    assert_eq!(
        h.encryption_algo, ORACLE_ENC_ALGO_AES256GCM,
        "an encrypted .ros file declares AES-256-GCM"
    );
    assert_eq!(
        h.kdf_algo, ORACLE_KDF_ALGO_FILE_BOUND,
        "the file must declare the file-uuid-bound Argon2id chain (3). Id 2 is the \
         superseded chain whose file key was expanded from a per-export random id that \
         was never written down — a file declaring it can never be opened by anyone, \
         and the two must be distinguishable ON THE WIRE"
    );
    assert!(
        h.kdf_iterations >= 1 && h.kdf_iterations <= 16,
        "Argon2 t_cost {} is outside the sane range the reader clamps to — a header \
         claiming thousands of passes is a PBKDF2 count in an Argon2 field",
        h.kdf_iterations
    );
    assert_ne!(h.kdf_salt, [0u8; 16], "a real KDF salt");
    assert_ne!(h.file_iv, [0u8; 8], "a real file IV");
    assert_ne!(
        h.file_uuid, [0u8; 16],
        "file_uuid is the KDF file id — an all-zero one would mean every file this \
         writer produces shares a file key for a given password and salt"
    );

    // Header CRC, our own, over the v3.2 range. `kdf_algo`, `kdf_salt`,
    // `file_iv` and `file_uuid` all live inside it (bytes 32..48, 64..96) —
    // under v3.1 they were under no checksum at all.
    assert_eq!(
        my_crc32(&f.header_crc_input()),
        h.stored_crc32,
        "header CRC-32 over bytes 0..12 ++ 16..128"
    );

    // Layout and the declared CRCs, over the CIPHERTEXT as it sits on disk.
    let payload_end = f
        .audit_layout()
        .unwrap_or_else(|e| panic!("layout audit failed: {e}"));
    for e in &f.entries {
        assert_eq!(
            my_crc32(f.chunk_bytes(e)),
            e.declared_crc32,
            "{}: declared CRC-32 must cover the on-disk (encrypted) bytes",
            e.name()
        );
        assert_eq!(
            e.compressed_size,
            0,
            "{}: the writer never compresses",
            e.name()
        );
        assert_eq!(e.size_on_disk(), e.uncompressed_size, "{}", e.name());
    }

    // ── 3b. Which chunks are ciphertext, and which deliberately are not ─
    let mut encrypted_names: Vec<String> = Vec::new();
    for e in &f.entries {
        let plaintext_by_design = &e.fourcc == b"META" || &e.fourcc == b"SIGN";
        assert_eq!(
            e.encrypted,
            !plaintext_by_design,
            "{}: encryption flag",
            e.name()
        );
        if e.encrypted {
            assert_eq!(
                e.enc_algo,
                ORACLE_ENC_ALGO_AES256GCM,
                "{}: enc_algo must name AES-256-GCM",
                e.name()
            );
            encrypted_names.push(e.name());
        } else {
            assert_eq!(e.enc_algo, 0, "{}: enc_algo on a plaintext chunk", e.name());
        }
    }
    assert!(
        encrypted_names.iter().any(|n| n == "HIST") && encrypted_names.iter().any(|n| n == "PROV"),
        "the mandatory content chunks must be encrypted; got {encrypted_names:?}"
    );
    println!("[oracle-enc] encrypted chunks on disk: {encrypted_names:?}");

    // Every FourCC in the table is distinct. Asserted, not assumed: it is
    // the sole reason the writer's nonce rule (positional index) and the
    // reader's (index of the first entry with this FourCC) agree — and
    // nothing in the format enforces it. See `OracleFile::table_index`.
    let mut seen: Vec<[u8; 4]> = Vec::new();
    for e in &f.entries {
        assert!(
            !seen.contains(&e.fourcc),
            "two chunks share the FourCC {} — the writer's per-chunk nonce is its \
             positional index while the reader's is the index of the FIRST entry with \
             that FourCC, so on such a file the two disagree and the second chunk is \
             sealed under one nonce and opened under another. Nothing in \
             `ChunkTable::validate` or the layout audit forbids this shape.",
            e.name()
        );
        seen.push(e.fourcc);
    }

    // The index entry's own `chunk_iv` / `key_id` / `auth_tag` fields stay ZERO
    // even on an encrypted file — the nonce is DERIVED, not stored, and the tag
    // lives inside the ciphertext. Pinning this matters: a writer that started
    // storing the IV there would create a second, unauthenticated source for a
    // value the AEAD depends on.
    for e in &f.entries {
        assert!(
            e.chunk_iv.iter().all(|&b| b == 0),
            "{}: the per-chunk IV is DERIVED from file_iv + FourCC + index, not \
             stored in the index entry",
            e.name()
        );
        assert!(e.key_id.iter().all(|&b| b == 0), "{}: key_id", e.name());
        assert!(e.auth_tag.iter().all(|&b| b == 0), "{}: auth_tag", e.name());
    }

    // ── 3c. Derive every key ourselves, from the file and the password ──
    let t0 = std::time::Instant::now();
    let keys = OracleKeys::derive(ENC_PASSWORD, &h.kdf_salt, h.kdf_iterations, &h.file_uuid);
    let derive_ms = t0.elapsed().as_millis();
    println!(
        "[oracle-enc] own Argon2id(m=64MiB, t={}, p=4) + HKDF chain: {} ms",
        h.kdf_iterations, derive_ms
    );
    assert_ne!(
        keys.master, [0u8; 32],
        "Argon2id produced a zero master key"
    );
    assert_ne!(
        keys.file_key, keys.master,
        "the file key must be an HKDF expansion of the master, not the master itself"
    );

    // ── 3d. Rebuild each nonce and open each payload ourselves ──────────
    let mut plaintexts: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    for e in f.entries.iter().filter(|e| e.encrypted) {
        let idx = f.table_index(e);
        let key = keys.chunk_key(&e.fourcc);
        let iv = oracle_chunk_iv(&h.file_iv, &e.fourcc, idx);
        let ct = f.chunk_bytes(e);
        let pt = oracle_aes_gcm_open(&key, &iv, ct).unwrap_or_else(|| {
            panic!(
                "{}: AES-256-GCM rejected the payload under keys derived ENTIRELY from \
                 the file's own bytes (kdf_salt, kdf_iterations, file_uuid, file_iv) \
                 plus the password. That is the defect fixed on 2026-08-04 — every \
                 encrypted .ros file written before it was undecryptable by its own \
                 author — and this assertion is the independent statement that it is gone.",
                e.name()
            )
        });
        assert_eq!(
            ct.len(),
            pt.len() + ORACLE_GCM_TAG_LEN,
            "{}: on-disk size must be plaintext + one 16-byte GCM tag",
            e.name()
        );
        assert_ne!(
            &ct[..pt.len().min(ct.len())],
            &pt[..],
            "{}: the on-disk bytes are identical to the plaintext — the chunk is \
             flagged encrypted but is not",
            e.name()
        );
        println!(
            "[oracle-enc] {} opened: index {}, iv {}, {} ciphertext -> {} plaintext bytes",
            e.name(),
            idx,
            hex(&iv),
            ct.len(),
            pt.len()
        );
        plaintexts.push((e.fourcc, pt));
    }
    note(format!(
        "VERIFIED INDEPENDENTLY: every encrypted chunk ({}) opens under keys this file \
         derived itself — Argon2id(password, salt=header.kdf_salt, t=header.kdf_iterations, \
         m=64MiB, p=4) -> HKDF-SHA256(salt=ROSHERA_FILE_KEY_V3, info=header.file_uuid) -> \
         HKDF-SHA256(salt=ROSHERA_CHUNK_V3+FourCC, info=FourCC), with the nonce rebuilt as \
         file_iv || FourCC^BE(table index). `generate_key_set` and `ChunkEncryptor` were \
         never called. Derivation cost {} ms at the file's own t_cost={}.",
        encrypted_names.join(", "),
        derive_ms,
        h.kdf_iterations
    ));

    let plain_of = |fourcc: &[u8; 4]| -> Vec<u8> {
        plaintexts
            .iter()
            .find(|(fc, _)| fc == fourcc)
            .map(|(_, p)| p.clone())
            .unwrap_or_else(|| panic!("no plaintext for {}", String::from_utf8_lossy(fourcc)))
    };

    // ── 3e. THE property: the signature covers the CIPHERTEXT ───────────
    // This is what makes `verify_ros_file` possible at all — a custodian who
    // cannot read the file can still establish that it is intact and who signed
    // it. Nothing had independently confirmed it: the format's own reader and
    // writer both call `signed_leaf_set`, so they agree by construction
    // whichever bytes they feed it.
    let ct_leaves = f.signed_leaves();
    let signed_chunks = f.entries.len() - 1;
    assert_eq!(
        ct_leaves.len(),
        1 + 2 * signed_chunks,
        "v3.2 leaf shape is unchanged on an encrypted file: 1 header + n payloads + \
         n index entries"
    );
    let ct_root = my_merkle_root(&ct_leaves).expect("non-empty leaf set");

    let sign_entry = f.expect(b"SIGN");
    let sign_bytes = f.chunk_bytes(sign_entry).to_vec();
    let mp_sign = Mp::new(&sign_bytes, "SIGN");
    mp_sign.assert_exact_consumption();
    let signer = mp_sign.expect_get(0, "signer");
    let public_key = mp_sign.bytes_at(mp_sign.expect_get(signer, "public_key"));
    let signature = mp_sign.bytes_at(mp_sign.expect_get(signer, "signature"));
    let sig_meta = mp_sign.expect_get(signer, "metadata");
    let declared_signer_id = mp_sign.bytes_at(mp_sign.expect_get(sig_meta, "signer_id"));
    assert_eq!(public_key.len(), 32);
    assert_eq!(signature.len(), 64);
    assert_eq!(
        public_key.as_slice(),
        FileSigner::from_bytes(&SIGNING_KEY_SEED, [0u8; 16])
            .expect("derive the caller's verifying key")
            .verifying_key_bytes()
            .as_slice(),
        "the file's public key must be the caller-supplied key's"
    );
    assert_eq!(
        declared_signer_id.as_slice(),
        &my_sha256(&public_key)[..16],
        "signer_id must be sha256(public_key)[..16]"
    );
    assert!(
        !sign_entry.encrypted,
        "SIGN must never be encrypted: a verifier that needed the password to read \
         the signature could not verify without the password"
    );

    let rec = SignatureRecord {
        algorithm: SignatureAlgorithm::Ed25519,
        public_key: public_key.clone(),
        signature: signature.clone(),
        // Deliberately invented — the metadata plays no part in the check.
        metadata: FileSignatureMetadata {
            file_id: [0xAB; 16],
            timestamp: 1,
            signer_id: [0xCD; 16],
            signature_version: 999,
        },
    };
    assert!(
        SignatureVerifier::verify_signature(&ct_root, &rec).expect("verification runs"),
        "the Ed25519 signature must verify against OUR Merkle root over the \
         POST-ENCRYPTION on-disk bytes"
    );

    // …and it must NOT verify over the plaintext. Without this half, "the root
    // over the on-disk bytes verifies" is equally consistent with the signature
    // having been taken over the plaintext — which it cannot be, but the oracle
    // should not have to assume that.
    let pt_leaves = f.signed_leaves_with(|of, e| {
        if e.encrypted {
            plain_of(&e.fourcc)
        } else {
            of.chunk_bytes(e).to_vec()
        }
    });
    let pt_root = my_merkle_root(&pt_leaves).expect("non-empty");
    assert_ne!(
        pt_root, ct_root,
        "the plaintext and ciphertext leaf sets must hash differently"
    );
    assert!(
        !SignatureVerifier::verify_signature(&pt_root, &rec).expect("verification runs"),
        "the signature verified over the PLAINTEXT leaf set — it would then be \
         uncheckable without the password, and `verify_ros_file`'s whole contract \
         would be false"
    );
    note(format!(
        "VERIFIED INDEPENDENTLY (previously confirmed by no method other than the \
         format's own `signed_leaf_set`): the Ed25519 signature verifies against a \
         Merkle root computed over the POST-ENCRYPTION on-disk bytes ({}) and does NOT \
         verify against the same leaf set built from the decrypted payloads ({}). SIGN \
         is itself plaintext. Together these are exactly the conditions under which a \
         custodian with no password can still establish integrity and authorship.",
        hex(&ct_root),
        hex(&pt_root)
    ));

    // ── 3f. The decrypted contents are what the writer put in ───────────
    let hist_plain = plain_of(b"HIST");
    let hist = Mp::new(&hist_plain, "HIST(decrypted)");
    hist.assert_exact_consumption();
    assert_eq!(
        hist.int_at(hist.expect_get(0, "schema_version")),
        Some(1),
        "HIST schema version"
    );
    let hist_events = hist.array(hist.expect_get(0, "events"));
    assert_eq!(
        hist_events.len(),
        events.len(),
        "decrypted HIST event count, read from raw bytes, must equal the exported timeline"
    );
    assert_eq!(
        hist_events.len(),
        summary.hist_event_count,
        "and must equal the writer's own report"
    );
    let hist_seqs: Vec<i64> = hist_events
        .iter()
        .map(|&p| {
            hist.int_at(hist.expect_get(p, "sequence_number"))
                .expect("sequence_number is an integer")
        })
        .collect();
    let expected_seqs: Vec<i64> = events.iter().map(|e| e.sequence_number as i64).collect();
    assert_eq!(
        hist_seqs, expected_seqs,
        "decrypted HIST sequence numbers must equal the exported timeline's"
    );
    assert_eq!(
        hist.array(hist.expect_get(0, "branches")).len(),
        1,
        "one branch manifest"
    );

    let prov_plain = plain_of(b"PROV");
    let mp_prov = Mp::new(&prov_plain, "PROV(decrypted)");
    mp_prov.assert_exact_consumption();
    let commands = mp_prov.array(mp_prov.expect_get(0, "commands"));
    assert_eq!(
        commands.len(),
        summary.prov_command_count,
        "decrypted PROV command count must equal the writer's own report"
    );
    assert_eq!(
        commands.len(),
        hist_events.len(),
        "one PROV command per recorded operation"
    );
    let prov_seqs: Vec<i64> = commands
        .iter()
        .map(|&c| {
            mp_prov
                .int_at(mp_prov.expect_get(c, "sequence_num"))
                .expect("sequence_num is an integer")
        })
        .collect();
    assert_eq!(prov_seqs, expected_seqs, "PROV mirrors the timeline");

    // The header's AI hints are SIGNED statements about a chunk a password-free
    // reader cannot open — so they had better be true.
    let prov_entry = f.expect(b"PROV");
    assert_eq!(
        h.ai_command_count,
        commands.len() as u64,
        "header ai_command_count must state what the ENCRYPTED PROV chunk really \
         carries — the header is readable without the password, so it is the only \
         thing a password-free reader learns about provenance volume"
    );
    assert_eq!(h.ai_chunk_offset, prov_entry.offset, "ai_chunk_offset");

    // META stays plaintext by design, and is readable straight off disk.
    let meta_bytes = f.chunk_bytes(f.expect(b"META"));
    let meta: serde_json::Value =
        serde_json::from_slice(meta_bytes).expect("META is plaintext JSON on an encrypted file");
    assert_eq!(
        meta.get("vertices").and_then(|v| v.as_u64()),
        Some(model.vertices.len() as u64),
        "META vertex count"
    );
    assert!(
        meta.get("replay_status").is_some(),
        "META must carry the file's replay claim even when the content chunks are encrypted"
    );
    note(format!(
        "VERIFIED INDEPENDENTLY: the decrypted HIST carries {} events with sequence \
         numbers {:?}, PROV carries {} commands mirroring them, and the header's signed \
         ai_command_count / ai_chunk_offset agree with the encrypted PROV chunk they \
         describe. META is plaintext by design and readable with no key. All read through \
         this file's own MessagePack walker under the exact-consumption check.",
        hist_events.len(),
        hist_seqs,
        commands.len()
    ));

    // The format's own reader must agree — on the counts and on the verdict.
    let imported = import_ros(&path, Some(ENC_PASSWORD))
        .await
        .expect("the format's reader opens the pristine encrypted file");
    assert_eq!(
        imported.timeline.len(),
        hist_events.len(),
        "reader vs oracle"
    );
    assert_eq!(
        imported.aipr.commands.len(),
        commands.len(),
        "reader vs oracle"
    );
    match &imported.signature {
        RosSignatureVerdict::Verified { public_key: pk, .. } => {
            assert_eq!(pk, &hex(&public_key), "reader's key vs ours")
        }
        other => panic!("the format's reader disagrees with our oracle: {other:?}"),
    }
    match &summary.signature {
        RosWriteSignature::Signed { public_key: pk, .. } => assert_eq!(pk, &hex(&public_key)),
        RosWriteSignature::Unsigned => panic!("writer reported Unsigned for a signed export"),
    }

    // ═══════════════════════════════════════════════════════════════════
    // KNOWN-BAD INPUTS — an oracle that has not been observed failing is
    // not known to work.
    // ═══════════════════════════════════════════════════════════════════

    let hist_entry = f.expect(b"HIST");
    let hist_idx = f.table_index(hist_entry);
    let hist_key = keys.chunk_key(b"HIST");
    let hist_iv = oracle_chunk_iv(&h.file_iv, b"HIST", hist_idx);

    // ── KB1. Flip one ciphertext byte ───────────────────────────────────
    let bad_ct = dir.join("oracle_enc_flipped_ciphertext.ros");
    {
        let mut bytes = f.bytes.clone();
        let target = hist_entry.offset as usize + hist_entry.size_on_disk() as usize / 2;
        bytes[target] ^= 0x01;
        std::fs::write(&bad_ct, &bytes).expect("write");

        let t = OracleFile::open(&bad_ct);
        let t_hist = t.expect(b"HIST");
        assert!(
            oracle_aes_gcm_open(&hist_key, &hist_iv, t.chunk_bytes(t_hist)).is_none(),
            "KNOWN-BAD: our AES-256-GCM open accepted a flipped ciphertext byte — the \
             tag is not being checked"
        );
        assert_ne!(
            my_merkle_root(&t.signed_leaves()).expect("non-empty"),
            ct_root,
            "a flipped ciphertext byte must move our Merkle root — if it does not, the \
             signature does not cover the encrypted payload"
        );
        assert_ne!(
            my_crc32(t.chunk_bytes(t_hist)),
            t_hist.declared_crc32,
            "and it must break HIST's declared CRC-32"
        );
        // The format's own reader refuses with NO password, because every
        // integrity gate precedes key derivation.
        match verify_ros_file(&bad_ct).await {
            Ok(rep) => panic!(
                "the password-free verifier accepted a flipped ciphertext byte: {:?}",
                rep.signature
            ),
            Err(e) => println!("[oracle-enc] KB1: password-free verifier REFUSED — {e:?}"),
        }
        println!(
            "[oracle-enc] KB1 (HIST ciphertext byte flip): tag REJECTED, root moved, CRC broke."
        );
    }

    // ── KB2. The wrong password ─────────────────────────────────────────
    {
        let t1 = std::time::Instant::now();
        let wrong = OracleKeys::derive(
            ENC_WRONG_PASSWORD,
            &h.kdf_salt,
            h.kdf_iterations,
            &h.file_uuid,
        );
        println!(
            "[oracle-enc] second Argon2id derivation (wrong password): {} ms",
            t1.elapsed().as_millis()
        );
        assert_ne!(
            wrong.master, keys.master,
            "a different password must produce a different Argon2id master key"
        );
        assert_ne!(wrong.file_key, keys.file_key, "and a different file key");
        for e in f.entries.iter().filter(|e| e.encrypted) {
            assert_ne!(
                wrong.chunk_key(&e.fourcc),
                keys.chunk_key(&e.fourcc),
                "{}: every chunk key must differ under a wrong password",
                e.name()
            );
            let iv = oracle_chunk_iv(&h.file_iv, &e.fourcc, f.table_index(e));
            assert!(
                oracle_aes_gcm_open(&wrong.chunk_key(&e.fourcc), &iv, f.chunk_bytes(e)).is_none(),
                "KNOWN-BAD: {} opened under the WRONG password",
                e.name()
            );
        }
        // The pristine file is untouched, so its signature still verifies: a
        // wrong password is not tampering, and the two must not be conflated.
        // This is the fact the reader's diagnosis rests on.
        assert!(
            SignatureVerifier::verify_signature(&ct_root, &rec).expect("runs"),
            "a wrong password must not disturb the file's signature"
        );
        println!(
            "[oracle-enc] KB2 (wrong password): every derived key differs, every tag REJECTED."
        );
    }

    // ── KB3. Rewrite file_uuid — the KDF binding, and the signature ─────
    // The whole point of the 2026-08-04 fix is that the file key is expanded
    // from `header.file_uuid`. If that were not true, changing the uuid would
    // leave decryption working.
    let bad_uuid = dir.join("oracle_enc_rewritten_uuid.ros");
    {
        let mut bytes = f.bytes.clone();
        bytes[32] ^= 0x01;
        // A competent tamperer repairs the CRC; otherwise the widened header
        // CRC catches this and nothing is learned about the KDF.
        repair_header_crc(&mut bytes);
        std::fs::write(&bad_uuid, &bytes).expect("write");

        let t = OracleFile::open(&bad_uuid);
        assert_eq!(
            my_crc32(&t.header_crc_input()),
            t.header.stored_crc32,
            "the repaired header CRC must validate — a checksum an attacker can \
             recompute is not what protects the header"
        );
        assert_ne!(t.header.file_uuid, h.file_uuid, "the uuid really changed");

        // Re-derive the WHOLE chain from the rewritten header, Argon2 and
        // all. Reusing `keys.master` here and asserting it equals itself
        // would be a check that cannot fail; running the derivation again
        // makes both halves of the asymmetry measured facts: the master
        // key is Argon2id(password, salt) and must NOT move, the file key
        // is HKDF over file_uuid and MUST.
        let moved = OracleKeys::derive(
            ENC_PASSWORD,
            &t.header.kdf_salt,
            t.header.kdf_iterations,
            &t.header.file_uuid,
        );
        assert_eq!(
            moved.master, keys.master,
            "the master key must NOT depend on file_uuid — it is Argon2id(password, \
             salt), and the salt did not change"
        );
        assert_ne!(
            moved.file_key, keys.file_key,
            "the file key MUST depend on file_uuid — this is the binding the fix \
             introduced, and without it the uuid would be decoration"
        );
        let t_hist = t.expect(b"HIST");
        assert!(
            oracle_aes_gcm_open(
                &moved.chunk_key(b"HIST"),
                &oracle_chunk_iv(&t.header.file_iv, b"HIST", t.table_index(t_hist)),
                t.chunk_bytes(t_hist)
            )
            .is_none(),
            "KNOWN-BAD: keys re-derived from the REWRITTEN uuid opened the chunk — the \
             file key is not actually bound to file_uuid"
        );
        // And the signature must break, because file_uuid (bytes 32..48) lies
        // inside the normalized header leaf.
        assert_ne!(
            my_merkle_root(&t.signed_leaves()).expect("non-empty"),
            ct_root,
            "file_uuid lives at header bytes 32..48, outside every zeroed range, so \
             rewriting it MUST move the signed root"
        );

        // The reader must now say the file was ALTERED — not that the password
        // is wrong. On a signed file that distinction is decidable, and this
        // asserts the reader decides it.
        let err = import_ros(&bad_uuid, Some(ENC_PASSWORD))
            .await
            .err()
            .expect("a rewritten file_uuid must break the derived keys");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("Decryption of HIST failed"),
            "the failure must name the failing decryption: {msg}"
        );
        assert!(
            msg.contains("ALTERED after it was signed"),
            "the CORRECT password was supplied and the file's own signature proves the \
             header was rewritten — the reader must say so rather than let the user \
             conclude they mistyped: {msg}"
        );
        assert!(
            !msg.contains("DIAGNOSIS: the supplied password is wrong"),
            "reporting this as a wrong password would be the lie: {msg}"
        );
        note(format!(
            "VERIFIED INDEPENDENTLY: rewriting one bit of file_uuid (header CRC repaired) \
             leaves the Argon2id master key UNCHANGED and changes the HKDF file key, so \
             every chunk key moves and our own AES-256-GCM open rejects — the file key is \
             genuinely bound to file_uuid. The same edit moves the signed Merkle root, and \
             `import_ros` reports the cause as tampering rather than as a wrong password."
        ));
    }

    // ── KB4. Rewrite file_iv — the nonce, and the signature ─────────────
    let bad_iv = dir.join("oracle_enc_rewritten_file_iv.ros");
    {
        let mut bytes = f.bytes.clone();
        bytes[88] ^= 0x01;
        repair_header_crc(&mut bytes);
        std::fs::write(&bad_iv, &bytes).expect("write");

        let t = OracleFile::open(&bad_iv);
        assert_ne!(t.header.file_iv, h.file_iv, "the file IV really changed");
        let t_hist = t.expect(b"HIST");
        let moved_iv = oracle_chunk_iv(&t.header.file_iv, b"HIST", t.table_index(t_hist));
        assert_ne!(
            moved_iv, hist_iv,
            "the derived nonce must move with file_iv"
        );
        assert!(
            oracle_aes_gcm_open(&hist_key, &moved_iv, t.chunk_bytes(t_hist)).is_none(),
            "KNOWN-BAD: the chunk opened under a nonce rebuilt from a rewritten file_iv \
             — the AEAD nonce is not actually derived from the header"
        );
        // The correct nonce still opens the (unmodified) ciphertext, which
        // proves the rejection above is about the nonce and nothing else.
        assert!(
            oracle_aes_gcm_open(&hist_key, &hist_iv, t.chunk_bytes(t_hist)).is_some(),
            "the ciphertext itself is untouched; only the header moved"
        );
        assert_ne!(
            my_merkle_root(&t.signed_leaves()).expect("non-empty"),
            ct_root,
            "file_iv lives at header bytes 88..96, inside the signed leaf"
        );
        println!(
            "[oracle-enc] KB4 (file_iv rewritten, CRC repaired): nonce moved, tag REJECTED, \
             root moved."
        );
    }

    note(
        "PROOF OF BITE: four known-bad inputs, each observed rejecting in the same run \
         that the pristine file passed — a flipped ciphertext byte (tag + root + CRC), a \
         wrong password (every derived key differs, every tag rejects, signature \
         untouched), a rewritten file_uuid (master unchanged, file key moved, tag \
         rejects, root moves) and a rewritten file_iv (nonce moves, tag rejects, root \
         moves). Both header edits had their CRC-32 repaired first, so neither was caught \
         by a checksum."
            .to_string(),
    );

    println!(
        "[oracle-enc] file is {} bytes = 128 header + {} payload + {} index ({} entries)",
        f.bytes.len(),
        payload_end - 128,
        f.entries.len() * 96,
        f.entries.len()
    );
    println!(
        "\n══════ ENCRYPTED-PATH ORACLE FINDINGS ({}) ══════",
        findings.len()
    );
    for (i, fnd) in findings.iter().enumerate() {
        println!("{:>2}. {fnd}\n", i + 1);
    }
    println!("artifacts:");
    for p in [&path, &bad_ct, &bad_uuid, &bad_iv] {
        println!("  {}", p.display());
    }
}
