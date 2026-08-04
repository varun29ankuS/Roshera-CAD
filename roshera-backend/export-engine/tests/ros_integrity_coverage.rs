// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! .ros v3.2 integrity coverage — the adversarial cases, as regressions.
//!
//! `ros_independent_oracle.rs` found four defects in the v3.1 format by
//! parsing a real signed file from its own byte offsets. This file pins
//! the fixes as ordinary regression tests, so they fail loudly if any of
//! them is reverted, without needing the whole oracle to run:
//!
//! 1. a forged `signer_id` is refused (`ForgedSignerId`) even though the
//!    Ed25519 signature over the file's bytes still verifies;
//! 2. a corrupted DECLARED chunk CRC-32 is refused — the field is
//!    written on every chunk and, until v3.2, validated on none;
//! 3. a rewritten header field is refused twice over: by the widened
//!    header CRC when left unrepaired, and by the SIGNATURE when
//!    repaired;
//! 4. swapped chunk FourCC labels are refused — the 96-byte index
//!    entries are Merkle leaves, so relabelling moves the root even
//!    though no payload byte changes.
//!
//! Plus the two latent divergences the oracle flagged (declared
//! compression, big-endian headers), the file-length check that stands in
//! for signing `file_size`, and the named state a pre-v3.2 signed file
//! reads as.
//!
//! # Adversarial competence
//!
//! Every probe that is meant to prove the SIGNATURE bites first repairs
//! the checksums a real tamperer would repair. A CRC is not a security
//! control — anyone who can edit a byte can recompute it — so a test that
//! is caught by a CRC has proved nothing about the signature. Where both
//! layers should fire, both are asserted, separately.

use export_engine::formats::ros::{
    export_brep_to_ros, import_ros, verify_ros_file, HistData, RosExportOptions, RosExportPayload,
    RosKeyRecoverability, RosReplayStatus, RosSignatureVerdict,
};
use export_engine::formats::timeline_chunk::BranchManifest;
use geometry_engine::primitives::topology_builder::BRepModel;
use ros_format::chunk::ChunkTable;
use ros_format::signature::SignatureChunk;
use ros_format::util::{crc32, sha256, to_hex};
use ros_format::{
    AICommandTracker, ChunkType, CommandType, FileHeader, PrivacySettings, TrackingLevel,
    CHUNK_INDEX_ENTRY_SIZE, HEADER_SIZE,
};
use shared_types::ExportError;
use std::io::Cursor;
use std::path::Path;
use tempfile::TempDir;
use timeline_engine::{
    Author, BranchId, BranchMetadata, BranchPurpose, BranchState, EventId, EventMetadata,
    ForkPoint, Operation, OperationInputs, OperationOutputs, PrimitiveType, TimelineEvent,
};

/// Fixed, caller-supplied Ed25519 seed — the "author's key".
const SIGNING_KEY: [u8; 32] = [0x3cu8; 32];

// ═══════════════════════════════════════════════════════════════════════
// Fixture
// ═══════════════════════════════════════════════════════════════════════

fn synth_event(branch: BranchId, seq: u64) -> TimelineEvent {
    TimelineEvent {
        id: EventId::new(),
        sequence_number: seq,
        timestamp: chrono::Utc::now(),
        author: Author::System,
        operation: Operation::CreatePrimitive {
            primitive_type: PrimitiveType::Box,
            parameters: serde_json::json!({ "size": 1.0 }),
        },
        inputs: OperationInputs::default(),
        outputs: OperationOutputs::default(),
        metadata: EventMetadata {
            description: Some(format!("integrity fixture event {}", seq)),
            branch_id: branch,
            tags: vec![],
            properties: Default::default(),
        },
    }
}

fn synth_branch_manifest(id: BranchId) -> BranchManifest {
    BranchManifest {
        id,
        name: "main".to_string(),
        parent: None,
        fork_point: ForkPoint {
            branch_id: id,
            event_index: 0,
            timestamp: chrono::Utc::now(),
        },
        state: BranchState::Active,
        metadata: BranchMetadata {
            created_by: Author::System,
            created_at: chrono::Utc::now(),
            purpose: BranchPurpose::UserExploration {
                description: "integrity coverage fixture".to_string(),
            },
            ai_context: None,
            checkpoints: vec![],
        },
        protected: true,
        hidden: false,
    }
}

/// Write a signed .ros file and return its bytes.
async fn signed_file(path: &Path) -> Vec<u8> {
    let mut model = BRepModel::new();
    for i in 0..6 {
        model.vertices.add(i as f64, 0.5 * i as f64, 0.0);
    }
    let branch = BranchId::main();
    let history = HistData::new(
        vec![synth_branch_manifest(branch)],
        vec![synth_event(branch, 0), synth_event(branch, 1)],
    );
    export_brep_to_ros(
        RosExportPayload {
            model: &model,
            history: Some(history),
            aipr: None,
        },
        path,
        RosExportOptions {
            sign: true,
            signing_key: Some(SIGNING_KEY),
            ..RosExportOptions::default()
        },
    )
    .await
    .expect("signed export");
    std::fs::read(path).expect("read the signed file back")
}

fn parse(bytes: &[u8]) -> (FileHeader, ChunkTable) {
    let mut cursor = Cursor::new(bytes.to_vec());
    let header = FileHeader::read_from(&mut cursor).expect("header read");
    let table = ChunkTable::read_from(&mut cursor, header.index_offset, header.index_entry_count)
        .expect("chunk table read");
    (header, table)
}

/// Position of a chunk type in the table, and the file offset of its
/// 96-byte index entry.
fn entry_slot(bytes: &[u8], chunk_type: ChunkType) -> (usize, usize) {
    let (header, table) = parse(bytes);
    let pos = table
        .iter()
        .position(|e| e.chunk_type == chunk_type.as_fourcc())
        .unwrap_or_else(|| panic!("{} is not in the chunk table", chunk_type.as_str()));
    (
        pos,
        header.index_offset as usize + pos * CHUNK_INDEX_ENTRY_SIZE,
    )
}

/// Rewrite the header CRC-32 the way a competent tamperer would, under
/// the v3.2 rule (bytes 0..12 ++ 16..128).
fn repair_header_crc(bytes: &mut [u8]) {
    let mut input = bytes[0..12].to_vec();
    input.extend_from_slice(&bytes[16..HEADER_SIZE]);
    let crc = crc32(&input);
    bytes[12..16].copy_from_slice(&crc.to_le_bytes());
}

/// Rewrite one chunk's DECLARED crc32 to match the bytes on disk.
fn repair_declared_crc(bytes: &mut [u8], chunk_type: ChunkType) {
    let (_, table) = parse(bytes);
    let (pos, entry_off) = entry_slot(bytes, chunk_type);
    let entry = table.get(pos).expect("entry");
    let span = entry.offset as usize..(entry.offset + entry.size_on_disk()) as usize;
    let crc = crc32(&bytes[span]);
    bytes[entry_off + 32..entry_off + 36].copy_from_slice(&crc.to_le_bytes());
}

fn expect_export_failed(err: ExportError) -> String {
    match err {
        ExportError::ExportFailed { reason } => reason,
        other => panic!("expected ExportFailed, got {:?}", other),
    }
}

