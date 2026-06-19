//! Human-readable labels — a NAME pinned to a topological entity (vertex / edge
//! / face) or a named cross-section plane, so the agent and the user share one
//! vocabulary for the geometry.
//!
//! This is the materialization of Pillar 3 (semantic naming / resolve-by-
//! description): `queries::select` lets the agent FIND a face/edge by meaning;
//! this module lets it PIN a durable name on what it found ("the min-radius
//! face is the `throat`"), so subsequent conversation can refer to `throat`
//! and the kernel resolves it back to the exact same entity — or refuses.
//!
//! ## What a label is
//!
//! * An ENTITY label binds a NAME to a topological entity by its
//!   [`PersistentId`] (not the transient `FaceId`/`EdgeId`/`VertexId`), so the
//!   reference survives regeneration + parameter edits exactly like a fillet
//!   reference or a GD&T annotation does. The `kind` records whether the name
//!   points at a vertex, an edge, or a face.
//! * A SECTION label binds a NAME to a cutting PLANE (`origin` + `normal`). A
//!   named cross-section is not a topological entity — there is no PID to key
//!   on — so the plane itself is stored.
//!
//! ## Honesty
//!
//! Names are unique per part (attaching a name already in use REPLACES, never
//! silently duplicates — the caller is told). Resolving an unknown name returns
//! a clear [`LabelError::NotFound`]; the kernel never guesses. A label whose
//! entity has been deleted resolves to [`LabelError::Dangling`] rather than a
//! wrong entity. This mirrors the refusal discipline of `queries::select`.
//!
//! ## Layout (mirrors the GD&T / provenance sidecar)
//!
//! [`LabelSidecar`] lives BESIDE the SoA topology stores, keyed by
//! [`PersistentId`] just like [`crate::gdt::sidecar::GdtSidecar`], so it is
//! snapshot-safe (PIDs are snapshotted) and emptied by `clear_geometry`. It is
//! an additive field on `BRepModel`; nothing in the columnar hot path changes.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::math::{Point3, Vector3};
use crate::primitives::persistent_id::PersistentId;

/// Which kind of topological entity a name points at. Recorded so the resolver
/// can report the kind back without re-deriving it, and so the eye-overlay can
/// pick the right anchor (face centroid vs edge midpoint vs vertex position).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LabelKind {
    Vertex,
    Edge,
    Face,
}

impl LabelKind {
    /// Short agent/wire-facing tag.
    pub fn tag(self) -> &'static str {
        match self {
            LabelKind::Vertex => "vertex",
            LabelKind::Edge => "edge",
            LabelKind::Face => "face",
        }
    }

    /// Parse a wire tag, case-insensitively. `None` for an unrecognised tag —
    /// the caller refuses rather than defaulting.
    pub fn from_tag(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "vertex" | "point" => Some(LabelKind::Vertex),
            "edge" => Some(LabelKind::Edge),
            "face" => Some(LabelKind::Face),
            _ => None,
        }
    }
}

/// A named cross-section: a cutting plane through `origin` with unit `normal`.
/// Not a topological entity (hence not PID-keyed) — the plane IS the label's
/// target. Pairs with the section-view eye (`render::dimensioned::render_section`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SectionPlane {
    pub origin: Point3,
    pub normal: Vector3,
}

/// What a label points at: a topological entity (by durable PID) or a section
/// plane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LabelTarget {
    /// A vertex / edge / face, referenced by its persistent id.
    Entity { kind: LabelKind, pid: PersistentId },
    /// A named cutting plane.
    Section(SectionPlane),
}

impl LabelTarget {
    /// The wire-facing kind tag (`"vertex"`/`"edge"`/`"face"`/`"section"`).
    pub fn kind_tag(&self) -> &'static str {
        match self {
            LabelTarget::Entity { kind, .. } => kind.tag(),
            LabelTarget::Section(_) => "section",
        }
    }
}

