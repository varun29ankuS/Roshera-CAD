//! Reconcile the truth eye (certificate), scene eye (render), and semantic eye
//! (feature recognition). Pure logic only — the api-server does all rendering
//! and passes frames in, keeping these functions unit-testable with no I/O.

use crate::primitives::feature_recognition::RecognizedFeature;
use crate::primitives::surface::SurfaceType;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

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

/// Verdict from the render-free cross-check (maps onto the certificate's
/// `EyesConsistency` dimension). Kept separate from the cert enum so this module
/// has no dependency cycle with `provenance`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EyesVerdict {
    Consistent,
    Inconsistent,
    NotApplicable,
}

/// The surface type a feature's semantic label *implies* it must sit on. A
/// recognized feature whose faces do not carry this surface type was
/// mislabelled — a semantic↔surface mismatch. Exhaustive over all six
/// `RecognizedFeature` variants (compiler-enforced; no catch-all), so adding a
/// new feature kind forces a decision here rather than silently passing.
fn expected_surface(feature: &RecognizedFeature) -> SurfaceType {
    match feature {
        // A hole and a cylindrical boss are both cylindrical walls.
        RecognizedFeature::ThroughHole { .. }
        | RecognizedFeature::BlindHole { .. }
        | RecognizedFeature::CylindricalBoss { .. } => SurfaceType::Cylinder,
        // A constant-radius fillet blend is toroidal.
        RecognizedFeature::Fillet { .. } => SurfaceType::Torus,
        // A spherical feature is a sphere segment.
        RecognizedFeature::SphericalFeature { .. } => SurfaceType::Sphere,
        // A chamfer is a flat bevel — a planar face.
        RecognizedFeature::Chamfer { .. } => SurfaceType::Plane,
    }
}

