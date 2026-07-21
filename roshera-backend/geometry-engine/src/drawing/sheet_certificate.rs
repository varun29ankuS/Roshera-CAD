//! Sheet readback certificate (campaign #55 Slice 2) — the "can't lie" moat
//! extended from the 3D solid and the 2D sketch to the ENGINEERING SHEET.
//!
//! A Roshera drawing is a *projection of model truth*; comprehension is the
//! inverse map. The design rule (spec §3.1): **restore identity by construction
//! at build time; verify by re-measurement at read time; refuse where neither is
//! possible.** Never infer from coordinates, never from pixels. Concretely, an
//! answer to any sheet question is a triple:
//!
//! ```text
//! (sheet fact) + (provenance: model entity by PID / face id) + (live check: re-measured value, verdict)
//! ```
//!
//! with verdict ∈ `consistent | stale | dangling | render_only | unprovenanced`
//! — the sheet-level analogue of GD&T's tri-state `Conformance`
//! (`gdt/verify.rs`). A drawing is a SNAPSHOT; the live check is what makes
//! readback *certified* rather than merely structured: if the model changed
//! after the sheet was built, the certificate says so instead of parroting stale
//! ink.
//!
//! ## Re-measurement doctrine (analytic, never the mesh)
//!
//! Every live check re-reads the referenced entity NOW from analytic surfaces —
//! `readable::extract_dimensions` (which reads off analytic surfaces / exact
//! curves, never the tessellation) and PID resolution against the live topology
//! store. The display/export tessellation is NEVER consulted, exactly as
//! `gdt::verify`.
//!
//! ## Honesty contract (gate-enforced by the mutation-proof tests)
//!
//! A certificate that stays green when the model is mutated under it is FAKE.
//! The tests re-measure a bore that moved (→ `stale`) and a datum face that was
//! consumed (→ `dangling`); a `consistent` fact's live value must match the
//! built sheet value within the drawing-correctness campaign's 0.1 mm fixture
//! oracle. No numeric answer is ever presented without either a passing live
//! check or an explicit non-`consistent` verdict.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::primitives::persistent_id::PersistentId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::readable::{extract_dimensions, DatumDescriptor, DimensionRecord};

use super::hole_table::HoleSite;
use super::section_comprehension::{section_cut_through, SectionCutKind, SectionCutThrough};
use super::types::{Drawing, ViewSource};
use super::verify::{verify_drawing, DrawingQualityReport};

/// Consistency oracle for a sheet dimension's live re-measurement, in kernel
/// millimetres. Matches the drawing-correctness campaign's fixture oracle
/// (memory `drawing-correctness-campaign.md`): a `consistent` fact's live value
/// must equal the built sheet value within this bound. The "0.2 mm demo" figure
/// is anecdote — 0.1 mm is the enforced gate.
pub const CERT_DIM_ORACLE_MM: f64 = 0.1;

/// The sheet-level verdict on one readable fact — the analogue of GD&T's
/// tri-state `Conformance` honesty, widened for the snapshot/provenance axes a
/// drawing adds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SheetVerdict {
    /// The stored sheet value matches the live re-measurement within the
    /// dimensioning oracle ([`CERT_DIM_ORACLE_MM`]), and the provenance
    /// resolves. The fact is TRUE of the current model.
    Consistent,
    /// The referenced entity is still live but its value MOVED — the sheet ink
    /// is stale relative to the current model. Carries both numbers so a reader
    /// sees the drift.
    Stale,
    /// The provenance no longer resolves: the PID does not map to a face
    /// (consumed by a boolean, or the model was cleared). Same semantics as
    /// `DatumResolution::Dangling`.
    Dangling,
    /// The target is INK with no model referent — a shaded raster, a hatch
    /// texture, a free-form title-block cell. Readback refuses to answer a
    /// numeric question here rather than fabricate one.
    RenderOnly,
    /// The fact carries no durable provenance: a pre-#55 sheet, or an entity
    /// whose feature op does not yet mint PID lineage. Rebuild the sheet to
    /// upgrade — never a fabricated identity.
    Unprovenanced,
}

impl SheetVerdict {
    /// Human/agent-facing lower-case name (for compact readback lines).
    pub fn label(self) -> &'static str {
        match self {
            SheetVerdict::Consistent => "consistent",
            SheetVerdict::Stale => "stale",
            SheetVerdict::Dangling => "dangling",
            SheetVerdict::RenderOnly => "render_only",
            SheetVerdict::Unprovenanced => "unprovenanced",
        }
    }
}

