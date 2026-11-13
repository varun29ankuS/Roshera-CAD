//! GD&T annotation sidecar — the per-model store binding tolerances and feature
//! control frames to features by [`PersistentId`].
//!
//! Mirrors the kernel's existing sidecar pattern (persistent-id maps,
//! provenance): the GD&T data lives *beside* the SoA topology stores rather than
//! embedded in them, keeping the columnar cache layout lean and a tolerance
//! probe an O(1) hashmap lookup off the math hot path. It is snapshot/clear-safe
//! the same way the PID sidecar is — annotations key off persistent ids (which
//! ARE snapshotted), so taking/restoring a model snapshot carries the GD&T with
//! it, and `clear_geometry` empties it.
//!
//! Keying by [`PersistentId`] (not the transient `FaceId`) is deliberate: a
//! tolerance authored as "flatness of THIS face" must follow the face across
//! regeneration and parameter edits, exactly like a fillet reference does.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::gdt::model::{Annotation, Datum};
use crate::primitives::persistent_id::PersistentId;
use crate::primitives::solid::SolidId;

/// Where an IDENTIFIED annotation lives: the feature it is keyed by and the
/// part (solid) that owns it.
///
/// The owner is what lets a dangling annotation still be attributed to its
/// part: once the feature face is consumed, the persistent id resolves to no
/// face, so face membership can no longer say whose tolerance it was. The
/// owner follows the part across a boolean ([`GdtSidecar::hand_on`]), exactly
/// like the part's datum reference frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnotationHome {
    /// The feature (face) persistent id the annotation is keyed by.
    pub feature: PersistentId,
    /// The solid (part) that owns the annotation.
    pub solid: SolidId,
}

/// All GD&T data attached to one model: annotations keyed by the feature's
/// persistent id, plus the datum table keyed by drawing label.
///
/// ## Identified annotations
///
/// An annotation authored through a recorded design event carries a stable id
/// (minted when it was created, recorded in the event, re-used by replay), so
/// it can be edited or deleted later and a sheet can name exactly which frame
/// it inked. `slot_ids` is kept index-aligned with `annotations` (a missing or
/// short slot list means "no id" — annotations attached with the legacy
/// [`GdtSidecar::attach`]); `homes` maps each id to its feature and owner.
/// Both are private so the alignment is an invariant of this type.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GdtSidecar {
    /// One feature may carry several annotations (e.g. a size tolerance AND a
    /// cylindricity FCF on the same bore), so the value is a list.
    annotations: HashMap<PersistentId, Vec<Annotation>>,
    /// Datum definitions keyed by label (`"A"`, `"B"`, …).
    datums: HashMap<String, Datum>,
    /// Per-feature annotation ids, index-aligned with `annotations`.
    /// `serde(default)`: sidecars serialized before ids existed load with
    /// every annotation unidentified.
    #[serde(default)]
    slot_ids: HashMap<PersistentId, Vec<Option<String>>>,
    /// Identified annotation id → its feature and owning part.
    #[serde(default)]
    homes: HashMap<String, AnnotationHome>,
}