/// Decode a hex string produced by [`to_hex`].
fn from_hex(hex: &str) -> Vec<u8> {
    (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex"))
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════
// Control: the pristine artifact still verifies end to end
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn pristine_signed_file_verifies_and_binds_its_signer_id() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("pristine.ros");
    let bytes = signed_file(&path).await;

    let (header, _) = parse(&bytes);
    assert_eq!(
        (header.major_version, header.minor_version),
        (3, 2),
        "the v3.2 integrity scheme is version-gated, so files written under it must say 3.2"
    );

    let imported = import_ros(&path, None).await.expect("import");
    match &imported.signature {
        RosSignatureVerdict::Verified {
            signer_id,
            public_key,
        } => {
            // `signer_id` is now an authenticated fact, not carried metadata:
            // it must be the derivation of the key beside it.
            let pk_bytes = (0..public_key.len() / 2)
                .map(|i| u8::from_str_radix(&public_key[2 * i..2 * i + 2], 16).expect("hex"))
                .collect::<Vec<u8>>();
            assert_eq!(
                signer_id,
                &to_hex(&sha256(&pk_bytes)[..16]),
                "the reported signer_id must equal sha256(public_key)[..16]"
            );
        }
        other => panic!("pristine signed file must verify, got {:?}", other),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 1. Forged signer_id
// ═══════════════════════════════════════════════════════════════════════

/// Rewrite the SIGN record's `signer_id` while keeping the encoded length
/// identical (so no downstream offset moves) and repairing SIGN's
/// declared CRC (so the CRC gate is not what catches it).
///
/// SIGN is excluded from its own Merkle leaves — a signature cannot cover
/// itself — so this edit leaves the root untouched and the Ed25519
/// signature verifying. Only re-deriving `signer_id` from the record's own
/// public key catches it.
fn forge_signer_id(bytes: &mut Vec<u8>) -> (String, String) {
    let (_, table) = parse(bytes);
    let sign = table
        .find_by_type(ChunkType::SIGN)
        .expect("SIGN entry")
        .clone();
    let span = sign.offset as usize..(sign.offset + sign.size_on_disk()) as usize;
    let chunk = SignatureChunk::deserialize(&bytes[span.clone()]).expect("SIGN parses");

    let original = chunk.signer.metadata.signer_id;
    let mut forged = original;
    // Same MessagePack integer encoding class (<0x80 is a 1-byte fixint,
    // >=0x80 is a 2-byte uint8) so the re-serialized chunk is the same
    // length and the file's layout is untouched.
    forged[0] = if original[0] < 0x80 {
        if original[0] == 0x11 {
            0x22
        } else {
            0x11
        }
    } else if original[0] == 0x91 {
        0x92
    } else {
        0x91
    };
    assert_ne!(forged, original, "the forgery must change the id");

    let mut forged_chunk = chunk.clone();
    forged_chunk.signer.metadata.signer_id = forged;
    let reserialized = forged_chunk.serialize();
    assert_eq!(
        reserialized.len(),
        span.len(),
        "the forged SIGN chunk must be the same length, or the probe would relayout the file"
    );
    bytes[span].copy_from_slice(&reserialized);
    repair_declared_crc(bytes, ChunkType::SIGN);

    (to_hex(&original), to_hex(&forged))
}

#[tokio::test]
async fn forged_signer_id_is_refused_beside_a_valid_signature() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("forged_signer.ros");
    let pristine = signed_file(&path).await;

    // Control: the pristine file verifies, so anything below is caused by
    // the forgery alone.
    assert!(matches!(
        import_ros(&path, None).await.expect("import").signature,
        RosSignatureVerdict::Verified { .. }
    ));

    let mut bytes = pristine.clone();
    let (genuine_id, forged_id) = forge_signer_id(&mut bytes);
    std::fs::write(&path, &bytes).expect("write forged file");

    // The edit must be confined to the SIGN chunk: every other byte, and
    // therefore every Merkle leaf, is untouched.
    let (_, table) = parse(&pristine);
    let sign = table.find_by_type(ChunkType::SIGN).expect("SIGN");
    assert_eq!(
        bytes[..sign.offset as usize],
        pristine[..sign.offset as usize],
        "the forgery must not disturb any signed byte"
    );

    let imported = import_ros(&path, None)
        .await
        .expect("the file is still structurally readable");
    match &imported.signature {
        RosSignatureVerdict::ForgedSignerId {
            declared_signer_id,
            derived_signer_id,
            public_key,
        } => {
            assert_eq!(declared_signer_id, &forged_id, "the id the file claims");
            assert_eq!(
                derived_signer_id, &genuine_id,
                "the id the record's own public key derives to"
            );
            let pk_bytes = (0..public_key.len() / 2)
                .map(|i| u8::from_str_radix(&public_key[2 * i..2 * i + 2], 16).expect("hex"))
                .collect::<Vec<u8>>();
            assert_eq!(
                derived_signer_id,
                &to_hex(&sha256(&pk_bytes)[..16]),
                "the derivation must be sha256(public_key)[..16]"
            );
        }
        RosSignatureVerdict::Verified { signer_id, .. } => panic!(
            "the reader reported Verified {{ signer_id: {} }} for an attacker-chosen \
             identity — a forged IP-attribution claim presented beside a genuine signature",
            signer_id
        ),
        other => panic!(
            "expected ForgedSignerId (the signature itself is genuine), got {:?}",
            other
        ),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 2. Declared chunk CRC-32
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn corrupted_declared_chunk_crc_is_refused() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("bad_declared_crc.ros");
    let mut bytes = signed_file(&path).await;

    // Corrupt GEOM's DECLARED crc32 in the chunk index. No payload byte
    // changes; under v3.1 this passed unchallenged because no code path
    // ever called `Chunk::verify_crc`.
    let (_, entry_off) = entry_slot(&bytes, ChunkType::GEOM);
    bytes[entry_off + 32] ^= 0xFF;
    std::fs::write(&path, &bytes).expect("write");

    let reason = expect_export_failed(
        import_ros(&path, None)
            .await
            .err()
            .expect("a chunk whose declared CRC does not match its bytes must be refused"),
    );
    assert!(
        reason.contains("CRC-32") && reason.contains("GEOM"),
        "the refusal must name the chunk and the failing CRC, got: {reason}"
    );
}

#[tokio::test]
async fn corrupted_payload_with_stale_declared_crc_is_refused() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("bad_payload_crc.ros");
    let mut bytes = signed_file(&path).await;

    // The naive tamper: flip a payload byte and leave the declared CRC
    // stale. The CRC gate must catch this before the signature is
    // consulted — `ros_sign_chunk::tampered_hist_byte_flips_verdict_to_
    // invalid` covers the competent version, where the CRC is repaired
    // and only the signature can bite.
    let (_, table) = parse(&bytes);
    let hist = table.find_by_type(ChunkType::HIST).expect("HIST");
    let target = hist.offset as usize + hist.size_on_disk() as usize / 2;
    bytes[target] ^= 0x01;
    std::fs::write(&path, &bytes).expect("write");

    let reason = expect_export_failed(
        import_ros(&path, None)
            .await
            .err()
            .expect("a payload byte flip with a stale declared CRC must be refused"),
    );
    assert!(
        reason.contains("CRC-32") && reason.contains("HIST"),
        "the refusal must name HIST and the failing CRC, got: {reason}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 3. Header coverage
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn header_field_flip_with_stale_crc_is_refused_by_the_widened_crc() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("header_naive.ros");
    let mut bytes = signed_file(&path).await;

    // Byte 67 is `ai_tracking`. Under v3.1 the header CRC covered bytes
    // 0..12 only, so this edit was invisible to every checksum in the
    // file — the oracle proved it by flipping the header to claim
    // Forensic tracking while PROV said Detailed, and the file still read
    // Verified.
    assert_ne!(bytes[67], 2, "fixture writes Detailed, not Forensic");
    bytes[67] = 2;
    std::fs::write(&path, &bytes).expect("write");

    let reason = expect_export_failed(
        import_ros(&path, None)
            .await
            .err()
            .expect("a header edit with a stale CRC must be refused"),
    );
    assert!(
        reason.contains("Failed to read header") && reason.contains("CRC"),
        "the refusal must come from the header CRC, got: {reason}"
    );
}

#[tokio::test]
async fn header_field_flip_with_repaired_crc_is_refused_by_the_signature() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("header_competent.ros");
    let mut bytes = signed_file(&path).await;

    bytes[67] = 2; // claim Forensic tracking
    repair_header_crc(&mut bytes);
    std::fs::write(&path, &bytes).expect("write");

    // The CRC now validates — as it always will for anyone who can edit
    // the file. Only the signature separates a corrupted header from a
    // rewritten one, and it can only do that because the normalized
    // header image is a Merkle leaf.
    let (header, _) = parse(&bytes);
    assert_eq!(header.ai_tracking, 2, "the header now claims Forensic");

    let imported = import_ros(&path, None)
        .await
        .expect("a CRC-repaired header still parses");
    match &imported.signature {
        RosSignatureVerdict::Invalid { reason } => assert!(
            reason.contains("does not match"),
            "the verdict must state the mismatch, got: {reason}"
        ),
        other => panic!(
            "a rewritten header must break the signature, got {:?} — the header is \
             outside the signature again",
            other
        ),
    }
}

#[tokio::test]
async fn file_length_mismatch_is_refused() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("extended.ros");
    let mut bytes = signed_file(&path).await;

    // `file_size` is excluded from the signed header leaf (it is not
    // known when the signature is computed), so it is checked against the
    // file's real length instead. Appending a trailer is invisible to
    // every checksum and to the signature.
    bytes.push(0x00);
    std::fs::write(&path, &bytes).expect("write");

    let reason = expect_export_failed(
        import_ros(&path, None)
            .await
            .err()
            .expect("a file longer than its declared file_size must be refused"),
    );
    assert!(
        reason.contains("file_size"),
        "the refusal must name file_size, got: {reason}"
    );
}

/// THE competent version of `file_length_mismatch_is_refused`.
///
/// `index_offset` and `file_size` are the two header fields excluded from
/// the signed leaf, and they are exploitable TOGETHER: insert padding
/// between the last payload and the chunk index, bump both by the padding
/// size, and repair the header CRC. Every payload stays at its declared
/// offset (payload leaves unchanged), every 96-byte index entry is
/// byte-identical and merely relocated (entry leaves unchanged), and all
/// three edited header fields are blanked in the header leaf — so the
/// Merkle root does not move and the signature still verifies.
/// `ChunkTable::validate` does not see it either: it checks overlaps, not
/// gaps.
///
/// Only an exact layout audit catches this, which is why `import_ros`
/// carries one.
#[tokio::test]
async fn padding_smuggled_before_the_chunk_index_is_refused() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("smuggled_padding.ros");
    let pristine = signed_file(&path).await;
    let (header, _) = parse(&pristine);

    const PAD: usize = 8;
    let index_start = header.index_offset as usize;
    let mut bytes = Vec::with_capacity(pristine.len() + PAD);
    bytes.extend_from_slice(&pristine[..index_start]);
    bytes.extend_from_slice(&[0xAAu8; PAD]); // the smuggled bytes
    bytes.extend_from_slice(&pristine[index_start..]);

    // Bump the two excluded header fields so the file is self-consistent
    // by every other measure, then repair the CRC.
    let new_index_offset = header.index_offset + PAD as u64;
    let new_file_size = header.file_size + PAD as u64;
    bytes[48..56].copy_from_slice(&new_index_offset.to_le_bytes());
    bytes[16..24].copy_from_slice(&new_file_size.to_le_bytes());
    repair_header_crc(&mut bytes);
    std::fs::write(&path, &bytes).expect("write");

    // Everything a naive reader would check now agrees: the header CRC
    // validates, file_size equals the real length, no chunk overlaps, and
    // every declared chunk CRC still matches its (unmoved) bytes.
    let (tampered_header, tampered_table) = parse(&bytes);
    assert_eq!(tampered_header.file_size, bytes.len() as u64);
    tampered_table
        .validate()
        .expect("the chunk table still validates — it checks overlaps, not gaps");
    for entry in tampered_table.iter() {
        let span = entry.offset as usize..(entry.offset + entry.size_on_disk()) as usize;
        assert!(
            entry.verify_crc(&bytes[span]),
            "{} still matches its declared CRC — no payload byte moved",
            ChunkType::from_fourcc(entry.chunk_type).as_str()
        );
    }

    let reason = match import_ros(&path, None).await {
        Ok(imported) => panic!(
            "padding smuggled into a signed file's dead space was ACCEPTED; the \
             signature verdict was {:?} — the Merkle root does not move, because \
             every payload and every index entry is byte-identical and the three \
             header fields the attacker edited are blanked in the header leaf",
            imported.signature
        ),
        Err(e) => expect_export_failed(e),
    };
    assert!(
        reason.contains("dead space"),
        "the refusal must name the layout hole, got: {reason}"
    );
}

