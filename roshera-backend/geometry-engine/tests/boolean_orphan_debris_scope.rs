//! Certificate-integrity: a broken boolean's fallout must not poison every
//! subsequent part's per-part certificate.
//!
//! ## The defect (reproduced live 2026-07-14, root-caused here)
//!
//! A cross-drilled manifold hits the OPEN cyl-cyl saddle bug (#35): the second
//! Difference COMPLETES but the result solid is UNSOUND (open edges). Its
//! boundary-edge / connectivity errors are stamped `solid_id: None` by the
//! model-wide `check_topology_gaps` pass (it records only the `face_id`). A
//! subsequently created PLAIN CYLINDER PRIMITIVE (itself watertight / oriented
//! / manifold) then certifies `brep_valid = false` with ConnectivityError
//! "Boundary edge N detected" entries located at the OTHER solid's faces —
//! because `validate_solid_scoped` kept every `solid_id: None` error for every
//! part (`None => true`). One broken boolean poisons EVERY part's certificate.
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
//! 3. The unsound saddle solid still reports ITS OWN defect (`brep_valid=false`).
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

/// Build the cross-drilled manifold that trips the #35 saddle: 80×40×40 block,
/// analytic vertical bore (Ok/sound), then an analytic horizontal bore that
/// crosses the first at the saddle (Ok but UNSOUND). Returns (model, result).
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

/// The unsound saddle solid honestly owns ITS OWN defect: its per-part
/// certificate must reflect `brep_valid = false` (the boundary-edge errors on
/// its own faces), even though those same errors no longer leak to other parts.
#[test]
fn unsound_saddle_solid_still_reports_its_own_defect() {
    let (mut m, res) = build_saddle_manifold();
    let cert = m.certify_solid(res);
    assert!(
        !cert.brep_valid,
        "the unsound saddle result solid must still report its OWN brep defect \
         (boundary edges on its faces); the attribution fix must not hide a \
         solid's own errors",
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