impl GdtSidecar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach an annotation to a feature (by persistent id). Returns the new
    /// count of annotations on that feature.
    pub fn attach(&mut self, feature: PersistentId, annotation: Annotation) -> usize {
        self.push_slot(feature, None, annotation)
    }

    /// Push an annotation and its (optional) id, keeping `slot_ids` aligned.
    fn push_slot(
        &mut self,
        feature: PersistentId,
        id: Option<String>,
        annotation: Annotation,
    ) -> usize {
        let list = self.annotations.entry(feature).or_default();
        list.push(annotation);
        let len = list.len();
        let slots = self.slot_ids.entry(feature).or_default();
        slots.resize(len - 1, None);
        slots.push(id);
        len
    }

    /// Attach an IDENTIFIED annotation owned by `solid`. Returns `false` (and
    /// changes nothing) when `id` is already in use — ids are unique per
    /// model, so a second create under the same id is a different history.
    pub fn attach_identified(
        &mut self,
        id: &str,
        solid: SolidId,
        feature: PersistentId,
        annotation: Annotation,
    ) -> bool {
        if self.homes.contains_key(id) {
            return false;
        }
        self.push_slot(feature, Some(id.to_string()), annotation);
        self.homes
            .insert(id.to_string(), AnnotationHome { feature, solid });
        true
    }

    /// Position of identified annotation `id` within its feature's list.
    fn slot_of(&self, id: &str) -> Option<(AnnotationHome, usize)> {
        let home = *self.homes.get(id)?;
        let index = self
            .slot_ids
            .get(&home.feature)?
            .iter()
            .position(|s| s.as_deref() == Some(id))?;
        Some((home, index))
    }

    /// The identified annotation `id`: where it lives and what it says.
    pub fn identified(&self, id: &str) -> Option<(AnnotationHome, &Annotation)> {
        let (home, index) = self.slot_of(id)?;
        let annotation = self.annotations.get(&home.feature)?.get(index)?;
        Some((home, annotation))
    }

    /// Remove identified annotation `id`, returning where it lived and what it
    /// said. `None` (nothing changed) when no annotation carries the id.
    pub fn remove_identified(&mut self, id: &str) -> Option<(AnnotationHome, Annotation)> {
        let (home, index) = self.slot_of(id)?;
        let list = self.annotations.get_mut(&home.feature)?;
        if index >= list.len() {
            return None;
        }
        let annotation = list.remove(index);
        let now_empty = list.is_empty();
        if let Some(slots) = self.slot_ids.get_mut(&home.feature) {
            if index < slots.len() {
                slots.remove(index);
            }
        }
        if now_empty {
            self.annotations.remove(&home.feature);
            self.slot_ids.remove(&home.feature);
        }
        self.homes.remove(id);
        Some((home, annotation))
    }

    /// The id of the annotation at `index` on `feature`, if it has one.
    pub fn annotation_id(&self, feature: PersistentId, index: usize) -> Option<&str> {
        self.slot_ids.get(&feature)?.get(index)?.as_deref()
    }

    /// Every identified annotation owned by `solid`, as `(id, home)`, sorted
    /// by id so callers iterate deterministically.
    pub fn identified_of(&self, solid: SolidId) -> Vec<(String, AnnotationHome)> {
        let mut out: Vec<(String, AnnotationHome)> = self
            .homes
            .iter()
            .filter(|(_, h)| h.solid == solid)
            .map(|(id, h)| (id.clone(), *h))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Hand the annotations owned by any `consumed` solid on to `result` —
    /// the part followed through an operation that consumed its solid and
    /// made a new one (a boolean). The feature keys do not change: a face that
    /// survives the operation keeps its persistent id, and one that does not
    /// leaves its annotation dangling, still owned by the part.
    pub fn hand_on(&mut self, consumed: &[SolidId], result: SolidId) {
        for home in self.homes.values_mut() {
            if consumed.contains(&home.solid) {
                home.solid = result;
            }
        }
    }

    /// All annotations on a feature, or an empty slice if none.
    pub fn annotations(&self, feature: PersistentId) -> &[Annotation] {
        self.annotations
            .get(&feature)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Remove every annotation on a feature, returning them.
    pub fn clear_feature(&mut self, feature: PersistentId) -> Vec<Annotation> {
        if let Some(slots) = self.slot_ids.remove(&feature) {
            for id in slots.into_iter().flatten() {
                self.homes.remove(&id);
            }
        }
        self.annotations.remove(&feature).unwrap_or_default()
    }

    /// Iterate `(feature, &annotations)` over every annotated feature.
    pub fn iter(&self) -> impl Iterator<Item = (PersistentId, &[Annotation])> {
        self.annotations.iter().map(|(k, v)| (*k, v.as_slice()))
    }

    /// Number of distinct annotated features.
    pub fn annotated_feature_count(&self) -> usize {
        self.annotations.len()
    }

    /// Define (or replace) a datum by its label.
    pub fn set_datum(&mut self, datum: Datum) {
        self.datums.insert(datum.label.clone(), datum);
    }

    /// Look up a datum by label.
    pub fn datum(&self, label: &str) -> Option<&Datum> {
        self.datums.get(label)
    }

    /// Iterate the datum table.
    pub fn datums(&self) -> impl Iterator<Item = &Datum> {
        self.datums.values()
    }

    /// Empty the sidecar — paired with `BRepModel::clear_geometry`, since the
    /// annotations are bound to topology that is being discarded.
    pub fn clear(&mut self) {
        self.annotations.clear();
        self.datums.clear();
        self.slot_ids.clear();
        self.homes.clear();
    }

    /// True when nothing is attached — the default state of a fresh model.
    pub fn is_empty(&self) -> bool {
        self.annotations.is_empty() && self.datums.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gdt::model::{DatumKind, FeatureControlFrame, GeometricCharacteristic};

    #[test]
    fn attach_and_read_back() {
        let mut s = GdtSidecar::new();
        assert!(s.is_empty());
        let f = PersistentId::root(b"face-A");
        let ann = Annotation::Geometric(FeatureControlFrame::form(
            GeometricCharacteristic::Flatness,
            0.05,
        ));
        assert_eq!(s.attach(f, ann.clone()), 1);
        assert_eq!(s.annotations(f), &[ann]);
        assert_eq!(s.annotated_feature_count(), 1);
        assert!(!s.is_empty());
    }

    #[test]
    fn multiple_annotations_per_feature() {
        let mut s = GdtSidecar::new();
        let f = PersistentId::root(b"bore");
        s.attach(
            f,
            Annotation::Geometric(FeatureControlFrame::form(
                GeometricCharacteristic::Cylindricity,
                0.02,
            )),
        );
        s.attach(
            f,
            Annotation::Geometric(FeatureControlFrame::form(
                GeometricCharacteristic::Circularity,
                0.01,
            )),
        );
        assert_eq!(s.annotations(f).len(), 2);
    }

    #[test]
    fn datum_table() {
        let mut s = GdtSidecar::new();
        let feat = PersistentId::root(b"datum-feature");
        s.set_datum(Datum::new("A", DatumKind::Plane, feat));
        assert_eq!(s.datum("A").map(|d| d.kind), Some(DatumKind::Plane));
        assert!(s.datum("B").is_none());
    }

    #[test]
    fn clear_empties_everything() {
        let mut s = GdtSidecar::new();
        let f = PersistentId::root(b"x");
        s.attach(
            f,
            Annotation::Geometric(FeatureControlFrame::form(
                GeometricCharacteristic::Flatness,
                0.1,
            )),
        );
        s.set_datum(Datum::new("A", DatumKind::Axis, f));
        s.clear();
        assert!(s.is_empty());
    }

    #[test]
    fn identified_annotations_are_addressable_and_removable() {
        let mut s = GdtSidecar::new();
        let f = PersistentId::root(b"face");
        let flat = Annotation::Geometric(FeatureControlFrame::form(
            GeometricCharacteristic::Flatness,
            0.05,
        ));
        let legacy = Annotation::Geometric(FeatureControlFrame::form(
            GeometricCharacteristic::Flatness,
            0.2,
        ));
        s.attach(f, legacy.clone());
        assert!(s.attach_identified("fcf-1", 7, f, flat.clone()));
        assert!(
            !s.attach_identified("fcf-1", 7, f, flat.clone()),
            "a second create under the same id changes nothing"
        );
        assert_eq!(s.annotations(f).len(), 2);
        assert_eq!(s.annotation_id(f, 0), None, "the legacy attach has no id");
        assert_eq!(s.annotation_id(f, 1), Some("fcf-1"));
        let (home, ann) = s.identified("fcf-1").expect("identified");
        assert_eq!(
            home,
            AnnotationHome {
                feature: f,
                solid: 7
            }
        );
        assert_eq!(ann, &flat);
        s.hand_on(&[7, 8], 9);
        assert_eq!(s.identified_of(9).len(), 1, "the owner followed the part");
        assert!(s.identified_of(7).is_empty());
        let (_, removed) = s.remove_identified("fcf-1").expect("removed");
        assert_eq!(removed, flat);
        assert!(s.identified("fcf-1").is_none());
        assert_eq!(s.annotations(f), &[legacy], "the other annotation stays");
        assert!(s.remove_identified("fcf-1").is_none());
    }

    #[test]
    fn sidecar_round_trips_through_json() {
        let mut s = GdtSidecar::new();
        let f = PersistentId::root(b"face");
        s.attach(
            f,
            Annotation::Geometric(FeatureControlFrame::form(
                GeometricCharacteristic::Flatness,
                0.05,
            )),
        );
        let json = serde_json::to_string(&s).expect("serialize");
        let back: GdtSidecar = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(s, back);
    }
}
