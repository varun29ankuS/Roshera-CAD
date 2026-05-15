//! Spatial indexing for kernel broad-phase queries.
//!
//! # Why
//!
//! The kernel's hot paths — boolean face-pair pruning, edge proximity
//! checks, datum-anchor resolution, agent-initiated "show me every
//! face that …" queries — all reduce to the same primitive:
//!
//! > Given a query AABB, return every entity whose AABB intersects it.
//!
//! A naive O(|A| × |B|) loop is acceptable for two boxes (36 face pairs)
//! and lethal for two filleted assemblies (10k+ face pairs).
//!
//! # Design contract
//!
//! All consumers depend on the [`SpatialIndex`] trait, never on a
//! specific implementation. This is deliberate: the right index for
//! 2026 (a CPU R*-tree via [`rstar`]) is not necessarily the right
//! index for 2030 (a GPU LBVH for million-face raycast, or a learned
//! index for static assemblies). Behind the trait, the impl is
//! swappable; in front of it, call sites never change.
//!
//! # Current impl
//!
//! [`RstarIndex`] wraps the [`rstar`] crate's `RTree`. R*-tree is
//! chosen over Qbvh / BVH alternatives because:
//!
//! - The kernel edits topology continuously (fillet adds faces,
//!   boolean splits them, draft mutates them). Dynamic insert/remove
//!   in `O(log n)` matters more than the marginally better query
//!   constants of a static BVH.
//! - `rstar` is pure Rust with no `nalgebra` transitive dep, keeping
//!   our coordinate type system single-rooted at `math::Point3` /
//!   `math::Vector3`.
//! - The R*-tree algorithm is 35 years old, has not been displaced
//!   for dynamic 3D AABB workloads, and rstar itself is a foundational
//!   crate in the Rust geospatial ecosystem.
//!
//! # Out of scope (this slice)
//!
//! - Ray-AABB traversal queries. The mouse-over pipeline routes
//!   through the frontend Three.js raycaster (Task #47); server-side
//!   ray queries are deferred until a kernel consumer needs them.
//! - kNN / nearest-neighbour queries. Same reason: no current consumer.
//! - Wiring into [`crate::operations::boolean::compute_face_intersections`].
//!   That is the next slice and ships with its own benchmark + regression
//!   guard.

pub mod rstar_index;

use crate::math::bbox::BBox;

pub use rstar_index::RstarIndex;

/// Spatial index over entities keyed by `Id`, indexed by axis-aligned
/// bounding boxes.
///
/// # Invariants
///
/// - `insert(id, bbox)` followed by `remove(id)` returns `true` and
///   leaves the index in the same state as before the insert.
/// - `query_aabb(q)` returns every `id` whose stored `BBox`
///   intersects `q` (touching counts as intersecting — this matches
///   [`BBox::intersects`]).
/// - `bulk_load` is functionally equivalent to repeated `insert`
///   calls but typically produces a better-balanced index. Prefer
///   it for initial population.
/// - `Id` equality is the identity. Two entries with the same `Id`
///   are the same entry; inserting an `Id` that already exists is
///   implementation-defined (do not rely on it — remove first).
pub trait SpatialIndex<Id: Copy + Eq>: Send + Sync {
    /// Construct an empty index.
    fn new() -> Self
    where
        Self: Sized;

    /// Construct an index from an iterator of `(id, bbox)` pairs.
    ///
    /// Prefer this over repeated `insert` when initial population
    /// size is known — the underlying tree builder can produce a
    /// better-balanced structure given the full input upfront.
    fn bulk_load(items: impl IntoIterator<Item = (Id, BBox)>) -> Self
    where
        Self: Sized;

    /// Insert a single entry.
    fn insert(&mut self, id: Id, bbox: BBox);

    /// Remove the entry with the given `Id`. Returns `true` if an
    /// entry was actually removed, `false` if no entry with that
    /// `Id` existed.
    fn remove(&mut self, id: Id) -> bool;

    /// Return every `Id` whose stored `BBox` intersects `query`.
    ///
    /// Order of results is implementation-defined; do not rely on it.
    fn query_aabb(&self, query: BBox) -> Vec<Id>;

    /// Number of entries in the index.
    fn len(&self) -> usize;

    /// Whether the index is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Point3;

    /// Helper: a tiny axis-aligned box centred on `c` with half-extent `h`.
    fn bbox_around(c: [f64; 3], h: f64) -> BBox {
        BBox::new_validated(
            Point3::new(c[0] - h, c[1] - h, c[2] - h),
            Point3::new(c[0] + h, c[1] + h, c[2] + h),
        )
    }

    #[test]
    fn empty_index_reports_empty() {
        let idx: RstarIndex<u32> = RstarIndex::new();
        assert!(idx.is_empty());
        assert_eq!(idx.len(), 0);
        assert!(idx.query_aabb(bbox_around([0.0, 0.0, 0.0], 1.0)).is_empty());
    }

    #[test]
    fn insert_then_query_finds_the_entry() {
        let mut idx: RstarIndex<u32> = RstarIndex::new();
        idx.insert(42, bbox_around([5.0, 5.0, 5.0], 1.0));

        let hits = idx.query_aabb(bbox_around([5.5, 5.5, 5.5], 1.0));
        assert_eq!(hits, vec![42]);
    }

