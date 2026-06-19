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

/// One label: its target plus an optional human description (free text the
/// agent/user attached, e.g. "minimum-area throat of the nozzle").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Label {
    pub target: LabelTarget,
    pub description: Option<String>,
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
}

/// Outcome of [`LabelSidecar::attach`]: whether the name was newly created or
/// replaced an existing label, so the caller can tell the user honestly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachOutcome {
    Created,
    Replaced,
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
    pub fn attach(&mut self, name: &str, label: Label) -> Result<AttachOutcome, LabelError> {
        let key = Self::validate_name(name)?.to_string();
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

    fn face_label(pid: PersistentId) -> Label {
        Label {
            target: LabelTarget::Entity {
                kind: LabelKind::Face,
                pid,
            },
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
                description: None,
            },
        )
        .expect("attach");
        let json = serde_json::to_string(&s).expect("serialize");
        let back: LabelSidecar = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(s, back);
    }
}
