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

use super::hole_table::{HoleSite, UNKNOWN_DEPTH_LABEL};
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
    /// The referenced entity is still live but the sheet's claim about it no
    /// longer holds — the ink is stale relative to the current model. Carries
    /// both numbers so a reader sees the drift, and those numbers always
    /// describe the fact's OWN quantity (the one its `value` / `unit` / `label`
    /// name). A fact can be stale for a claim that quantity does not carry — a
    /// hole row whose diameter still matches but whose depth does not — and
    /// then the verdict alone discloses it, with [`LiveCheck::detail`] naming
    /// which claim in words. The numbers are never repurposed to a different
    /// quantity mid-fact.
    ///
    /// Read `stale` as **"this sheet cannot be confirmed against the model"**,
    /// not "this sheet is wrong". Both reach it: a bore genuinely re-drilled
    /// (the ink IS wrong) and a pre-Task-17 row claiming THRU with no recorded
    /// depth (the bore may well be through — nothing on the sheet can prove
    /// it). The kernel refuses to certify either, and refuses equally to
    /// guess which one it is looking at.
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
    /// The MODEL carries this feature and the SHEET does not. The inverse of
    /// every other verdict: those judge ink against the model, this judges the
    /// model against the ink. A drawing that silently drops a bore is not a
    /// faithful snapshot, so an omission makes the sheet unsound.
    Omitted,
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
            SheetVerdict::Omitted => "omitted",
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
    /// NOT a sheet element: a live model feature the sheet has no counterpart
    /// for. Emitted by the live-side walk in [`certify_drawing`] so a reader is
    /// told what the drawing leaves out, not only whether what it shows is true.
    Omitted,
}

/// The live re-measurement attached to a [`SheetFact`].
///
/// Not `Copy`: [`Self::detail`] owns a `String`. Every consumer reads the
/// scalar fields, so this costs nothing at the call sites.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveCheck {
    /// The value the kernel measured NOW from the referenced entity, in the
    /// fact's unit. `None` for non-numeric facts (FCF/datum/section) and for
    /// facts whose provenance did not resolve.
    pub measured: Option<f64>,
    /// `|measured − sheet_value|` when both are present; `None` otherwise.
    pub deviation: Option<f64>,
    /// The verdict.
    pub verdict: SheetVerdict,
    /// WHICH claim the verdict is about, named, when `value` / `measured`
    /// cannot say so on their own.
    ///
    /// A sheet element can ink more than one claim — a hole row carries a
    /// diameter AND a depth — while a fact has exactly one numeric slot, which
    /// belongs to the quantity its `value` / `unit` / `label` name. So a row
    /// stale on its DEPTH reports `{value: 10.0, measured: 10.0, deviation:
    /// 0.0, verdict: stale}`, which is correct in every field and still leaves
    /// a reader asking "stale how? the diameter matches." This field answers
    /// that in words — `"depth: sheet THRU, live unmeasured"` — instead of
    /// forcing the numbers to carry a quantity they are not about.
    ///
    /// `None` when the verdict is already fully explained by the numbers (an
    /// ordinary diameter drift) or carries no numbers at all. `serde(default)`
    /// keeps pre-Task-17 certificates deserializing.
    #[serde(default)]
    pub detail: Option<String>,
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
    /// Live model features the sheet has no counterpart for.
    /// `serde(default)` keeps pre-Task-17 certificates deserializing.
    #[serde(default)]
    pub omitted: usize,
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
                SheetVerdict::Omitted => c.omitted += 1,
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
    /// True when NO fact is `stale`, `dangling` or `omitted` — the sheet is a
    /// faithful snapshot of the current model in BOTH directions.
    ///
    /// Precisely, `sound` asserts all three of:
    /// - every value the sheet inks still re-measures to what it says (no
    ///   `stale`);
    /// - every entity the sheet names still resolves in the live model (no
    ///   `dangling`);
    /// - every bore the live model carries has a counterpart on the sheet (no
    ///   `omitted`).
    ///
    /// The third clause is what makes this a two-way claim. Until Task 17 the
    /// certificate walked only the sheet's own collections, so it could say
    /// "everything drawn here is true" and mean it, while the part had a hole
    /// the drawing never mentioned — a sound-looking sheet that would be
    /// machined wrong. Auditing only what is already inked cannot detect an
    /// omission; the live-side walk in [`certify_drawing`] is what does.
    ///
    /// `render_only` / `unprovenanced` facts still do not make a sheet unsound:
    /// they are honest absences, not lies.
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
    /// Facts whose verdict is `stale`, `dangling` or `omitted` — the ones a
    /// reader must not trust the sheet on. Exactly the set `sound` is the
    /// emptiness of, so the two can never disagree.
    pub fn unsound_facts(&self) -> impl Iterator<Item = &SheetFact> {
        self.facts.iter().filter(|f| {
            matches!(
                f.live.verdict,
                SheetVerdict::Stale | SheetVerdict::Dangling | SheetVerdict::Omitted
            )
        })
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
            detail: None,
        },
        Some(p) => match live_by_pid.get(p) {
            None => LiveCheck {
                measured: None,
                deviation: None,
                verdict: SheetVerdict::Dangling,
                detail: None,
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
                    detail: None,
                }
            }
        },
    }
}

