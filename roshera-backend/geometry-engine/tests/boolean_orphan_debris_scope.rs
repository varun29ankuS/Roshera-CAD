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