/// What kind of sheet element a [`SheetFact`] certifies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SheetFactKind {
    /// A dimension callout (diameter / length / extent / position / angle).
    Dimension,
    /// A hole-table row.
    Hole,
    /// A GD&T feature control frame block.
    Fcf,
    /// A GD&T datum feature symbol.
    DatumSymbol,
    /// The SECTION A-A cutting plane.
    Section,
    /// A structured sheet note (unit + general tolerance).
    Note,
    /// Ink with no model referent (raster pictorial, hatch texture).
    RenderOnly,
}

/// The live re-measurement attached to a [`SheetFact`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LiveCheck {
    /// The value the kernel measured NOW from the referenced entity, in the
    /// fact's unit. `None` for non-numeric facts (FCF/datum/section) and for
    /// facts whose provenance did not resolve.
    pub measured: Option<f64>,
    /// `|measured − sheet_value|` when both are present; `None` otherwise.
    pub deviation: Option<f64>,
    /// The verdict.
    pub verdict: SheetVerdict,
}

/// One certified readable fact on the sheet: the stored value + its provenance +
/// a live re-measurement with a verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SheetFact {
    /// Which kind of sheet element this fact is.
    pub kind: SheetFactKind,
    /// Index into `Drawing::views` of the owning view, when the fact belongs to
    /// a specific view. `None` for sheet-scoped facts (notes, section).
    pub owner_view: Option<usize>,
    /// The human label as inked on the sheet.
    pub label: String,
    /// The stored sheet value, in `unit`. `None` for non-numeric facts.
    pub value: Option<f64>,
    /// The unit of `value` / `live.measured` (e.g. `"mm"`, `"deg"`). Empty for
    /// non-numeric facts.
    pub unit: String,
    /// Hex-encoded `PersistentId` provenance, when the fact carries one.
    pub pid: Option<String>,
    /// B-Rep face ids the fact spans, when known.
    pub face_ids: Vec<u32>,
    /// Reference datum, when the fact carries one (position dims, hole rows).
    pub datum: Option<DatumDescriptor>,
    /// Bound GD&T dimensional tolerance (campaign #55 Slice 4), when this fact's
    /// feature carries one — the join that makes "the toleranced diameter" a
    /// certified answer. `None` for an untoleranced fact (readback falls back to
    /// the sheet's general tolerance, explicitly labelled). `#[serde(default)]`
    /// keeps Slice-2 certificates deserializing.
    #[serde(default)]
    pub tolerance: Option<super::types::ToleranceRef>,
    /// The live re-measurement + verdict.
    pub live: LiveCheck,
}

/// Per-verdict fact tallies over a certificate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerdictCounts {
    pub consistent: usize,
    pub stale: usize,
    pub dangling: usize,
    pub render_only: usize,
    pub unprovenanced: usize,
}

impl VerdictCounts {
    fn tally(facts: &[SheetFact]) -> Self {
        let mut c = Self::default();
        for f in facts {
            match f.live.verdict {
                SheetVerdict::Consistent => c.consistent += 1,
                SheetVerdict::Stale => c.stale += 1,
                SheetVerdict::Dangling => c.dangling += 1,
                SheetVerdict::RenderOnly => c.render_only += 1,
                SheetVerdict::Unprovenanced => c.unprovenanced += 1,
            }
        }
        c
    }
}

/// The kernel's self-certified, can't-lie verdict on a drawing SHEET.
///
/// One call answers both "is the sheet READABLE?" (the embedded layout
/// [`DrawingQualityReport`]) and "is the sheet TRUE?" (the per-fact live
/// checks + `sound`).
///
/// (`DrawingQualityReport` is not `PartialEq`, so neither is this — compare the
/// `facts` / `counts` / `sound` fields directly when equality is needed.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SheetReadbackCertificate {
    /// Every readable fact on the sheet, with provenance + live check.
    pub facts: Vec<SheetFact>,
    /// Per-verdict tallies.
    pub counts: VerdictCounts,
    /// True when NO fact is `stale` or `dangling` — the sheet is a faithful
    /// snapshot of the current model. (`render_only` / `unprovenanced` facts do
    /// not make a sheet unsound: they are honest absences, not lies.)
    pub sound: bool,
    /// The layout-quality report (the existing 2D perception oracle), embedded
    /// so one certificate covers readability + truth.
    pub quality: DrawingQualityReport,
    /// The ordered SECTION A-A cut-through list (campaign #55 Slice 3), derived
    /// live against the model: what the section plane passes through (bores with
    /// their tags, outer walls, interior webs) in reading order. `None` when the
    /// sheet carries no section. `#[serde(default)]` keeps Slice-2 certificates
    /// (no cut-through) deserializing.
    #[serde(default)]
    pub section_cuts: Option<SectionCutThrough>,
}

impl SheetReadbackCertificate {
    /// Facts whose verdict is `stale` or `dangling` — the ones a reader must not
    /// trust.
    pub fn unsound_facts(&self) -> impl Iterator<Item = &SheetFact> {
        self.facts
            .iter()
            .filter(|f| matches!(f.live.verdict, SheetVerdict::Stale | SheetVerdict::Dangling))
    }
}