/// Live-check a hole-table row by re-measuring its bore faces against the
/// analytic table (holes carry face ids, not a PID).
///
/// A hole row inks TWO claims — a diameter and a depth — and both are
/// re-measured here. Checking only the diameter certified half a row and left
/// the DEPTH column, the one a machinist drills to, unaudited.
///
/// `measured` / `deviation` on the returned check always describe the DIAMETER —
/// the quantity this fact's `value`, `unit` and `label` are about. A row stale
/// on its depth alone says so through the verdict; putting a depth in those
/// slots would read as a re-drilled diameter and would break
/// [`LiveCheck::deviation`]'s own `|measured - sheet_value|` contract.
///
/// A depth-driven `stale` names its finding in [`LiveCheck::detail`]
/// (`"depth: sheet THRU, live unmeasured"`), because the numbers alone would
/// read as a fully consistent row.
///
/// The depth rules, in order:
/// - the row records a depth (`depth_mm`) and the model measures a different
///   one → `stale`;
/// - the row records a depth and the model measures NONE → `stale`: the ink
///   asserts a length the kernel can no longer establish;
/// - the row records no depth but its LABEL claims one → `stale`. This is the
///   pre-Task-17 sheet, whether the label reads `THRU` (the old "fallback for
///   degenerate depth") or `↧ 6.00`: a claim standing on no measurement at
///   all. The predicate is the label rather than `is_through` precisely so the
///   blind case is covered too — keying on the flag certified exactly the row
///   whose ink asserts a number the struct cannot prove. Rebuilding the sheet
///   upgrades it; nothing here fabricates the missing number;
/// - the row records no depth and its label is [`UNKNOWN_DEPTH_LABEL`] →
///   the depth is silent, so there is nothing to contradict. An honest absence
///   is not a defect.
fn hole_live_check(site: &HoleSite, live_dims: &[DimensionRecord]) -> LiveCheck {
    let face_entities: &[u32] = &site.face_entities;
    let sheet_dia = site.diameter_mm;
    if face_entities.is_empty() {
        return LiveCheck {
            measured: None,
            deviation: None,
            verdict: SheetVerdict::Unprovenanced,
            detail: None,
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
            detail: None,
        },
        Some(v) => {
            let dev = (v - sheet_dia).abs();
            if dev > CERT_DIM_ORACLE_MM {
                return LiveCheck {
                    measured: Some(v),
                    deviation: Some(dev),
                    verdict: SheetVerdict::Stale,
                    detail: None,
                };
            }
            // The diameter holds; now the depth claim beside it.
            let live_depth = live_dims
                .iter()
                .find(|d| {
                    d.kind == "length" && d.entities.iter().any(|e| face_entities.contains(e))
                })
                .map(|d| d.value);
            // A depth finding is disclosed by the VERDICT. `measured` and
            // `deviation` keep describing the diameter — the quantity this
            // fact's `value`, `unit` and `label` are about — because a reader
            // handed {value: 10.0, measured: 20.0} on a row labelled "Ø10.00"
            // would read a re-drilled diameter that nothing measured, and
            // `deviation`'s contract (`|measured - sheet_value|`) would not
            // even hold across the two quantities.
            //
            // What the row CLAIMS about depth is what its label says, not what
            // `is_through` says: a pre-Task-17 sheet deserialises with
            // `depth_mm: None` while its label still reads "↧ 6.00", and that
            // row asserts six millimetres with nothing behind it just as surely
            // as a bare THRU does. A row is silent only when it renders
            // `UNKNOWN_DEPTH_LABEL`.
            let live_str = match live_depth {
                Some(l) => format!("{l:.2}"),
                None => "unmeasured".to_string(),
            };
            let depth_stale = match (site.depth_mm, live_depth) {
                // Recorded depth vs a live one that moved.
                (Some(sheet_depth), Some(l)) => (l - sheet_depth).abs() > CERT_DIM_ORACLE_MM,
                // Ink records a depth the model no longer measures.
                (Some(_), None) => true,
                // No recorded depth: stale iff the label nonetheless claims one.
                (None, _) => site.depth_label != UNKNOWN_DEPTH_LABEL,
            };
            if depth_stale {
                let sheet_str = match site.depth_mm {
                    Some(d) => format!("{d:.2}"),
                    None => site.depth_label.clone(),
                };
                return LiveCheck {
                    measured: Some(v),
                    deviation: Some(dev),
                    verdict: SheetVerdict::Stale,
                    detail: Some(format!("depth: sheet {sheet_str}, live {live_str}")),
                };
            }
            LiveCheck {
                measured: Some(v),
                deviation: Some(dev),
                verdict: SheetVerdict::Consistent,
                detail: None,
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
            detail: None,
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
                detail: None,
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
            detail: None,
        };
    }
    if cut_through.is_empty() {
        return LiveCheck {
            measured: Some(0.0),
            deviation: None,
            verdict: SheetVerdict::Stale,
            detail: None,
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
                    detail: None,
                };
            }
        }
    }
    LiveCheck {
        measured: Some(cut_through.cuts.len() as f64),
        deviation: None,
        verdict: SheetVerdict::Consistent,
        detail: None,
    }
}

