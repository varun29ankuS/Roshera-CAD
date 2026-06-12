//! Invariants for the `readable` query layer — the agent-facing surface that
//! lets an LLM ask "how far apart are these parts", "what's this part's
//! oriented bounding box", "list the parts". These pin the mathematical
//! relationships a caller can rely on: distance symmetry and separation,
//! OBB extents matching the solid, list-count consistency.

use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::{transform_solid, TransformOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    let mut b = TopologyBuilder::new(model);
    match b.create_box_3d(w, h, d).expect("box") {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn shift_x(model: &mut BRepModel, id: SolidId, dx: f64) {
    transform_solid(
        model,
        id,
        Matrix4::from_translation(&Vector3::new(dx, 0.0, 0.0)),
        TransformOptions::default(),
    )
    .expect("translate");
}

fn rel_close(a: f64, b: f64, tol: f64) -> bool {
    if b == 0.0 {
        a.abs() <= tol
    } else {
        ((a - b) / b).abs() <= tol
    }
}

// =====================================================================
// part_distance: symmetry, self-zero, non-negative, separation value.
// =====================================================================

#[test]
fn distance_is_symmetric() {
    let mut model = BRepModel::new();
    let a = make_box(&mut model, 2.0, 2.0, 2.0);
    let b = make_box(&mut model, 2.0, 2.0, 2.0);
    shift_x(&mut model, b, 5.0);
    let ab = model.part_distance(a, b).expect("a→b");
    let ba = model.part_distance(b, a).expect("b→a");
    assert!(
        rel_close(ab.center_to_center, ba.center_to_center, 1e-9),
        "asymmetric center dist"
    );
    assert!(
        rel_close(ab.surface_to_surface, ba.surface_to_surface, 1e-9),
        "asymmetric surface dist"
    );
}

#[test]
fn distance_to_self_is_zero() {
    let mut model = BRepModel::new();
    let a = make_box(&mut model, 3.0, 4.0, 5.0);
    let d = model.part_distance(a, a).expect("a→a");
    assert!(
        d.center_to_center.abs() <= 1e-9,
        "self center distance {}",
        d.center_to_center
    );
    assert!(d.bbox_overlap, "a part must overlap itself");
}

#[test]
fn distance_is_non_negative() {
    for dx in [0.0, 1.0, 2.5, 5.0, 100.0] {
        let mut model = BRepModel::new();
        let a = make_box(&mut model, 2.0, 2.0, 2.0);
        let b = make_box(&mut model, 2.0, 2.0, 2.0);
        shift_x(&mut model, b, dx);
        let d = model.part_distance(a, b).expect("dist");
        assert!(d.center_to_center >= 0.0, "negative center dist at dx={dx}");
        assert!(
            d.surface_to_surface >= 0.0,
            "negative surface dist at dx={dx}"
        );
    }
}

#[test]
fn center_distance_matches_translation() {
    for dx in [3.0, 5.0, 10.0, 25.0] {
        let mut model = BRepModel::new();
        let a = make_box(&mut model, 2.0, 2.0, 2.0);
        let b = make_box(&mut model, 2.0, 2.0, 2.0);
        shift_x(&mut model, b, dx);
        let d = model.part_distance(a, b).expect("dist");
        // Both 2³ boxes start centred at the origin; shifting B by dx puts
        // the centres exactly dx apart.
        assert!(
            rel_close(d.center_to_center, dx, 1e-6),
            "center distance {} != {dx}",
            d.center_to_center
        );
    }
}

#[test]
fn surface_distance_is_gap_between_separated_boxes() {
    // Two unit-half boxes (full width 2, so half-width 1). Separated by dx,
    // the surface gap along x is dx - 2 once they part (dx > 2).
    for dx in [3.0, 4.0, 6.0] {
        let mut model = BRepModel::new();
        let a = make_box(&mut model, 2.0, 2.0, 2.0);
        let b = make_box(&mut model, 2.0, 2.0, 2.0);
        shift_x(&mut model, b, dx);
        let d = model.part_distance(a, b).expect("dist");
        assert!(!d.bbox_overlap, "boxes at dx={dx} must not overlap");
        assert!(
            rel_close(d.surface_to_surface, dx - 2.0, 1e-6),
            "surface gap {} != {} at dx={dx}",
            d.surface_to_surface,
            dx - 2.0
        );
    }
}

#[test]
fn overlapping_boxes_report_overlap_and_zero_gap() {
    for dx in [0.0, 0.5, 1.0, 1.5] {
        let mut model = BRepModel::new();
        let a = make_box(&mut model, 2.0, 2.0, 2.0);
        let b = make_box(&mut model, 2.0, 2.0, 2.0);
        shift_x(&mut model, b, dx);
        let d = model.part_distance(a, b).expect("dist");
        assert!(d.bbox_overlap, "boxes at dx={dx} (<2) must overlap");
        assert!(
            d.surface_to_surface <= 1e-6,
            "overlap gap {} != 0 at dx={dx}",
            d.surface_to_surface
        );
    }
}

#[test]
fn surface_distance_never_exceeds_center_distance() {
    for dx in [0.0, 1.0, 3.0, 7.0, 20.0] {
        let mut model = BRepModel::new();
        let a = make_box(&mut model, 2.0, 2.0, 2.0);
        let b = make_box(&mut model, 2.0, 2.0, 2.0);
        shift_x(&mut model, b, dx);
        let d = model.part_distance(a, b).expect("dist");
        assert!(
            d.surface_to_surface <= d.center_to_center + 1e-9,
            "surface {} > center {} at dx={dx}",
            d.surface_to_surface,
            d.center_to_center
        );
    }
}

// =====================================================================
// oriented_bbox_for: extents match the box, axes orthonormal, volume.
// =====================================================================

fn sorted3(mut a: [f64; 3]) -> [f64; 3] {
    a.sort_by(|x, y| x.partial_cmp(y).unwrap());
    a
}

macro_rules! obb_extents_test {
    ($name:ident, $w:expr, $h:expr, $d:expr) => {
        #[test]
        fn $name() {
            let mut model = BRepModel::new();
            let id = make_box(&mut model, $w, $h, $d);
            let obb = model.oriented_bbox_for(id).expect("obb");
            // Half-extents (order-independent) match the box half-dimensions.
            let got = sorted3(obb.half_extents);
            let want = sorted3([$w as f64 / 2.0, $h as f64 / 2.0, $d as f64 / 2.0]);
            for k in 0..3 {
                assert!(
                    rel_close(got[k], want[k], 0.05),
                    "half-extent[{k}] {} vs {} for {}x{}x{}",
                    got[k],
                    want[k],
                    $w,
                    $h,
                    $d
                );
            }
            // OBB volume (8·∏ half-extents) matches the box volume.
            let obb_vol = 8.0 * obb.half_extents[0] * obb.half_extents[1] * obb.half_extents[2];
            assert!(
                rel_close(obb_vol, ($w as f64) * ($h as f64) * ($d as f64), 0.05),
                "obb volume {obb_vol} vs box volume"
            );
            // Axes orthonormal.
            let ax = obb.axes;
            for i in 0..3 {
                let li = (ax[i][0].powi(2) + ax[i][1].powi(2) + ax[i][2].powi(2)).sqrt();
                assert!((li - 1.0).abs() <= 1e-6, "axis {i} not unit: {li}");
                for j in (i + 1)..3 {
                    let dot = ax[i][0] * ax[j][0] + ax[i][1] * ax[j][1] + ax[i][2] * ax[j][2];
                    assert!(dot.abs() <= 1e-6, "axes {i},{j} not orthogonal: {dot}");
                }
            }
        }
    };
}

obb_extents_test!(obb_cube, 2.0, 2.0, 2.0);
obb_extents_test!(obb_2_4_6, 2.0, 4.0, 6.0);
obb_extents_test!(obb_slab, 10.0, 1.0, 8.0);
obb_extents_test!(obb_3_3_9, 3.0, 3.0, 9.0);
obb_extents_test!(obb_5_2_7, 5.0, 2.0, 7.0);
obb_extents_test!(obb_unit, 1.0, 1.0, 1.0);

// =====================================================================
// list_parts: count tracks the number of solids.
// =====================================================================

#[test]
fn empty_model_lists_no_parts() {
    let model = BRepModel::new();
    assert_eq!(model.list_parts().len(), 0);
}

#[test]
fn list_parts_counts_every_solid() {
    let mut model = BRepModel::new();
    for i in 0..5 {
        let id = make_box(&mut model, 1.0, 1.0, 1.0);
        shift_x(&mut model, id, 5.0 * i as f64); // spread them out
    }
    assert_eq!(
        model.list_parts().len(),
        5,
        "list_parts must report all 5 solids"
    );
}

#[test]
fn list_parts_grows_as_solids_are_added() {
    let mut model = BRepModel::new();
    assert_eq!(model.list_parts().len(), 0);
    make_box(&mut model, 1.0, 1.0, 1.0);
    assert_eq!(model.list_parts().len(), 1);
    let b = make_box(&mut model, 1.0, 1.0, 1.0);
    shift_x(&mut model, b, 10.0);
    assert_eq!(model.list_parts().len(), 2);
}

// =====================================================================
// query_face principal curvatures: exact analytic values per surface
// class — flat [0,0], sphere r → [±1/r, ±1/r], cylinder r → [±1/r, 0].
// =====================================================================

#[test]
fn face_principal_curvatures_match_analytic() {
    let mut model = BRepModel::new();
    let bx = make_box(&mut model, 2.0, 2.0, 2.0);
    let sph = {
        let mut b = TopologyBuilder::new(&mut model);
        match b
            .create_sphere_3d(Point3::new(10.0, 0.0, 0.0), 1.0)
            .expect("sphere")
        {
            GeometryId::Solid(id) => id,
            other => panic!("expected solid, got {other:?}"),
        }
    };
    let cyl = {
        let mut b = TopologyBuilder::new(&mut model);
        match b
            .create_cylinder_3d(Point3::new(20.0, 0.0, 0.0), Vector3::Z, 0.5, 2.0)
            .expect("cylinder")
        {
            GeometryId::Solid(id) => id,
            other => panic!("expected solid, got {other:?}"),
        }
    };

    let faces_of = |model: &BRepModel, id: SolidId| -> Vec<u32> {
        let solid = model.solids.get(id).expect("solid");
        model
            .shells
            .get(solid.outer_shell)
            .expect("shell")
            .faces
            .clone()
    };

    // Every box face is flat: [0, 0].
    for fid in faces_of(&model, bx) {
        let r = model.query_face(fid).expect("box face report");
        let [k1, k2] = r.principal_curvatures.expect("box face curvatures");
        assert!(
            k1.abs() < 1e-9 && k2.abs() < 1e-9,
            "box face {fid}: expected flat, got [{k1}, {k2}]"
        );
    }

    // Sphere r=1: |k1| = |k2| = 1, equal signs (umbilic).
    for fid in faces_of(&model, sph) {
        let r = model.query_face(fid).expect("sphere face report");
        let [k1, k2] = r.principal_curvatures.expect("sphere face curvatures");
        assert!(
            rel_close(k1.abs(), 1.0, 1e-6) && rel_close(k2.abs(), 1.0, 1e-6),
            "sphere face {fid}: expected |k|=1 umbilic, got [{k1}, {k2}]"
        );
    }

    // Cylinder r=0.5: lateral face [±2, 0]; caps are flat [0, 0].
    let mut saw_lateral = false;
    for fid in faces_of(&model, cyl) {
        let r = model.query_face(fid).expect("cyl face report");
        let [k1, k2] = r.principal_curvatures.expect("cyl face curvatures");
        if r.surface_type == "cylinder" {
            saw_lateral = true;
            assert!(
                rel_close(k1.abs(), 2.0, 1e-6) && k2.abs() < 1e-9,
                "cyl lateral {fid}: expected [±2, 0], got [{k1}, {k2}]"
            );
        } else {
            assert!(
                k1.abs() < 1e-9 && k2.abs() < 1e-9,
                "cyl cap {fid}: expected flat, got [{k1}, {k2}]"
            );
        }
    }
    assert!(saw_lateral, "no cylindrical lateral face found");
}
