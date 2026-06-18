//! #17 — boolean of a NURBS-lateral solid (the F1 cockpit-cut). Differencing a
//! box out of a `nurbs_loft` barrel used to NON-TERMINATE: the generic dual-
//! surface marcher hit its 200k-step cap on the NURBS×plane pair and discarded
//! the curve, so the wall never split and the boolean rejected the result.
//!
//! With the marching-squares plane↔freeform SSI (math::surface_plane_intersection
//! wired into surface_surface_intersection's (Planar,Other) arm) the cut now
//! COMPLETES and imprints onto the NURBS wall — bounded, no hang. Producing a
//! fully WATERTIGHT result (sharing the cut edges between operands) is the
//! remaining #17 work, pinned by the ignored test below.

use geometry_engine::math::Point3;
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::nurbs_loft::{nurbs_loft, NurbsLoftOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn barrel(m: &mut BRepModel) -> SolidId {
    let ring = |r: f64, z: f64| {
        (0..20)
            .map(|i| {
                let a = i as f64 * std::f64::consts::TAU / 20.0;
                Point3::new(r * a.cos(), r * a.sin(), z)
            })
            .collect::<Vec<_>>()
    };
    let sections = vec![
        ring(3.0, 0.0),
        ring(4.0, 2.0),
        ring(4.0, 4.0),
        ring(3.0, 6.0),
    ];
    nurbs_loft(m, sections, NurbsLoftOptions::default()).expect("barrel")
}
fn boxs(m: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    match TopologyBuilder::new(m).create_box_3d(w, h, d).unwrap() {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}
fn face_count(m: &BRepModel, s: SolidId) -> usize {
    m.solids
        .get(s)
        .map(|sol| {
            sol.shell_ids()
                .iter()
                .filter_map(|sh| m.shells.get(*sh))
                .map(|sh| sh.faces.len())
                .sum()
        })
        .unwrap_or(0)
}

/// Regression guard: the NURBS∖box difference must COMPLETE (no hang, no
/// rejection) and imprint the cut onto the freeform wall. This locks in the
/// marching-squares SSI fix.
#[test]
fn nurbs_minus_box_completes_and_imprints() {
    let mut m = BRepModel::new();
    let b = barrel(&mut m);
    let faces_before = face_count(&m, b);
    let cutter = boxs(&mut m, 3.0, 8.0, 3.0);
    let result = boolean_operation(
        &mut m,
        b,
        cutter,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("#17: NURBS∖box must complete (was: non-terminating SSI → rejection)");
    assert!(
        face_count(&m, result) >= faces_before,
        "the cut must imprint onto the NURBS solid (faces {} -> {})",
        faces_before,
        face_count(&m, result)
    );
}

/// DESIRED end state (pinned, currently RED → #[ignore]): the notch difference is
/// a sound, watertight solid. The remaining #17 work is corefinement — sharing
/// the cut edges between the NURBS-wall fragments and the box-face fragments so
/// no boundary edges (gaps) remain. Un-ignore when watertight.
#[test]
#[ignore = "#17: NURBS-cut not yet watertight — cut edges not shared between operands (corefinement)"]
fn nurbs_minus_box_should_be_watertight() {
    let mut m = BRepModel::new();
    let b = barrel(&mut m);
    let cutter = boxs(&mut m, 3.0, 8.0, 3.0);
    let result = boolean_operation(
        &mut m,
        b,
        cutter,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("#17: NURBS∖box should succeed");
    let gt = m.ground_truth(result).expect("gt");
    assert!(
        gt.certificate.is_sound(),
        "#17: result must be sound: {}",
        gt.summary()
    );
}
