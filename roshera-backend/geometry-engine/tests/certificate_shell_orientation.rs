// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The certificate sees an inside-out shell.
//!
//! Every conjunct the certificate had before this file — B-Rep validity,
//! watertight, manifold, `oriented`, self-intersection, tessellation and mesh
//! quality — is blind to a shell whose faces ALL point the wrong way: the mesh
//! is still closed, still manifold, still consistently wound (each triangle
//! agrees with its neighbours), and each facet still agrees with its own
//! face's reversed normal. Measured before the fix, a box with every face
//! reversed, a two-body solid with its peer body reversed and a hollow with
//! its void reversed ALL certified `sound = true`.
//!
//! The global fact that decides it is the sign of the volume each shell
//! encloses: positive for a body (outer or peer shell), negative for a void.
//! The inverted solids here are made by reversing a shell's faces and loops in
//! place — exactly what an orientation-reversing transform without the
//! orientation fix leaves behind.

use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::offset::OffsetType;
use geometry_engine::operations::{
    boolean_operation, mirror, offset_solid, transform_solid, BooleanOp, BooleanOptions,
    CommonOptions, OffsetOptions, TransformOptions,
};
use geometry_engine::primitives::provenance::ShellRole;
use geometry_engine::primitives::shell::ShellId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn solid_of(id: Result<GeometryId, impl std::fmt::Debug>) -> SolidId {
    match id {
        Ok(GeometryId::Solid(s)) => s,
        other => panic!("expected a solid; got {other:?}"),
    }
}

fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    solid_of(TopologyBuilder::new(model).create_box_3d(w, h, d))
}

/// Reverse every face of `shell` and every loop of those faces — the exact
/// state an orientation-reversing transform leaves when nothing flips it back.
/// Then dirty every solid's cached certificate through the store's mutable
/// accessor (its conservative invalidation backstop).
fn invert_shell(model: &mut BRepModel, shell: ShellId) {
    let faces = model.shells.get(shell).expect("shell").faces.clone();
    for fid in faces {
        let loops = {
            let f = model.faces.get(fid).expect("face");
            let mut l = vec![f.outer_loop];
            l.extend(f.inner_loops.iter().copied());
            l
        };
        let f = model.faces.get_mut(fid).expect("face");
        f.orientation = f.orientation.flipped();
        for lid in loops {
            let lp = model.loops.get_mut(lid).expect("loop");
            lp.edges.reverse();
            lp.orientations.reverse();
            for o in lp.orientations.iter_mut() {
                *o = !*o;
            }
        }
    }
    let solids: Vec<SolidId> = model.solids.iter().map(|(id, _)| id).collect();
    for s in solids {
        let _ = model.solids.get_mut(s);
    }
}

/// Two disjoint 10³ boxes united: one outer shell, one peer body at x = 30.
fn two_body(model: &mut BRepModel) -> SolidId {
    let a = make_box(model, 10.0, 10.0, 10.0);
    let b = make_box(model, 10.0, 10.0, 10.0);
    transform_solid(
        model,
        b,
        Matrix4::from_translation(&Vector3::new(30.0, 0.0, 0.0)),
        TransformOptions::default(),
    )
    .expect("translate");
    let id = boolean_operation(model, a, b, BooleanOp::Union, BooleanOptions::default())
        .expect("disjoint union");
    assert_eq!(
        model.solids.get(id).expect("solid").peer_shells.len(),
        1,
        "fixture: one peer body"
    );
    id
}

/// A 10³ box hollowed to a closed 1mm wall: one outer shell, one void shell.
fn hollow(model: &mut BRepModel) -> SolidId {
    let s = make_box(model, 10.0, 10.0, 10.0);
    let opts = OffsetOptions {
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        offset_type: OffsetType::Distance(1.0),
        ..Default::default()
    };
    let id = offset_solid(model, s, 1.0, vec![], opts).expect("closed hollow");
    assert_eq!(
        model.solids.get(id).expect("solid").inner_shells.len(),
        1,
        "fixture: one void"
    );
    id
}

/// The certificate refuses `id`, and names the new conjunct as the reason.
fn assert_refused_as_inside_out(
    model: &mut BRepModel,
    id: SolidId,
    what: &str,
    shell: ShellId,
    role: ShellRole,
) {
    let cert = model.certify_solid(id);
    assert!(!cert.shells_outward, "{what}: shells_outward must be false");
    assert_eq!(
        cert.misoriented_shells.len(),
        1,
        "{what}: exactly the reversed shell is the witness: {:?}",
        cert.misoriented_shells
    );
    let w = &cert.misoriented_shells[0];
    assert_eq!((w.shell_id, w.role), (shell, role), "{what}: witness {w:?}");
    assert!(
        w.signed_volume * role.expected_volume_sign() < 0.0,
        "{what}: the witness carries the wrong-signed volume: {w:?}"
    );
    assert!(
        !cert.is_sound(),
        "{what} must certify UNSOUND; errors {:?}",
        cert.errors
    );
    let reason = cert
        .errors
        .iter()
        .find(|e| e.contains("shells_outward=false"))
        .unwrap_or_else(|| panic!("{what}: no shells_outward reason in {:?}", cert.errors));
    assert!(!reason.contains("  "), "no whitespace runs: {reason:?}");
}

