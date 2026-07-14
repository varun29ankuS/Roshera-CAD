//! Reconcile the truth eye (certificate), scene eye (render), and semantic eye
//! (feature recognition). Pure logic only — the api-server does all rendering
//! and passes frames in, keeping these functions unit-testable with no I/O.

use crate::primitives::feature_recognition::RecognizedFeature;
use crate::primitives::provenance::ValidityCertificate;
use crate::primitives::surface::SurfaceType;
use crate::render::RenderFrame;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A live face visible from at most this many viewpoints is "barely inspected".
const LOW_COVERAGE_VIEWS: usize = 2;

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

/// The DEEP advisory cross-eye reconcile. Renders are done by the api-server
/// and passed in as `faceid_frames` (one FaceIds frame per viewpoint) and a
/// single `diagnostic_frame`; this function is pure so the whole reconcile is
/// unit-testable with no I/O. It NEVER changes `is_sound` — every finding is
/// advisory. Four axes:
///
/// 1. **Coverage** — per-face SEEN-COUNT across viewpoints (not binary): unseen
///    faces and barely-inspected (low-coverage) faces are flagged `Info`. A single
///    normal is meaningless for a curved face, so we make NO internal-vs-occluded
///    claim — seen-count is the only honest signal.
/// 2. **Truth↔Scene** — the diagnostic render's open/non-manifold edge counts vs
///    the certificate's (`Warning` on disagreement), plus a worst-face visual
///    confirmation: if the cert names a worst tessellation/mesh-quality face that
///    no viewpoint saw, the scene eye cannot corroborate the truth eye's own worst
///    finding (`Info`).
/// 3. **Scene↔Semantic** — per-feature MIN coverage (a feature is only as visible
///    as its least-visible face): never-visible → `Warning`, barely-visible → `Info`.
/// 4. **Truth↔Semantic mirror** — a feature referencing a dead (non-live) face is
///    an `Error` (mirrors the cert's gating `eyes_consistent` into the report).
///
/// `status` is `DiscrepanciesFound` iff any discrepancy is `Error`/`Warning`;
/// `Info`-only reports are `Clean` (coverage notes are observations, not defects).
#[allow(clippy::too_many_arguments)]
pub fn reconcile_full(
    solid_id: u32,
    cert_fingerprint: u64,
    live_face_ids: &HashSet<u32>,
    cert: &ValidityCertificate,
    features: &[RecognizedFeature],
    faceid_frames: &[RenderFrame],
    diagnostic_frame: &RenderFrame,
    viewpoints: u32,
    duration_ms: u64,
) -> ReconcileReport {
    let mut discrepancies: Vec<Discrepancy> = Vec::new();

    // ---- Axis 1: Coverage — per-face seen-count across the FaceIds frames. ----
    let mut seen_count: HashMap<u32, usize> = HashMap::with_capacity(live_face_ids.len());
    for &fid in live_face_ids {
        let count = faceid_frames
            .iter()
            .filter(|frame| frame.face_legend.iter().any(|(id, _)| *id == fid))
            .count();
        seen_count.insert(fid, count);
    }
    // Deterministic, sorted partition into seen / unseen.
    let mut by_face: Vec<(u32, usize)> = seen_count.iter().map(|(&f, &c)| (f, c)).collect();
    by_face.sort_unstable();
    let seen: Vec<u32> = by_face
        .iter()
        .filter(|(_, c)| *c > 0)
        .map(|(f, _)| *f)
        .collect();
    let unseen: Vec<u32> = by_face
        .iter()
        .filter(|(_, c)| *c == 0)
        .map(|(f, _)| *f)
        .collect();
    let seen_set: HashSet<u32> = seen.iter().copied().collect();

    for (fid, count) in &by_face {
        if *count == 0 {
            discrepancies.push(Discrepancy {
                axis: ReconcileAxis::Coverage,
                severity: Severity::Info,
                faces: vec![*fid],
                edges: vec![],
                message: format!(
                    "face {fid} not visible from any of {viewpoints} viewpoints (internal, occluded, or degenerate)"
                ),
                truth_says: "face is part of the certified solid".into(),
                scene_says: "face never appears in any render".into(),
                semantic_says: "n/a".into(),
            });
        } else if *count <= LOW_COVERAGE_VIEWS {
            discrepancies.push(Discrepancy {
                axis: ReconcileAxis::Coverage,
                severity: Severity::Info,
                faces: vec![*fid],
                edges: vec![],
                message: format!(
                    "face {fid} barely inspected: seen from only {count}/{viewpoints} viewpoints"
                ),
                truth_says: "face is part of the certified solid".into(),
                scene_says: format!("seen from {count} viewpoint(s)"),
                semantic_says: "n/a".into(),
            });
        }
    }

    // ---- Axis 2: Truth↔Scene — edge-count agreement. ----
    //
    // The certificate and the diagnostic render tessellate the SAME B-Rep at
    // DIFFERENT resolutions: `compute_certificate` re-tessellates with
    // chord_tolerance=0.1 / max_segments=100 (`topology_builder.rs`), while
    // the diagnostic render uses `TessellationParams::coarse()`
    // (chord_tolerance=0.01 / max_segments=20 — tuned for speed across many
    // reconcile viewpoints, see `reconcile_task.rs`). A single continuous
    // boundary/non-manifold curve therefore chops into a DIFFERENT NUMBER of
    // mesh edges under each tessellation, so the raw integer counts
    // disagreeing is an expected artifact of that resolution difference, not
    // itself evidence the two eyes disagree about the solid. The only
    // methodology-independent signal is PRESENCE: does one eye see a defect
    // (count > 0) while the other sees none?
    let truth_has_open = cert.boundary_edges > 0;
    let scene_has_open = diagnostic_frame.open_edges > 0;
    let truth_has_nonmanifold = cert.nonmanifold_edges > 0;
    let scene_has_nonmanifold = diagnostic_frame.nonmanifold_edges > 0;
    let counts_differ = diagnostic_frame.open_edges != cert.boundary_edges
        || diagnostic_frame.nonmanifold_edges != cert.nonmanifold_edges;
    if truth_has_open != scene_has_open || truth_has_nonmanifold != scene_has_nonmanifold {
        // A real cross-eye disagreement: one eye claims the solid is clean,
        // the other claims it isn't. Tessellation resolution cannot explain
        // a zero-vs-nonzero split, so this is worth a human's attention.
        discrepancies.push(Discrepancy {
            axis: ReconcileAxis::TruthScene,
            severity: Severity::Warning,
            faces: vec![],
            edges: vec![],
            message:
                "certificate and diagnostic render DISAGREE on whether an open/non-manifold edge exists at all (not just on the count)"
                    .into(),
            truth_says: format!(
                "boundary={} nonmanifold={}",
                cert.boundary_edges, cert.nonmanifold_edges
            ),
            scene_says: format!(
                "open={} nonmanifold={}",
                diagnostic_frame.open_edges, diagnostic_frame.nonmanifold_edges
            ),
            semantic_says: "n/a".into(),
        });
    } else if counts_differ {
        // Both eyes agree a defect exists (or that the solid is clean); the
        // raw counts differ only because the two tessellations subdivide the
        // same defect curve(s) at different resolutions. Advisory-only: not
        // a sign the eyes disagree, so `Info` rather than `Warning`.
        discrepancies.push(Discrepancy {
            axis: ReconcileAxis::TruthScene,
            severity: Severity::Info,
            faces: vec![],
            edges: vec![],
            message:
                "certificate and diagnostic render agree a defect exists but their edge counts differ — expected, since the two eyes tessellate the same B-Rep at different resolutions (cert: chord=0.1/max_segments=100; diagnostic: coarse chord=0.01/max_segments=20)"
                    .into(),
            truth_says: format!(
                "boundary={} nonmanifold={}",
                cert.boundary_edges, cert.nonmanifold_edges
            ),
            scene_says: format!(
                "open={} nonmanifold={}",
                diagnostic_frame.open_edges, diagnostic_frame.nonmanifold_edges
            ),
            semantic_says: "n/a".into(),
        });
    }

    // ---- Axis 2 (cont.): worst-face visual confirmation. ----
    // Both defect structs expose a `face_id`, so the cert's own worst-quality face
    // is resolvable. If that face was never rendered, the scene eye cannot
    // corroborate the truth eye's worst finding.
    let mut worst_ids: Vec<u64> = Vec::new();
    if let Some(defect) = &cert.tessellation.worst_face {
        worst_ids.push(defect.face_id);
    }
    if let Some(defect) = &cert.mesh_quality.worst_face {
        worst_ids.push(defect.face_id);
    }
    worst_ids.sort_unstable();
    worst_ids.dedup();
    for wid in worst_ids {
        // Face legends are u32; a worst id outside u32 can never match a rendered
        // face, so we cannot correlate it and honestly skip rather than guess.
        if let Ok(w32) = u32::try_from(wid) {
            if !seen_set.contains(&w32) {
                discrepancies.push(Discrepancy {
                    axis: ReconcileAxis::TruthScene,
                    severity: Severity::Info,
                    faces: vec![w32],
                    edges: vec![],
                    message: format!(
                        "certificate's worst-quality face {w32} is not visible from any viewpoint — the scene eye cannot corroborate the truth eye's own worst finding"
                    ),
                    truth_says: "certificate names this its worst-quality face".into(),
                    scene_says: "face not visible in any render".into(),
                    semantic_says: "n/a".into(),
                });
            }
        }
    }

    // ---- Axes 3 & 4: per-feature Scene↔Semantic and Truth↔Semantic. ----
    for feature in features {
        let ids = feature.face_ids();

        // Truth↔Semantic mirror: dead (non-live) face references are an Error.
        let dead: Vec<u32> = ids
            .iter()
            .copied()
            .filter(|fid| !live_face_ids.contains(fid))
            .collect();
        if !dead.is_empty() {
            discrepancies.push(Discrepancy {
                axis: ReconcileAxis::TruthSemantic,
                severity: Severity::Error,
                faces: dead.clone(),
                edges: vec![],
                message: format!(
                    "recognized {} references dead face(s) {:?}",
                    feature.feature_type(),
                    dead
                ),
                truth_says: "these face ids are not in the live topology".into(),
                scene_says: "n/a".into(),
                semantic_says: feature.to_description(),
            });
        }

        // Scene↔Semantic: a feature is only as visible as its least-visible face.
        // A face absent from `seen_count` (dead) counts as 0.
        let min_seen = ids
            .iter()
            .map(|fid| seen_count.get(fid).copied().unwrap_or(0))
            .min()
            .unwrap_or(0);
        if min_seen == 0 {
            discrepancies.push(Discrepancy {
                axis: ReconcileAxis::SceneSemantic,
                severity: Severity::Warning,
                faces: ids.clone(),
                edges: vec![],
                message: format!(
                    "recognized {} is never visible in any render",
                    feature.feature_type()
                ),
                truth_says: "n/a".into(),
                scene_says: "no face of this feature appears in any render".into(),
                semantic_says: feature.to_description(),
            });
        } else if min_seen <= LOW_COVERAGE_VIEWS {
            discrepancies.push(Discrepancy {
                axis: ReconcileAxis::SceneSemantic,
                severity: Severity::Info,
                faces: ids.clone(),
                edges: vec![],
                message: format!(
                    "recognized {} barely visible: {}/{} viewpoints",
                    feature.feature_type(),
                    min_seen,
                    viewpoints
                ),
                truth_says: "n/a".into(),
                scene_says: format!("least-visible face seen from {min_seen} viewpoint(s)"),
                semantic_says: feature.to_description(),
            });
        }
    }

    let status = if discrepancies
        .iter()
        .any(|d| matches!(d.severity, Severity::Error | Severity::Warning))
    {
        ReconcileStatus::DiscrepanciesFound
    } else {
        ReconcileStatus::Clean
    };

    ReconcileReport {
        solid_id,
        cert_fingerprint,
        status,
        discrepancies,
        coverage: Coverage {
            seen,
            unseen,
            total: live_face_ids.len(),
        },
        viewpoints,
        duration_ms,
    }
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

#[cfg(test)]
mod reconcile_full_tests {
    use super::*;
    use crate::primitives::feature_recognition::RecognizedFeature;
    use crate::primitives::provenance::{TessFaceDefect, ValidityCertificate};
    use crate::render::RenderFrame;
    use std::collections::HashSet;

    /// A FaceIds render frame whose legend contains exactly `ids`.
    fn face_frame(ids: &[u32]) -> RenderFrame {
        RenderFrame {
            width: 1,
            height: 1,
            pixels: vec![],
            face_legend: ids.iter().map(|&id| (id, [10, 20, 30])).collect(),
            open_edges: 0,
            nonmanifold_edges: 0,
        }
    }

    /// A Diagnostic render frame carrying the two edge counts.
    fn diag_frame(open: usize, nonmanifold: usize) -> RenderFrame {
        RenderFrame {
            width: 1,
            height: 1,
            pixels: vec![],
            face_legend: vec![],
            open_edges: open,
            nonmanifold_edges: nonmanifold,
        }
    }

    fn live(ids: &[u32]) -> HashSet<u32> {
        ids.iter().copied().collect()
    }

    /// Find the first discrepancy on `axis` at `severity` whose faces contain `fid`.
    fn find_for_face<'a>(
        report: &'a ReconcileReport,
        axis: ReconcileAxis,
        severity: Severity,
        fid: u32,
    ) -> Option<&'a Discrepancy> {
        report
            .discrepancies
            .iter()
            .find(|d| d.axis == axis && d.severity == severity && d.faces.contains(&fid))
    }

    #[test]
    fn unseen_face_becomes_coverage_info() {
        // Faces 1,2 covered by three frames each; face 3 in no legend.
        let frames = vec![
            face_frame(&[1, 2]),
            face_frame(&[1, 2]),
            face_frame(&[1, 2]),
        ];
        let cert = ValidityCertificate::fully_sound_for_test();
        let report = reconcile_full(
            7,
            99,
            &live(&[1, 2, 3]),
            &cert,
            &[],
            &frames,
            &diag_frame(0, 0),
            3,
            5,
        );
        let d = find_for_face(&report, ReconcileAxis::Coverage, Severity::Info, 3)
            .expect("unseen face 3 must yield a Coverage Info discrepancy");
        assert!(d.message.contains("not visible"), "message: {}", d.message);
        assert_eq!(report.coverage.unseen, vec![3]);
        assert_eq!(report.coverage.seen, vec![1, 2]);
        assert_eq!(report.coverage.total, 3);
    }

    #[test]
    fn low_coverage_face_flagged() {
        // Five frames; face 1 appears in exactly one (seen_count == 1 <= 2).
        let frames = vec![
            face_frame(&[1]),
            face_frame(&[]),
            face_frame(&[]),
            face_frame(&[]),
            face_frame(&[]),
        ];
        let cert = ValidityCertificate::fully_sound_for_test();
        let report = reconcile_full(
            7,
            99,
            &live(&[1]),
            &cert,
            &[],
            &frames,
            &diag_frame(0, 0),
            5,
            0,
        );
        let d = find_for_face(&report, ReconcileAxis::Coverage, Severity::Info, 1)
            .expect("low-coverage face 1 must yield a Coverage Info discrepancy");
        assert!(
            d.message.contains("barely inspected") && d.message.contains("1/5"),
            "message: {}",
            d.message
        );
        assert!(report.coverage.seen.contains(&1));
        assert!(report.coverage.unseen.is_empty());
    }

    #[test]
    fn well_covered_face_no_flag() {
        // Face 1 seen in all five frames — no coverage discrepancy for it.
        let frames = vec![
            face_frame(&[1]),
            face_frame(&[1]),
            face_frame(&[1]),
            face_frame(&[1]),
            face_frame(&[1]),
        ];
        let cert = ValidityCertificate::fully_sound_for_test();
        let report = reconcile_full(
            7,
            99,
            &live(&[1]),
            &cert,
            &[],
            &frames,
            &diag_frame(0, 0),
            5,
            0,
        );
        assert!(
            report
                .discrepancies
                .iter()
                .all(|d| d.axis != ReconcileAxis::Coverage),
            "a well-covered face must not raise any Coverage discrepancy"
        );
    }

    #[test]
    fn count_mismatch_is_truthscene_warning() {
        // Diagnostic reports 4 open edges; cert says 0 boundary edges.
        let frames = vec![face_frame(&[1]), face_frame(&[1]), face_frame(&[1])];
        let cert = ValidityCertificate::fully_sound_for_test();
        let report = reconcile_full(
            7,
            99,
            &live(&[1]),
            &cert,
            &[],
            &frames,
            &diag_frame(4, 0),
            3,
            0,
        );
        let d = report
            .discrepancies
            .iter()
            .find(|d| d.axis == ReconcileAxis::TruthScene && d.severity == Severity::Warning)
            .expect("count mismatch must yield a TruthScene Warning");
        assert!(
            d.truth_says.contains("boundary=0"),
            "truth: {}",
            d.truth_says
        );
        assert!(d.scene_says.contains("open=4"), "scene: {}", d.scene_says);
        assert_eq!(report.status, ReconcileStatus::DiscrepanciesFound);
    }

    #[test]
    fn count_mismatch_with_both_nonzero_is_info_not_warning() {
        // Both eyes agree a defect exists (nonzero on both sides), but the
        // raw counts differ because the cert and the diagnostic render
        // tessellate the same B-Rep at different resolutions (see the Axis 2
        // doc comment in `reconcile_full`). This must NOT be a Warning — the
        // eyes are not actually disagreeing about the solid, only about how
        // many mesh edges the same continuous defect curve chops into.
        let frames = vec![face_frame(&[1]), face_frame(&[1]), face_frame(&[1])];
        let mut cert = ValidityCertificate::fully_sound_for_test();
        cert.boundary_edges = 100;
        cert.nonmanifold_edges = 0;
        let report = reconcile_full(
            7,
            99,
            &live(&[1]),
            &cert,
            &[],
            &frames,
            &diag_frame(20, 0),
            3,
            0,
        );
        assert!(
            report
                .discrepancies
                .iter()
                .all(|d| !(d.axis == ReconcileAxis::TruthScene && d.severity == Severity::Warning)),
            "both eyes agreeing a defect exists (just at different tessellation \
             resolutions) must not raise a TruthScene Warning; discrepancies={:?}",
            report.discrepancies
        );
        let info = report
            .discrepancies
            .iter()
            .find(|d| d.axis == ReconcileAxis::TruthScene && d.severity == Severity::Info)
            .expect("differing nonzero counts must still surface a TruthScene Info note");
        assert!(
            info.message.contains("different resolutions"),
            "Info message must explain the tessellation-resolution artifact; message: {}",
            info.message
        );
        assert_eq!(
            report.status,
            ReconcileStatus::Clean,
            "Info-only discrepancies must not flip status away from Clean"
        );
    }

    #[test]
    fn counts_match_no_truthscene_warning() {
        let frames = vec![face_frame(&[1]), face_frame(&[1]), face_frame(&[1])];
        let cert = ValidityCertificate::fully_sound_for_test();
        let report = reconcile_full(
            7,
            99,
            &live(&[1]),
            &cert,
            &[],
            &frames,
            &diag_frame(0, 0),
            3,
            0,
        );
        assert!(
            report
                .discrepancies
                .iter()
                .all(|d| !(d.axis == ReconcileAxis::TruthScene && d.severity == Severity::Warning)),
            "matching counts must not raise a TruthScene Warning"
        );
    }

    #[test]
    fn worst_face_not_visible_is_info() {
        // Cert names face 7 as its worst tessellation face; it appears in no legend.
        let frames = vec![face_frame(&[1]), face_frame(&[1]), face_frame(&[1])];
        let mut cert = ValidityCertificate::fully_sound_for_test();
        cert.tessellation.worst_face = Some(TessFaceDefect {
            face_id: 7,
            triangles: 12,
            degenerate_triangles: 0,
            normal_agreement: 0.4,
            analytic_normal_agreement: 0.4,
        });
        let report = reconcile_full(
            7,
            99,
            &live(&[1, 7]),
            &cert,
            &[],
            &frames,
            &diag_frame(0, 0),
            3,
            0,
        );
        let d = find_for_face(&report, ReconcileAxis::TruthScene, Severity::Info, 7)
            .expect("an invisible worst face must yield a TruthScene Info");
        assert!(
            d.message.contains("worst-quality face"),
            "message: {}",
            d.message
        );
    }

    #[test]
    fn feature_never_visible_is_scenesemantic_warning() {
        // Feature on face 5; face 5 is live but appears in no legend.
        let frames = vec![face_frame(&[1]), face_frame(&[1]), face_frame(&[1])];
        let cert = ValidityCertificate::fully_sound_for_test();
        let feature = RecognizedFeature::ThroughHole {
            diameter: 2.0,
            axis: [0.0, 0.0, 1.0],
            face_id: 5,
        };
        let report = reconcile_full(
            7,
            99,
            &live(&[1, 5]),
            &cert,
            std::slice::from_ref(&feature),
            &frames,
            &diag_frame(0, 0),
            3,
            0,
        );
        let d = report
            .discrepancies
            .iter()
            .find(|d| d.axis == ReconcileAxis::SceneSemantic && d.severity == Severity::Warning)
            .expect("an invisible feature must yield a SceneSemantic Warning");
        assert_eq!(d.semantic_says, feature.to_description());
        assert!(
            d.message.contains("never visible"),
            "message: {}",
            d.message
        );
    }

    #[test]
    fn feature_barely_visible_is_scenesemantic_info() {
        // Feature on face 5, seen in one of five frames -> Info, not Warning.
        let frames = vec![
            face_frame(&[5]),
            face_frame(&[]),
            face_frame(&[]),
            face_frame(&[]),
            face_frame(&[]),
        ];
        let cert = ValidityCertificate::fully_sound_for_test();
        let feature = RecognizedFeature::ThroughHole {
            diameter: 2.0,
            axis: [0.0, 0.0, 1.0],
            face_id: 5,
        };
        let report = reconcile_full(
            7,
            99,
            &live(&[5]),
            &cert,
            std::slice::from_ref(&feature),
            &frames,
            &diag_frame(0, 0),
            5,
            0,
        );
        let info = report
            .discrepancies
            .iter()
            .find(|d| d.axis == ReconcileAxis::SceneSemantic && d.severity == Severity::Info)
            .expect("a barely-visible feature must yield a SceneSemantic Info");
        assert!(info.message.contains("1/5"), "message: {}", info.message);
        assert!(
            report
                .discrepancies
                .iter()
                .all(|d| !(d.axis == ReconcileAxis::SceneSemantic
                    && d.severity == Severity::Warning)),
            "a barely-visible (but seen) feature must not be a Warning"
        );
    }

    #[test]
    fn dead_feature_face_is_truthsemantic_error() {
        // Feature references face 99, which is not live.
        let frames = vec![face_frame(&[1]), face_frame(&[1]), face_frame(&[1])];
        let cert = ValidityCertificate::fully_sound_for_test();
        let feature = RecognizedFeature::ThroughHole {
            diameter: 2.0,
            axis: [0.0, 0.0, 1.0],
            face_id: 99,
        };
        let report = reconcile_full(
            7,
            99,
            &live(&[1]),
            &cert,
            std::slice::from_ref(&feature),
            &frames,
            &diag_frame(0, 0),
            3,
            0,
        );
        let d = find_for_face(&report, ReconcileAxis::TruthSemantic, Severity::Error, 99)
            .expect("a dead feature face must yield a TruthSemantic Error");
        assert!(d.message.contains("dead face"), "message: {}", d.message);
        assert_eq!(report.status, ReconcileStatus::DiscrepanciesFound);
    }

    #[test]
    fn all_clean_is_clean_status() {
        // Every live face well covered; feature visible; counts match; no dead faces.
        let frames = vec![
            face_frame(&[1, 2]),
            face_frame(&[1, 2]),
            face_frame(&[1, 2]),
            face_frame(&[1, 2]),
            face_frame(&[1, 2]),
        ];
        let cert = ValidityCertificate::fully_sound_for_test();
        let feature = RecognizedFeature::ThroughHole {
            diameter: 2.0,
            axis: [0.0, 0.0, 1.0],
            face_id: 1,
        };
        let report = reconcile_full(
            7,
            99,
            &live(&[1, 2]),
            &cert,
            std::slice::from_ref(&feature),
            &frames,
            &diag_frame(0, 0),
            5,
            0,
        );
        assert_eq!(report.status, ReconcileStatus::Clean);
        assert!(
            report
                .discrepancies
                .iter()
                .all(|d| d.severity == Severity::Info),
            "a fully clean part must have no Error/Warning discrepancies"
        );
    }

    /// THE FUNDRAISING BENCHMARK — "the cert says sound; the dual-eye catches
    /// what a single eye cannot."
    ///
    /// A solid with three live faces: 10 (outer wall A), 11 (outer wall B), and
    /// 12 (an internal cavity wall — topologically live, geometrically enclosed).
    /// The kernel certifies it SOUND: B-Rep valid, watertight, manifold,
    /// self-intersection-free. Every viewpoint render sees ONLY the outer walls;
    /// face 12 is permanently occluded — no orbit direction reveals it.
    ///
    /// A cert-only system returns `is_sound() == true` and has no further signal:
    /// it cannot distinguish "face 12 is inspectable" from "face 12 is a blind
    /// cavity". The dual-eye's second tier (scene renders) adds: "face 12 never
    /// appears in any render." Together they surface the Coverage discrepancy the
    /// truth cert alone cannot produce.
    ///
    /// This is the class of defect a Siemens NX / CATIA inspector would call
    /// "a face the probe cannot reach" — a sound solid whose internal geometry is
    /// a manufacturing blind spot. The dual-eye raises the flag so the agent can
    /// explicitly acknowledge the occlusion rather than treating the solid as
    /// fully inspected when it is not.
    ///
    /// Genuine TDD: the assertion `r.coverage.unseen.contains(&12)` and the
    /// Coverage discrepancy for face 12 are RED before this feature existed
    /// (reconcile behaviour absent) and GREEN after.
    #[test]
    fn reconcile_catches_a_face_no_eye_can_see() {
        // Three live faces: outer walls (10, 11) plus internal cavity wall (12).
        let live_faces = live(&[10, 11, 12]);

        // Three viewpoints — each sees only the outer walls. The cavity (12) is
        // permanently occluded and never appears in any render legend.
        let frames = vec![
            face_frame(&[10, 11]),
            face_frame(&[10, 11]),
            face_frame(&[10, 11]),
        ];

        // Diagnostic frame: edge counts agree with the cert (boundary=0, nonmanifold=0).
        // This means NO TruthScene Warning — the cert and scene diagnostic AGREE.
        // The ONLY signal the dual-eye adds is Coverage: face 12 was never seen.
        let cert = ValidityCertificate::fully_sound_for_test();

        // Precondition: the truth eye gives a clean bill of health.
        // The whole point is that the cert ALONE cannot detect the blind cavity.
        assert!(
            cert.is_sound(),
            "the truth cert must report sound — the test exercises the scene eye's extra signal"
        );

        let report = reconcile_full(
            42,
            7,
            &live_faces,
            &cert,
            &[], // no semantic features — only the cert + scene axes are exercised
            &frames,
            &diag_frame(0, 0),
            3,
            0,
        );

        // The scene eye surfaces what the truth cert cannot: face 12 was never rendered.
        assert!(
            report.coverage.unseen.contains(&12),
            "face 12 (internal cavity) must land in coverage.unseen; unseen = {:?}",
            report.coverage.unseen
        );

        // A Coverage discrepancy is raised for face 12 — the dual-eye's caught-a-lie
        // signal. The cert says sound; the scene eye catches the blindspot.
        assert!(
            report
                .discrepancies
                .iter()
                .any(|d| d.axis == ReconcileAxis::Coverage && d.faces.contains(&12)),
            "face 12 must yield a Coverage discrepancy; discrepancies: {:?}",
            report.discrepancies
        );

        // Outer walls are visible and correctly accounted for.
        assert_eq!(
            report.coverage.seen,
            vec![10, 11],
            "outer walls must be seen"
        );
        assert_eq!(report.coverage.total, 3);
    }
}
