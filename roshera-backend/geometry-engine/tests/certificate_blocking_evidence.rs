// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Kernel Lens: an unsound certificate NAMES what blocks it -- each failing
//! check with the faces it is on and what was measured -- so the person sees
//! which faces to look at and the agent knows what to act on. A check that no
//! face can be blamed for stays on the solid; it is never dropped.

use geometry_engine::primitives::face::FaceOrientation;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn unit_box(model: &mut BRepModel) -> SolidId {
    match TopologyBuilder::new(model)
        .create_box_3d(2.0, 3.0, 4.0)
        .expect("box")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected a solid, got {other:?}"),
    }
}

#[test]
fn a_sound_box_has_no_blocking() {
    let mut model = BRepModel::new();
    let sid = unit_box(&mut model);
    let cert = model.certify_solid(sid);
    assert!(cert.is_sound(), "{:?}", cert.errors);
    assert!(cert.blocking.is_empty(), "{:#?}", cert.blocking);
}

#[test]
fn an_open_face_blocks_watertight_by_name() {
    let mut model = BRepModel::new();
    let sid = unit_box(&mut model);
    // Take one face out of the shell: the four faces around it now have
    // open edges.
    let shell = model.solids.get(sid).expect("solid").outer_shell;
    let removed = {
        let sh = model.shells.get_mut(shell).expect("shell");
        let f = sh.faces[0];
        sh.faces.retain(|&g| g != f);
        f
    };
    let cert = model.certify_solid(sid);
    assert!(!cert.watertight);
    let open: Vec<_> = cert
        .blocking
        .iter()
        .filter(|b| b.conjunct == "watertight")
        .collect();
    assert!(
        !open.is_empty(),
        "watertight names its faces: {:#?}",
        cert.blocking
    );
    for b in &open {
        let f = b.face.expect("an open edge is on a face");
        assert_ne!(f, removed, "the removed face is not in the solid");
        assert!(
            b.measured
                .iter()
                .any(|(k, v)| k == "open_edges" && *v > 0.0),
            "the count is measured: {b:?}"
        );
    }
}

#[test]
fn unattributed_conjunct_is_kept_on_the_solid() {
    let mut model = BRepModel::new();
    let sid = unit_box(&mut model);
    // Turn the whole box inside out: every face consistent with every
    // other, the shell enclosing negative volume. No single face is to blame.
    let shell = model.solids.get(sid).expect("solid").outer_shell;
    let faces = model.shells.get(shell).expect("shell").faces.clone();
    for f in faces {
        let loops = {
            let face = model.faces.get_mut(f).expect("face");
            face.orientation = match face.orientation {
                FaceOrientation::Forward => FaceOrientation::Backward,
                FaceOrientation::Backward => FaceOrientation::Forward,
            };
            std::iter::once(face.outer_loop)
                .chain(face.inner_loops.iter().copied())
                .collect::<Vec<_>>()
        };
        for l in loops {
            model.loops.get_mut(l).expect("loop").reverse();
        }
    }
    let cert = model.certify_solid(sid);
    assert!(!cert.is_sound());
    assert!(
        cert.blocking
            .iter()
            .any(|b| b.conjunct == "shells_outward" && b.face.is_none()),
        "the inside-out shell is named on the solid: {:#?}",
        cert.blocking
    );
}