#[test]
fn a_wholesale_inside_out_box_certifies_unsound() {
    let mut model = BRepModel::new();
    let id = make_box(&mut model, 10.0, 10.0, 10.0);
    let outer = model.solids.get(id).expect("solid").outer_shell;
    invert_shell(&mut model, outer);
    assert_refused_as_inside_out(
        &mut model,
        id,
        "a box with every face reversed",
        outer,
        ShellRole::Outer,
    );
}

#[test]
fn an_inside_out_peer_body_certifies_unsound() {
    let mut model = BRepModel::new();
    let id = two_body(&mut model);
    let peer = model.solids.get(id).expect("solid").peer_shells[0];
    invert_shell(&mut model, peer);
    assert_refused_as_inside_out(
        &mut model,
        id,
        "a two-body solid with its peer reversed",
        peer,
        ShellRole::Peer,
    );
}

#[test]
fn an_inside_out_void_certifies_unsound() {
    let mut model = BRepModel::new();
    let id = hollow(&mut model);
    let void = model.solids.get(id).expect("solid").inner_shells[0];
    invert_shell(&mut model, void);
    assert_refused_as_inside_out(
        &mut model,
        id,
        "a hollow with its void reversed",
        void,
        ShellRole::Void,
    );
}

#[test]
fn an_inside_out_hull_around_a_correct_void_certifies_unsound() {
    let mut model = BRepModel::new();
    let id = hollow(&mut model);
    let outer = model.solids.get(id).expect("solid").outer_shell;
    invert_shell(&mut model, outer);
    assert_refused_as_inside_out(
        &mut model,
        id,
        "a hollow with its hull reversed",
        outer,
        ShellRole::Outer,
    );
}

/// The conjunct must not refuse correctly oriented geometry of any kind: flat,
/// curved, multi-body, hollow, and a mirrored (orientation-fixed) solid.
#[test]
fn outward_solids_of_every_kind_stay_sound() {
    let mut cases: Vec<(&str, BRepModel, SolidId)> = Vec::new();

    let mut m = BRepModel::new();
    let id = make_box(&mut m, 10.0, 6.0, 4.0);
    cases.push(("box", m, id));

    let mut m = BRepModel::new();
    let id = solid_of(TopologyBuilder::new(&mut m).create_sphere_3d(Point3::ORIGIN, 5.0));
    cases.push(("sphere", m, id));

    let mut m = BRepModel::new();
    let id = solid_of(TopologyBuilder::new(&mut m).create_cylinder_3d(
        Point3::ORIGIN,
        Vector3::Z,
        3.0,
        8.0,
    ));
    cases.push(("cylinder", m, id));

    let mut m = BRepModel::new();
    let id = solid_of(TopologyBuilder::new(&mut m).create_cone_3d(
        Point3::ORIGIN,
        Vector3::Z,
        4.0,
        1.0,
        6.0,
    ));
    cases.push(("cone frustum", m, id));

    let mut m = BRepModel::new();
    let id = solid_of(TopologyBuilder::new(&mut m).create_torus_3d(
        Point3::ORIGIN,
        Vector3::Z,
        6.0,
        1.5,
    ));
    cases.push(("torus", m, id));

    let mut m = BRepModel::new();
    let id = two_body(&mut m);
    cases.push(("two-body union", m, id));

    let mut m = BRepModel::new();
    let id = hollow(&mut m);
    cases.push(("closed hollow", m, id));

    let mut m = BRepModel::new();
    let block = make_box(&mut m, 20.0, 20.0, 10.0);
    let bore = solid_of(TopologyBuilder::new(&mut m).create_cylinder_3d(
        Point3::new(0.0, 0.0, -10.0),
        Vector3::Z,
        3.0,
        30.0,
    ));
    let id = boolean_operation(
        &mut m,
        block,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("through-bore");
    cases.push(("block with a through-bore", m, id));

    let mut m = BRepModel::new();
    let id = make_box(&mut m, 10.0, 6.0, 4.0);
    mirror(
        &mut m,
        vec![id],
        Point3::new(12.0, 0.0, 0.0),
        Vector3::new(1.0, 1.0, 0.0),
        TransformOptions::default(),
    )
    .expect("mirror");
    cases.push(("mirrored box", m, id));

    let mut m = BRepModel::new();
    let id = solid_of(TopologyBuilder::new(&mut m).create_cylinder_3d(
        Point3::ORIGIN,
        Vector3::Z,
        3.0,
        8.0,
    ));
    mirror(
        &mut m,
        vec![id],
        Point3::new(0.0, 0.0, 20.0),
        Vector3::Z,
        TransformOptions::default(),
    )
    .expect("mirror");
    cases.push(("mirrored cylinder", m, id));

    for (what, mut model, id) in cases {
        let cert = model.certify_solid(id);
        assert!(
            !cert.errors.iter().any(|e| e.contains("shells_outward")),
            "{what}: an outward solid must not be refused as inside-out: {:?}",
            cert.errors
        );
        assert!(cert.shells_outward, "{what}: {:?}", cert.misoriented_shells);
        assert!(
            cert.is_sound(),
            "{what} must certify sound: {:?}",
            cert.errors
        );
    }
}
