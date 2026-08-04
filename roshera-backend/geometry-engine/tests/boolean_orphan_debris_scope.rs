// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Certificate-integrity: a broken boolean's fallout must not poison every
//! subsequent part's per-part certificate.
//!
//! ## The defect (reproduced live 2026-07-14, root-caused here)
//!
//! ORIGINALLY: a cross-drilled manifold hit the OPEN cyl-cyl saddle bug (#35):
//! the second Difference COMPLETED but the result solid was UNSOUND (open edges).
//! Its boundary-edge / connectivity errors were stamped `solid_id: None` by the
//! model-wide `check_topology_gaps` pass (it records only the `face_id`). A
//! subsequently created PLAIN CYLINDER PRIMITIVE (itself watertight / oriented
//! / manifold) then certified `brep_valid = false` with ConnectivityError
//! "Boundary edge N detected" entries located at the OTHER solid's faces —
//! because `validate_solid_scoped` kept every `solid_id: None` error for every
//! part (`None => true`). One broken boolean poisoned EVERY part's certificate.
//!
//! UPDATE (#35 Slice-1, 2026-07-15): the analytic equal-radius saddle is now
//! SOUND, so it no longer produces the `solid_id: None` errors that seeded the
//! mis-attribution. The attribution fix itself is unchanged and still guarded —
//! by `model_debris_counted_isolated_and_swept_by_delete`, which synthesizes a
//! genuine orphan face directly (no dependence on the saddle bug). The
//! saddle-based tests below now run against the sound saddle as regression
//! guards (no orphans, per-part soundness, and the saddle staying sound).
//!
//! Instrumentation (see the campaign report) confirmed the faithful analytic
//! saddle leaves NO literal orphan topology — the boolean's Slice-2 prune +
//! full snapshot rollback already close that escape. The poisoning is pure
//! MIS-ATTRIBUTION: the errors belong to the live-but-unsound RESULT solid,
//! not to orphans. The fix attributes each `solid_id: None` error to the solid
//! whose live topology carries the located face; genuinely-orphan topology (a
//! face owned by no solid) is accounted once at model scope
//! (`model_debris_orphan_faces`) instead of appearing in every part's verdict.
//!
//! ## What GREEN looks like
//!
//! 1. No debris escape: after the saddle boolean, no face is live in the store
//!    but owned by no solid.
//! 2. Per-part honesty: the independent primitive's own certificate is SOUND
//!    (`brep_valid = true`), reflecting ITS OWN topology, not the alien errors.
//! 3. The saddle result solid certifies SOUND (`brep_valid=true`) post-#35
//!    Slice-1 — a regression guard that the saddle stays closed.
//! 4. Genuine orphan debris is counted at model scope, isolated from clean
//!    parts, and swept by `delete_part`.

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::extrude::{extrude_polygon_regions, PolygonRegion};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid, got {o:?}"),
    }
}

/// Every face live in `model.faces` but reachable from NO solid.
fn orphan_faces(model: &BRepModel) -> Vec<u32> {
    let mut owned = std::collections::HashSet::new();
    for (_sid, solid) in model.solids.iter() {
        let mut shells = vec![solid.outer_shell];
        shells.extend_from_slice(&solid.inner_shells);
        for sh in shells {
            if let Some(shell) = model.shells.get(sh) {
                for &fid in &shell.faces {
                    owned.insert(fid);
                }
            }
        }
    }
    model
        .faces
        .iter()
        .map(|(fid, _)| fid)
        .filter(|fid| !owned.contains(fid))
        .collect()
}

/// Build the cross-drilled manifold at the #35 saddle: 80×40×40 block, analytic
/// vertical bore, then an analytic horizontal bore crossing the first at the
/// equal-radius perpendicular saddle. Post-Slice-1 the second Difference now
/// returns an `Ok` SOUND solid (it used to be `Ok` but UNSOUND with open edges);
/// these tests use it as a clean cross-drilled fixture. Returns (model, result).
fn build_saddle_manifold() -> (BRepModel, SolidId) {
    let tol = Tolerance::default();
    let mut m = BRepModel::new();
    let block = extrude_polygon_regions(
        &mut m,
        Point3::ORIGIN,
        Vector3::X,
        Vector3::Y,
        &[PolygonRegion {
            outer: vec![[0.0, 0.0], [80.0, 0.0], [80.0, 40.0], [0.0, 40.0]],
            holes: vec![],
        }],
        40.0,
        None,
        tol,
    )
    .expect("block");
    let vbore = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(40.0, 20.0, -5.0), Vector3::Z, 10.0, 50.0)
        .expect("vbore"));
    let b1 = boolean_operation(
        &mut m,
        block,
        vbore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("vbore diff");
    let hbore = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(-5.0, 20.0, 20.0), Vector3::X, 10.0, 90.0)
        .expect("hbore"));
    let res = boolean_operation(
        &mut m,
        b1,
        hbore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("hbore diff (saddle) must still return a result solid");
    (m, res)
}