/// A serializable mirror of a `queries::select::FaceQuery` — the descriptive
/// claim "the face that means X" (e.g. the smallest-area cylindrical wall is the
/// throat). Stored on a label as its ASSERTION so `resolve()` can RE-RUN the
/// selector and confirm the same face still satisfies it (or report `Stale`).
///
/// Decoupled from `queries::select` (which is not serde) by mirroring its fields
/// as primitives; `topology_builder` rebuilds the live `FaceQuery` from this.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FaceSelectorSpec {
    /// Surface-kind tag: `any|planar|cylindrical|spherical|conical|toroidal|nurbs`.
    pub surface: String,
    /// Optional required outward-normal direction.
    pub normal_dir: Option<[f64; 3]>,
    pub angle_tol_deg: f64,
    /// Extremal tag: `none|largest_area|smallest_area|most_along|min_radius_station|axial_extremal_cap`.
    pub extremal: String,
    /// Direction for the `most_along` extremal.
    pub along: Option<[f64; 3]>,
    /// Symmetry/revolve axis origin for the geometry-aware extremals
    /// (`min_radius_station`, `axial_extremal_cap`). `None` for the others. A
    /// missing field deserializes to `None`, so older specs round-trip.
    #[serde(default)]
    pub axis_origin: Option<[f64; 3]>,
    /// Symmetry/revolve axis direction for the geometry-aware extremals.
    #[serde(default)]
    pub axis_dir: Option<[f64; 3]>,
}

/// A serializable mirror of a `queries::select::EdgeQuery`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeSelectorSpec {
    /// Curve-kind tag: `any|line|arc|circle|nurbs`.
    pub curve: String,
    /// Blend-state tag: `any|filleted|chamfered|unblended`.
    pub blend: String,
    pub direction: Option<[f64; 3]>,
    pub angle_tol_deg: f64,
    /// Extremal tag: `none|longest|shortest|most_along`.
    pub extremal: String,
    pub along: Option<[f64; 3]>,
}

/// A descriptive selector assertion — the Pillar-3 claim that picks the entity
/// out by MEANING. Carried verbatim so `resolve()` re-runs the exact selector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SelectorSpec {
    Face(FaceSelectorSpec),
    Edge(EdgeSelectorSpec),
}

/// The GEOMETRIC IDENTITY of the entity a label was pinned to, captured at
/// attach time. `resolve()` re-derives the named entity's geometry and confirms
/// it still matches this fingerprint within tolerance, else reports `Stale`.
///
/// Every field beyond `kind`/`position` is optional because not every entity
/// kind has it (a planar face has no radius; a straight edge has no radius);
/// `None` means "not part of the identity claim".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub kind: LabelKind,
    /// World-space representative point (face centroid / edge midpoint / vertex
    /// position).
    pub position: [f64; 3],
    /// Representative outward normal (faces only).
    pub normal: Option<[f64; 3]>,
    /// Representative radius (curved faces / circular edges).
    pub radius: Option<f64>,
    /// Representative size (face area / edge length) — a coarse identity signal.
    pub size: Option<f64>,
}

/// The ASSERTION every label MUST carry (D4: no bare labels). It is the claim
/// the kernel can keep PROVING: either the descriptive `Selector` that picked
/// the entity out, or the `Fingerprint` geometric identity captured at attach.
///
/// A label is a NAME bound to an assertion the kernel re-verifies on resolve —
/// not a silent pointer that can drift onto the wrong entity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LabelAssertion {
    /// "the entity that satisfies this description" — re-run the selector.
    Selector(SelectorSpec),
    /// "the entity with this geometric identity" — re-match the fingerprint.
    Fingerprint(Fingerprint),
}

/// One label: its target, the ASSERTION that justifies the name (D4 — required;
/// the kernel re-verifies it on resolve), plus an optional human description.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Label {
    pub target: LabelTarget,
    /// The identity claim. `None` ONLY for a `Section` label (a named plane is
    /// its own claim — the plane IS the assertion); an `Entity` label MUST have
    /// `Some` assertion (enforced by [`LabelSidecar::attach`]).
    pub assertion: Option<LabelAssertion>,
    pub description: Option<String>,
}