/// The same attack applied after the chunk index rather than before it.
#[tokio::test]
async fn padding_smuggled_after_the_chunk_index_is_refused() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("trailing_padding.ros");
    let mut bytes = signed_file(&path).await;
    let (header, _) = parse(&bytes);

    const PAD: usize = 16;
    bytes.extend_from_slice(&[0x5Au8; PAD]);
    bytes[16..24].copy_from_slice(&(header.file_size + PAD as u64).to_le_bytes());
    repair_header_crc(&mut bytes);
    std::fs::write(&path, &bytes).expect("write");

    let reason = expect_export_failed(
        import_ros(&path, None)
            .await
            .err()
            .expect("a trailer appended to a signed file must be refused"),
    );
    assert!(
        reason.contains("chunk index ends at"),
        "the refusal must name the index end, got: {reason}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 4. Chunk index coverage — FourCC swap
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn swapped_fourcc_labels_are_refused() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("swapped_fourcc.ros");
    let pristine = signed_file(&path).await;
    let mut bytes = pristine.clone();

    // Swap META's and GEOM's labels. Table ORDER is preserved and no
    // payload byte moves, so under v3.1 — where the leaves were the
    // payloads alone — the Merkle root was unchanged and the swap was
    // invisible.
    let (_, meta_off) = entry_slot(&bytes, ChunkType::META);
    let (_, geom_off) = entry_slot(&bytes, ChunkType::GEOM);
    bytes[meta_off..meta_off + 4].copy_from_slice(b"GEOM");
    bytes[geom_off..geom_off + 4].copy_from_slice(b"META");
    std::fs::write(&path, &bytes).expect("write");

    let (header, _) = parse(&pristine);
    assert_eq!(
        bytes[HEADER_SIZE..header.index_offset as usize],
        pristine[HEADER_SIZE..header.index_offset as usize],
        "the swap must not move a single payload byte — that is what made it invisible"
    );

    // NOTE ON WHAT THIS TEST CAN AND CANNOT PROVE. Once META and GEOM
    // trade labels, the chunk the reader now believes is GEOM holds
    // META's JSON, so `import_ros` refuses at the deserialization step
    // regardless of whether the signature covers the index. The literal
    // "swapped FourCC labels" case from the audit is therefore refused,
    // but that refusal is NOT evidence of index coverage — mutation
    // testing confirmed this test stays green with the index removed from
    // the leaf set. The two tests below isolate the signature instead;
    // this one stays because the audit named the case and because a
    // silent success here would still be a defect.
    match import_ros(&path, None).await {
        Ok(imported) => match &imported.signature {
            RosSignatureVerdict::Invalid { .. } => {}
            other => panic!(
                "a file with swapped chunk labels must not report {:?} — the chunk index \
                 is outside the signature again",
                other
            ),
        },
        Err(e) => {
            let reason = expect_export_failed(e);
            assert!(
                reason.contains("GEOM"),
                "the refusal must name the chunk it could not read, got: {reason}"
            );
        }
    }
}

