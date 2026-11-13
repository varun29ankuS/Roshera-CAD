// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! `fdm.overhang` must not call the face a part is printed ON an overhang.
//!
//! Found 2026-11-13 on the hub flange: printing +Z, the flange's flat
//! bottom (it sits on the build plate) read as a 90° overhang -- the rule
//! measured every downward face, including the one resting on the bed.
//!
//! The fixture holds both cases in one solid: a narrow cylinder standing on
//! the bed with a wide one on top of it (a mushroom). The narrow one's
//! bottom rests on the bed and is no overhang; the wide one's underside,
//! 10 mm up, hangs in the air and IS a 90° overhang -- so the fix cannot
//! simply drop every downward face.

use geometry_engine::dfm::packs::evaluate;
use geometry_engine::dfm::{PackParams, Verdict};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::{Plane, SurfaceType};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn cylinder(model: &mut BRepModel, z: f64, r: f64, h: f64) -> SolidId {
    let mut b = TopologyBuilder::new(model);
    match b
        .create_cylinder_3d(Point3::new(0.0, 0.0, z), Vector3::Z, r, h)
        .expect("cylinder")
    {
        GeometryId::Solid(id) => id,
        o => panic!("cylinder: {o:?}"),
    }
}

fn mushroom() -> (BRepModel, SolidId, Vec<FaceId>) {
    let mut model = BRepModel::new();
    let stem = cylinder(&mut model, 0.0, 10.0, 10.0);
    let cap = cylinder(&mut model, 10.0, 20.0, 10.0);
    let id = boolean_operation(
        &mut model,
        stem,
        cap,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("union");
    let solid = model.solids.get(id).expect("solid");
    let mut faces = Vec::new();
    for shell in solid.all_shells() {
        faces.extend(
            model
                .shells
                .get(shell)
                .expect("shell")
                .faces
                .iter()
                .copied(),
        );
    }
    (model, id, faces)
}

/// Height (z) of a planar face whose outward normal points down, or None.
fn downward_plane_height(model: &BRepModel, f: FaceId) -> Option<f64> {
    let face = model.faces.get(f)?;
    let surface = model.surfaces.get(face.surface_id)?;
    if surface.surface_type() != SurfaceType::Plane {
        return None;
    }
    let plane = surface.as_any().downcast_ref::<Plane>()?;
    let n = plane.normal * face.orientation.sign();
    (n.z < -0.999).then_some(plane.origin.z)
}

#[test]
fn the_face_on_the_bed_is_not_an_overhang_but_a_hanging_underside_is() {
    let (model, id, faces) = mushroom();
    let bed: Vec<FaceId> = faces
        .iter()
        .copied()
        .filter(|&f| downward_plane_height(&model, f).is_some_and(|z| z.abs() < 1e-9))
        .collect();
    let underside: Vec<FaceId> = faces
        .iter()
        .copied()
        .filter(|&f| downward_plane_height(&model, f).is_some_and(|z| (z - 10.0).abs() < 1e-9))
        .collect();
    assert_eq!(bed.len(), 1, "fixture: one face on the bed");
    assert_eq!(underside.len(), 1, "fixture: one hanging underside");

    let report = evaluate(
        PackParams::Fdm {
            nozzle_diameter: 0.4,
            build_direction: [0.0, 0.0, 1.0],
        },
        &model,
        id,
        &faces,
    )
    .expect("fdm report");
    let overhang = report
        .verdicts()
        .iter()
        .find(|v| v.rule == "fdm.overhang")
        .expect("overhang rule");
    match &overhang.verdict {
        Verdict::Violation {
            witnesses,
            measured,
            ..
        } => {
            assert_eq!(
                witnesses, &underside,
                "only the hanging underside overhangs"
            );
            assert!(
                !witnesses.contains(&bed[0]),
                "the bed face is not an overhang"
            );
            assert!((measured.value - 90.0).abs() < 1e-9, "{measured:?}");
        }
        other => panic!("the underside must still violate, got {other:?}"),
    }
}

#[test]
fn printed_the_other_way_up_the_cap_top_is_on_the_bed_and_nothing_overhangs() {
    // Build -Z: the wide cap's top now rests on the bed and the stem stands
    // up from it -- no face hangs, so the rule must not violate.
    let (model, id, faces) = mushroom();
    let report = evaluate(
        PackParams::Fdm {
            nozzle_diameter: 0.4,
            build_direction: [0.0, 0.0, -1.0],
        },
        &model,
        id,
        &faces,
    )
    .expect("fdm report");
    let overhang = report
        .verdicts()
        .iter()
        .find(|v| v.rule == "fdm.overhang")
        .expect("overhang rule");
    assert!(
        !matches!(overhang.verdict, Verdict::Violation { .. }),
        "upside down nothing overhangs, got {:?}",
        overhang.verdict
    );
}
