//! R*-tree-backed implementation of [`SpatialIndex`].
//!
//! Thin adapter over the [`rstar`] crate: wraps each `(Id, BBox)`
//! pair in an [`Entry`] struct that implements [`rstar::RTreeObject`],
//! and exposes the trait surface required by [`super::SpatialIndex`].
//!
//! Conversion between `math::BBox` and `rstar::AABB<[f64; 3]>`
//! happens at the trait boundary; the rest of the kernel never sees
//! rstar's AABB type.

use super::SpatialIndex;
use crate::math::bbox::BBox;
use rstar::{RTree, RTreeObject, AABB as RstarAabb};
use std::collections::HashMap;
use std::hash::Hash;

/// A single entry in an [`RstarIndex`]. Holds the caller's `Id` and
/// the `BBox` that the index is keyed on.
///
/// `PartialEq` compares by `Id` only — same id = same logical entry.
/// This is what makes `RTree::remove(&entry)` work given only the id.
#[derive(Debug, Clone, Copy)]
struct Entry<Id: Copy + Eq> {
    id: Id,
    bbox: BBox,
}

impl<Id: Copy + Eq> PartialEq for Entry<Id> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<Id: Copy + Eq + Send + Sync + 'static> RTreeObject for Entry<Id> {
    type Envelope = RstarAabb<[f64; 3]>;

    fn envelope(&self) -> Self::Envelope {
        RstarAabb::from_corners(
            [self.bbox.min.x, self.bbox.min.y, self.bbox.min.z],
            [self.bbox.max.x, self.bbox.max.y, self.bbox.max.z],
        )
    }
}

/// R*-tree-backed spatial index. See [`super::SpatialIndex`] for the
/// contract this implementation upholds.
///
/// # Internal representation
///
/// Two data structures cooperate:
///
/// - `tree: RTree<Entry<Id>>` — the rstar R*-tree, keyed on AABB
///   envelopes, used for `query_aabb` and bulk-load.
/// - `id_to_bbox: HashMap<Id, BBox>` — auxiliary id→bbox map.
///
/// The map exists because rstar's `RTree::remove(&T)` uses
/// `T::envelope()` to navigate to the leaf containing the target.
/// To remove by id alone, we must reconstruct the original envelope,
/// which means remembering it. The map is the simplest correct
/// answer; memory overhead is `~64 bytes/entry`, dwarfed by the
/// tree's own per-entry footprint.
#[derive(Debug)]
pub struct RstarIndex<Id: Copy + Eq + Hash + Send + Sync + 'static> {
    tree: RTree<Entry<Id>>,
    id_to_bbox: HashMap<Id, BBox>,
}

impl<Id: Copy + Eq + Hash + Send + Sync + 'static> RstarIndex<Id> {
    /// Convert a `math::BBox` to the rstar AABB used internally.
    fn to_rstar_aabb(bbox: &BBox) -> RstarAabb<[f64; 3]> {
        RstarAabb::from_corners(
            [bbox.min.x, bbox.min.y, bbox.min.z],
            [bbox.max.x, bbox.max.y, bbox.max.z],
        )
    }
}

impl<Id: Copy + Eq + Hash + Send + Sync + 'static> SpatialIndex<Id> for RstarIndex<Id> {
    fn new() -> Self {
        Self {
            tree: RTree::new(),
            id_to_bbox: HashMap::new(),
        }
    }

    fn bulk_load(items: impl IntoIterator<Item = (Id, BBox)>) -> Self {
        let collected: Vec<(Id, BBox)> = items.into_iter().collect();
        let mut id_to_bbox = HashMap::with_capacity(collected.len());
        for &(id, bbox) in &collected {
            id_to_bbox.insert(id, bbox);
        }
        let entries: Vec<Entry<Id>> = collected
            .into_iter()
            .map(|(id, bbox)| Entry { id, bbox })
            .collect();
        Self {
            tree: RTree::bulk_load(entries),
            id_to_bbox,
        }
    }

    fn insert(&mut self, id: Id, bbox: BBox) {
        self.tree.insert(Entry { id, bbox });
        self.id_to_bbox.insert(id, bbox);
    }

    fn remove(&mut self, id: Id) -> bool {
        // Look up the original bbox so rstar's tree traversal lands
        // in the right leaf. Without the real envelope, `tree.remove`
        // would search the wrong subtree (or none) and return None
        // even though the entry exists.
        let bbox = match self.id_to_bbox.remove(&id) {
            Some(b) => b,
            None => return false,
        };
        let entry = Entry { id, bbox };
        self.tree.remove(&entry).is_some()
    }

    fn query_aabb(&self, query: BBox) -> Vec<Id> {
        let env = Self::to_rstar_aabb(&query);
        self.tree
            .locate_in_envelope_intersecting(&env)
            .map(|e| e.id)
            .collect()
    }

    fn len(&self) -> usize {
        self.tree.size()
    }
}
