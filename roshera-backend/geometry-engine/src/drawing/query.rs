//! Typed query surface (campaign #55 Slice 5) — the agent's certified readback
//! verb over a Roshera sheet.
//!
//! [`answer_query`] answers a typed, scoped [`DrawingQuery`] against a sheet +
//! its live [`SheetReadbackCertificate`]. Every answer carries provenance (PIDs
//! / face ids / datums that feed straight back into `measure_faces` / `gdt_fcf`
//! / `label_resolve`) and a live-check verdict, and honest-refuses
//! (`render_only` / `unprovenanced`) rather than fabricate.
//!
//! Queries are TYPED — NL→query mapping is the agent's job (API-only AI policy).
//! The kernel owns this logic; the api-server handler is a thin wrapper (the
//! backend-driven doctrine: "the api-server orchestrates but contains no
//! geometric logic").

use serde::{Deserialize, Serialize};

use super::section_comprehension::SectionCutThrough;
use super::sheet_certificate::{SheetFact, SheetFactKind, SheetReadbackCertificate, SheetVerdict};
use super::types::{Drawing, ToleranceRef};

/// A typed, scoped question against a certified sheet. Every kind honest-refuses
/// (a typed [`DrawingAnswer::Refused`]) rather than fabricate.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DrawingQuery {
    /// The toleranced diameter of a bore — by hole tag, face id, or dimension
    /// PID. Answers with resolved limits + provenance + live check, or the
    /// general tolerance explicitly labelled when no feature tolerance is bound.
    TolerancedDiameter {
        #[serde(default)]
        tag: Option<String>,
        #[serde(default)]
        face_id: Option<u32>,
        #[serde(default)]
        pid: Option<String>,
    },
    /// A feature control frame — by index, feature PID, or a datum letter it
    /// references. Answers which datums it references and whether each is live.
    Fcf {
        #[serde(default)]
        index: Option<usize>,
        #[serde(default)]
        feature_pid: Option<String>,
        #[serde(default)]
        datum: Option<String>,
    },
    /// What SECTION A-A cuts through (the ordered cut-through list).
    SectionCuts {},
    /// The dimension(s) spanning an entity — by face id, PID, or label.
    DimensionOf {
        #[serde(default)]
        face_id: Option<u32>,
        #[serde(default)]
        pid: Option<String>,
        #[serde(default)]
        label: Option<String>,
    },
    /// A hole-table row by tag.
    Hole { tag: String },
    /// What ink is at a view-space coordinate — refuses (`render_only`) on the
    /// shaded raster and section hatch (pure ink with no model referent).
    EntityAt { view: usize, xy_mm: [f64; 2] },
}

/// One datum reference's live status, resolved from the sheet's restored datum
/// provenance (never from the ink letters).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DatumStatus {
    pub label: String,
    pub feature_pid: Option<String>,
    /// `live` | `dangling` | `unprovenanced`.
    pub status: String,
}

/// The FCF answer: characteristic + tolerance + ordered datum references with
/// live status + the block's provenance verdict.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FcfAnswer {
    pub index: usize,
    pub characteristic_glyph: String,
    pub tolerance_text: String,
    pub feature_pid: Option<String>,
    pub datums: Vec<DatumStatus>,
    pub verdict: SheetVerdict,
}

/// The toleranced-diameter answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TolerancedDiameterAnswer {
    pub label: String,
    pub value: f64,
    pub unit: String,
    /// `feature` (a bound GD&T tolerance) | `general` (the sheet's general
    /// tolerance, applied explicitly).
    pub tolerance_source: String,
    /// Resolved absolute `[lower, upper]` limits, when available. `None` for an
    /// unresolved ISO 286 fit class — never fabricated.
    pub limits: Option<[f64; 2]>,
    /// Fit designation (e.g. `"H7"`) when the bound tolerance is a fit class.
    pub designation: Option<String>,
    /// The general linear tolerance (± mm) when `tolerance_source == general`.
    pub general_pm_mm: Option<f64>,
    /// The general-tolerance standard (e.g. `"ISO 2768-m"`) for the fallback.
    pub general_standard: Option<String>,
    pub feature_pid: Option<String>,
    pub face_ids: Vec<u32>,
    /// The sheet fact's live-check verdict + re-measured value.
    pub verdict: SheetVerdict,
    pub measured: Option<f64>,
}

