//! Regression: a boolean `Difference` must not leave orphan operand faces
//! behind, and the surviving bore-wall cylinder must carry the LIVE clipped
//! `height_limits`, not the un-clipped cutter extent.
//!
//! ## The bug (diag `.superpowers/sdd/diag-slice2-boolean-orphan.md`)
//!
//! Boolean `Difference` retires its operands by removing only the `Solid`
//! records — it never removes the operands' faces, and the operands' husk
//! shells remain in the store referencing them. So after cutting a blind
//! cylinder bore into a box, the cutter's side-wall `Face` is left LIVE in
//! `model.faces` but reachable from NO solid (`find_parent_solid` == None):
//! an orphan.
//!
//! That orphan also aliases the LIVE bore-wall face's `Cylinder` surface
//! verbatim (the split pipeline copies `surface_id` without cloning), and
//! that shared cylinder still carries the cutter's ORIGINAL `height_limits`
//! (the un-clipped extent) — nothing re-trims the surviving wall after it is
//! clipped to the box.
//!
//! ## What GREEN looks like
//!
//! 1. Every `Face` in `model.faces` belongs to some shell of some solid.
//! 2. The surviving bore-wall `Cylinder`'s axial span equals the LIVE clipped
//!    bore depth, not the taller cutter extent.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::Cylinder;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// Set of every face id reachable from a live solid (walking
/// solid -> outer/inner shells -> faces). A face NOT in this set is an
/// orphan: live in `model.faces` but owned by no solid.
fn faces_owned_by_a_solid(model: &BRepModel) -> std::collections::HashSet<u32> {
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
    owned
}

fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_box_3d(w, h, d)
        .expect("box")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

fn make_cylinder(
    model: &mut BRepModel,
    base: Point3,
    axis: Vector3,
    r: f64,
    height: f64,
) -> SolidId {
    match TopologyBuilder::new(model)
        .create_cylinder_3d(base, axis, r, height)
        .expect("cyl")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

/// Box 40x40x40 (centred, z in [-20, 20]) minus a blind cylinder bore r=8.
///
/// Cutter: base at z=5, axis +Z, height 30 (top at z=35). It enters the box
/// interior at z=5 (its base cap becomes the blind-pocket floor) and exits
/// cleanly through the top face at z=20. So the LIVE bore wall spans z in
/// [5, 20] — an axial extent of 15 — while the cutter cylinder's stored
/// `height_limits` describe the full 30 of cutter height.
#[test]
fn difference_leaves_no_orphan_faces_and_retrims_bore_cylinder() {
    let mut m = BRepModel::new();
    let boxs = make_box(&mut m, 40.0, 40.0, 40.0);
    let bore = make_cylinder(&mut m, Point3::new(0.0, 0.0, 5.0), Vector3::Z, 8.0, 30.0);

    let result = boolean_operation(
        &mut m,
        boxs,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("blind-bore difference must succeed");
    assert!(
        m.solids.get(result).is_some(),
        "difference must yield a live result solid",
    );

    // (1) No orphan faces: every live face is owned by a solid.
    let owned = faces_owned_by_a_solid(&m);
    let orphans: Vec<u32> = m
        .faces
        .iter()
        .map(|(fid, _)| fid)
        .filter(|fid| !owned.contains(fid))
        .collect();
    assert!(
        orphans.is_empty(),
        "boolean Difference left {} orphan face(s) live in the store but owned by \
         no solid: {:?}",
        orphans.len(),
        orphans,
    );

    // (2) The surviving bore-wall cylinder carries the LIVE clipped extent.
    // The only Cylinder surface among the result faces is the bore wall; its
    // axial span must equal the clipped bore depth (15), not the cutter's
    // un-clipped height (30).
    const CLIPPED_DEPTH: f64 = 15.0;
    let mut checked = 0usize;
    for &fid in &owned {
        let Some(face) = m.faces.get(fid) else {
            continue;
        };
        let Some(surf) = m.surfaces.get(face.surface_id) else {
            continue;
        };
        if let Some(cyl) = surf.as_any().downcast_ref::<Cylinder>() {
            let limits = cyl
                .height_limits
                .expect("bore-wall cylinder must be finite");
            let span = limits[1] - limits[0];
            assert!(
                (span - CLIPPED_DEPTH).abs() < 1e-6,
                "surviving bore-wall cylinder axial span {span} does not match the \
                 LIVE clipped bore depth {CLIPPED_DEPTH} (stale un-clipped cutter \
                 extent is 30); height_limits = {limits:?}",
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 1,
        "expected at least one surviving bore-wall Cylinder face in the result",
    );
}