    #[test]
    fn query_outside_returns_empty() {
        let mut idx: RstarIndex<u32> = RstarIndex::new();
        idx.insert(1, bbox_around([0.0, 0.0, 0.0], 1.0));
        idx.insert(2, bbox_around([100.0, 100.0, 100.0], 1.0));

        let hits = idx.query_aabb(bbox_around([50.0, 50.0, 50.0], 1.0));
        assert!(hits.is_empty());
    }

    #[test]
    fn query_overlapping_returns_all_overlapping_entries() {
        let mut idx: RstarIndex<u32> = RstarIndex::new();
        // Three boxes overlapping the query region, one far away.
        idx.insert(10, bbox_around([0.0, 0.0, 0.0], 2.0));
        idx.insert(11, bbox_around([1.0, 0.0, 0.0], 2.0));
        idx.insert(12, bbox_around([-1.0, 0.0, 0.0], 2.0));
        idx.insert(99, bbox_around([1000.0, 1000.0, 1000.0], 1.0));

        let mut hits = idx.query_aabb(bbox_around([0.0, 0.0, 0.0], 0.5));
        hits.sort_unstable();
        assert_eq!(hits, vec![10, 11, 12]);
    }

    #[test]
    fn remove_existing_returns_true_and_clears() {
        let mut idx: RstarIndex<u32> = RstarIndex::new();
        idx.insert(7, bbox_around([0.0, 0.0, 0.0], 1.0));
        assert_eq!(idx.len(), 1);

        assert!(idx.remove(7));
        assert!(idx.is_empty());
        assert!(idx.query_aabb(bbox_around([0.0, 0.0, 0.0], 5.0)).is_empty());
    }

    #[test]
    fn remove_missing_returns_false_and_is_noop() {
        let mut idx: RstarIndex<u32> = RstarIndex::new();
        idx.insert(7, bbox_around([0.0, 0.0, 0.0], 1.0));

        assert!(!idx.remove(999));
        assert_eq!(idx.len(), 1);
    }

    #[test]
    fn bulk_load_yields_same_query_results_as_serial_inserts() {
        let items: Vec<(u32, BBox)> = (0..32)
            .map(|i| {
                let f = f64::from(i);
                (i, bbox_around([f, 0.0, 0.0], 0.4))
            })
            .collect();

        let bulk: RstarIndex<u32> = RstarIndex::bulk_load(items.clone());
        let mut serial: RstarIndex<u32> = RstarIndex::new();
        for (id, bb) in items {
            serial.insert(id, bb);
        }

        let q = bbox_around([10.0, 0.0, 0.0], 1.5);
        let mut hits_bulk = bulk.query_aabb(q);
        let mut hits_serial = serial.query_aabb(q);
        hits_bulk.sort_unstable();
        hits_serial.sort_unstable();
        assert_eq!(hits_bulk, hits_serial);
    }

    #[test]
    fn touching_aabbs_count_as_intersecting() {
        // Mirrors BBox::intersects semantics — touching faces yields true.
        let mut idx: RstarIndex<u32> = RstarIndex::new();
        idx.insert(
            1,
            BBox::new_validated(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 1.0, 1.0)),
        );

        // Query exactly touching the +x face of the stored box.
        let q = BBox::new_validated(Point3::new(1.0, 0.0, 0.0), Point3::new(2.0, 1.0, 1.0));
        let hits = idx.query_aabb(q);
        assert_eq!(hits, vec![1]);
    }

    #[test]
    fn insert_remove_insert_roundtrip_preserves_state() {
        let mut idx: RstarIndex<u32> = RstarIndex::new();
        let bb = bbox_around([3.0, 4.0, 5.0], 0.5);

        idx.insert(1, bb);
        assert_eq!(idx.query_aabb(bb), vec![1]);

        idx.remove(1);
        assert!(idx.is_empty());

        idx.insert(1, bb);
        assert_eq!(idx.query_aabb(bb), vec![1]);
    }

    #[test]
    fn large_population_query_returns_correct_subset() {
        // Sanity-check the index on a non-trivial population: 1000
        // unit boxes on a 10×10×10 lattice. Query a small region;
        // verify the hit set matches the brute-force ground truth.
        let mut idx: RstarIndex<u32> = RstarIndex::new();
        let mut ground_truth: Vec<(u32, BBox)> = Vec::new();
        let mut id: u32 = 0;
        for x in 0..10 {
            for y in 0..10 {
                for z in 0..10 {
                    let bb = bbox_around([f64::from(x), f64::from(y), f64::from(z)], 0.3);
                    idx.insert(id, bb);
                    ground_truth.push((id, bb));
                    id += 1;
                }
            }
        }

        let q = bbox_around([5.0, 5.0, 5.0], 1.0);
        let mut expected: Vec<u32> = ground_truth
            .iter()
            .filter(|(_, bb)| bb.intersects(&q))
            .map(|(i, _)| *i)
            .collect();
        let mut got = idx.query_aabb(q);
        expected.sort_unstable();
        got.sort_unstable();
        assert_eq!(got, expected);
    }
}