/// The entity-at answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EntityAtAnswer {
    /// `dimension` | `circle`.
    pub role: String,
    pub label: Option<String>,
    pub face_ids: Vec<u32>,
    pub pid: Option<String>,
}

/// The answer to a [`DrawingQuery`]. `Refused` carries a typed reason + the
/// verdict that classifies the refusal (`render_only` / `unprovenanced`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
pub enum DrawingAnswer {
    TolerancedDiameter(TolerancedDiameterAnswer),
    Fcf(FcfAnswer),
    SectionCuts(SectionCutThrough),
    Dimensions {
        facts: Vec<SheetFact>,
    },
    Hole {
        fact: SheetFact,
        tolerance: Option<ToleranceRef>,
    },
    EntityAt(EntityAtAnswer),
    Refused {
        reason: String,
        refusal: SheetVerdict,
    },
}

fn refused(reason: impl Into<String>, refusal: SheetVerdict) -> DrawingAnswer {
    DrawingAnswer::Refused {
        reason: reason.into(),
        refusal,
    }
}

/// Distance from `p` to the segment `[a, b]` in view space.
fn point_seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-18 {
        return ((p[0] - a[0]).powi(2) + (p[1] - a[1]).powi(2)).sqrt();
    }
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0);
    let (cx, cy) = (a[0] + t * dx, a[1] + t * dy);
    ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt()
}