/// Parse a hex-encoded `PersistentId` (`{:032x}`) back into a [`PersistentId`].
/// Returns `None` for malformed input — never a fabricated id.
fn parse_pid(hex: &str) -> Option<PersistentId> {
    u128::from_str_radix(hex.trim(), 16).ok().map(PersistentId)
}

/// Live-check a dimension against the freshly re-measured analytic table,
/// keyed by durable PID.
fn dimension_live_check(
    pid: &Option<String>,
    sheet_value: f64,
    live_by_pid: &HashMap<String, f64>,
) -> LiveCheck {
    match pid {
        None => LiveCheck {
            measured: None,
            deviation: None,
            verdict: SheetVerdict::Unprovenanced,
        },
        Some(p) => match live_by_pid.get(p) {
            None => LiveCheck {
                measured: None,
                deviation: None,
                verdict: SheetVerdict::Dangling,
            },
            Some(&live) => {
                let dev = (live - sheet_value).abs();
                let verdict = if dev <= CERT_DIM_ORACLE_MM {
                    SheetVerdict::Consistent
                } else {
                    SheetVerdict::Stale
                };
                LiveCheck {
                    measured: Some(live),
                    deviation: Some(dev),
                    verdict,
                }
            }
        },
    }
}

/// Live-check a hole-table row by re-measuring the diameter of its bore faces
/// against the analytic table (holes carry face ids, not a PID).
fn hole_live_check(
    face_entities: &[u32],
    sheet_dia: f64,
    live_dims: &[DimensionRecord],
) -> LiveCheck {
    if face_entities.is_empty() {
        return LiveCheck {
            measured: None,
            deviation: None,
            verdict: SheetVerdict::Unprovenanced,
        };
    }
    let live = live_dims
        .iter()
        .find(|d| d.kind == "diameter" && d.entities.iter().any(|e| face_entities.contains(e)))
        .map(|d| d.value);
    match live {
        None => LiveCheck {
            measured: None,
            deviation: None,
            verdict: SheetVerdict::Dangling,
        },
        Some(v) => {
            let dev = (v - sheet_dia).abs();
            let verdict = if dev <= CERT_DIM_ORACLE_MM {
                SheetVerdict::Consistent
            } else {
                SheetVerdict::Stale
            };
            LiveCheck {
                measured: Some(v),
                deviation: Some(dev),
                verdict,
            }
        }
    }
}

/// Live-check a PID-anchored annotation (FCF / datum symbol): does the feature
/// still resolve to a live face? This is the sheet-side analogue of
/// `DatumResolution::{Live, Dangling}` — the GD&T tolerance VERDICT itself is
/// bound in Slice 4; here we certify the provenance link.
fn pid_resolve_check(model: &BRepModel, feature_pid: &Option<String>) -> LiveCheck {
    match feature_pid {
        None => LiveCheck {
            measured: None,
            deviation: None,
            verdict: SheetVerdict::Unprovenanced,
        },
        Some(hex) => {
            let resolved = parse_pid(hex).and_then(|p| model.face_by_pid(p));
            let verdict = if resolved.is_some() {
                SheetVerdict::Consistent
            } else {
                SheetVerdict::Dangling
            };
            LiveCheck {
                measured: None,
                deviation: None,
                verdict,
            }
        }
    }
}

/// Live diameter of a bore from its (analytic) cylindrical face — `2·radius`,
/// read straight off the surface, never the span or the mesh. `None` when no
/// face id resolves to a cylinder.
fn live_bore_diameter(model: &BRepModel, face_ids: &[u32]) -> Option<f64> {
    use crate::primitives::surface::Cylinder;
    for &fid in face_ids {
        let Some(face) = model.faces.get(fid) else {
            continue;
        };
        let Some(surface) = model.surfaces.get(face.surface_id) else {
            continue;
        };
        if let Some(cyl) = surface.as_any().downcast_ref::<Cylinder>() {
            return Some(cyl.radius * 2.0);
        }
    }
    None
}