/// Relabel GEOM to an unknown FourCC. The reader then sees a file with no
/// geometry cache — a valid shape — so every parse succeeds and the
/// SIGNATURE is the only thing that can notice that a signed file's
/// contents were silently dropped. Under v3.1 this was invisible: the
/// index was outside the leaves, so the root did not move.
#[tokio::test]
async fn relabelling_a_chunk_is_refused_by_the_signature() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("relabelled.ros");
    let mut bytes = signed_file(&path).await;

    let pristine = import_ros(&path, None).await.expect("import");
    assert!(
        pristine.snapshot.is_some(),
        "fixture precondition: the signed file carries a GEOM snapshot"
    );

    let (_, geom_off) = entry_slot(&bytes, ChunkType::GEOM);
    bytes[geom_off..geom_off + 4].copy_from_slice(b"XTRA");
    std::fs::write(&path, &bytes).expect("write");

    let imported = import_ros(&path, None)
        .await
        .expect("a file with no GEOM chunk is structurally valid and must still parse");
    assert!(
        imported.snapshot.is_none(),
        "the relabel really did drop the geometry cache from the reader's view"
    );
    match &imported.signature {
        RosSignatureVerdict::Invalid { reason } => assert!(
            reason.contains("does not match"),
            "the verdict must state the mismatch, got: {reason}"
        ),
        other => panic!(
            "relabelling a chunk silently removed signed content and the file still \
             reported {:?} — the chunk index is outside the signature",
            other
        ),
    }
}

/// Edit a chunk index field the reader ignores entirely (`access_level`,
/// bytes 80..84 of the 96-byte entry). Nothing parses differently, no CRC
/// covers it, and no cross-chunk comparison can see it. If the signature
/// does not cover the index bytes, nothing in the format ever will.
#[tokio::test]
async fn chunk_index_metadata_edit_is_refused_by_the_signature() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("index_metadata.ros");
    let mut bytes = signed_file(&path).await;

    let (_, geom_off) = entry_slot(&bytes, ChunkType::GEOM);
    assert_eq!(
        &bytes[geom_off + 80..geom_off + 84],
        &[0u8; 4],
        "fixture precondition: access_level starts at zero"
    );
    bytes[geom_off + 80..geom_off + 84].copy_from_slice(&7u32.to_le_bytes());
    std::fs::write(&path, &bytes).expect("write");

    let imported = import_ros(&path, None)
        .await
        .expect("an access_level edit changes nothing structural");
    let (_, table) = parse(&bytes);
    assert_eq!(
        table
            .find_by_type(ChunkType::GEOM)
            .expect("GEOM")
            .access_level,
        7,
        "the edit landed"
    );
    match &imported.signature {
        RosSignatureVerdict::Invalid { reason } => assert!(
            reason.contains("does not match"),
            "the verdict must state the mismatch, got: {reason}"
        ),
        other => panic!(
            "a chunk index byte was rewritten and the file still reported {:?} — no \
             checksum and no parse covers this field, so only the signature can",
            other
        ),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// The two latent divergences the oracle flagged
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn declared_compression_is_refused_rather_than_misread() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("declared_compression.ros");
    let mut bytes = signed_file(&path).await;

    // `size_on_disk()` returns `compressed_size` when non-zero, so a
    // chunk that declares one makes the SIGNED byte range and the PARSED
    // byte range diverge. This engine has no decompressor, so the state
    // is refused rather than silently misread.
    let (_, entry_off) = entry_slot(&bytes, ChunkType::GEOM);
    bytes[entry_off + 24..entry_off + 32].copy_from_slice(&8u64.to_le_bytes());
    std::fs::write(&path, &bytes).expect("write");

    let reason = expect_export_failed(
        import_ros(&path, None)
            .await
            .err()
            .expect("a chunk declaring compression must be refused"),
    );
    assert!(
        reason.contains("compression") && reason.contains("GEOM"),
        "the refusal must name the chunk and the compression claim, got: {reason}"
    );
}