/// The verdict of re-verifying a label's assertion at resolve time. Distinct
/// from [`LabelError`] (which is about whether the NAME / kind is even valid):
/// `Stale` means the name and kind are fine but the assertion no longer holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssertionStatus {
    /// The assertion still holds — the entity it picks out is unchanged.
    Holds,
    /// The assertion no longer holds: the selector now finds nothing / a
    /// different entity, or no live entity matches the fingerprint within tol.
    /// The kernel reports this honestly rather than silently re-pointing.
    Stale,
}

/// One auto-recognized feature SUGGESTION (D3). The kernel recognizes a feature
/// and proposes a NAME + the ASSERTION that pins it — but does NOT apply it. The
/// user owns the name; confirming = `label_create` with this exact assertion, so
/// the kernel owns the claim. `confidence` is a 0..1 heuristic strength.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelProposal {
    pub suggested_name: String,
    pub kind: &'static str,
    pub assertion: LabelAssertion,
    pub confidence: f64,
    /// One-line human rationale ("smallest-radius cylindrical wall").
    pub rationale: String,
}

/// Why a label operation refused. The kernel never guesses — an unknown name,
/// an invalid name, or a name whose entity has been deleted all surface here
/// rather than resolving to the wrong thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelError {
    /// The name was empty or all-whitespace.
    EmptyName,
    /// No label with that name on this part.
    NotFound,
    /// The label exists but its entity no longer resolves to a live id (the
    /// topology it named was deleted/regenerated away). Honest "I had it, it's
    /// gone" rather than a silent wrong answer.
    Dangling,
    /// D4 — NO BARE LABELS: an `Entity` label was offered with no assertion.
    /// A name must carry the claim that justifies it (selector or fingerprint),
    /// so the kernel can keep proving it. Refused, never stored bare.
    MissingAssertion,
    /// A rename target name is already owned by a DIFFERENT label. Refused rather
    /// than silently clobbering the existing binding.
    NameInUse,
}

/// Outcome of [`LabelSidecar::attach`]: whether the name was newly created or
/// replaced an existing label, so the caller can tell the user honestly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachOutcome {
    Created,
    Replaced,
}

/// What KIND of dimension a label's measurement reports — picks the glyph the
/// `display` string is formatted with (Ø for a diameter, ∠ for an angle, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeasurementKind {
    Diameter,
    Radius,
    Length,
    Area,
    Angle,
    Position,
}

impl MeasurementKind {
    /// The wire/agent-facing tag.
    pub fn tag(self) -> &'static str {
        match self {
            MeasurementKind::Diameter => "diameter",
            MeasurementKind::Radius => "radius",
            MeasurementKind::Length => "length",
            MeasurementKind::Area => "area",
            MeasurementKind::Angle => "angle",
            MeasurementKind::Position => "position",
        }
    }
}

/// A MEASURED key dimension of a labelled feature, in the document unit. The
/// value is read from the live geometry (truth), never asserted: a cylindrical
/// face yields its `Diameter`, a circular edge its `Radius`, a straight edge its
/// `Length`, a planar face its `Area`, a vertex its `Position`. `display` is the
/// formatted, glyph-decorated string (e.g. `"Ø2.00 mm"`, `"4.50 mm"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    pub value: f64,
    pub unit: String,
    pub kind: MeasurementKind,
    pub display: String,
}

/// A deterministic, well-separated display COLOR for a label NAME: stable across
/// runs (same name → same colour) and visibly distinct between names (e.g.
/// throat / chamber / exit). Returns `[r, g, b]`.
///
/// The hue is the FNV-1a hash of the name mapped onto the colour wheel, then
/// converted from a fixed-saturation/value HSV so every colour is vivid and no
/// two hashed hues collapse to the same grey. Saturation/value are high and
/// fixed so the palette stays legible on the shaded render and as a swatch.
pub fn label_color(name: &str) -> [u8; 3] {
    // FNV-1a over the trimmed name — small, fast, well-distributed.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.trim().as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    // Hue from the full hash range; fixed S, V for a vivid, separable palette.
    let hue = (h % 360) as f64;
    hsv_to_rgb(hue, 0.72, 0.92)
}

