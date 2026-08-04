// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
// Same header `ros_independent_oracle.rs` and `ros_integrity_coverage.rs`
// carry; this file predates the convention and was clippy-red at HEAD.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! .ros SIGN chunk — the signature must be real, not decorative.
//!
//! RED-first context: before this suite, `sign: true` set the header's
//! "I am signed" flag and wrote NO SIGN chunk — a file that asserts a
//! signature it does not carry, with nothing on the read path that
//! would ever notice. These tests pin the repaired contract:
//!
//! 1. a signed file round-trips and VERIFIES, and the SIGN chunk exists
//!    in the chunk table (not merely the header flag);
//! 2. flipping one byte inside the signed region (HIST) flips the
//!    verdict to `Invalid` — the signature detects tampering;
//! 3. a header claiming a signature over a file with no SIGN chunk (the
//!    exact shape every pre-fix "signed" file has) is a hard typed
//!    error;
//! 4. `sign: true` without a signing key is a typed refusal — no
//!    ephemeral throwaway key is minted;
//! 5. an unsigned file imports fine and reports `Unsigned`.

use export_engine::formats::ros::{
    export_brep_to_ros, import_ros, HistData, RosExportOptions, RosExportPayload,
    RosSignatureVerdict, RosWriteSignature,
};
use export_engine::formats::timeline_chunk::BranchManifest;
use geometry_engine::primitives::topology_builder::BRepModel;
use ros_format::chunk::ChunkTable;
use ros_format::{ChunkType, FileHeader};
use shared_types::ExportError;
use std::io::Cursor;
use std::path::Path;
use tempfile::TempDir;
use timeline_engine::{
    Author, BranchId, BranchMetadata, BranchPurpose, BranchState, EventId, EventMetadata,
    ForkPoint, Operation, OperationInputs, OperationOutputs, PrimitiveType, TimelineEvent,
};

/// Fixed, caller-supplied Ed25519 seed — the "author's key" in these tests.
const SIGNING_KEY: [u8; 32] = [7u8; 32];

/// Unique ASCII marker embedded in HIST (as a branch name) so the
/// tamper test can locate a byte that is inside the signed HIST chunk
/// yet keeps the MessagePack structurally valid when flipped (string
/// payload bytes are opaque to the decoder).
const TAMPER_MARKER: &str = "TAMPER-TARGET-BRANCH-8f3a1c";

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
            description: Some(format!("synth event {}", seq)),
            branch_id: branch,
            tags: vec![],
            properties: Default::default(),
        },
    }
}

fn synth_branch_manifest(id: BranchId, name: &str) -> BranchManifest {
    BranchManifest {
        id,
        name: name.to_string(),
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
                description: "sign test branch".to_string(),
            },
            ai_context: None,
            checkpoints: vec![],
        },
        protected: false,
        hidden: false,
    }
}

fn model_with_vertices() -> BRepModel {
    let mut model = BRepModel::new();
    for i in 0..6 {
        model.vertices.add(i as f64, 0.5 * i as f64, 0.0);
    }
    model
}