#[test]
fn broken_boolean_leaves_no_orphan_debris() {
    let (m, _res) = build_saddle_manifold();
    let orphans = orphan_faces(&m);
    assert!(
        orphans.is_empty(),
        "the saddle boolean left {} orphan face(s) live in the store but owned by \
         no solid: {:?} — debris escaped the boolean",
        orphans.len(),
        orphans,
    );
}

#[test]
fn independent_primitive_certifies_sound_after_broken_boolean() {
    let (mut m, _res) = build_saddle_manifold();

    // A brand-new PLAIN CYLINDER primitive: its own 3 faces, watertight,
    // oriented, manifold — sound on its own topology.
    let cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(300.0, 0.0, 0.0), Vector3::Z, 5.0, 20.0)
        .expect("independent cylinder primitive"));

    let cert = m.certify_solid(cyl);
    assert!(
        cert.brep_valid,
        "the independent primitive's OWN certificate must be brep_valid; instead it \
         certified UNSOUND with alien errors from the broken manifold's debris: {:?}",
        cert.errors,
    );
    assert!(
        cert.is_sound(),
        "the independent primitive must be SOUND on its own topology; cert: {:?}",
        cert.errors,
    );
}

/// #35 Slice-1 REGRESSION GUARD (updated 2026-07-15): the analytic equal-radius
/// perpendicular saddle that this file was built around is NO LONGER unsound —
/// Slice 1 (shared crossing vertices + saddle-lateral splitter + saddle-annulus
/// tessellator) closed it. The result solid now certifies `brep_valid = true` /
/// `is_sound()`. This asserts that soundness so a regression back to the open-edge
/// saddle is caught here. The mis-attribution isolation invariant this file
/// protects (one part's real defect never leaks into another part's certificate)
/// no longer relies on the saddle being unsound — it is carried by
/// `model_debris_counted_isolated_and_swept_by_delete`, which synthesizes a
/// genuine orphan face directly (no dependency on any kernel bug).
#[test]
fn saddle_solid_certifies_sound_after_35_slice1_fix() {
    let (mut m, res) = build_saddle_manifold();
    let cert = m.certify_solid(res);
    assert!(
        cert.brep_valid && cert.is_sound(),
        "#35 Slice-1: the analytic equal-radius saddle result must certify SOUND \
         (brep_valid + is_sound); a regression to the open-edge saddle would trip \
         this. brep_valid={}, errors={:?}",
        cert.brep_valid,
        cert.errors,
    );
}

/// Model-level debris accounting + `delete_part` sweep. A face live in the
/// store but owned by no solid is orphan debris. It must:
///   * be counted at model scope (`model_debris_orphan_faces` > 0) — honesty;
///   * NOT poison an independent part's own certificate (it stays sound);
///   * be swept by `delete_part` (fix #3), zeroing the debris count.
///
/// The orphan is synthesized by removing a face from a box's shell (leaving the
/// face live in `model.faces` but owned by no shell) — a faithful stand-in for
/// the unattributed topology a broken op can leave, without depending on a
/// specific kernel bug to produce it.
#[test]
fn model_debris_counted_isolated_and_swept_by_delete() {
    use geometry_engine::operations::delete::delete_solid;
    use geometry_engine::primitives::validation::count_orphan_faces;

    let mut m = BRepModel::new();
    let boxs = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(10.0, 10.0, 10.0)
        .expect("box"));
    // An independent, clean cylinder primitive — the "part under test".
    let cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(300.0, 0.0, 0.0), Vector3::Z, 5.0, 20.0)
        .expect("cyl"));

    // Orphan one of the box's faces: remove it from the box's outer shell.
    let orphan_face = {
        let solid = m.solids.get(boxs).expect("box solid");
        let shell_id = solid.outer_shell;
        let fid = *m
            .shells
            .get(shell_id)
            .expect("box shell")
            .faces
            .first()
            .expect("box shell has faces");
        m.shells
            .get_mut(shell_id)
            .expect("box shell mut")
            .remove_face(fid);
        fid
    };
    assert!(
        orphan_faces(&m).contains(&orphan_face),
        "test setup: the removed face must now be an orphan",
    );

    // Honesty: model-level debris is counted.
    assert!(
        count_orphan_faces(&m) >= 1,
        "orphan face must be counted at model scope",
    );

    // Isolation: the independent cylinder's OWN certificate is sound, and it
    // reports the debris honestly via the model-level field (nonzero) without
    // letting it affect its own soundness.
    let cert = m.certify_solid(cyl);
    assert!(
        cert.is_sound(),
        "the independent primitive must stay SOUND despite model debris; cert errors: {:?}",
        cert.errors,
    );
    assert!(
        cert.model_debris_orphan_faces >= 1,
        "the certificate must surface the model-level orphan-debris count (honesty)",
    );

    // Sweep: deleting the debris-producing box prunes the orphan.
    let _ = delete_solid(&mut m, boxs, true).expect("delete box");
    assert!(
        orphan_faces(&m).is_empty(),
        "delete_part must sweep unattributed orphan debris; remaining: {:?}",
        orphan_faces(&m),
    );
    let cert2 = m.certify_solid(cyl);
    assert_eq!(
        cert2.model_debris_orphan_faces, 0,
        "after delete_part sweep, the model-debris count must be zero",
    );
    assert!(cert2.is_sound(), "cylinder still sound after the sweep");
}