/// Live-check the SECTION cutting plane against the derived cut-through list
/// (campaign #55 Slice 3). Three ways a section goes stale:
/// - the solid id no longer resolves (`dangling`);
/// - the plane no longer passes through ANY material — the geometry moved out
///   from under the sheet (`stale`);
/// - a bore the plane crosses re-measures to a diameter that no longer matches
///   the sheet's hole table — the bore was re-drilled after the sheet was built
///   (`stale`).
///
/// Otherwise `consistent`. The `measured` field carries the count of cut faces
/// so a reader sees the section is non-empty.
fn section_live_check(
    model: &BRepModel,
    solid_id: Option<SolidId>,
    cut_through: &SectionCutThrough,
    hole_sites: &[HoleSite],
) -> LiveCheck {
    if solid_id.is_none() {
        return LiveCheck {
            measured: None,
            deviation: None,
            verdict: SheetVerdict::Dangling,
        };
    }
    if cut_through.is_empty() {
        return LiveCheck {
            measured: Some(0.0),
            deviation: None,
            verdict: SheetVerdict::Stale,
        };
    }
    // Re-drill detection: every tagged bore the plane crosses must still match
    // the sheet's hole-table diameter within the dimensioning oracle.
    for cut in &cut_through.cuts {
        if cut.kind != SectionCutKind::Bore {
            continue;
        }
        let Some(tag) = &cut.hole_tag else { continue };
        let Some(site) = hole_sites.iter().find(|h| &h.tag == tag) else {
            continue;
        };
        if let Some(live_dia) = live_bore_diameter(model, &cut.face_ids) {
            if (live_dia - site.diameter_mm).abs() > CERT_DIM_ORACLE_MM {
                return LiveCheck {
                    measured: Some(live_dia),
                    deviation: Some((live_dia - site.diameter_mm).abs()),
                    verdict: SheetVerdict::Stale,
                };
            }
        }
    }
    LiveCheck {
        measured: Some(cut_through.cuts.len() as f64),
        deviation: None,
        verdict: SheetVerdict::Consistent,
    }
}