/// Render-free deterministic cross-check (Truth↔Semantic): every recognized
/// feature must (a) reference a live face and (b) sit on the surface type its
/// semantic label implies. `NotApplicable` when there is nothing to cross-check.
/// This is the SOLE check that gates `is_sound`, so it fails CLOSED: a live face
/// with no known surface type is `Inconsistent` (cannot verify), and any
/// semantic↔surface mismatch is `Inconsistent`.
pub fn check_eyes_consistent(
    live_face_ids: &HashSet<u32>,
    face_surfaces: &HashMap<u32, SurfaceType>,
    features: &[RecognizedFeature],
) -> EyesVerdict {
    if features.is_empty() {
        return EyesVerdict::NotApplicable;
    }
    for feature in features {
        let expected = expected_surface(feature);
        for fid in feature.face_ids() {
            // Stale/dead face reference — the feature points at a face that no
            // longer exists in the live topology.
            if !live_face_ids.contains(&fid) {
                return EyesVerdict::Inconsistent;
            }
            match face_surfaces.get(&fid) {
                // Live face with no known surface type — fail closed, we cannot
                // certify a feature we cannot check.
                None => return EyesVerdict::Inconsistent,
                // Semantic label contradicts the actual surface geometry.
                Some(actual) if *actual != expected => return EyesVerdict::Inconsistent,
                Some(_) => {}
            }
        }
    }
    EyesVerdict::Consistent
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

#[cfg(test)]
mod consistency_tests {
    use super::*;
    use crate::primitives::feature_recognition::RecognizedFeature;
    use crate::primitives::surface::SurfaceType;
    use std::collections::{HashMap, HashSet};

    fn live(ids: &[u32]) -> HashSet<u32> {
        ids.iter().copied().collect()
    }

    fn surfaces(pairs: &[(u32, SurfaceType)]) -> HashMap<u32, SurfaceType> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn no_features_is_not_applicable() {
        assert_eq!(
            check_eyes_consistent(&live(&[1, 2]), &surfaces(&[]), &[]),
            EyesVerdict::NotApplicable
        );
    }

    #[test]
    fn hole_on_cylinder_is_consistent() {
        let f = vec![RecognizedFeature::ThroughHole {
            diameter: 2.0,
            axis: [0.0, 0.0, 1.0],
            face_id: 2,
        }];
        assert_eq!(
            check_eyes_consistent(
                &live(&[1, 2, 3]),
                &surfaces(&[(2, SurfaceType::Cylinder)]),
                &f
            ),
            EyesVerdict::Consistent
        );
    }

    #[test]
    fn feature_on_dead_face_is_inconsistent() {
        // face 99 is not live — a stale/dead face reference.
        let f = vec![RecognizedFeature::Fillet {
            radius: 1.0,
            face_ids: vec![2, 99],
        }];
        assert_eq!(
            check_eyes_consistent(
                &live(&[1, 2, 3]),
                &surfaces(&[(2, SurfaceType::Torus), (99, SurfaceType::Torus)]),
                &f
            ),
            EyesVerdict::Inconsistent
        );
    }

    #[test]
    fn hole_on_plane_is_inconsistent() {
        // A hole must sit on a Cylinder; a Plane surface is a mislabelled feature.
        let f = vec![RecognizedFeature::ThroughHole {
            diameter: 2.0,
            axis: [0.0, 0.0, 1.0],
            face_id: 2,
        }];
        assert_eq!(
            check_eyes_consistent(&live(&[1, 2, 3]), &surfaces(&[(2, SurfaceType::Plane)]), &f),
            EyesVerdict::Inconsistent
        );
    }

    #[test]
    fn fillet_on_cylinder_is_inconsistent() {
        // A fillet must sit on a Torus; a Cylinder surface is a mislabelled feature.
        let f = vec![RecognizedFeature::Fillet {
            radius: 1.0,
            face_ids: vec![4],
        }];
        assert_eq!(
            check_eyes_consistent(&live(&[1, 4]), &surfaces(&[(4, SurfaceType::Cylinder)]), &f),
            EyesVerdict::Inconsistent
        );
    }

    #[test]
    fn sphere_feature_on_cone_is_inconsistent() {
        // A spherical feature must sit on a Sphere; a Cone surface is mislabelled.
        let f = vec![RecognizedFeature::SphericalFeature {
            radius: 3.0,
            face_id: 5,
        }];
        assert_eq!(
            check_eyes_consistent(&live(&[1, 5]), &surfaces(&[(5, SurfaceType::Cone)]), &f),
            EyesVerdict::Inconsistent
        );
    }

    #[test]
    fn chamfer_on_torus_is_inconsistent() {
        // A chamfer must sit on a Plane; a Torus surface is mislabelled.
        let f = vec![RecognizedFeature::Chamfer {
            distance: 0.5,
            face_ids: vec![6],
        }];
        assert_eq!(
            check_eyes_consistent(&live(&[1, 6]), &surfaces(&[(6, SurfaceType::Torus)]), &f),
            EyesVerdict::Inconsistent
        );
    }

    #[test]
    fn boss_on_cylinder_is_consistent() {
        let f = vec![RecognizedFeature::CylindricalBoss {
            diameter: 4.0,
            height: 10.0,
            face_id: 7,
        }];
        assert_eq!(
            check_eyes_consistent(&live(&[1, 7]), &surfaces(&[(7, SurfaceType::Cylinder)]), &f),
            EyesVerdict::Consistent
        );
    }

    #[test]
    fn live_face_missing_surface_is_inconsistent() {
        // Face 8 is live but has no known surface type — cannot verify, fail closed.
        let f = vec![RecognizedFeature::ThroughHole {
            diameter: 2.0,
            axis: [0.0, 0.0, 1.0],
            face_id: 8,
        }];
        assert_eq!(
            check_eyes_consistent(&live(&[1, 8]), &surfaces(&[]), &f),
            EyesVerdict::Inconsistent
        );
    }

    #[test]
    fn multi_feature_one_mismatch_is_inconsistent() {
        // First feature is correct (hole on cylinder); second is mislabelled
        // (fillet on cylinder). One bad feature poisons the whole verdict.
        let f = vec![
            RecognizedFeature::ThroughHole {
                diameter: 2.0,
                axis: [0.0, 0.0, 1.0],
                face_id: 2,
            },
            RecognizedFeature::Fillet {
                radius: 1.0,
                face_ids: vec![4],
            },
        ];
        assert_eq!(
            check_eyes_consistent(
                &live(&[1, 2, 4]),
                &surfaces(&[(2, SurfaceType::Cylinder), (4, SurfaceType::Cylinder)]),
                &f
            ),
            EyesVerdict::Inconsistent
        );
    }
}