/// Walk the LIVE model for bores the SHEET has no counterpart for, one
/// [`SheetFactKind::Omitted`] fact each.
///
/// # Why this direction exists
///
/// Every other check in this module reads a sheet element and asks the model
/// whether it is still true. That can only ever find WRONG ink; it is
/// structurally blind to MISSING ink. A drawing that never tabled a bore has no
/// row to check, `section_live_check` skips the untagged cut it produces
/// (`let Some(tag) = &cut.hole_tag else { continue }`), and the certificate
/// reports `sound: true` for a part that would be machined with a hole missing.
/// A one-way certificate cannot certify completeness, and completeness is what
/// a shop reader assumes when they are handed a sound sheet.
///
/// # Grouping and the bore qualifier
///
/// Omissions are grouped by the LIVE `"diameter"` dimension record, which is the
/// same unit `attach_hole_table_from_dims` builds a row from: `dedupe_coincident`
/// has already merged a seam-split bore wall's per-face records into one record
/// naming every face, so one physical bore yields one omission rather than one
/// per face. The bore qualifier is likewise the same
/// [`crate::readable::bore_face_ids`] material-side rule the table uses, so a
/// boss or the part's own OD — which also carry diameter records — is never
/// reported as a missing hole.
///
/// This subsumes the section-side case: `section_comprehension::classify_face`
/// draws `SectionCutKind::Bore` from that identical `bore_face_ids` set, and
/// `extract_dimensions` walks the identical shell list, so every bore the
/// section plane cuts has a diameter record here. Walking the records rather
/// than the cuts reports the same bores AND the ones the plane happens to miss.
fn omitted_bore_facts(
    solid_id: Option<SolidId>,
    model: &BRepModel,
    drawing: &Drawing,
    live_dims: &[DimensionRecord],
) -> Vec<SheetFact> {
    use std::collections::HashSet;

    let Some(solid) = solid_id else {
        return Vec::new();
    };
    let live_bores = crate::readable::bore_face_ids(model, solid);
    if live_bores.is_empty() {
        return Vec::new();
    }
    let tabled: HashSet<u32> = drawing
        .hole_sites
        .iter()
        .flat_map(|h| h.face_entities.iter().copied())
        .collect();

    let mut out = Vec::new();
    for d in live_dims {
        if d.kind != "diameter" || d.entities.is_empty() {
            continue;
        }
        // Bores only — a boss or the part silhouette is not a missing hole.
        if !d.entities.iter().any(|e| live_bores.contains(e)) {
            continue;
        }
        // The sheet already carries a row naming one of these faces.
        if d.entities.iter().any(|e| tabled.contains(e)) {
            continue;
        }
        out.push(SheetFact {
            kind: SheetFactKind::Omitted,
            owner_view: drawing.axial_view_idx,
            label: format!("bore {} on the model has no hole-table row", d.label),
            value: Some(d.value),
            unit: d.unit.clone(),
            pid: d.pid.clone(),
            face_ids: d.entities.clone(),
            datum: None,
            tolerance: None,
            live: LiveCheck {
                measured: Some(d.value),
                deviation: None,
                verdict: SheetVerdict::Omitted,
                detail: None,
            },
        });
    }
    out
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
                    detail: None,
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
                    detail: None,
                },
            });
        }
    }

    // ── Hole-table rows ───────────────────────────────────────────────────────
    for hole in &drawing.hole_sites {
        let live = hole_live_check(hole, &live_dims);
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
                detail: None,
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

    // ── LIVE-SIDE WALK: what the model has and the sheet does not ─────────────
    facts.extend(omitted_bore_facts(solid_id, model, drawing, &live_dims));

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
                detail: if unit_matches {
                    None
                } else {
                    Some(format!(
                        "document unit: sheet {:?}, model {:?}",
                        drawing.document_unit,
                        model.document_unit()
                    ))
                },
            },
        });
    }

    let counts = VerdictCounts::tally(&facts);
    let sound = counts.stale == 0 && counts.dangling == 0 && counts.omitted == 0;
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
            ellipses: Vec::new(),
            hidden_ellipses: Vec::new(),
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

    // ── Task 17: depth honesty + the one-way certificate ──────────────────────

    /// A hand-built hole row, so the depth live-check can be exercised against a
    /// dimension table under test control.
    fn hole_row(face_entities: Vec<u32>, dia: f64, depth_mm: Option<f64>, thru: bool) -> HoleSite {
        HoleSite {
            tag: "A1".to_string(),
            group: "A".to_string(),
            diameter_mm: dia,
            x_label: "\u{2014}".to_string(),
            y_label: "\u{2014}".to_string(),
            x_mm: 0.0,
            y_mm: 0.0,
            dia_label: format!("\u{00D8}{dia:.2}"),
            depth_label: if thru {
                "THRU".to_string()
            } else {
                "\u{2014}".to_string()
            },
            is_through: thru,
            depth_mm,
            axial_centre: None,
            world_centre: None,
            face_entities,
            datum: None,
            tolerance: None,
        }
    }

    fn rec(kind: &str, value: f64, fid: u32) -> DimensionRecord {
        DimensionRecord {
            id: format!("{kind}-{fid}"),
            kind: kind.to_string(),
            value,
            unit: "mm".to_string(),
            label: format!("{kind} {value:.2}"),
            entities: vec![fid],
            anchor: [0.0, 0.0, 0.0],
            direction: [0.0, 0.0, 1.0],
            axis: Some([0.0, 0.0, 1.0]),
            pid: None,
            datum: None,
        }
    }

    /// HONESTY GATE (Task 17a): the live check re-measures DEPTH as well as
    /// diameter. A row claiming THRU on a bore the model measures no depth for
    /// is not `consistent` — the sheet asserts a fact the kernel cannot
    /// corroborate, and the certificate must say so rather than pass the
    /// diameter and stay silent about the claim beside it.
    ///
    /// Mutation: drop the depth arm from `hole_live_check` → every assertion
    /// below that is not `Consistent` goes RED.
    #[test]
    fn hole_row_claiming_thru_on_an_unmeasured_bore_is_not_consistent() {
        // The model measures the bore's DIAMETER but no length for it.
        let live = vec![rec("diameter", 10.0, 7)];

        let claims_thru = hole_row(vec![7], 10.0, None, true);
        let check = hole_live_check(&claims_thru, &live);
        assert_ne!(
            check.verdict,
            SheetVerdict::Consistent,
            "a THRU claim with no measured depth is not a certified fact: {check:?}"
        );
        assert_eq!(
            check.verdict,
            SheetVerdict::Stale,
            "the sheet's ink outruns the model — that is stale, not an absence: {check:?}"
        );

        // The honest sibling: the same bore rendered "\u{2014}" claims nothing,
        // so nothing about it is stale.
        let claims_nothing = hole_row(vec![7], 10.0, None, false);
        assert_eq!(
            hole_live_check(&claims_nothing, &live).verdict,
            SheetVerdict::Consistent,
            "an unmeasured depth honestly rendered as unknown asserts nothing"
        );

        // A row whose recorded depth still matches the model is consistent; one
        // whose bore was re-drilled deeper is stale, carrying the new number.
        let live_deep = vec![rec("diameter", 10.0, 7), rec("length", 20.0, 7)];
        assert_eq!(
            hole_live_check(&hole_row(vec![7], 10.0, Some(20.0), true), &live_deep).verdict,
            SheetVerdict::Consistent,
            "a corroborated THRU row is consistent"
        );
        let moved = hole_live_check(&hole_row(vec![7], 10.0, Some(6.0), false), &live_deep);
        assert_eq!(
            moved.verdict,
            SheetVerdict::Stale,
            "a blind row on a bore now 20 mm deep is stale ink: {moved:?}"
        );
        // The finding is the DEPTH, but this fact's numbers are the DIAMETER's:
        // a reader handed measured=20.0 beside a label reading "Ø10.00" would
        // conclude the bore was re-drilled, which nothing measured. The
        // quantity a fact reports must be the quantity it names.
        assert_eq!(
            moved.measured,
            Some(10.0),
            "a stale row still reports its own quantity, the diameter: {moved:?}"
        );
        assert_eq!(
            moved.deviation,
            Some(0.0),
            "and its deviation stays |measured - value| on that quantity: {moved:?}"
        );
    }

    /// HONESTY GATE (Task 17, fix round 1, item 3): a depth-stale row must NAME
    /// the claim that is stale.
    ///
    /// The numeric slots correctly describe the diameter, which still matches —
    /// so on the numbers alone the fact reads `{value: 10.0, measured: 10.0,
    /// deviation: 0.0, verdict: stale}`: every field right, and a reader left
    /// asking "stale how?". `LiveCheck::detail` answers in words.
    ///
    /// Mutation: return `detail: None` from the depth arm → RED.
    #[test]
    fn a_depth_stale_row_names_which_claim_is_stale() {
        let live = vec![rec("diameter", 10.0, 7), rec("length", 20.0, 7)];

        // Recorded depth that moved.
        let moved = hole_live_check(&hole_row(vec![7], 10.0, Some(6.0), false), &live);
        let d = moved
            .detail
            .as_deref()
            .expect("a depth-stale row names the finding");
        assert!(
            d.contains("depth") && d.contains("6.00") && d.contains("20.00"),
            "the detail names the claim and both numbers: {d:?}"
        );
        assert!(!d.contains("  "), "no absorbed indentation: {d:?}");

        // THRU claimed with nothing behind it, and no live depth either.
        let bare = vec![rec("diameter", 10.0, 7)];
        let thru = hole_live_check(&hole_row(vec![7], 10.0, None, true), &bare);
        let t = thru
            .detail
            .as_deref()
            .expect("a THRU claim names its finding");
        assert!(
            t.contains("depth") && t.contains("THRU") && t.contains("unmeasured"),
            "the detail says the THRU claim has no live depth behind it: {t:?}"
        );
        assert!(!t.contains("  "), "no absorbed indentation: {t:?}");

        // A consistent row asserts nothing extra to explain.
        let ok = hole_live_check(&hole_row(vec![7], 10.0, Some(20.0), true), &live);
        assert_eq!(ok.verdict, SheetVerdict::Consistent);
        assert!(
            ok.detail.is_none(),
            "a consistent row has no finding to name: {ok:?}"
        );
    }

    /// HONESTY GATE (Task 17, fix round 1, MINOR): a pre-Task-17 BLIND row is
    /// checked too, not just a THRU one.
    ///
    /// An old sheet deserialises with `depth_mm: None` while its `depth_label`
    /// still reads "↧ 6.00" — it claims a depth of six millimetres and carries
    /// no measurement to back it. Keying the check on `is_through` alone
    /// certified exactly that row `consistent`: the one class of row where the
    /// ink asserts a number the struct cannot prove. The predicate is the
    /// LABEL, not the flag — a row is silent only when it renders the unknown
    /// glyph.
    ///
    /// Mutation: drop the `depth_label != UNKNOWN_DEPTH_LABEL` clause → RED.
    #[test]
    fn a_legacy_blind_row_with_no_recorded_depth_is_not_certified() {
        let live = vec![rec("diameter", 10.0, 7), rec("length", 20.0, 7)];

        // Pre-Task-17 blind row: label claims 6 mm, no `depth_mm` behind it.
        let mut legacy = hole_row(vec![7], 10.0, None, false);
        legacy.depth_label = "\u{21A7} 6.00".to_string();
        let check = hole_live_check(&legacy, &live);
        assert_eq!(
            check.verdict,
            SheetVerdict::Stale,
            "a blind row claiming 6.00 with nothing behind it is not certified: {check:?}"
        );
        assert!(
            check
                .detail
                .as_deref()
                .is_some_and(|d| d.contains("6.00") && d.contains("depth")),
            "and it names the unbacked claim: {check:?}"
        );

        // The honest sibling is unchanged: the unknown glyph claims nothing.
        let silent = hole_row(vec![7], 10.0, None, false);
        assert_eq!(silent.depth_label, UNKNOWN_DEPTH_LABEL);
        assert_eq!(
            hole_live_check(&silent, &live).verdict,
            SheetVerdict::Consistent,
            "a row rendering the unknown glyph asserts nothing to contradict"
        );
    }

    /// Build a 40×40×20 plate with TWO Ø10 through bores at (±10, 0), both on
    /// the line the section plane cuts along.
    fn twin_bored_plate() -> (BRepModel, SolidId) {
        use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
        let mut m = BRepModel::new();
        m.set_event_key(Some("twin-bore-plate".to_string()));
        let mut part = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(40.0, 40.0, 20.0)
            .expect("plate"));
        for (i, x) in [-10.0_f64, 10.0].into_iter().enumerate() {
            // One event key per drilling: two booleans under a SHARED key mint
            // the same PID for two different result faces and the kernel's own
            // `assign_boolean_face_pids` invariant refuses the pass.
            m.set_event_key(Some(format!("twin-bore-{i}")));
            let bore = sid(TopologyBuilder::new(&mut m)
                .create_cylinder_3d(Point3::new(x, 0.0, -20.0), Vector3::Z, 5.0, 80.0)
                .expect("bore"));
            part = boolean_operation(
                &mut m,
                part,
                bore,
                BooleanOp::Difference,
                BooleanOptions::default(),
            )
            .expect("difference");
        }
        m.set_event_key(None);
        (m, part)
    }

    /// HONESTY GATE (Task 17b): the certificate walks the LIVE side too.
    ///
    /// `certify_drawing` used to iterate only the sheet's own collections, so a
    /// feature the MODEL carries and the SHEET does not was invisible to it: the
    /// section's cut-through would list the bore, `section_live_check` would hit
    /// `let Some(tag) = &cut.hole_tag else { continue }`, and the certificate
    /// returned `sound: true` for a drawing that omits a hole a machinist must
    /// drill. A certificate that only ever audits what is already inked can
    /// never report an omission.
    ///
    /// # Why the omission is made sheet-side, not model-side
    ///
    /// The literal "drill a second bore into the model afterwards" mutation
    /// rebuilds the topology: every face id churns, so the sheet's EXISTING hole
    /// rows dangle and its dimension PIDs go stale, and `sound` is already false
    /// before this fix for reasons that have nothing to do with the omission.
    /// That makes it useless as a gate. Dropping one row from a sheet whose
    /// every other fact still resolves isolates exactly the defect: pre-fix this
    /// certificate is `sound == true` with the bore standing untabled in its own
    /// section cut.
    ///
    /// Mutation: delete the live-side omission walk from `certify_drawing` → the
    /// `Omitted` fact and `!sound` assertions go RED.
    #[test]
    fn a_bore_the_sheet_omits_makes_the_certificate_unsound() {
        use crate::drawing::dimensioning::standard_drawing_auto;

        let (m, part) = twin_bored_plate();
        let mut drawing = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");
        assert!(
            drawing.hole_sites.len() >= 2,
            "the fixture must table both bores: {:?}",
            drawing.hole_sites
        );

        // Control: with both bores tabled the sheet is sound.
        let cert0 = certify_drawing(&m, &drawing);
        assert!(
            cert0.sound,
            "the complete sheet must certify sound: counts={:?} unsound={:?}",
            cert0.counts,
            cert0.unsound_facts().collect::<Vec<_>>()
        );
        assert_eq!(cert0.counts.omitted, 0, "nothing is omitted yet");

        // Drop one row: the sheet now omits a bore the model carries.
        let dropped = drawing.hole_sites.remove(0);
        let cert = certify_drawing(&m, &drawing);

        // The SECTION path is genuinely exercised: the plane still crosses the
        // untabled bore, and its cut now carries no hole tag — the `continue`
        // site this gate exists for.
        let ct = cert
            .section_cuts
            .as_ref()
            .expect("the fixture sheet carries a SECTION, so the certificate must carry its cuts");
        let untagged = ct.cuts.iter().any(|c| {
            c.kind == SectionCutKind::Bore
                && c.hole_tag.is_none()
                && c.face_ids.iter().any(|f| dropped.face_entities.contains(f))
        });
        assert!(
            untagged,
            "the dropped bore must appear in the section cut with no tag: {ct:?}"
        );

        let omitted: Vec<&SheetFact> = cert
            .facts
            .iter()
            .filter(|f| f.kind == SheetFactKind::Omitted)
            .collect();
        assert!(
            omitted.iter().any(|f| {
                f.face_ids
                    .iter()
                    .any(|id| dropped.face_entities.contains(id))
                    && f.value
                        .map(|v| (v - dropped.diameter_mm).abs() <= CERT_DIM_ORACLE_MM)
                        .unwrap_or(false)
            }),
            "an Omitted fact must name the untabled bore by face and diameter; \
             dropped={dropped:?} omitted={omitted:?}"
        );
        for f in &omitted {
            assert_eq!(f.live.verdict, SheetVerdict::Omitted);
            assert!(
                !f.label.contains("  "),
                "no absorbed indentation in a user-facing label: {:?}",
                f.label
            );
        }
        assert!(
            cert.counts.omitted > 0,
            "omissions are counted: {:?}",
            cert.counts
        );
        assert!(
            !cert.sound,
            "a sheet that omits a bore the model carries is NOT sound: {:?}",
            cert.counts
        );
        assert!(
            cert.unsound_facts()
                .any(|f| f.kind == SheetFactKind::Omitted),
            "the omission must reach the facts-a-reader-must-not-trust list"
        );
    }
}