/// Certify a drawing sheet against the LIVE model: build one [`SheetFact`] per
/// readable element, each with provenance + a re-measured live check + a
/// verdict, plus the embedded layout quality report.
///
/// Cost is bounded: the analytic dimension table is re-measured ONCE (analytic
/// surface reads, never tessellation), PID resolution is O(1) hashmap lookups,
/// and the section re-cut is a single analytic plane∩solid pass.
pub fn certify_drawing(model: &BRepModel, drawing: &Drawing) -> SheetReadbackCertificate {
    let quality = verify_drawing(drawing);

    // Resolve the drawn solid from the first Part view.
    let solid_id = drawing.views.first().map(|v| match v.source {
        ViewSource::Part { solid_id, .. } => solid_id,
    });

    // Re-measure the analytic dimension table ONCE.
    let live_dims: Vec<DimensionRecord> = solid_id
        .map(|s| extract_dimensions(model, s))
        .unwrap_or_default();
    let mut live_by_pid: HashMap<String, f64> = HashMap::new();
    for d in &live_dims {
        if let Some(pid) = &d.pid {
            live_by_pid.insert(pid.clone(), d.value);
        }
    }

    let mut facts: Vec<SheetFact> = Vec::new();

    // ── Per-view facts ────────────────────────────────────────────────────────
    for (vi, view) in drawing.views.iter().enumerate() {
        for dim in &view.dimensions {
            let live = dimension_live_check(&dim.pid, dim.value, &live_by_pid);
            facts.push(SheetFact {
                kind: SheetFactKind::Dimension,
                owner_view: Some(vi),
                label: dim.label.clone(),
                value: Some(dim.value),
                unit: dim.unit.clone(),
                pid: dim.pid.clone(),
                face_ids: dim.entities.clone(),
                datum: dim.datum.clone(),
                tolerance: dim.tolerance.clone(),
                live,
            });
        }
        // Shaded pictorial raster: pixels by design — refuse, never answer.
        if view.shaded_raster.is_some() {
            facts.push(SheetFact {
                kind: SheetFactKind::RenderOnly,
                owner_view: Some(vi),
                label: "shaded pictorial (raster)".to_string(),
                value: None,
                unit: String::new(),
                pid: None,
                face_ids: Vec::new(),
                datum: None,
                tolerance: None,
                live: LiveCheck {
                    measured: None,
                    deviation: None,
                    verdict: SheetVerdict::RenderOnly,
                },
            });
        }
        // Section hatch: evidence of material, not geometry — refuse.
        if !view.hatch_polylines.is_empty() {
            facts.push(SheetFact {
                kind: SheetFactKind::RenderOnly,
                owner_view: Some(vi),
                label: "section hatch (material evidence)".to_string(),
                value: None,
                unit: String::new(),
                pid: None,
                face_ids: Vec::new(),
                datum: None,
                tolerance: None,
                live: LiveCheck {
                    measured: None,
                    deviation: None,
                    verdict: SheetVerdict::RenderOnly,
                },
            });
        }
    }

    // ── Hole-table rows ───────────────────────────────────────────────────────
    for hole in &drawing.hole_sites {
        let live = hole_live_check(&hole.face_entities, hole.diameter_mm, &live_dims);
        facts.push(SheetFact {
            kind: SheetFactKind::Hole,
            owner_view: drawing.axial_view_idx,
            label: format!("{} {}", hole.tag, hole.dia_label),
            value: Some(hole.diameter_mm),
            unit: "mm".to_string(),
            pid: None,
            face_ids: hole.face_entities.clone(),
            datum: hole.datum.clone(),
            tolerance: hole.tolerance.clone(),
            live,
        });
    }

    // ── FCF blocks ────────────────────────────────────────────────────────────
    for fcf in &drawing.fcf_blocks {
        let live = pid_resolve_check(model, &fcf.feature_pid);
        facts.push(SheetFact {
            kind: SheetFactKind::Fcf,
            owner_view: Some(fcf.owner_view),
            label: fcf.full_text(),
            value: None,
            unit: String::new(),
            pid: fcf.feature_pid.clone(),
            face_ids: Vec::new(),
            datum: None,
            tolerance: None,
            live,
        });
    }

    // ── Datum symbols ─────────────────────────────────────────────────────────
    for ds in &drawing.datum_symbols {
        let live = pid_resolve_check(model, &ds.feature_pid);
        facts.push(SheetFact {
            kind: SheetFactKind::DatumSymbol,
            owner_view: Some(ds.owner_view),
            label: format!("datum {}", ds.label),
            value: None,
            unit: String::new(),
            pid: ds.feature_pid.clone(),
            face_ids: Vec::new(),
            datum: None,
            tolerance: None,
            live,
        });
    }

    // ── Section plane + cut-through (Slice 3) ─────────────────────────────────
    let mut section_cuts: Option<SectionCutThrough> = None;
    if let Some(sec) = &drawing.section {
        let ct = solid_id.map(|s| section_cut_through(model, s, sec, &drawing.hole_sites));
        let live = match &ct {
            Some(cut_through) => {
                section_live_check(model, solid_id, cut_through, &drawing.hole_sites)
            }
            None => LiveCheck {
                measured: None,
                deviation: None,
                verdict: SheetVerdict::Dangling,
            },
        };
        facts.push(SheetFact {
            kind: SheetFactKind::Section,
            owner_view: Some(sec.section_view_idx),
            label: "SECTION A-A".to_string(),
            value: None,
            unit: String::new(),
            pid: None,
            face_ids: Vec::new(),
            datum: None,
            tolerance: None,
            live,
        });
        section_cuts = ct;
    }

    // ── Structured note: document unit + general tolerance ────────────────────
    {
        let unit_matches = drawing.document_unit == model.document_unit();
        facts.push(SheetFact {
            kind: SheetFactKind::Note,
            owner_view: None,
            label: format!(
                "general tolerance \u{00B1}{:.3} mm ({})",
                drawing.general_tolerance.linear_mm,
                if drawing.general_tolerance.standard.is_empty() {
                    "no ISO class"
                } else {
                    drawing.general_tolerance.standard.as_str()
                }
            ),
            value: Some(drawing.general_tolerance.linear_mm),
            unit: "mm".to_string(),
            pid: None,
            face_ids: Vec::new(),
            datum: None,
            tolerance: None,
            live: LiveCheck {
                measured: None,
                deviation: None,
                verdict: if unit_matches {
                    SheetVerdict::Consistent
                } else {
                    SheetVerdict::Stale
                },
            },
        });
    }

    let counts = VerdictCounts::tally(&facts);
    let sound = counts.stale == 0 && counts.dangling == 0;
    SheetReadbackCertificate {
        facts,
        counts,
        sound,
        quality,
        section_cuts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawing::dimensioning::auto_dimensions;
    use crate::drawing::types::{
        Drawing, ProjectedView, ProjectedViewId, ProjectionType, SheetSize, ViewExtent,
    };
    use crate::math::{Point3, Vector3};
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    /// Build a bare cylinder with a durable PID lineage (event key set) at
    /// `radius`, returning the model + solid id.
    fn cylinder_model(key: &str, radius: f64) -> (BRepModel, SolidId) {
        let mut m = BRepModel::new();
        m.set_event_key(Some(key.to_string()));
        let s = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, radius, 40.0)
            .expect("cyl"));
        m.set_event_key(None);
        (m, s)
    }

    /// A minimal single-view drawing carrying the analytic dimensions of the
    /// solid (each Dimension2d now carries its `pid`).
    fn drawing_of(model: &BRepModel, solid: SolidId, part: uuid::Uuid) -> Drawing {
        let dims = auto_dimensions(model, solid, ProjectionType::Front);
        let view = ProjectedView {
            id: ProjectedViewId::new(),
            name: "FRONT".to_string(),
            projection: ProjectionType::Front,
            source: ViewSource::Part {
                part_id: part,
                solid_id: solid,
            },
            position_mm: [100.0, 100.0],
            scale: 1.0,
            polylines: Vec::new(),
            extent: ViewExtent::empty(),
            dimensions: dims,
            centerlines: Vec::new(),
            hidden_polylines: Vec::new(),
            circles: Vec::new(),
            hidden_circles: Vec::new(),
            shaded_raster: None,
            hatch_polylines: Vec::new(),
            polyline_sources: Vec::new(),
        };
        let mut d = Drawing::new("cert-fixture", SheetSize::A3);
        d.add_view(view);
        d
    }

    /// Every dimension on a freshly-built sheet has a resolving PID and a
    /// `consistent` live check — the sheet is TRUE of the model it came from.
    #[test]
    fn fresh_sheet_dimensions_are_consistent_and_provenanced() {
        let (m, s) = cylinder_model("cyl-a", 10.0);
        let d = drawing_of(&m, s, uuid::Uuid::nil());
        let cert = certify_drawing(&m, &d);

        let dim_facts: Vec<&SheetFact> = cert
            .facts
            .iter()
            .filter(|f| f.kind == SheetFactKind::Dimension)
            .collect();
        assert!(!dim_facts.is_empty(), "cylinder must produce dimensions");
        // At least the Ø20 diameter fact must be provenanced + consistent.
        let dia = dim_facts
            .iter()
            .find(|f| f.value.map(|v| (v - 20.0).abs() < 1e-6).unwrap_or(false))
            .expect("Ø20 diameter fact present");
        assert!(dia.pid.is_some(), "diameter fact must carry a PID: {dia:?}");
        assert_eq!(
            dia.live.verdict,
            SheetVerdict::Consistent,
            "fresh diameter must be consistent: {dia:?}"
        );
        assert!(cert.sound, "a fresh sheet must be sound: {:?}", cert.counts);
    }

    /// MUTATION GATE (a): the bore MOVED (same lineage, larger radius) → the
    /// diameter fact must flip to `stale`, carrying the new measured value.
    /// Reverting to the original model must restore `consistent` — proving the
    /// verdict tracks the model, not a memorised answer.
    #[test]
    fn moved_feature_reports_stale() {
        let (m_a, s_a) = cylinder_model("cyl-x", 10.0);
        let d = drawing_of(&m_a, s_a, uuid::Uuid::nil());

        // A second model with the SAME event-key lineage (→ same PIDs) but a
        // LARGER radius: the durable dimension PID is radius-independent, so the
        // sheet's Ø20 fact re-measures against Ø24 → stale.
        let (m_b, _s_b) = cylinder_model("cyl-x", 12.0);
        let cert = certify_drawing(&m_b, &d);
        let dia = cert
            .facts
            .iter()
            .find(|f| {
                f.kind == SheetFactKind::Dimension
                    && f.value.map(|v| (v - 20.0).abs() < 1e-6).unwrap_or(false)
            })
            .expect("Ø20 fact");
        assert_eq!(
            dia.live.verdict,
            SheetVerdict::Stale,
            "a moved feature must report stale: {dia:?}"
        );
        assert_eq!(
            dia.live.measured.map(|v| (v - 24.0).abs() < 1e-6),
            Some(true),
            "stale fact must carry the NEW measured value (Ø24): {dia:?}"
        );
        assert!(!cert.sound, "a stale sheet is not sound");

        // Revert (certify against the original model) → consistent again.
        let cert_a = certify_drawing(&m_a, &d);
        let dia_a = cert_a
            .facts
            .iter()
            .find(|f| {
                f.kind == SheetFactKind::Dimension
                    && f.value.map(|v| (v - 20.0).abs() < 1e-6).unwrap_or(false)
            })
            .expect("Ø20 fact");
        assert_eq!(
            dia_a.live.verdict,
            SheetVerdict::Consistent,
            "reverting the mutation restores consistent"
        );
    }

    /// MUTATION GATE (b): the feature's PID is consumed (removed from the
    /// inverse map) → the diameter fact must flip to `dangling` (its durable
    /// identity no longer resolves), never a fabricated pass.
    #[test]
    fn consumed_feature_reports_dangling() {
        let (mut m, s) = cylinder_model("cyl-d", 10.0);
        let d = drawing_of(&m, s, uuid::Uuid::nil());
        // The Ø20 diameter fact (a FEATURE dim naming its face — face_ids
        // non-empty, unlike the whole-part extents) must be provenanced BEFORE
        // the mutation.
        let is_diameter = |f: &&SheetFact| {
            f.kind == SheetFactKind::Dimension
                && !f.face_ids.is_empty()
                && f.value.map(|v| (v - 20.0).abs() < 1e-6).unwrap_or(false)
        };
        let cert0 = certify_drawing(&m, &d);
        let dia0 = cert0.facts.iter().find(is_diameter).expect("Ø20 fact");
        assert!(
            dia0.pid.is_some(),
            "diameter must be provenanced pre-mutation"
        );
        assert_eq!(dia0.live.verdict, SheetVerdict::Consistent);

        // Consume the bore FACE: strip the forward + inverse face-PID maps so
        // the diameter's durable identity no longer resolves in the re-measured
        // table (the sheet-side analogue of `DatumResolution::Dangling`).
        m.face_pids.clear();
        m.pid_to_face.clear();
        let cert = certify_drawing(&m, &d);
        let dia = cert.facts.iter().find(is_diameter).expect("Ø20 fact");
        assert_eq!(
            dia.live.verdict,
            SheetVerdict::Dangling,
            "a consumed feature's dimension must dangle: {dia:?}"
        );
        assert!(!cert.sound, "a dangling sheet is not sound");
    }

    /// A sheet dimension with NO PID (pre-#55 / PID-less feature) reports
    /// `unprovenanced` — an honest absence, never a fabricated identity.
    #[test]
    fn pidless_dimension_reports_unprovenanced() {
        // Strip ALL PID maps (face + solid) BEFORE deriving the sheet, so
        // `auto_dimensions` mints `pid: None` on every callout — modelling a
        // pre-PID solid / an op that does not yet mint PID lineage.
        let (mut m, s) = cylinder_model("cyl-u", 10.0);
        m.face_pids.clear();
        m.pid_to_face.clear();
        m.solid_pids.clear();
        let d = drawing_of(&m, s, uuid::Uuid::nil());
        let cert = certify_drawing(&m, &d);
        let dia = cert
            .facts
            .iter()
            .find(|f| {
                f.kind == SheetFactKind::Dimension
                    && f.value.map(|v| (v - 20.0).abs() < 1e-6).unwrap_or(false)
            })
            .expect("Ø20 fact");
        assert!(dia.pid.is_none(), "PID-less feature has no PID");
        assert_eq!(dia.live.verdict, SheetVerdict::Unprovenanced);
    }

    /// SLICE 3: a sheet's SECTION certifies its cut-through list, and a bore
    /// RE-DRILLED larger after the sheet was built flips the Section fact to
    /// `stale` (the plane still cuts material, but the bore diameter moved).
    #[test]
    fn re_drilled_bore_makes_section_stale() {
        use crate::drawing::dimensioning::standard_drawing_auto;
        use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};

        // Build a plate with a Ø10 THROUGH bore, then its standard sheet — this
        // populates the hole table AND attaches SECTION A-A through the bore.
        fn bored_plate(radius: f64) -> (BRepModel, SolidId) {
            let mut m = BRepModel::new();
            m.set_event_key(Some("plate-bore".to_string()));
            let plate = sid(TopologyBuilder::new(&mut m)
                .create_box_3d(40.0, 40.0, 20.0)
                .expect("plate"));
            let bore = sid(TopologyBuilder::new(&mut m)
                .create_cylinder_3d(Point3::new(0.0, 0.0, -20.0), Vector3::Z, radius, 80.0)
                .expect("bore"));
            let part = boolean_operation(
                &mut m,
                plate,
                bore,
                BooleanOp::Difference,
                BooleanOptions::default(),
            )
            .expect("difference");
            m.set_event_key(None);
            (m, part)
        }

        let (m_a, s_a) = bored_plate(5.0);
        let drawing = standard_drawing_auto(&m_a, s_a, uuid::Uuid::nil()).expect("sheet");
        assert!(
            drawing.section.is_some(),
            "a bored plate must carry SECTION A-A"
        );

        // Fresh: section consistent, cut-through lists the bore.
        let cert_a = certify_drawing(&m_a, &drawing);
        let sec_a = cert_a
            .facts
            .iter()
            .find(|f| f.kind == SheetFactKind::Section)
            .expect("section fact");
        assert_eq!(
            sec_a.live.verdict,
            SheetVerdict::Consistent,
            "fresh section must be consistent: {sec_a:?}"
        );
        let ct = cert_a
            .section_cuts
            .as_ref()
            .expect("cut-through present on a sectioned sheet");
        assert!(
            ct.cuts.iter().any(|c| c.kind == SectionCutKind::Bore),
            "cut-through must list the bore: {ct:?}"
        );

        // Re-drill: same construction/lineage, bore now Ø16. Certify the ORIGINAL
        // sheet (still says Ø10) against the widened model.
        let (m_b, _s_b) = bored_plate(8.0);
        let cert_b = certify_drawing(&m_b, &drawing);
        let sec_b = cert_b
            .facts
            .iter()
            .find(|f| f.kind == SheetFactKind::Section)
            .expect("section fact");
        assert_eq!(
            sec_b.live.verdict,
            SheetVerdict::Stale,
            "a re-drilled bore must make the section stale: {sec_b:?}"
        );
        assert!(!cert_b.sound, "a stale section makes the sheet unsound");
    }

    // ── Slice 4: tolerance binding ────────────────────────────────────────────

    /// Build a 40×40×20 plate with a Ø10 THROUGH bore, ensure the bore face
    /// carries a PID, and return `(model, part, bore_face_id, bore_pid)`.
    fn bored_plate_with_bore_pid() -> (BRepModel, SolidId, u32, PersistentId) {
        use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
        use crate::readable::bore_face_ids;
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
        let bore_fid = *bore_face_ids(&m, part).iter().next().expect("a bore face");
        // Ensure the bore face resolves by PID (seed one if the boolean did not
        // mint lineage) so an annotation can be authored on it.
        let pid = m.face_pids.get(&bore_fid).copied().unwrap_or_else(|| {
            let p = PersistentId::root(b"slice4-bore-face");
            m.face_pids.insert(bore_fid, p);
            m.pid_to_face.insert(p, bore_fid);
            p
        });
        (m, part, bore_fid, pid)
    }

    /// A Ø±0.05 size tolerance authored on the bore face is JOINED to the sheet's
    /// hole row — "the toleranced diameter of the bore" answers with resolved
    /// limits + provenance.
    #[test]
    fn bore_diameter_carries_authored_tolerance_limits() {
        use crate::drawing::dimensioning::standard_drawing_auto;
        use crate::gdt::model::{Annotation, DimensionalTolerance};

        let (mut m, part, _bore_fid, pid) = bored_plate_with_bore_pid();
        m.gdt.attach(
            pid,
            Annotation::Dimensional(DimensionalTolerance::symmetric(10.0, 0.05)),
        );
        let drawing = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");

        // The hole row must now carry the tolerance with resolved limits.
        let hole = drawing
            .hole_sites
            .iter()
            .find(|h| h.tolerance.is_some())
            .expect("a hole row must carry the authored tolerance");
        let tref = hole.tolerance.as_ref().expect("tolerance");
        assert_eq!(tref.kind, "plus_minus");
        assert!(
            tref.feature_pid.is_some(),
            "tolerance names its feature PID"
        );
        let [lo, hi] = tref.limits.expect("plus_minus resolves numeric limits");
        assert!(
            (lo - 9.95).abs() < 1e-6 && (hi - 10.05).abs() < 1e-6,
            "Ø10 ±0.05 must resolve to [9.95, 10.05]: {tref:?}"
        );

        // And the certificate's Hole fact surfaces it.
        let cert = certify_drawing(&m, &drawing);
        let hole_fact = cert
            .facts
            .iter()
            .find(|f| f.kind == SheetFactKind::Hole && f.tolerance.is_some())
            .expect("hole fact carries tolerance");
        assert!(hole_fact.tolerance.as_ref().unwrap().limits.is_some());
    }

    /// An ISO 286 `H7` FIT class must answer *designation without limits* — the
    /// numeric envelope is NOT fabricated (the honesty pass-through of
    /// `DimensionalTolerance::limit_range`).
    #[test]
    fn fit_class_tolerance_refuses_fabricated_limits() {
        use crate::drawing::dimensioning::standard_drawing_auto;
        use crate::gdt::model::{Annotation, DimensionalTolerance};

        let (mut m, part, _bore_fid, pid) = bored_plate_with_bore_pid();
        m.gdt.attach(
            pid,
            Annotation::Dimensional(DimensionalTolerance::fit(10.0, "H7")),
        );
        let drawing = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");

        let hole = drawing
            .hole_sites
            .iter()
            .find(|h| h.tolerance.is_some())
            .expect("hole row carries the fit tolerance");
        let tref = hole.tolerance.as_ref().expect("tolerance");
        assert_eq!(tref.kind, "fit");
        assert_eq!(
            tref.designation.as_deref(),
            Some("H7"),
            "fit designation is disclosed: {tref:?}"
        );
        assert!(
            tref.limits.is_none(),
            "an unresolved fit must NOT fabricate numeric limits: {tref:?}"
        );
    }

    /// The raster pictorial is refused (`render_only`) — the certificate never
    /// answers a numeric question from pixels.
    #[test]
    fn raster_is_render_only() {
        use crate::drawing::types::ShadedRaster;
        let (m, s) = cylinder_model("cyl-r", 10.0);
        let mut d = drawing_of(&m, s, uuid::Uuid::nil());
        d.views[0].shaded_raster = Some(ShadedRaster {
            png_base64: "AA==".to_string(),
            px_width: 1,
            px_height: 1,
        });
        let cert = certify_drawing(&m, &d);
        let raster = cert
            .facts
            .iter()
            .find(|f| f.kind == SheetFactKind::RenderOnly)
            .expect("render-only raster fact");
        assert_eq!(raster.live.verdict, SheetVerdict::RenderOnly);
        assert!(
            raster.value.is_none(),
            "a render-only fact carries no numeric answer"
        );
    }
}
