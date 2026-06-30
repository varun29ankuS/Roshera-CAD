//! Reconcile the truth eye (certificate), scene eye (render), and semantic eye
//! (feature recognition). Pure logic only — the api-server does all rendering
//! and passes frames in, keeping these functions unit-testable with no I/O.

use serde::{Deserialize, Serialize};

use crate::primitives::feature_recognition::RecognizedFeature;
use crate::primitives::provenance::ValidityCertificate;
use crate::render::RenderFrame;
use std::collections::HashSet;

/// Which pair of eyes a discrepancy is between.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReconcileAxis {
    TruthScene,
    Coverage,
    TruthSemantic,
    SceneSemantic,
}

/// Advisory severity. Never affects `is_sound` — gating lives in the certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReconcileStatus {
    Pending,
    Clean,
    DiscrepanciesFound,
}

/// One eyes-disagree finding, with all three channels' accounts side by side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Discrepancy {
    pub axis: ReconcileAxis,
    pub severity: Severity,
    pub faces: Vec<u32>,
    pub edges: Vec<u32>,
    pub message: String,
    pub truth_says: String,
    pub scene_says: String,
    pub semantic_says: String,
}

/// Which faces the scene eye could and could not see across all viewpoints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Coverage {
    pub seen: Vec<u32>,
    pub unseen: Vec<u32>,
    pub total: usize,
}

/// The heavy-tier advisory report. NEVER changes `is_sound`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReconcileReport {
    pub solid_id: u32,
    pub cert_fingerprint: u64,
    pub status: ReconcileStatus,
    pub discrepancies: Vec<Discrepancy>,
    pub coverage: Coverage,
    pub viewpoints: u32,
    pub duration_ms: u64,
}

#[cfg(test)]
mod type_tests {
    use super::*;

    #[test]
    fn report_serializes_round_trip() {
        let r = ReconcileReport {
            solid_id: 1,
            cert_fingerprint: 42,
            status: ReconcileStatus::DiscrepanciesFound,
            discrepancies: vec![Discrepancy {
                axis: ReconcileAxis::Coverage,
                severity: Severity::Info,
                faces: vec![9],
                edges: vec![],
                message: "face 9 not visible from any viewpoint".into(),
                truth_says: "face 9 is part of a sound solid".into(),
                scene_says: "face 9 never appears in any render".into(),
                semantic_says: "n/a".into(),
            }],
            coverage: Coverage {
                seen: vec![1, 2],
                unseen: vec![9],
                total: 3,
            },
            viewpoints: 26,
            duration_ms: 12,
        };
        let json = serde_json::to_string(&r).expect("serialize");
        let back: ReconcileReport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.status, ReconcileStatus::DiscrepanciesFound);
        assert_eq!(back.coverage.unseen, vec![9]);
    }
}
