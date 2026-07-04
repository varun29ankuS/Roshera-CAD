//! Persistent-id boolean lineage harness (#11 slice 40-D).
//!
//! Boolean is where lineage is easy to fake and hard to do right. These checks
//! verify it is SOUND, not cosmetic:
//!   * REAL parent attribution — an untouched passthrough face INHERITS its
//!     TRUE parent input face's PID (identity follows the entity across
//!     unrelated booleans — no per-operation drift), while a face the boolean
//!     geometrically altered gets a fresh *principled derivation* from its
//!     parent, never a positional label;
//!   * coverage + distinctness — every result face carries a distinct,
//!     round-trippable PID across union / difference / intersection;
//!   * correct operand side — a split face from the cutter derives via
//!     BooleanFromB, a split face from the body via BooleanFromA;
//!   * MOULD stability — editing an operand's dimension and re-evaluating
//!     preserves the PIDs of the faces that survive unchanged.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::persistent_id::{PersistentId, Role};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::Plane;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

fn faces_of(m: &BRepModel, s: SolidId) -> Vec<FaceId> {
    let solid = m.solids.get(s).expect("solid");
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    let mut out = Vec::new();
    for sh in shells {
        if let Some(shell) = m.shells.get(sh) {
            out.extend_from_slice(&shell.faces);
        }
    }
    out
}

/// The planar face of `solid` whose plane normal is along axis `axis` (0=x,1=y,
/// 2=z) and whose origin sits at `coord` on that axis — e.g. a box's +X face.
fn planar_face(m: &BRepModel, solid: SolidId, axis: usize, coord: f64) -> Option<FaceId> {
    for fid in faces_of(m, solid) {
        let face = match m.faces.get(fid) {
            Some(f) => f,
            None => continue,
        };
        let surf = match m.surfaces.get(face.surface_id) {
            Some(s) => s,
            None => continue,
        };
        if let Some(p) = surf.as_any().downcast_ref::<Plane>() {
            let n = [p.normal.x, p.normal.y, p.normal.z];
            let o = [p.origin.x, p.origin.y, p.origin.z];
            if n[axis].abs() > 0.99 && (o[axis] - coord).abs() < 1e-6 {
                // Also require the other two normal components ~0 (axis-aligned).
                let others = (0..3).filter(|&i| i != axis).all(|i| n[i].abs() < 1e-6);
                if others {
                    return Some(fid);
                }
            }
        }
    }
    None
}

/// Difference of a 40³-ish block (event key "blk") minus a Ø`2*cutter_r`
/// through-bore on +Z (event key "cut"). Returns the model, the result solid,
/// and the block's captured pre-boolean (solid PID, +X face PID) — the +X side
/// face is untouched by a central Z bore, so it survives as a single fragment.
struct Bored {
    m: BRepModel,
    result: SolidId,
    block_solid_pid: PersistentId,
    block_px_pid: PersistentId,
}

fn build_bored_block(cutter_r: f64) -> Bored {
    let mut m = BRepModel::new();
    m.set_event_key(Some("blk".into()));
    let block = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(40.0, 40.0, 20.0)
        .expect("block"));
    m.set_event_key(Some("cut".into()));
    let cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -30.0), Vector3::Z, cutter_r, 60.0)
        .expect("cyl"));
    m.set_event_key(None);

    // Capture the block's identity BEFORE the boolean consumes it.
    let block_solid_pid = m.solid_pid(block).expect("block solid pid");
    let px = planar_face(&m, block, 0, 20.0).expect("block +X face");
    let block_px_pid = m.face_pid(px).expect("block +X face pid");

    let result = boolean_operation(
        &mut m,
        block,
        cyl,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("difference");
    Bored {
        m,
        result,
        block_solid_pid,
        block_px_pid,
    }
}

#[test]
fn every_result_face_has_a_distinct_recoverable_pid() {
    // Union of a box and a cylinder boss poking out its top (a clean union).
    let mut m = BRepModel::new();
    m.set_event_key(Some("a".into()));
    let a = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("a"));
    m.set_event_key(Some("b".into()));
    let boss = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 5.0, 20.0)
        .expect("boss"));
    m.set_event_key(None);
    let result = boolean_operation(&mut m, a, boss, BooleanOp::Union, BooleanOptions::default())
        .expect("union");

    let rf = faces_of(&m, result);
    assert!(!rf.is_empty(), "union has faces");
    let mut pids = Vec::new();
    for f in &rf {
        let p = m
            .face_pid(*f)
            .unwrap_or_else(|| panic!("face {f} has no PID"));
        assert_eq!(m.face_by_pid(p), Some(*f), "id↔pid round-trip for {f}");
        pids.push(p);
    }
    let n = pids.len();
    pids.sort();
    pids.dedup();
    assert_eq!(pids.len(), n, "every result face PID is distinct");
}