#[tokio::test]
async fn big_endian_header_is_refused() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("big_endian.ros");
    let mut bytes = signed_file(&path).await;

    // `FileHeader` honours the endianness byte; `ChunkIndexEntry` is
    // hard-coded LittleEndian. A file whose two halves disagree cannot be
    // read consistently, so it is refused by name rather than misparsed.
    let mut header = {
        let mut cursor = Cursor::new(bytes.clone());
        FileHeader::read_from(&mut cursor).expect("header read")
    };
    header.endianness = ros_format::Endianness::Big;
    {
        let mut cursor = Cursor::new(&mut bytes);
        header.write_to(&mut cursor).expect("header rewrite");
    }
    std::fs::write(&path, &bytes).expect("write");

    let reason = expect_export_failed(
        import_ros(&path, None)
            .await
            .err()
            .expect("a big-endian header must be refused"),
    );
    assert!(
        reason.contains("big-endian"),
        "the refusal must name the byte order, got: {reason}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Signed AND encrypted — the header leaf now carries the KDF material
// ═══════════════════════════════════════════════════════════════════════

const ENCRYPTION_PASSWORD: &str = "integrity-coverage-password";

/// One character off — enough to change the Argon2id master key and
/// therefore every chunk key.
const WRONG_PASSWORD: &str = "integrity-coverage-passwore";

/// Vertices in the encrypted fixture's model.
const ENCRYPTED_FIXTURE_VERTICES: usize = 4;

/// `kdf_algo` value an encrypted .ros file must carry: Argon2id with the
/// file key bound to the header's `file_uuid`. Written literally here
/// rather than imported from `ros_format::keys`, so a change to the
/// constant cannot silently redefine what this suite is asserting — this
/// is a WIRE value.
const KDF_ALGO_FILE_BOUND_ON_THE_WIRE: u8 = 3;

/// The superseded id, on the wire.
const KDF_ALGO_UNBOUND_ON_THE_WIRE: u8 = 2;

/// Byte offset of `kdf_algo` in the 128-byte header image.
const KDF_ALGO_BYTE: usize = 65;

/// Byte offset of `file_uuid` in the 128-byte header image.
const FILE_UUID_BYTES: std::ops::Range<usize> = 32..48;

/// Write an encrypted .ros file carrying real HIST, PROV and GEOM
/// content, and return the branch its single timeline event belongs to.
async fn encrypted_file(path: &Path, sign: bool) -> BranchId {
    let mut model = BRepModel::new();
    for i in 0..ENCRYPTED_FIXTURE_VERTICES {
        model.vertices.add(i as f64, 0.0, 0.0);
    }
    let branch = BranchId::main();

    let mut tracker =
        AICommandTracker::new(TrackingLevel::Detailed, PrivacySettings::default(), None);
    tracker.start_session(Some("encrypted-fixture".to_string()));
    tracker
        .track_command(
            CommandType::Create,
            [0u8; 32],
            1,
            "encrypt this model",
            "wrote four vertices",
            &["solid:0".to_string()],
            0.91,
            7,
            None,
        )
        .expect("track command");

    export_brep_to_ros(
        RosExportPayload {
            model: &model,
            history: Some(HistData::new(
                vec![synth_branch_manifest(branch)],
                vec![synth_event(branch, 0)],
            )),
            aipr: Some(tracker),
        },
        path,
        RosExportOptions {
            sign,
            signing_key: if sign { Some(SIGNING_KEY) } else { None },
            password: Some(ENCRYPTION_PASSWORD.to_string()),
            ..RosExportOptions::default()
        },
    )
    .await
    .expect("encrypted export");
    branch
}

/// The v3.2 header leaf covers `encryption_algo`, `kdf_algo`,
/// `kdf_iterations`, `kdf_salt` and `file_iv`, which are only non-zero on
/// an encrypted file — so `sign: true` together with `password: Some(..)`
/// exercises header-leaf bytes no other test reaches.
///
/// # History
///
/// The test that stood here asserted the OPPOSITE: that no encrypted
/// .ros file could ever be reopened. That was true and not a mistake.
/// `SoftwareKeyManager::generate_key_set` drew `file_id = random_16()`
/// on every call and expanded the file key — and hence every chunk key —
/// from it, and no writer ever put that id in the file, so an importer
/// invented a different one and the AES-256-GCM tag rejected. Signing was
/// not involved; the unsigned case failed identically. It went unnoticed
/// because the only encryption test in the suite exported and never
/// imported — a write-path test mistaken for coverage.
///
/// The fix binds the KDF file id to `header.file_uuid`, which is already
/// written and, on a signed file, already inside the signed Merkle leaf
/// set — so no wire bytes were added and nothing was removed from the
/// derivation. The `kdf_algo` id moved 2 → 3 to say so on the wire; see
/// `an_encrypted_file_written_under_the_old_kdf_is_refused_by_name`.
#[tokio::test]
async fn an_encrypted_file_round_trips_under_its_own_password() {
    let dir = TempDir::new().expect("tempdir");

    for (label, sign) in [("signed", true), ("unsigned", false)] {
        let path = dir.path().join(format!("encrypted_{label}.ros"));
        let branch = encrypted_file(&path, sign).await;

        let bytes = std::fs::read(&path).expect("read");
        let (header, table) = parse(&bytes);
        assert!(header.feature_flags.encrypted(), "{label}: encrypted flag");
        assert_eq!(header.feature_flags.has_signature(), sign, "{label}: claim");
        assert_ne!(
            header.kdf_salt, [0u8; 16],
            "{label}: an encrypted file carries real KDF salt — header-leaf bytes no \
             other test covers"
        );
        assert_ne!(header.file_iv, [0u8; 8], "{label}: real file IV");
        assert_eq!(
            header.kdf_algo, KDF_ALGO_FILE_BOUND_ON_THE_WIRE,
            "{label}: an encrypted file must declare the file-uuid-bound KDF chain, \
             so a reader can tell it apart from the superseded one whose key material \
             was never persisted"
        );
        assert_eq!(
            bytes[KDF_ALGO_BYTE], KDF_ALGO_FILE_BOUND_ON_THE_WIRE,
            "{label}: kdf_algo must sit at header byte {KDF_ALGO_BYTE} — the offset \
             the signed header leaf and the independent oracle both read"
        );

        // The payloads really are ciphertext: everything except META
        // (plaintext JSON by design) and SIGN (never encrypted, so a
        // verifier can reach it without a password).
        for entry in table.iter() {
            let plaintext_by_design = entry.chunk_type == ChunkType::META.as_fourcc()
                || entry.chunk_type == ChunkType::SIGN.as_fourcc();
            assert_eq!(
                entry.encrypted,
                !plaintext_by_design,
                "{label}: {} chunk encryption flag",
                ChunkType::from_fourcc(entry.chunk_type).as_str()
            );
        }

        // The round trip itself, with the CORRECT password.
        let imported = import_ros(&path, Some(ENCRYPTION_PASSWORD))
            .await
            .unwrap_or_else(|e| panic!("{label}: encrypted round trip failed: {:?}", e));

        // HIST decrypted and parsed.
        assert_eq!(imported.timeline.len(), 1, "{label}: HIST events");
        assert_eq!(
            imported.timeline[0].metadata.branch_id, branch,
            "{label}: HIST event branch"
        );
        assert_eq!(imported.branches.len(), 1, "{label}: HIST branches");

        // PROV decrypted and parsed.
        assert_eq!(
            imported.aipr.tracking_level,
            TrackingLevel::Detailed,
            "{label}: PROV tracking level"
        );
        assert_eq!(imported.aipr.commands.len(), 1, "{label}: PROV commands");
        assert_eq!(
            imported.aipr.commands[0].affected_objects,
            vec!["solid:0".to_string()],
            "{label}: PROV command contents"
        );

        // META (never encrypted) still reads, and the file's own replay
        // claim survives alongside the encrypted chunks. The claim is
        // `Incomplete` here, not `Verified`, because the fixture's
        // synthetic `CreatePrimitive` event has no replay handler — a
        // property of the fixture, not of encryption. What matters is
        // that the writer ATTEMPTED and META carries the real verdict:
        // `NotAttempted` would mean the claim was lost.
        assert!(
            !matches!(imported.replay_status, RosReplayStatus::NotAttempted),
            "{label}: META must carry the writer's real replay verdict, got {:?}",
            imported.replay_status
        );

        // The signature verdict — unreachable on an encrypted file until
        // this fix, because the reader refused to get this far without
        // decrypting.
        match (&imported.signature, sign) {
            (RosSignatureVerdict::Verified { .. }, true) => {}
            (RosSignatureVerdict::Unsigned, false) => {}
            (other, _) => panic!("{label}: unexpected signature verdict {:?}", other),
        }

        // GEOM decrypted, parsed, and materialised.
        assert!(imported.snapshot.is_some(), "{label}: GEOM cache present");
        let model = imported
            .into_model()
            .unwrap_or_else(|e| panic!("{label}: materialise model: {:?}", e));
        assert_eq!(
            model.vertices.len(),
            ENCRYPTED_FIXTURE_VERTICES,
            "{label}: GEOM contents"
        );
    }
}

/// A wrong password is a typed refusal, not a panic and not a garbled
/// parse. The AEAD tag rejects before a single byte reaches
/// `rmp_serde`, which is what keeps a wrong key from being reported as
/// "corrupt file".
#[tokio::test]
async fn a_wrong_password_is_refused_with_a_typed_error() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("wrong_password.ros");
    encrypted_file(&path, true).await;

    let reason = expect_export_failed(
        import_ros(&path, Some(WRONG_PASSWORD))
            .await
            .err()
            .expect("a wrong password must not open the file"),
    );
    assert!(
        reason.contains("Decryption of HIST failed"),
        "the refusal must name the failing decryption, got: {reason}"
    );
    assert!(
        !reason.contains("deserialize"),
        "a wrong password must be caught by the AES-256-GCM tag, never by a \
         MessagePack parse of garbage — that would report a key error as file \
         corruption. Got: {reason}"
    );
    // And the correct password still works on the very same file, so the
    // refusal above is about the key and nothing else.
    import_ros(&path, Some(ENCRYPTION_PASSWORD))
        .await
        .expect("the correct password still opens the file");
}

/// No password at all is refused before anything is decrypted, and with
/// a different message than a wrong one — "you did not supply a key" and
/// "your key is wrong" are different facts.
#[tokio::test]
async fn an_encrypted_file_without_a_password_is_refused_before_decryption() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("no_password.ros");
    encrypted_file(&path, false).await;

    let reason = expect_export_failed(
        import_ros(&path, None)
            .await
            .err()
            .expect("import without the password fails"),
    );
    assert!(
        reason.contains("Password required"),
        "expected the password refusal, got: {reason}"
    );
}