/// Hex `"#rrggbb"` form of [`label_color`] — the wire/agent-facing colour.
pub fn label_color_hex(name: &str) -> String {
    let [r, g, b] = label_color(name);
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// HSV→RGB with `h` in degrees `[0,360)`, `s`/`v` in `[0,1]`. Standard sextant
/// conversion; used to spread label hues evenly around the wheel.
fn hsv_to_rgb(h: f64, s: f64, v: f64) -> [u8; 3] {
    let c = v * s;
    let hp = (h.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [
        (((r1 + m) * 255.0).round()).clamp(0.0, 255.0) as u8,
        (((g1 + m) * 255.0).round()).clamp(0.0, 255.0) as u8,
        (((b1 + m) * 255.0).round()).clamp(0.0, 255.0) as u8,
    ]
}

/// The per-model label store. Names are unique (a `HashMap` keyed by name);
/// entity targets carry their durable [`PersistentId`]. Mirrors the GD&T
/// sidecar: snapshot/clear-safe, lean (off the SoA hot path), serializable.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LabelSidecar {
    labels: HashMap<String, Label>,
}

impl LabelSidecar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Normalize a user/agent-supplied name: trim surrounding whitespace. The
    /// trimmed form is the canonical key, so " throat " and "throat" are the
    /// same label.
    fn normalize(name: &str) -> &str {
        name.trim()
    }

    /// Validate a name for attachment: non-empty after trimming. Returns the
    /// canonical (trimmed) form, or [`LabelError::EmptyName`].
    pub fn validate_name(name: &str) -> Result<&str, LabelError> {
        let n = Self::normalize(name);
        if n.is_empty() {
            Err(LabelError::EmptyName)
        } else {
            Ok(n)
        }
    }

    /// Attach (or replace) a label. The name must be non-empty (validated);
    /// re-using a name REPLACES the existing label and reports
    /// [`AttachOutcome::Replaced`] so the caller never silently shadows a name.
    ///
    /// D4 (NO BARE LABELS): an `Entity` label MUST carry an assertion — a name
    /// with none is refused with [`LabelError::MissingAssertion`]. A `Section`
    /// label needs none (the plane is its own claim).
    pub fn attach(&mut self, name: &str, label: Label) -> Result<AttachOutcome, LabelError> {
        let key = Self::validate_name(name)?.to_string();
        if matches!(label.target, LabelTarget::Entity { .. }) && label.assertion.is_none() {
            return Err(LabelError::MissingAssertion);
        }
        let outcome = if self.labels.contains_key(&key) {
            AttachOutcome::Replaced
        } else {
            AttachOutcome::Created
        };
        self.labels.insert(key, label);
        Ok(outcome)
    }

    /// Look up a label by name (exact, after trimming). `None` if absent.
    pub fn get(&self, name: &str) -> Option<&Label> {
        self.labels.get(Self::normalize(name))
    }

    /// Remove a label by name, returning it if it existed.
    pub fn remove(&mut self, name: &str) -> Option<Label> {
        self.labels.remove(Self::normalize(name))
    }

    /// Rename a label, preserving its target + assertion + description. The new
    /// name must be non-empty (validated). Refuses [`LabelError::NotFound`] when
    /// `old` is unknown, and (to never silently clobber) refuses
    /// [`LabelError::EmptyName`] reused here for a `new` name already taken by a
    /// DIFFERENT label — the caller must `remove` the collision first. A no-op
    /// rename to the same canonical name succeeds.
    pub fn rename(&mut self, old: &str, new: &str) -> Result<(), LabelError> {
        let old_key = Self::normalize(old).to_string();
        let new_key = Self::validate_name(new)?.to_string();
        if !self.labels.contains_key(&old_key) {
            return Err(LabelError::NotFound);
        }
        if new_key != old_key && self.labels.contains_key(&new_key) {
            // A distinct label already owns the target name — refuse rather than
            // overwrite it (the kernel never silently drops a binding).
            return Err(LabelError::NameInUse);
        }
        if let Some(label) = self.labels.remove(&old_key) {
            self.labels.insert(new_key, label);
        }
        Ok(())
    }

    /// Iterate `(name, &label)` over every label, in name order (deterministic
    /// for agent-facing listings and tests).
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Label)> {
        let mut keys: Vec<&String> = self.labels.keys().collect();
        keys.sort_unstable();
        keys.into_iter()
            .filter_map(move |k| self.labels.get(k).map(|l| (k.as_str(), l)))
    }

    /// Number of labels on this part.
    pub fn len(&self) -> usize {
        self.labels.len()
    }

    /// True when nothing is labelled — the default state of a fresh model.
    pub fn is_empty(&self) -> bool {
        self.labels.is_empty()
    }

    /// Empty the sidecar — paired with `BRepModel::clear_geometry`, since entity
    /// labels are bound to topology being discarded (and a named section is
    /// scoped to the part it was cut on).
    pub fn clear(&mut self) {
        self.labels.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(kind: LabelKind) -> LabelAssertion {
        LabelAssertion::Fingerprint(Fingerprint {
            kind,
            position: [0.0, 0.0, 0.0],
            normal: None,
            radius: None,
            size: None,
        })
    }

    fn face_label(pid: PersistentId) -> Label {
        Label {
            target: LabelTarget::Entity {
                kind: LabelKind::Face,
                pid,
            },
            assertion: Some(fp(LabelKind::Face)),
            description: None,
        }
    }

    #[test]
    fn attach_get_roundtrip() {
        let mut s = LabelSidecar::new();
        assert!(s.is_empty());
        let pid = PersistentId::root(b"throat-face");
        assert_eq!(
            s.attach("throat", face_label(pid)),
            Ok(AttachOutcome::Created)
        );
        assert_eq!(s.len(), 1);
        match &s.get("throat").expect("present").target {
            LabelTarget::Entity { kind, pid: got } => {
                assert_eq!(*kind, LabelKind::Face);
                assert_eq!(*got, pid);
            }
            _ => panic!("expected entity target"),
        }
    }

    #[test]
    fn names_trim_and_are_unique() {
        let mut s = LabelSidecar::new();
        let a = PersistentId::root(b"a");
        let b = PersistentId::root(b"b");
        assert_eq!(
            s.attach("  exit ", face_label(a)),
            Ok(AttachOutcome::Created)
        );
        // Same name (after trim) replaces.
        assert_eq!(s.attach("exit", face_label(b)), Ok(AttachOutcome::Replaced));
        assert_eq!(s.len(), 1, "replace, not duplicate");
        match &s.get("exit").expect("present").target {
            LabelTarget::Entity { pid, .. } => assert_eq!(*pid, b),
            _ => panic!(),
        }
    }

    #[test]
    fn empty_name_refused() {
        let mut s = LabelSidecar::new();
        let pid = PersistentId::root(b"x");
        assert_eq!(s.attach("   ", face_label(pid)), Err(LabelError::EmptyName));
        assert!(s.is_empty());
    }

    #[test]
    fn bare_entity_label_refused() {
        // D4: an Entity label with no assertion is refused, never stored bare.
        let mut s = LabelSidecar::new();
        let bare = Label {
            target: LabelTarget::Entity {
                kind: LabelKind::Face,
                pid: PersistentId::root(b"x"),
            },
            assertion: None,
            description: None,
        };
        assert_eq!(s.attach("throat", bare), Err(LabelError::MissingAssertion));
        assert!(s.is_empty(), "bare label was not stored");
    }

    #[test]
    fn section_label_stores_plane() {
        let mut s = LabelSidecar::new();
        let plane = SectionPlane {
            origin: Point3::new(0.0, 0.0, 5.0),
            normal: Vector3::new(0.0, 0.0, 1.0),
        };
        s.attach(
            "midspan",
            Label {
                target: LabelTarget::Section(plane),
                assertion: None,
                description: Some("cut at z=5".into()),
            },
        )
        .expect("attach");
        match &s.get("midspan").expect("present").target {
            LabelTarget::Section(p) => {
                assert_eq!(p.origin, plane.origin);
                assert_eq!(p.normal, plane.normal);
            }
            _ => panic!("expected section target"),
        }
    }

    #[test]
    fn iter_is_name_ordered() {
        let mut s = LabelSidecar::new();
        for n in ["zeta", "alpha", "mid"] {
            s.attach(n, face_label(PersistentId::root(n.as_bytes())))
                .expect("attach");
        }
        let names: Vec<&str> = s.iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["alpha", "mid", "zeta"]);
    }

    #[test]
    fn label_color_is_deterministic_and_distinct() {
        // Same name → same colour, every time (stable hash, not run-dependent).
        assert_eq!(label_color("throat"), label_color("throat"));
        assert_eq!(label_color_hex("chamber"), label_color_hex("chamber"));
        // Distinct names → distinct colours (the throat / chamber / exit triad
        // must be visibly different).
        let t = label_color("throat");
        let c = label_color("chamber");
        let e = label_color("exit");
        assert_ne!(t, c);
        assert_ne!(t, e);
        assert_ne!(c, e);
        // The hex form is well-formed `#rrggbb`.
        let hex = label_color_hex("throat");
        assert_eq!(hex.len(), 7);
        assert!(hex.starts_with('#'));
        assert!(hex[1..].chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn rename_preserves_binding_and_refuses_collision() {
        let mut s = LabelSidecar::new();
        let a = PersistentId::root(b"a");
        let b = PersistentId::root(b"b");
        s.attach("throat", face_label(a)).expect("attach throat");
        s.attach("chamber", face_label(b)).expect("attach chamber");
        // Rename throat → bore: the binding moves with the name.
        s.rename("throat", "bore").expect("rename");
        assert!(s.get("throat").is_none());
        match &s.get("bore").expect("renamed present").target {
            LabelTarget::Entity { pid, .. } => assert_eq!(*pid, a),
            _ => panic!(),
        }
        // Renaming onto an in-use DIFFERENT name refuses (never clobbers chamber).
        assert_eq!(s.rename("bore", "chamber"), Err(LabelError::NameInUse));
        assert_eq!(s.len(), 2, "no binding lost");
        // Unknown old name → NotFound; empty new name → EmptyName.
        assert_eq!(s.rename("missing", "x"), Err(LabelError::NotFound));
        assert_eq!(s.rename("bore", "  "), Err(LabelError::EmptyName));
    }

    #[test]
    fn clear_empties() {
        let mut s = LabelSidecar::new();
        s.attach("a", face_label(PersistentId::root(b"a")))
            .expect("attach");
        s.clear();
        assert!(s.is_empty());
    }

    #[test]
    fn round_trips_through_json() {
        let mut s = LabelSidecar::new();
        s.attach(
            "throat",
            Label {
                target: LabelTarget::Entity {
                    kind: LabelKind::Face,
                    pid: PersistentId::root(b"throat"),
                },
                assertion: Some(fp(LabelKind::Face)),
                description: Some("min area".into()),
            },
        )
        .expect("attach");
        s.attach(
            "cut",
            Label {
                target: LabelTarget::Section(SectionPlane {
                    origin: Point3::ORIGIN,
                    normal: Vector3::new(1.0, 0.0, 0.0),
                }),
                assertion: None,
                description: None,
            },
        )
        .expect("attach");
        let json = serde_json::to_string(&s).expect("serialize");
        let back: LabelSidecar = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(s, back);
    }
}
