//! `ModelSnapshot` — deep copy of a [`BRepModel`] for transactional
//! operations.
//!
//! Many kernel operations (fillet, boolean, shell, extrude) mutate
//! several entity stores before they know whether they can commit.
//! When such an operation fails partway through the model is left in
//! an inconsistent state — boundary edges from a half-built fillet,
//! orphaned faces from a broken boolean, validation-failure aftermath
//! from a degenerate spine. The historical answer was "rerun the
//! whole timeline from the previous valid checkpoint", which is
//! O(history) for a single failure.
//!
//! This module provides the load-bearing primitive for F2-δ
//! (validation lifecycle): take a snapshot before the op, drop it
//! cheaply if the op succeeds, restore it if the op fails. It is
//! also the primitive used by the push-pull preview endpoint
//! (PP-1) so the live drag can extrude+tessellate at one offset and
//! roll back before the next drag tick.
//!
//! ## What the snapshot contains
//!
//! - All nine entity stores (`Vertex` / `Curve` / `PCurve` /
//!   `Edge` / `Loop` / `Surface` / `Face` / `Shell` / `Solid`)
//!   deep-copied through each store's `pub(crate) fn deep_copy`.
//!   The deep copies own their own `DashMap` indexes and break
//!   the `Arc<RwLock<…>>` sharing inside each `Solid`'s `features`
//!   and `history` (see `Solid::deep_copy`).
//! - `sketch_planes`, `datums`, `datum_graph`, `location_cache`,
//!   `tolerance` — every field of the model that contributes to
//!   geometric identity.
//!
//! ## What the snapshot deliberately does NOT capture
//!
//! - `recorder`: a snapshot/restore round-trip is exactly the
//!   semantics of "the operation never happened". Replaying a
//!   recorder event for an operation that never happened would be
//!   wrong. Operation cancellation surfaces through the recorder
//!   contract (only successful ops emit events); the snapshot
//!   layer does not interact with the recorder.
//!
//! ## Cost
//!
//! Approximately O(N) over total topology cardinality, with a copy
//! of every `Vec`-backed buffer and every `DashMap` index rebuilt
//! by iteration. For a 10 k-vertex / 30 k-edge / 5 k-face part this
//! is well under a millisecond and well under 10 MB resident, which
//! is the budget the live-preview path requires.

use crate::math::Tolerance;
use crate::primitives::topology_builder::{BRepModel, SketchPlane};
use dashmap::DashMap;

use crate::primitives::curve::CurveStore;
use crate::primitives::datum::{DatumGraph, DatumStore, LocationDescriptorCache};
use crate::primitives::edge::EdgeStore;
use crate::primitives::face::FaceStore;
use crate::primitives::p_curve::PCurveStore;
use crate::primitives::r#loop::LoopStore;
use crate::primitives::shell::ShellStore;
use crate::primitives::solid::SolidStore;
use crate::primitives::surface::SurfaceStore;
use crate::primitives::vertex::VertexStore;

/// Owned deep copy of a [`BRepModel`].
///
/// Constructed via [`ModelSnapshot::take`]; consumed by
/// [`ModelSnapshot::restore`]. Dropping a snapshot without restoring
/// is the success path — the original model has moved on and the
/// snapshot's owned allocations are freed.
#[derive(Debug)]
pub struct ModelSnapshot {
    vertices: VertexStore,
    curves: CurveStore,
    pcurves: PCurveStore,
    edges: EdgeStore,
    loops: LoopStore,
    surfaces: SurfaceStore,
    faces: FaceStore,
    shells: ShellStore,
    solids: SolidStore,
    sketch_planes: DashMap<String, SketchPlane>,
    datums: DatumStore,
    datum_graph: DatumGraph,
    location_cache: LocationDescriptorCache,
    tolerance: Tolerance,
}

impl ModelSnapshot {
    /// Take a deep snapshot of `model`.
    ///
    /// The snapshot is a value-type owning its own allocations; the
    /// original model is borrowed immutably. Cost is O(total topology
    /// cardinality) — see module docs.
    pub fn take(model: &BRepModel) -> Self {
        let sketch_planes = DashMap::with_capacity(model.sketch_planes.len());
        for kv in model.sketch_planes.iter() {
            sketch_planes.insert(kv.key().clone(), kv.value().clone());
        }
        Self {
            vertices: model.vertices.deep_copy(),
            curves: model.curves.deep_copy(),
            pcurves: model.pcurves.deep_copy(),
            edges: model.edges.deep_copy(),
            loops: model.loops.deep_copy(),
            surfaces: model.surfaces.deep_copy(),
            faces: model.faces.deep_copy(),
            shells: model.shells.deep_copy(),
            solids: model.solids.deep_copy(),
            sketch_planes,
            datums: model.datums.deep_copy(),
            datum_graph: model.datum_graph.deep_copy(),
            location_cache: model.location_cache.deep_copy(),
            tolerance: model.tolerance,
        }
    }

