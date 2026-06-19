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

/// All GD&T data attached to one model: annotations keyed by the feature's
/// persistent id, plus the datum table keyed by drawing label.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GdtSidecar {
    /// One feature may carry several annotations (e.g. a size tolerance AND a
    /// cylindricity FCF on the same bore), so the value is a list.
    annotations: HashMap<PersistentId, Vec<Annotation>>,
    /// Datum definitions keyed by label (`"A"`, `"B"`, …).
    datums: HashMap<String, Datum>,
}

impl GdtSidecar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach an annotation to a feature (by persistent id). Returns the new
    /// count of annotations on that feature.
    pub fn attach(&mut self, feature: PersistentId, annotation: Annotation) -> usize {
        let list = self.annotations.entry(feature).or_default();
        list.push(annotation);
        list.len()
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