/// Answer a [`DrawingQuery`] against the sheet + its live certificate.
pub fn answer_query(
    drawing: &Drawing,
    cert: &SheetReadbackCertificate,
    q: &DrawingQuery,
) -> DrawingAnswer {
    match q {
        DrawingQuery::SectionCuts {} => match &cert.section_cuts {
            Some(ct) => DrawingAnswer::SectionCuts(ct.clone()),
            None => refused(
                "this sheet carries no SECTION view — nothing to cut through",
                SheetVerdict::Unprovenanced,
            ),
        },

        DrawingQuery::TolerancedDiameter { tag, face_id, pid } => {
            // Find the matching diameter-bearing fact (a hole row, or a diameter
            // dimension) from the live certificate.
            let fact = cert.facts.iter().find(|f| match f.kind {
                SheetFactKind::Hole => {
                    tag.as_ref()
                        .map(|t| f.label.split_whitespace().next() == Some(t.as_str()))
                        .unwrap_or(false)
                        || face_id
                            .map(|fid| f.face_ids.contains(&fid))
                            .unwrap_or(false)
                }
                SheetFactKind::Dimension => {
                    face_id
                        .map(|fid| f.face_ids.contains(&fid))
                        .unwrap_or(false)
                        || (pid.is_some() && f.pid.as_deref() == pid.as_deref())
                }
                _ => false,
            });
            let Some(fact) = fact else {
                return refused(
                    "no toleranced diameter matches this selector",
                    SheetVerdict::Unprovenanced,
                );
            };
            let value = fact.value.unwrap_or(0.0);
            let (source, limits, designation, gpm, gstd) = match &fact.tolerance {
                Some(t) => (
                    "feature".to_string(),
                    t.limits,
                    t.designation.clone(),
                    None,
                    None,
                ),
                None => (
                    "general".to_string(),
                    None,
                    None,
                    Some(drawing.general_tolerance.linear_mm),
                    Some(drawing.general_tolerance.standard.clone()),
                ),
            };
            DrawingAnswer::TolerancedDiameter(TolerancedDiameterAnswer {
                label: fact.label.clone(),
                value,
                unit: fact.unit.clone(),
                tolerance_source: source,
                limits,
                designation,
                general_pm_mm: gpm,
                general_standard: gstd,
                feature_pid: fact.tolerance.as_ref().and_then(|t| t.feature_pid.clone()),
                face_ids: fact.face_ids.clone(),
                verdict: fact.live.verdict,
                measured: fact.live.measured,
            })
        }

        DrawingQuery::Fcf {
            index,
            feature_pid,
            datum,
        } => {
            let picked = drawing.fcf_blocks.iter().enumerate().find(|(i, b)| {
                index.map(|ix| ix == *i).unwrap_or(false)
                    || (feature_pid.is_some() && b.feature_pid.as_deref() == feature_pid.as_deref())
                    || datum
                        .as_ref()
                        .map(|d| b.datum_labels.iter().any(|l| l == d))
                        .unwrap_or(false)
            });
            let Some((idx, block)) = picked else {
                return refused("no FCF matches this selector", SheetVerdict::Unprovenanced);
            };
            // Resolve each referenced datum's live status from the sheet's datum
            // symbols (restored feature PIDs) + the certificate verdicts — never
            // from the ink letters.
            let datums: Vec<DatumStatus> = block
                .datum_labels
                .iter()
                .map(|label| {
                    let sym = drawing.datum_symbols.iter().find(|s| &s.label == label);
                    let fact = cert.facts.iter().find(|f| {
                        f.kind == SheetFactKind::DatumSymbol && f.label == format!("datum {label}")
                    });
                    let status = match fact.map(|f| f.live.verdict) {
                        Some(SheetVerdict::Consistent) => "live",
                        Some(SheetVerdict::Dangling) => "dangling",
                        _ => "unprovenanced",
                    };
                    DatumStatus {
                        label: label.clone(),
                        feature_pid: sym.and_then(|s| s.feature_pid.clone()),
                        status: status.to_string(),
                    }
                })
                .collect();
            let verdict = cert
                .facts
                .iter()
                .find(|f| {
                    f.kind == SheetFactKind::Fcf && f.pid.as_deref() == block.feature_pid.as_deref()
                })
                .map(|f| f.live.verdict)
                .unwrap_or(SheetVerdict::Unprovenanced);
            DrawingAnswer::Fcf(FcfAnswer {
                index: idx,
                characteristic_glyph: block.characteristic_glyph.clone(),
                tolerance_text: block.tolerance_text.clone(),
                feature_pid: block.feature_pid.clone(),
                datums,
                verdict,
            })
        }

        DrawingQuery::DimensionOf {
            face_id,
            pid,
            label,
        } => {
            let facts: Vec<SheetFact> = cert
                .facts
                .iter()
                .filter(|f| f.kind == SheetFactKind::Dimension)
                .filter(|f| {
                    face_id
                        .map(|fid| f.face_ids.contains(&fid))
                        .unwrap_or(false)
                        || (pid.is_some() && f.pid.as_deref() == pid.as_deref())
                        || label
                            .as_ref()
                            .map(|l| f.label.contains(l.as_str()))
                            .unwrap_or(false)
                })
                .cloned()
                .collect();
            if facts.is_empty() {
                return refused(
                    "no dimension matches this selector",
                    SheetVerdict::Unprovenanced,
                );
            }
            DrawingAnswer::Dimensions { facts }
        }

        DrawingQuery::Hole { tag } => {
            let fact = cert.facts.iter().find(|f| {
                f.kind == SheetFactKind::Hole
                    && f.label.split_whitespace().next() == Some(tag.as_str())
            });
            match fact {
                Some(f) => DrawingAnswer::Hole {
                    fact: f.clone(),
                    tolerance: f.tolerance.clone(),
                },
                None => refused(
                    format!("no hole row tagged {tag}"),
                    SheetVerdict::Unprovenanced,
                ),
            }
        }

        DrawingQuery::EntityAt { view, xy_mm } => {
            let Some(v) = drawing.views.get(*view) else {
                return refused("no such view index", SheetVerdict::Unprovenanced);
            };
            let p = *xy_mm;
            const HIT_TOL: f64 = 1.0;
            // Hatch = material texture, pure ink → render_only.
            if v.hatch_polylines.iter().any(|poly| {
                poly.points
                    .windows(2)
                    .any(|w| point_seg_dist(p, w[0], w[1]) <= HIT_TOL)
            }) {
                return refused(
                    "section hatch is material texture (ink), not geometry",
                    SheetVerdict::RenderOnly,
                );
            }
            // Shaded raster pictorial cell → render_only.
            if v.shaded_raster.is_some()
                && p[0] >= v.extent.min_x - HIT_TOL
                && p[0] <= v.extent.max_x + HIT_TOL
                && p[1] >= v.extent.min_y - HIT_TOL
                && p[1] <= v.extent.max_y + HIT_TOL
            {
                return refused(
                    "shaded pictorial is a raster image (pixels), not geometry",
                    SheetVerdict::RenderOnly,
                );
            }
            // Provenanced circle (carries face ids).
            if let Some(c) = v.circles.iter().find(|c| {
                let d = ((p[0] - c.cx).powi(2) + (p[1] - c.cy).powi(2)).sqrt();
                (d - c.r).abs() <= HIT_TOL
            }) {
                return DrawingAnswer::EntityAt(EntityAtAnswer {
                    role: "circle".to_string(),
                    label: None,
                    face_ids: c.face_ids.clone(),
                    pid: None,
                });
            }
            // Provenanced dimension (span endpoints).
            if let Some(d) = v
                .dimensions
                .iter()
                .find(|d| point_seg_dist(p, d.a, d.b) <= HIT_TOL)
            {
                return DrawingAnswer::EntityAt(EntityAtAnswer {
                    role: "dimension".to_string(),
                    label: Some(d.label.clone()),
                    face_ids: d.entities.clone(),
                    pid: d.pid.clone(),
                });
            }
            // Anonymous polyline / empty space: lineage genuinely lost → refuse.
            refused(
                "no provenanced entity at this coordinate (anonymous ink or empty space)",
                SheetVerdict::Unprovenanced,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawing::dimensioning::standard_drawing_auto;
    use crate::drawing::sheet_certificate::certify_drawing;
    use crate::gdt::drf::designate_datum;
    use crate::gdt::model::{
        Annotation, DimensionalTolerance, FeatureControlFrame, GeometricCharacteristic,
    };
    use crate::math::{Point3, Vector3};
    use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use crate::primitives::persistent_id::PersistentId;
    use crate::primitives::solid::SolidId;
    use crate::primitives::surface::Plane;
    use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
    use crate::readable::bore_face_ids;

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    /// The founder-question fixture: a 40×40×20 plate with a Ø10 THROUGH bore,
    /// a datum A on the top face, an FCF (perpendicularity to A) on the bore,
    /// and a Ø10 ±0.05 size tolerance on the bore. Returns the built sheet + a
    /// certificate against the live model.
    fn founder_fixture() -> (BRepModel, Drawing) {
        let mut m = BRepModel::new();
        let plate = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(40.0, 40.0, 20.0)
            .expect("plate"));
        let bore = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, -20.0), Vector3::Z, 5.0, 80.0)
            .expect("bore"));
        let part = boolean_operation(
            &mut m,
            plate,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("difference");

        // Datum A: the top planar face (normal +Z).
        let top_face = {
            let solid = m.solids.get(part).expect("solid");
            let mut chosen = None;
            let mut shells = vec![solid.outer_shell];
            shells.extend_from_slice(&solid.inner_shells);
            for sh in shells {
                if let Some(shell) = m.shells.get(sh) {
                    for &fid in &shell.faces {
                        if let Some(face) = m.faces.get(fid) {
                            if let Some(surf) = m.surfaces.get(face.surface_id) {
                                if let Some(pl) = surf.as_any().downcast_ref::<Plane>() {
                                    let n = pl.normal * face.orientation.sign();
                                    if n.z > 0.9 {
                                        chosen = Some(fid);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            chosen.expect("a top planar face")
        };
        // Ensure the datum face resolves by PID (seed if the boolean did not mint
        // lineage), then designate it as datum A.
        if m.face_pids.get(&top_face).is_none() {
            let p = PersistentId::root(b"founder-datum-A");
            m.face_pids.insert(top_face, p);
            m.pid_to_face.insert(p, top_face);
        }
        designate_datum(&mut m, part, "A", top_face).expect("datum A");

        // Bore face: ensure a PID, then author the FCF + size tolerance on it.
        let bore_fid = *bore_face_ids(&m, part).iter().next().expect("bore face");
        let pid = m.face_pids.get(&bore_fid).copied().unwrap_or_else(|| {
            let p = PersistentId::root(b"founder-bore");
            m.face_pids.insert(bore_fid, p);
            m.pid_to_face.insert(p, bore_fid);
            p
        });
        m.gdt.attach(
            pid,
            Annotation::Geometric(FeatureControlFrame::orientation(
                GeometricCharacteristic::Perpendicularity,
                0.05,
                "A",
            )),
        );
        m.gdt.attach(
            pid,
            Annotation::Dimensional(DimensionalTolerance::symmetric(10.0, 0.05)),
        );

        let drawing = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");
        (m, drawing)
    }

    /// FOUNDER Q1: "the toleranced diameter of the bore" → limits + provenance +
    /// a live-check verdict.
    #[test]
    fn founder_toleranced_diameter() {
        let (m, drawing) = founder_fixture();
        let cert = certify_drawing(&m, &drawing);
        let tag = drawing.hole_sites.first().map(|h| h.tag.clone()).unwrap();
        let ans = answer_query(
            &drawing,
            &cert,
            &DrawingQuery::TolerancedDiameter {
                tag: Some(tag),
                face_id: None,
                pid: None,
            },
        );
        match ans {
            DrawingAnswer::TolerancedDiameter(a) => {
                assert_eq!(a.tolerance_source, "feature", "{a:?}");
                let [lo, hi] = a.limits.expect("resolved limits");
                assert!(
                    (lo - 9.95).abs() < 1e-6 && (hi - 10.05).abs() < 1e-6,
                    "{a:?}"
                );
                assert!(a.feature_pid.is_some(), "provenance present: {a:?}");
                assert_eq!(a.verdict, SheetVerdict::Consistent, "{a:?}");
            }
            other => panic!("expected a toleranced diameter, got {other:?}"),
        }
    }

    /// FOUNDER Q2: "which datum does this FCF reference, and is it live?" →
    /// datum A, live, resolved from restored provenance (not the ink letter).
    #[test]
    fn founder_fcf_datum_reference() {
        let (m, drawing) = founder_fixture();
        let cert = certify_drawing(&m, &drawing);
        assert!(
            !drawing.fcf_blocks.is_empty(),
            "the sheet must carry the FCF"
        );
        let ans = answer_query(
            &drawing,
            &cert,
            &DrawingQuery::Fcf {
                index: Some(0),
                feature_pid: None,
                datum: None,
            },
        );
        match ans {
            DrawingAnswer::Fcf(a) => {
                let d = a.datums.iter().find(|d| d.label == "A").expect("datum A");
                assert_eq!(d.status, "live", "datum A must resolve live: {a:?}");
                assert!(
                    d.feature_pid.is_some(),
                    "datum names its feature PID: {a:?}"
                );
            }
            other => panic!("expected an FCF answer, got {other:?}"),
        }
    }

    /// FOUNDER Q3: "what does SECTION A-A cut through?" → the bore is listed.
    #[test]
    fn founder_section_cuts() {
        let (m, drawing) = founder_fixture();
        let cert = certify_drawing(&m, &drawing);
        let ans = answer_query(&drawing, &cert, &DrawingQuery::SectionCuts {});
        match ans {
            DrawingAnswer::SectionCuts(ct) => {
                assert!(
                    ct.cuts
                        .iter()
                        .any(|c| c.kind == crate::drawing::SectionCutKind::Bore),
                    "SECTION A-A must cut the bore: {ct:?}"
                );
            }
            other => panic!("expected section cuts, got {other:?}"),
        }
    }

    /// A sheet with NO section refuses `section_cuts` (unprovenanced), never a
    /// fabricated empty answer.
    #[test]
    fn section_cuts_refuses_when_no_section() {
        let mut m = BRepModel::new();
        let cube = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("cube"));
        let drawing = standard_drawing_auto(&m, cube, uuid::Uuid::nil()).expect("sheet");
        assert!(drawing.section.is_none(), "a plain cube has no section");
        let cert = certify_drawing(&m, &drawing);
        let ans = answer_query(&drawing, &cert, &DrawingQuery::SectionCuts {});
        assert!(
            matches!(
                ans,
                DrawingAnswer::Refused {
                    refusal: SheetVerdict::Unprovenanced,
                    ..
                }
            ),
            "no section → unprovenanced refusal: {ans:?}"
        );
    }

    /// `entity_at` on the section HATCH returns a typed `render_only` refusal —
    /// hatch is material evidence (ink), never answered as geometry.
    #[test]
    fn entity_at_hatch_is_render_only() {
        let (m, drawing) = founder_fixture();
        let cert = certify_drawing(&m, &drawing);
        // Find the section view + a point on one of its hatch segments.
        let (vi, p) = drawing
            .views
            .iter()
            .enumerate()
            .find_map(|(i, v)| {
                v.hatch_polylines
                    .iter()
                    .find(|poly| poly.points.len() >= 2)
                    .map(|poly| {
                        let a = poly.points[0];
                        let b = poly.points[1];
                        (i, [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1])])
                    })
            })
            .expect("a hatch segment to probe");
        let ans = answer_query(
            &drawing,
            &cert,
            &DrawingQuery::EntityAt { view: vi, xy_mm: p },
        );
        assert!(
            matches!(
                ans,
                DrawingAnswer::Refused {
                    refusal: SheetVerdict::RenderOnly,
                    ..
                }
            ),
            "a hatch coordinate must refuse render_only: {ans:?}"
        );
    }

    /// `entity_at` on the shaded raster (isometric pictorial cell) refuses
    /// `render_only` — pixels are never answered as geometry.
    #[test]
    fn entity_at_raster_is_render_only() {
        let (m, drawing) = founder_fixture();
        let cert = certify_drawing(&m, &drawing);
        // The isometric view carries the shaded raster; probe its centre.
        let probe = drawing.views.iter().enumerate().find_map(|(i, v)| {
            v.shaded_raster.as_ref().map(|_| {
                (
                    i,
                    [
                        0.5 * (v.extent.min_x + v.extent.max_x),
                        0.5 * (v.extent.min_y + v.extent.max_y),
                    ],
                )
            })
        });
        let Some((vi, p)) = probe else {
            // No raster on this fixture's layout → nothing to assert (the hatch
            // test already covers the render_only path). Skip cleanly.
            return;
        };
        let ans = answer_query(
            &drawing,
            &cert,
            &DrawingQuery::EntityAt { view: vi, xy_mm: p },
        );
        assert!(
            matches!(
                ans,
                DrawingAnswer::Refused {
                    refusal: SheetVerdict::RenderOnly,
                    ..
                }
            ),
            "a raster coordinate must refuse render_only: {ans:?}"
        );
    }
}