    /// Restore `model` to the state captured at `take` time.
    ///
    /// Consumes the snapshot. The `recorder` field is intentionally
    /// preserved on the model — see module docs.
    pub fn restore(self, model: &mut BRepModel) {
        model.vertices = self.vertices;
        model.curves = self.curves;
        model.pcurves = self.pcurves;
        model.edges = self.edges;
        model.loops = self.loops;
        model.surfaces = self.surfaces;
        model.faces = self.faces;
        model.shells = self.shells;
        model.solids = self.solids;
        model.sketch_planes = self.sketch_planes;
        model.datums = self.datums;
        model.datum_graph = self.datum_graph;
        model.location_cache = self.location_cache;
        model.tolerance = self.tolerance;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Point3, Vector3};
    use crate::primitives::solid::HistoryNode;
    use crate::primitives::topology_builder::TopologyBuilder;
    use std::collections::HashMap;

    fn build_box_model() -> BRepModel {
        let mut model = BRepModel::new();
        {
            let mut builder = TopologyBuilder::new(&mut model);
            let _ = builder.create_box_3d(10.0, 10.0, 10.0);
        }
        model
    }

    fn topology_counts(model: &BRepModel) -> (usize, usize, usize, usize, usize, usize) {
        (
            model.vertices.len(),
            model.edges.len(),
            model.loops.len(),
            model.faces.len(),
            model.shells.len(),
            model.solids.len(),
        )
    }

    fn marker_history_node(id: u32, solid: crate::primitives::solid::SolidId) -> HistoryNode {
        HistoryNode {
            id,
            operation: "snapshot_test_marker".into(),
            inputs: Vec::new(),
            output: solid,
            parameters: HashMap::new(),
            timestamp: std::time::SystemTime::now(),
        }
    }

    #[test]
    fn snapshot_roundtrip_on_empty_model() {
        let mut model = BRepModel::new();
        let snap = ModelSnapshot::take(&model);
        // Mutate something trivial: add a sketch plane.
        let plane = SketchPlane::new(
            "scratch".into(),
            Point3::ORIGIN,
            Vector3::new(0.0, 0.0, 1.0),
            10.0,
        );
        model.sketch_planes.insert("scratch".into(), plane);
        assert_eq!(model.sketch_planes.len(), 1);
        snap.restore(&mut model);
        assert_eq!(model.sketch_planes.len(), 0);
    }

    #[test]
    fn snapshot_roundtrip_preserves_box_topology() {
        let mut model = build_box_model();
        let before = topology_counts(&model);
        let snap = ModelSnapshot::take(&model);

        // Mutate: add a second box. Counts will not match `before`.
        {
            let mut builder = TopologyBuilder::new(&mut model);
            let _ = builder.create_box_3d(1.0, 1.0, 1.0);
        }
        let after_mutation = topology_counts(&model);
        assert_ne!(before, after_mutation, "second box must change counts");

        snap.restore(&mut model);
        let after_restore = topology_counts(&model);
        assert_eq!(
            before, after_restore,
            "restore must reset topology counts to snapshot time"
        );
    }

    #[test]
    fn snapshot_breaks_solid_arc_sharing() {
        // Solid carries `features` and `history` as `Arc<RwLock<…>>`;
        // a naive derived `Clone` would share the inner allocation
        // and a later mutation through the original would leak into
        // the snapshot. `Solid::deep_copy` unshares — verify.
        let mut model = build_box_model();
        let solid_id = model
            .solids
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("box build produced exactly one solid");
        let history_before = model
            .solids
            .get(solid_id)
            .expect("solid id valid by construction")
            .get_history()
            .len();
        let snap = ModelSnapshot::take(&model);

        // Push a history node on the live solid after the snapshot.
        {
            let solid = model
                .solids
                .get_mut(solid_id)
                .expect("solid id valid by construction");
            solid.add_history(marker_history_node(99_999, solid_id));
            assert_eq!(
                solid.get_history().len(),
                history_before + 1,
                "mutation must extend history"
            );
        }

        snap.restore(&mut model);
        let solid_after = model
            .solids
            .get(solid_id)
            .expect("solid id stable across snapshot/restore");
        assert_eq!(
            solid_after.get_history().len(),
            history_before,
            "restored solid must not see post-snapshot history mutation"
        );

        // Make sure the box is still valid by spot-checking basic counts.
        assert!(model.vertices.len() >= 8);
        assert!(model.faces.len() >= 6);
    }

    #[test]
    fn snapshot_roundtrip_preserves_and_rolls_back_pcurves() {
        use crate::math::Point2;
        use crate::primitives::curve::ParameterRange;
        use crate::primitives::p_curve::{PCurve, PCurve2dKind};

        let mut model = BRepModel::new();
        let seed_id = model
            .pcurves
            .add(PCurve::new(
                7,
                PCurve2dKind::Line {
                    start: Point2::ZERO,
                    end: Point2::new(1.0, 0.0),
                },
                ParameterRange::unit(),
                1e-6,
            ))
            .expect("seed pcurve");
        assert_eq!(model.pcurves.len(), 1);

        let snap = ModelSnapshot::take(&model);

        // Mutate the live store after the snapshot.
        let _added = model
            .pcurves
            .add(PCurve::new(
                11,
                PCurve2dKind::Line {
                    start: Point2::ZERO,
                    end: Point2::new(0.0, 1.0),
                },
                ParameterRange::unit(),
                1e-6,
            ))
            .expect("post-snapshot pcurve");
        assert_eq!(model.pcurves.len(), 2);

        snap.restore(&mut model);
        assert_eq!(model.pcurves.len(), 1, "restore must roll back pcurve adds");
        let kept = model.pcurves.get(seed_id).expect("seed pcurve survives");
        assert_eq!(kept.face, 7);
    }
}