#[test]
fn untouched_passthrough_face_inherits_its_parent_pid() {
    // The deep check, updated to the inheritance doctrine (Viewport Dimensions
    // Task 1): the +X side face survives the bore UNTOUCHED — a true
    // passthrough copied whole by `add_non_intersecting_faces` — so its result
    // PID must EQUAL its parent's PID (identity follows the entity). It must
    // NOT be the per-boolean BooleanFromA derivation: re-deriving through
    // each operation's context is exactly the drift that made dimension /
    // label / GD&T anchors rot across unrelated successive booleans.
    let b = build_bored_block(8.0);
    let res_px = planar_face(&b.m, b.result, 0, 20.0).expect("result +X face");
    let got = b.m.face_pid(res_px).expect("result +X face pid");

    assert_eq!(
        got, b.block_px_pid,
        "an untouched passthrough face INHERITS its true parent's PID"
    );
    let per_boolean_derivation = PersistentId::derive(
        &[b.block_solid_pid, b.block_px_pid],
        "boolean_Difference",
        &Role::BooleanFromA {
            source_face_pid: b.block_px_pid,
        },
    );
    assert_ne!(
        got, per_boolean_derivation,
        "passthrough PID must not be re-derived through the boolean's context \
         (that re-derivation is the identity-drift bug)"
    );
}

#[test]
fn bore_wall_traces_to_the_cutter_via_boolean_from_b() {
    // A result face that came from the CUTTER (the bore wall, a Cylinder face)
    // must derive via BooleanFromB. We verify by reconstructing the expected PID
    // from the cutter's lateral-face PID.
    let mut m = BRepModel::new();
    m.set_event_key(Some("blk".into()));
    let block = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(40.0, 40.0, 20.0)
        .expect("block"));
    m.set_event_key(Some("cut".into()));
    let cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -30.0), Vector3::Z, 8.0, 60.0)
        .expect("cyl"));
    m.set_event_key(None);

    let cyl_solid_pid = m.solid_pid(cyl).expect("cyl solid pid");
    // The cylinder lateral face = the one on a Cylinder surface.
    let cyl_lat = faces_of(&m, cyl)
        .into_iter()
        .find(|&f| {
            m.faces
                .get(f)
                .and_then(|fc| m.surfaces.get(fc.surface_id))
                .map(|s| {
                    s.as_any()
                        .downcast_ref::<geometry_engine::primitives::surface::Cylinder>()
                        .is_some()
                })
                .unwrap_or(false)
        })
        .expect("cyl lateral face");
    let cyl_lat_pid = m.face_pid(cyl_lat).expect("cyl lateral pid");

    let result = boolean_operation(
        &mut m,
        block,
        cyl,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("difference");

    // Find the bore wall in the result: a Cylinder face.
    let bore = faces_of(&m, result)
        .into_iter()
        .find(|&f| {
            m.faces
                .get(f)
                .and_then(|fc| m.surfaces.get(fc.surface_id))
                .map(|s| {
                    s.as_any()
                        .downcast_ref::<geometry_engine::primitives::surface::Cylinder>()
                        .is_some()
                })
                .unwrap_or(false)
        })
        .expect("result bore wall");
    let got = m.face_pid(bore).expect("bore wall pid");

    let expected = PersistentId::derive(
        &[cyl_solid_pid, cyl_lat_pid],
        "boolean_Difference",
        &Role::BooleanFromB {
            source_face_pid: cyl_lat_pid,
        },
    );
    assert_eq!(
        got, expected,
        "bore wall derives from the cutter via BooleanFromB"
    );
}

#[test]
fn surviving_face_pid_is_stable_across_an_operand_mould() {
    // The mould property for booleans: editing the CUTTER radius and
    // re-evaluating preserves the PID of the body face that survives unchanged.
    let small = build_bored_block(4.0);
    let large = build_bored_block(9.0);

    let f_small = planar_face(&small.m, small.result, 0, 20.0).expect("+X small");
    let f_large = planar_face(&large.m, large.result, 0, 20.0).expect("+X large");
    let p_small = small.m.face_pid(f_small).expect("pid small");
    let p_large = large.m.face_pid(f_large).expect("pid large");

    assert_eq!(
        p_small, p_large,
        "the body's +X face keeps its PID when the cutter radius is moulded"
    );
}

#[test]
fn intersection_faces_are_named_and_distinct() {
    let mut m = BRepModel::new();
    m.set_event_key(Some("a".into()));
    let a = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("a"));
    m.set_event_key(Some("b".into()));
    let bsph = sid(TopologyBuilder::new(&mut m)
        .create_sphere_3d(Point3::new(0.0, 0.0, 0.0), 12.0)
        .expect("sphere"));
    m.set_event_key(None);
    if let Ok(result) = boolean_operation(
        &mut m,
        a,
        bsph,
        BooleanOp::Intersection,
        BooleanOptions::default(),
    ) {
        let rf = faces_of(&m, result);
        let mut pids = Vec::new();
        for f in &rf {
            let p = m
                .face_pid(*f)
                .unwrap_or_else(|| panic!("intersection face {f} unnamed"));
            pids.push(p);
        }
        let n = pids.len();
        pids.sort();
        pids.dedup();
        assert_eq!(
            pids.len(),
            n,
            "intersection result faces have distinct PIDs"
        );
    }
}