/// A capture recorder mirroring `api-server/src/main.rs::delete_solid_core`'s
/// production recording site, so this test can prove the swept debris
/// actually reaches the `deleted` wire channel end-to-end, not just the
/// in-process `Vec` `delete_solid` returns.
#[derive(Debug, Default)]
struct CaptureRecorder {
    events: std::sync::Mutex<Vec<geometry_engine::operations::recorder::RecordedOperation>>,
}

impl geometry_engine::operations::recorder::OperationRecorder for CaptureRecorder {
    fn record(
        &self,
        operation: geometry_engine::operations::recorder::RecordedOperation,
    ) -> Result<(), geometry_engine::operations::recorder::RecorderError> {
        self.events
            .lock()
            .expect("CaptureRecorder mutex poisoned")
            .push(operation);
        Ok(())
    }
}

/// THE DEFECT under test: `delete_solid` unconditionally calls
/// `prune_boolean_orphan_topology` at its tail, sweeping orphaned topology
/// MODEL-WIDE, then discards what it swept — no record at all. This
/// reproduces the boolean-husk shape WITHOUT running a boolean: build two
/// independent boxes, then retire the second box's `Solid` record only
/// (exactly what `boolean.rs` does to its operands), leaving its whole
/// shell/face/loop/edge/vertex chain reachable from nothing — genuine
/// pre-existing debris that predates the `delete_solid` call under test.
///
/// A previous investigation hit this exact shape while writing a different
/// test: building a box plus orphans, deleting the box, and finding the
/// prune had already eaten the orphans DURING `delete_solid`, before the
/// code under test ran — that is this defect made visible.
#[test]
fn delete_solid_reports_the_orphan_debris_it_sweeps() {
    use geometry_engine::operations::delete::{delete_solid, EntityType};
    use geometry_engine::operations::recorder::{
        entity_ref, ENTITY_EDGE, ENTITY_FACE, ENTITY_LOOP, ENTITY_SOLID, ENTITY_VERTEX,
    };

    let mut m = BRepModel::new();
    let keep = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(20.0, 14.0, 10.0)
        .expect("keep box"));
    let debris_solid = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(1.0, 1.0, 1.0)
        .expect("debris box"));

    // Snapshot every entity id owned ONLY by the debris box, before knocking
    // out its `Solid` record, so the assertions below do not depend on the
    // very sweep mechanism under test to discover what should be swept.
    let (shell_id, inner_shells) = {
        let solid = m.solids.get(debris_solid).expect("debris solid");
        (solid.outer_shell, solid.inner_shells.clone())
    };
    let mut debris: Vec<(EntityType, u32)> = vec![(EntityType::Shell, shell_id)];
    debris.extend(inner_shells.iter().map(|s| (EntityType::Shell, *s)));
    let mut face_ids = Vec::new();
    if let Some(shell) = m.shells.get(shell_id) {
        face_ids.extend(shell.faces.iter().copied());
    }
    let mut loop_ids = Vec::new();
    for face_id in &face_ids {
        debris.push((EntityType::Face, *face_id));
        if let Some(face) = m.faces.get(*face_id) {
            loop_ids.push(face.outer_loop);
            loop_ids.extend(face.inner_loops.iter().copied());
        }
    }
    let mut edge_ids = Vec::new();
    for loop_id in &loop_ids {
        debris.push((EntityType::Loop, *loop_id));
        if let Some(lp) = m.loops.get(*loop_id) {
            edge_ids.extend(lp.edges.iter().copied());
        }
    }
    edge_ids.sort_unstable();
    edge_ids.dedup();
    let mut vertex_ids = Vec::new();
    for edge_id in &edge_ids {
        debris.push((EntityType::Edge, *edge_id));
        if let Some(edge) = m.edges.get(*edge_id) {
            vertex_ids.push(edge.start_vertex);
            vertex_ids.push(edge.end_vertex);
        }
    }
    vertex_ids.sort_unstable();
    vertex_ids.dedup();
    for vertex_id in &vertex_ids {
        debris.push((EntityType::Vertex, *vertex_id));
    }
    assert!(
        !debris.is_empty(),
        "test setup: the debris box must have topology to strand"
    );

    // Retire ONLY the `Solid` record (mirrors `boolean.rs`'s operand
    // retirement), leaving the shell/face/loop/edge/vertex chain reachable
    // from nothing.
    m.solids.remove(debris_solid);

    let removed = delete_solid(&mut m, keep, true).expect("delete_solid succeeds");

    // The sweep DID happen (already true before the fix): the debris is
    // gone from the model. Every store here (`Vec`-backed, tombstoned on
    // `remove` to preserve indices) returns the tombstone verbatim from
    // `get()` rather than `None` — only `iter()` filters it out (checking
    // `id != INVALID_*_ID`), matching what `is_*_used` itself relies on. So
    // liveness here is checked via `iter()`, not `get(..).is_some()`.
    for (entity_type, id) in &debris {
        let still_present = match entity_type {
            EntityType::Shell => m.shells.iter().any(|(sid, _)| sid == *id),
            EntityType::Face => m.faces.iter().any(|(fid, _)| fid == *id),
            EntityType::Loop => m.loops.iter().any(|(lid, _)| lid == *id),
            EntityType::Edge => m.edges.iter().any(|(eid, _)| eid == *id),
            EntityType::Vertex => m.vertices.iter().any(|(vid, _)| vid == *id),
            EntityType::Solid => false,
        };
        assert!(
            !still_present,
            "test setup: debris {entity_type:?}:{id} should have been swept by the prune"
        );
    }

    // THE ASSERTION THAT MUST FAIL BEFORE THE FIX: `delete_solid`'s return
    // value must name every entity it caused to cease existing, including
    // the unrelated debris the unconditional model-wide prune ate along the
    // way. Before the fix `removed` contains only `keep`'s own primary +
    // cascade entries — the debris vanished with no record at all.
    for (entity_type, id) in &debris {
        assert!(
            removed.contains(&(*entity_type, *id)),
            "delete_solid's return must report swept debris {entity_type:?}:{id} — \
             it does not: {removed:?}"
        );
    }

    // End-to-end: simulate the production recording site
    // (`delete_solid_core`) and prove the swept debris reaches the
    // `deleted` wire channel, not just the in-process return value.
    let recorder = std::sync::Arc::new(CaptureRecorder::default());
    let deleted_refs: Vec<String> = removed
        .iter()
        .filter_map(|(kind, id)| match kind {
            EntityType::Solid => Some(entity_ref(ENTITY_SOLID, *id as u64)),
            EntityType::Face => Some(entity_ref(ENTITY_FACE, *id as u64)),
            EntityType::Loop => Some(entity_ref(ENTITY_LOOP, *id as u64)),
            EntityType::Edge => Some(entity_ref(ENTITY_EDGE, *id as u64)),
            EntityType::Vertex => Some(entity_ref(ENTITY_VERTEX, *id as u64)),
            EntityType::Shell => None,
        })
        .collect();
    geometry_engine::operations::recorder::OperationRecorder::record(
        recorder.as_ref(),
        geometry_engine::operations::recorder::RecordedOperation::new("delete_solid")
            .with_deleted_refs(deleted_refs),
    )
    .expect("capture record");
    let captured = recorder.events.lock().expect("mutex").clone();
    assert_eq!(captured.len(), 1);
    for (entity_type, id) in debris.iter().filter(|(t, _)| *t != EntityType::Shell) {
        let kind = match entity_type {
            EntityType::Face => ENTITY_FACE,
            EntityType::Loop => ENTITY_LOOP,
            EntityType::Edge => ENTITY_EDGE,
            EntityType::Vertex => ENTITY_VERTEX,
            EntityType::Solid => ENTITY_SOLID,
            EntityType::Shell => unreachable!("filtered above"),
        };
        let wire_ref = entity_ref(kind, *id as u64);
        assert!(
            captured[0].deleted.contains(&wire_ref),
            "recorded op must declare swept debris {wire_ref} on the deleted channel: {:?}",
            captured[0].deleted
        );
    }
}