/// `file_uuid` is not decoration in the KDF — it IS the file id every
/// chunk key descends from. Rewrite it (repairing the header CRC the way
/// a competent tamperer would) and the correct password stops working.
///
/// Done on the UNSIGNED file deliberately: on a signed one the signature
/// would also break, which would prove less about the KDF.
#[tokio::test]
async fn rewriting_file_uuid_breaks_decryption_because_the_kdf_is_bound_to_it() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("uuid_rewritten.ros");
    encrypted_file(&path, false).await;

    let mut bytes = std::fs::read(&path).expect("read");
    bytes[FILE_UUID_BYTES.start] ^= 0x01;
    repair_header_crc(&mut bytes);
    std::fs::write(&path, &bytes).expect("write");

    let reason = expect_export_failed(
        import_ros(&path, Some(ENCRYPTION_PASSWORD))
            .await
            .err()
            .expect("a rewritten file_uuid must break the derived keys"),
    );
    assert!(
        reason.contains("Decryption of HIST failed"),
        "expected the AEAD tag to reject keys derived from the rewritten uuid, \
         got: {reason}"
    );
}

/// The reader's `kdf_algo` GATE, in isolation.
///
/// This probe rewrites the id byte on a fresh file, so the artifact it
/// feeds the reader is *mislabelled*, not genuinely pre-fix: its
/// ciphertext is still perfectly openable under the correct password. That
/// is enough to pin the gate itself — that `kdf_algo = 2` is refused by
/// name before Argon2 runs, that `kdf_algo = 9` is declined as merely
/// unimplemented, and that the two states are not collapsed — but it is
/// NOT evidence that the refusal is right about the file.
///
/// The claim it cannot make — that a real pre-fix file's chunk keys are
/// irrecoverable — is proved separately, against a genuinely-unbound
/// artifact, by
/// [`a_genuine_pre_fix_artifact_is_unopenable_and_is_refused_by_name`].
#[tokio::test]
async fn an_encrypted_file_written_under_the_old_kdf_is_refused_by_name() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("legacy_kdf.ros");
    encrypted_file(&path, false).await;

    let mut bytes = std::fs::read(&path).expect("read");
    bytes[KDF_ALGO_BYTE] = KDF_ALGO_UNBOUND_ON_THE_WIRE;
    repair_header_crc(&mut bytes);
    std::fs::write(&path, &bytes).expect("write");

    let reason = expect_export_failed(
        import_ros(&path, Some(ENCRYPTION_PASSWORD))
            .await
            .err()
            .expect("a file under the superseded KDF chain must be refused"),
    );
    assert!(
        reason.contains("REFUSED") && reason.contains("KDF algorithm 2"),
        "the refusal must name the KDF chain, got: {reason}"
    );
    assert!(
        reason.contains("never written into the file") && reason.contains("exists nowhere"),
        "the refusal must say the key material was never persisted — not imply the \
         password was wrong. Got: {reason}"
    );

    // And a password-free verifier still reports the file honestly: it
    // can audit and inventory it, and says outright that its keys are
    // not recoverable.
    let report = verify_ros_file(&path)
        .await
        .expect("a legacy-KDF file is still auditable without a password");
    assert!(report.encrypted);
    assert_eq!(report.kdf_algo, KDF_ALGO_UNBOUND_ON_THE_WIRE);
    assert_eq!(
        report.key_recoverability,
        RosKeyRecoverability::NeverPersisted,
        "a file under the unbound KDF must be named UNRECOVERABLE — not merely \
         locked, and not lumped in with a chain this reader simply does not \
         implement, which is a different and far less final fact"
    );

    // And a chain nobody has ever defined is the OTHER state: this
    // reader declines to guess, but makes no claim that the material is
    // gone.
    let mut bytes = std::fs::read(&path).expect("read");
    bytes[KDF_ALGO_BYTE] = 9;
    repair_header_crc(&mut bytes);
    let unknown_path = dir.path().join("unknown_kdf.ros");
    std::fs::write(&unknown_path, &bytes).expect("write");
    let report = verify_ros_file(&unknown_path).await.expect("verify");
    assert_eq!(
        report.key_recoverability,
        RosKeyRecoverability::UnsupportedChain { kdf_algo: 9 }
    );
    let reason = expect_export_failed(
        import_ros(&unknown_path, Some(ENCRYPTION_PASSWORD))
            .await
            .err()
            .expect("an unknown KDF chain must be refused"),
    );
    assert!(
        reason.contains("will not guess"),
        "an unimplemented chain must be declined, not declared unrecoverable, \
         got: {reason}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// A GENUINE pre-fix artifact
// ═══════════════════════════════════════════════════════════════════════

/// Rewrite an encrypted .ros file's ciphertext so that it is exactly what
/// a PRE-FIX writer would have produced: every chunk re-encrypted under a
/// key set expanded from a `file_id` drawn at random and never written
/// into the file, and the header declaring the superseded `kdf_algo = 2`.
///
/// # Why this, and not a checked-in binary fixture
///
/// A fixture would be an opaque blob whose central property — that its
/// file id was random and is gone — cannot be read off it, checked, or
/// re-derived; a reviewer would have to take the filename's word for it,
/// and the artifact would silently rot the first time an unrelated field
/// moved. Building it here makes the defect legible in source: the id is
/// visibly `random_16()`, it is visibly never stored, and the test still
/// holds it in memory afterwards, which is what lets the assertions below
/// prove the ciphertext is WELL-FORMED under a key nobody can re-derive
/// rather than merely corrupt.
///
/// # Faithfulness
///
/// `SoftwareKeyManager::generate_key_set(password, salt, &random_16())` is
/// byte-for-byte the pre-fix `generate_key_set(password, salt)`: the fix
/// lifted that exact `random_16()` out of the function body and made it a
/// caller-supplied parameter, changing nothing else in the chain
/// (`ros-format/src/keys.rs`). Salt, `t_cost`, `file_iv`, chunk order,
/// per-chunk IVs and AES-256-GCM are all the writer's own, untouched.
///
/// The file is left at the CURRENT format version rather than back-dated
/// to v3.1. The refusal under test keys off `kdf_algo` alone and is
/// version-independent, so holding the version fixed isolates the KDF
/// chain as the single variable; the version-related state (a v≤3.1
/// signature) has its own test.
///
/// Returns `(unbound_key_set, plaintexts_by_fourcc)` — the material a
/// pre-fix export process held in RAM and then dropped forever.
fn rewrite_as_genuine_pre_fix_artifact(
    bytes: &mut [u8],
    password: &str,
) -> (ros_format::KeySet, Vec<([u8; 4], Vec<u8>)>) {
    use ros_format::keys::{KeyManager, SoftwareKeyManager};
    use ros_format::util::random_16;
    use ros_format::{ChunkEncryptor, EncryptionAlgorithm};

    let (header, table) = parse(bytes);
    assert!(
        header.feature_flags.encrypted(),
        "the source file must already be encrypted"
    );
    assert!(
        !header.feature_flags.has_signature(),
        "build the pre-fix artifact from an UNSIGNED file: re-encrypting every \
         payload would break a signature, and the signature breaking is not what \
         this test is about"
    );

    let manager = SoftwareKeyManager::with_clamped_time_cost(header.kdf_iterations);

    // The keys the CURRENT writer used — bound to the header's file_uuid,
    // so they re-derive from bytes the file carries. Needed only to
    // recover the plaintexts.
    let bound = manager
        .generate_key_set(password, &header.kdf_salt, &header.file_uuid)
        .expect("re-derive the writer's bound key set");

    // The keys a PRE-FIX writer used: expanded from an id that exists in
    // this local and nowhere else. It is never written to `bytes`.
    let never_persisted_file_id = random_16();
    assert_ne!(
        never_persisted_file_id, header.file_uuid,
        "a random 16-byte id colliding with the header uuid would silently turn \
         this artifact into a bound one"
    );
    let unbound = manager
        .generate_key_set(password, &header.kdf_salt, &never_persisted_file_id)
        .expect("derive the pre-fix, unbound key set");

    let bound_enc = ChunkEncryptor::new(EncryptionAlgorithm::AES256GCM, bound, header.file_iv);
    let unbound_enc = ChunkEncryptor::new(
        EncryptionAlgorithm::AES256GCM,
        unbound.clone(),
        header.file_iv,
    );

    let mut plaintexts: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    for (i, entry) in table.iter().enumerate() {
        if !entry.encrypted {
            continue;
        }
        let span = entry.offset as usize..(entry.offset + entry.size_on_disk()) as usize;
        let chunk_index = i as u32;
        let plain = bound_enc
            .decrypt_chunk(&entry.chunk_type, &bytes[span.clone()], chunk_index, None)
            .expect("the current writer's own ciphertext must decrypt under its own keys");
        let reciphered = unbound_enc
            .encrypt_chunk(&entry.chunk_type, &plain, chunk_index, None)
            .expect("re-encrypt under the never-persisted key set");
        // AES-256-GCM is length-preserving plus a fixed 16-byte tag, so
        // every offset, every declared size, `index_offset` and
        // `file_size` stay exactly as the writer laid them out. The
        // artifact differs from a real file only where a pre-fix writer's
        // would have.
        assert_eq!(
            reciphered.len(),
            span.len(),
            "re-encryption must be length-preserving or the layout would shift"
        );
        bytes[span].copy_from_slice(&reciphered);
        plaintexts.push((entry.chunk_type, plain));
    }
    assert!(
        !plaintexts.is_empty(),
        "the source file carried no encrypted chunk to rewrite"
    );

    // Declared CRCs first: `repair_declared_crc` re-parses the header,
    // which validates its CRC-32, so the header must still be internally
    // consistent while they are rewritten. Then the wire id that says
    // which chain produced these keys, then the header checksum.
    for (fourcc, _) in &plaintexts {
        repair_declared_crc(bytes, ChunkType::from_fourcc(*fourcc));
    }
    bytes[KDF_ALGO_BYTE] = KDF_ALGO_UNBOUND_ON_THE_WIRE;
    repair_header_crc(bytes);

    (unbound, plaintexts)
}

/// A file whose chunk keys were REALLY expanded from an id that was never
/// written down — not one that merely says so.
///
/// The existing gate test rewrites `kdf_algo` on a fresh file, which
/// proves the reader refuses the id but leaves the substantive claim
/// untested: that such a file's contents are gone. This one builds the
/// artifact, and asserts three things the mislabelled stand-in cannot:
///
/// 1. the file is STRUCTURALLY sound — it passes the whole password-free
///    audit — so the refusal below is the KDF gate and not a layout,
///    CRC or `file_size` reject arriving first;
/// 2. its ciphertext is WELL-FORMED, not corrupt: the key set the builder
///    still holds decrypts every chunk back to the exact bytes the writer
///    put in. This is what makes it a faithful pre-fix artifact rather
///    than a damaged file;
/// 3. that same ciphertext is nevertheless unopenable from the file's own
///    bytes — proved by relabelling it `kdf_algo = 3` to disable the gate
///    and watching the correct password fail on the AEAD tag.
///
/// (3) is also the measurement of what the 2 → 3 id bump actually buys.
/// The SAME bytes, under the SAME correct password, produce a named,
/// final refusal at id 2 and an undecidable tag failure at id 3.
#[tokio::test]
async fn a_genuine_pre_fix_artifact_is_unopenable_and_is_refused_by_name() {
    use ros_format::{ChunkEncryptor, EncryptionAlgorithm};

    let dir = TempDir::new().expect("tempdir");
    let source = dir.path().join("source_for_pre_fix.ros");
    encrypted_file(&source, false).await;

    let mut bytes = std::fs::read(&source).expect("read");
    let original_len = bytes.len();
    let (unbound, plaintexts) =
        rewrite_as_genuine_pre_fix_artifact(&mut bytes, ENCRYPTION_PASSWORD);
    assert_eq!(
        bytes.len(),
        original_len,
        "the artifact must be the same length as a real file of the same content"
    );

    let genuine = dir.path().join("genuine_pre_fix.ros");
    std::fs::write(&genuine, &bytes).expect("write the pre-fix artifact");

    // ── (1) Structurally a valid .ros file ──────────────────────────────
    // Without this, a refusal below could be coming from the layout audit
    // or a stale CRC and the test would prove nothing about the KDF.
    let report = verify_ros_file(&genuine).await.expect(
        "a pre-fix file is still a well-formed .ros file: header CRC, file_size, \
                 chunk table, layout audit and every declared chunk CRC-32 must pass",
    );
    assert!(report.encrypted, "the artifact is encrypted");
    assert_eq!(
        report.kdf_algo, KDF_ALGO_UNBOUND_ON_THE_WIRE,
        "the artifact declares the superseded chain"
    );
    assert_eq!(
        report.key_recoverability,
        RosKeyRecoverability::NeverPersisted,
        "a password-free custodian must be told the material is gone, not merely \
         that the file is locked"
    );
    assert_eq!(
        report.signature,
        RosSignatureVerdict::Unsigned,
        "built unsigned on purpose — see rewrite_as_genuine_pre_fix_artifact"
    );

    // ── (2) The ciphertext is well-formed under the vanished key ────────
    // The builder still holds the id a pre-fix export would have dropped
    // on process exit. Under it, every chunk decrypts to exactly what the
    // writer serialized. So this file is a faithful pre-fix artifact, not
    // a corrupted one — the distinction the whole test rests on.
    let (header, table) = parse(&bytes);
    let unbound_enc = ChunkEncryptor::new(EncryptionAlgorithm::AES256GCM, unbound, header.file_iv);
    let mut recovered = 0usize;
    for (i, entry) in table.iter().enumerate() {
        if !entry.encrypted {
            continue;
        }
        let span = entry.offset as usize..(entry.offset + entry.size_on_disk()) as usize;
        let plain = unbound_enc
            .decrypt_chunk(&entry.chunk_type, &bytes[span], i as u32, None)
            .unwrap_or_else(|e| {
                panic!(
                    "the never-persisted key set must open its own ciphertext — if it \
                     does not, this artifact is corrupt rather than pre-fix: {e}"
                )
            });
        let expected = plaintexts
            .iter()
            .find(|(fourcc, _)| *fourcc == entry.chunk_type)
            .map(|(_, p)| p)
            .unwrap_or_else(|| panic!("no plaintext recorded for {:?}", entry.chunk_type));
        assert_eq!(
            &plain,
            expected,
            "{} decrypts to different bytes than the writer put in",
            ChunkType::from_fourcc(entry.chunk_type).as_str()
        );
        recovered += 1;
    }
    assert_eq!(
        recovered,
        plaintexts.len(),
        "every encrypted chunk must round-trip under the vanished key"
    );

    // ── (3a) With the gate ON: refused by name, before Argon2 ───────────
    let reason = expect_export_failed(
        import_ros(&genuine, Some(ENCRYPTION_PASSWORD))
            .await
            .err()
            .expect("a genuinely unbound file must never open"),
    );
    assert!(
        reason.contains("REFUSED") && reason.contains("KDF algorithm 2"),
        "the refusal must name the chain: {reason}"
    );
    assert!(
        reason.contains("never written into the file") && reason.contains("exists nowhere"),
        "the refusal must state that the material is gone, not imply a wrong \
         password: {reason}"
    );
    assert!(
        reason.contains("Re-export the model from its source"),
        "a final refusal must say what to do instead: {reason}"
    );
    assert!(
        !reason.contains("Decryption of HIST failed"),
        "the gate must fire BEFORE any decryption is attempted — spending Argon2 to \
         reach a tag failure is exactly what the id bump exists to avoid: {reason}"
    );

    // ── (3b) With the gate OFF: the same bytes, the same correct
    //         password, and an undecidable failure ─────────────────────
    // Relabel the identical ciphertext as the file-bound chain. Nothing
    // about the key material changes; only the reader's willingness to
    // try. The correct password now reaches the AEAD tag and is rejected,
    // which is what every pre-fix file did to its own author.
    let mut relabelled = bytes.clone();
    relabelled[KDF_ALGO_BYTE] = KDF_ALGO_FILE_BOUND_ON_THE_WIRE;
    repair_header_crc(&mut relabelled);
    let relabelled_path = dir.path().join("genuine_pre_fix_relabelled.ros");
    std::fs::write(&relabelled_path, &relabelled).expect("write");
    assert_eq!(
        relabelled[HEADER_SIZE..],
        bytes[HEADER_SIZE..],
        "the A/B must differ ONLY in the header — one declared id and its checksum"
    );

    let reason = expect_export_failed(
        import_ros(&relabelled_path, Some(ENCRYPTION_PASSWORD))
            .await
            .err()
            .expect(
                "the correct password must still fail: the keys were never the \
                     file's to reproduce",
            ),
    );
    assert!(
        reason.contains("Decryption of HIST failed"),
        "with the gate disabled the failure must be the AEAD tag: {reason}"
    );
    assert!(
        reason.contains("UNDETERMINED"),
        "and it must be honest that it cannot say WHY — this unsigned file cannot \
         distinguish a wrong password from rewritten key material, which is \
         precisely the outcome the by-name refusal at (3a) replaces: {reason}"
    );
    assert!(
        !reason.contains("exists nowhere"),
        "without the id, the reader has no way to know the material is gone — that \
         knowledge comes from the wire id alone, and this asserts the id is \
         load-bearing rather than decorative: {reason}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Verify without the password
// ═══════════════════════════════════════════════════════════════════════

/// The v3.2 signature covers the POST-encryption on-disk bytes, and SIGN
/// is never encrypted, precisely so integrity is checkable without the
/// ability to read the file. `import_ros` demanded the password before it
/// would compute a verdict, so that property was real and unreachable;
/// `verify_ros_file` is the reachable form.
#[tokio::test]
async fn an_encrypted_signed_file_verifies_with_no_password() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("verify_no_password.ros");
    encrypted_file(&path, true).await;

    let report = verify_ros_file(&path)
        .await
        .expect("an encrypted file must be verifiable without its password");

    match &report.signature {
        RosSignatureVerdict::Verified {
            signer_id,
            public_key,
        } => {
            // The identity is an authenticated fact here too — the
            // no-password path must not report a weaker verdict under
            // the same name.
            assert_eq!(
                signer_id,
                &to_hex(&sha256(&from_hex(public_key))[..16]),
                "signer_id must re-derive from its own public key"
            );
        }
        other => panic!("encrypted+signed file must verify without a password, got {other:?}"),
    }

    assert_eq!(report.version, "3.2.0");
    assert!(
        report.encrypted,
        "the report must state the file is encrypted"
    );
    assert_eq!(report.kdf_algo, KDF_ALGO_FILE_BOUND_ON_THE_WIRE);
    assert_eq!(
        report.key_recoverability,
        RosKeyRecoverability::DerivableFromPassword
    );
    assert_eq!(report.ai_command_count, 1, "the header's own PROV summary");

    // The chunk INVENTORY — types, offsets, sizes, encryption flags and
    // CRCs, all of it metadata, none of it contents.
    let bytes = std::fs::read(&path).expect("read");
    let (header, table) = parse(&bytes);
    assert_eq!(report.file_uuid, to_hex(&header.file_uuid));
    assert_eq!(report.file_size, bytes.len() as u64);
    let reported: Vec<&str> = report
        .chunks
        .iter()
        .map(|c| c.chunk_type.as_str())
        .collect();
    assert_eq!(
        reported,
        vec!["META", "HIST", "PROV", "GEOM", "SIGN"],
        "the inventory must list every chunk, in table order"
    );
    assert_eq!(report.chunks.len(), table.len());
    for (summary, entry) in report.chunks.iter().zip(table.iter()) {
        assert_eq!(summary.offset, entry.offset);
        assert_eq!(summary.size_on_disk, entry.size_on_disk());
        assert_eq!(summary.encrypted, entry.encrypted);
        assert_eq!(summary.enc_algo, entry.enc_algo);
        assert_eq!(summary.crc32, entry.crc32);
    }
    // HIST is encrypted; SIGN is not, which is what makes the verdict
    // above reachable at all.
    let hist = report
        .chunks
        .iter()
        .find(|c| c.chunk_type == "HIST")
        .expect("HIST in the inventory");
    assert!(hist.encrypted, "HIST must be ciphertext on disk");
    let sign = report
        .chunks
        .iter()
        .find(|c| c.chunk_type == "SIGN")
        .expect("SIGN in the inventory");
    assert!(
        !sign.encrypted,
        "SIGN must never be encrypted — a signature nobody can read verifies nothing"
    );
}

/// Tamper with an encrypted signed file's ciphertext and the verdict
/// flips to `Invalid` — still with no password supplied.
///
/// The declared CRC is repaired first, as this file's standard requires:
/// a probe caught by a checksum has proved nothing about the signature.
#[tokio::test]
async fn tampering_an_encrypted_signed_file_flips_the_verdict_with_no_password() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("verify_tampered.ros");
    encrypted_file(&path, true).await;

    // Control: intact, and verified.
    match verify_ros_file(&path).await.expect("verify").signature {
        RosSignatureVerdict::Verified { .. } => {}
        other => panic!("control must verify, got {other:?}"),
    }

    let mut bytes = std::fs::read(&path).expect("read");
    let (_, table) = parse(&bytes);
    let hist_offset = table
        .find_by_type(ChunkType::HIST)
        .expect("HIST entry")
        .offset as usize;
    bytes[hist_offset] ^= 0xFF;
    repair_declared_crc(&mut bytes, ChunkType::HIST);
    repair_header_crc(&mut bytes);
    std::fs::write(&path, &bytes).expect("write");

    let report = verify_ros_file(&path)
        .await
        .expect("a tampered file still yields a verdict, not a parse error");
    match &report.signature {
        RosSignatureVerdict::Invalid { .. } => {}
        other => panic!("tampered ciphertext must read Invalid without a password, got {other:?}"),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// What happens to a file signed BEFORE this change
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn a_pre_v3_2_signed_file_reads_as_superseded_scheme_not_invalid() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("v3_1_signed.ros");
    let mut bytes = signed_file(&path).await;

    // Downgrade the declared minor version to 1 and restore the CRC under
    // the v3.1 rule (bytes 0..12 only) — the on-disk shape of a file
    // written by the old writer.
    bytes[9] = 1;
    let crc = crc32(&bytes[0..12]);
    bytes[12..16].copy_from_slice(&crc.to_le_bytes());
    std::fs::write(&path, &bytes).expect("write");

    let imported = import_ros(&path, None)
        .await
        .expect("a v3.1 file must still parse — only its SIGNATURE is superseded");
    match &imported.signature {
        RosSignatureVerdict::SupersededScheme {
            file_version,
            reason,
        } => {
            assert_eq!(file_version, "3.1.0");
            assert!(
                reason.contains("payloads alone"),
                "the state must explain WHY the old signature is not re-checkable, got: {reason}"
            );
        }
        RosSignatureVerdict::Invalid { reason } => panic!(
            "a v3.1-signed file must not be accused of tampering; it is untampered and \
             signed under a narrower scheme. reason was: {reason}"
        ),
        other => panic!("expected SupersededScheme, got {:?}", other),
    }
    // The file's contents are still readable — the version bump changes
    // what is SIGNED, not what is parseable.
    assert_eq!(imported.timeline.len(), 2, "HIST still reads");
}