async fn export_signed(path: &Path) -> export_engine::formats::ros::RosWriteSummary {
    let model = model_with_vertices();
    let branch = BranchId::main();
    let history = HistData::new(
        vec![synth_branch_manifest(branch, TAMPER_MARKER)],
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
    .expect("signed export should succeed")
}

fn read_chunk_table(bytes: Vec<u8>) -> (FileHeader, ChunkTable) {
    let mut cursor = Cursor::new(bytes);
    let header = FileHeader::read_from(&mut cursor).expect("header read");
    let table = ChunkTable::read_from(&mut cursor, header.index_offset, header.index_entry_count)
        .expect("chunk table read");
    (header, table)
}

/// 1. Signed file round-trips, verdict is Verified, and the SIGN chunk
///    EXISTS in the chunk table — the header flag alone is not evidence.
#[tokio::test]
async fn signed_file_roundtrips_verifies_and_carries_sign_chunk() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("signed.ros");
    let summary = export_signed(&path).await;

    // The writer reports the signature and the key it used.
    let (signer_id, public_key) = match &summary.signature {
        RosWriteSignature::Signed {
            signer_id,
            public_key,
        } => (signer_id.clone(), public_key.clone()),
        RosWriteSignature::Unsigned => panic!("writer reported Unsigned for a signed export"),
    };
    assert_eq!(public_key.len(), 64, "hex-encoded 32-byte Ed25519 key");

    // The SIGN chunk must exist in the chunk table on disk.
    let bytes = std::fs::read(&path).expect("read file");
    let (header, table) = read_chunk_table(bytes);
    assert!(
        table.find_by_type(ChunkType::SIGN).is_some(),
        "SIGN chunk must appear in the chunk table, not just the header flag"
    );
    assert!(header.feature_flags.has_signature());
    assert_eq!(header.signature_algo, 1, "Ed25519");

    // Import verifies the signature and reports the same key.
    let imported = import_ros(&path, None).await.expect("import");
    match &imported.signature {
        RosSignatureVerdict::Verified {
            signer_id: v_signer,
            public_key: v_key,
        } => {
            assert_eq!(v_key, &public_key, "verified key == writer-reported key");
            assert_eq!(v_signer, &signer_id);
        }
        other => panic!("expected Verified, got {:?}", other),
    }
    // Geometry still round-trips.
    let rebuilt = imported.into_model().expect("materialise");
    assert_eq!(rebuilt.vertices.len(), 6);
}

/// 2. Tamper detection: flip one byte inside the HIST chunk of a signed
///    file on disk; re-import must yield the typed Invalid verdict.
///    This is the test that proves the signature is real, not
///    decorative — mutation-proven against `verify_signature` returning
///    an unconditional Ok(true).
///
///    STRENGTHENED for .ros v3.2: the reader now validates every chunk's
///    declared CRC-32, so a naive byte flip is refused at the CRC gate
///    before the signature is ever consulted — which would leave this
///    test passing for the wrong reason. It therefore repairs HIST's
///    declared CRC after the flip, exactly as a competent tamperer would
///    (the CRC is unauthenticated on its own; only the signature binds
///    it). The original assertion is unchanged: the verdict must be
///    `Invalid` and the reason must state the mismatch. The naive flip is
///    covered separately by `ros_integrity_coverage.rs`.
#[tokio::test]
async fn tampered_hist_byte_flips_verdict_to_invalid() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("tampered.ros");
    export_signed(&path).await;

    let mut bytes = std::fs::read(&path).expect("read file");

    // Locate the marker string (it lives inside the HIST chunk as a
    // branch name) and flip a byte inside it. Confirm the byte really
    // is within the HIST chunk's on-disk span.
    let marker = TAMPER_MARKER.as_bytes();
    let pos = bytes
        .windows(marker.len())
        .position(|w| w == marker)
        .expect("tamper marker must appear in the file (inside HIST)");
    let (_, table) = read_chunk_table(bytes.clone());
    let hist = table
        .find_by_type(ChunkType::HIST)
        .expect("HIST entry exists");
    let hist_span = hist.offset as usize..(hist.offset + hist.uncompressed_size) as usize;
    assert!(
        hist_span.contains(&pos),
        "marker at {} must be inside HIST span {:?}",
        pos,
        hist_span
    );
    bytes[pos] ^= 0x01; // flip one bit inside the signed region

    // Repair HIST's declared CRC-32 so the flip survives the reader's
    // CRC gate and the SIGNATURE is what has to catch it. The declared
    // CRC lives at index-entry offset +32 (see `ChunkIndexEntry::
    // write_to`); HIST's entry is the i-th 96-byte slot in the index.
    {
        let (header, table) = read_chunk_table(bytes.clone());
        let hist_pos = table
            .iter()
            .position(|e| e.chunk_type == ChunkType::HIST.as_fourcc())
            .expect("HIST is in the table");
        let hist_entry = table.get(hist_pos).expect("HIST entry");
        let span =
            hist_entry.offset as usize..(hist_entry.offset + hist_entry.size_on_disk()) as usize;
        let repaired = ros_format::util::crc32(&bytes[span]);
        let crc_field =
            header.index_offset as usize + hist_pos * ros_format::CHUNK_INDEX_ENTRY_SIZE + 32;
        bytes[crc_field..crc_field + 4].copy_from_slice(&repaired.to_le_bytes());
    }
    std::fs::write(&path, &bytes).expect("write tampered file");

    let imported = import_ros(&path, None)
        .await
        .expect("import still parses (string payload bytes are opaque)");
    match &imported.signature {
        RosSignatureVerdict::Invalid { reason } => {
            assert!(
                reason.contains("does not match"),
                "reason should state the mismatch, got: {reason}"
            );
        }
        other => panic!(
            "tampered file must report Invalid, got {:?} — the signature is decorative",
            other
        ),
    }
}

/// 3. A header claiming a signature with no SIGN chunk — the exact
///    on-disk shape every file written by the pre-fix code has — is a
///    hard typed error, never a warning, never a silent pass.
#[tokio::test]
async fn header_signature_claim_without_sign_chunk_is_hard_error() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("forged_flag.ros");

    // Write a legitimate UNSIGNED file…
    let model = model_with_vertices();
    export_brep_to_ros(
        RosExportPayload {
            model: &model,
            history: None,
            aipr: None,
        },
        &path,
        RosExportOptions::default(),
    )
    .await
    .expect("unsigned export");

    // …then forge the header's signature claim, exactly what the old
    // `header.with_signature(1)`-only code produced.
    let mut bytes = std::fs::read(&path).expect("read file");
    let mut header = {
        let mut cursor = Cursor::new(bytes.clone());
        FileHeader::read_from(&mut cursor).expect("header read")
    };
    header.signature_algo = 1;
    header.feature_flags = header.feature_flags.with_signature();
    {
        let mut cursor = Cursor::new(&mut bytes);
        header.write_to(&mut cursor).expect("header rewrite");
    }
    std::fs::write(&path, &bytes).expect("write forged file");

    let err = match import_ros(&path, None).await {
        Err(e) => e,
        Ok(_) => panic!("a forged signature claim must be a hard error, but import succeeded"),
    };
    match err {
        ExportError::ExportFailed { reason } => {
            assert!(
                reason.contains("no SIGN chunk"),
                "error must name the missing SIGN chunk, got: {reason}"
            );
        }
        other => panic!("expected ExportFailed, got {:?}", other),
    }
}

/// 4. `sign: true` with no signing key is a typed refusal — the writer
///    must not mint an ephemeral key whose signature proves nothing
///    about authorship.
#[tokio::test]
async fn sign_without_key_is_typed_refusal() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("refused.ros");
    let model = model_with_vertices();

    let err = export_brep_to_ros(
        RosExportPayload {
            model: &model,
            history: None,
            aipr: None,
        },
        &path,
        RosExportOptions {
            sign: true,
            signing_key: None,
            ..RosExportOptions::default()
        },
    )
    .await
    .expect_err("sign without key must be refused");
    match err {
        ExportError::ExportFailed { reason } => {
            assert!(
                reason.starts_with("REFUSED:") && reason.contains("signing key"),
                "refusal must be explicit, got: {reason}"
            );
        }
        other => panic!("expected ExportFailed refusal, got {:?}", other),
    }
}

/// 5. Signing stays optional: an unsigned file imports fine and reports
///    `Unsigned` — distinct from any failure state.
#[tokio::test]
async fn unsigned_file_imports_and_reports_unsigned() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("unsigned.ros");
    let model = model_with_vertices();

    let summary = export_brep_to_ros(
        RosExportPayload {
            model: &model,
            history: None,
            aipr: None,
        },
        &path,
        RosExportOptions::default(),
    )
    .await
    .expect("unsigned export");
    assert_eq!(summary.signature, RosWriteSignature::Unsigned);

    // No SIGN chunk, no header claim.
    let (header, table) = read_chunk_table(std::fs::read(&path).expect("read file"));
    assert!(table.find_by_type(ChunkType::SIGN).is_none());
    assert!(!header.feature_flags.has_signature());
    assert_eq!(header.signature_algo, 0);

    let imported = import_ros(&path, None).await.expect("import");
    assert_eq!(imported.signature, RosSignatureVerdict::Unsigned);
}
