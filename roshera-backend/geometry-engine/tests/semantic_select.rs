//! PILLAR 3 gate — reference-by-description selection with ambiguity-refusal.
//! The kernel resolves "the +Z planar face" / "the largest +Z planar face" to a
//! concrete FaceId, and REFUSES (Ambiguous / NotFound) when the description
//! doesn't single one out — never guesses.

use geometry_engine::math::Vector3;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::queries::select::{
    resolve_face, Extremal, FaceQuery, SelectError, SurfaceKind,
};

fn box_solid(m: &mut BRepModel, w: f64, h: f64, d: f64) -> u32 {
    match TopologyBuilder::new(m).create_box_3d(w, h, d).unwrap() {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}

#[test]
fn resolves_the_plus_z_face_uniquely() {
    let mut m = BRepModel::new();
    let s = box_solid(&mut m, 40.0, 30.0, 20.0);
    // "the +Z planar face" → exactly one (the top).
    let q = FaceQuery::new(SurfaceKind::Planar).facing(Vector3::Z);
    let fid = resolve_face(&mut m, s, &q).expect("unique +Z planar face");
    // Its outward normal must actually point +Z.
    let face = m.faces.get(fid).unwrap();
    let b = face.uv_bounds;
    let n = face
        .normal_at(0.5 * (b[0] + b[1]), 0.5 * (b[2] + b[3]), &m.surfaces)
        .unwrap();
    assert!(
        n.dot(&Vector3::Z) > 0.9,
        "resolved face must face +Z, got {n:?}"
    );
}

#[test]
fn refuses_when_ambiguous() {
    let mut m = BRepModel::new();
    let s = box_solid(&mut m, 40.0, 30.0, 20.0);
    // "a planar face" with no direction/extremal → 6 candidates → REFUSE.
    let q = FaceQuery::new(SurfaceKind::Planar);
    match resolve_face(&mut m, s, &q) {
        Err(SelectError::Ambiguous(c)) => assert_eq!(c.len(), 6, "a box has 6 planar faces"),
        other => panic!("expected Ambiguous(6), got {other:?}"),
    }
}

#[test]
fn refuses_when_not_found() {
    let mut m = BRepModel::new();
    let s = box_solid(&mut m, 10.0, 10.0, 10.0);
    // No cylindrical faces on a box → NotFound.
    let q = FaceQuery::new(SurfaceKind::Cylindrical);
    assert_eq!(resolve_face(&mut m, s, &q), Err(SelectError::NotFound));
}

#[test]
fn extremal_breaks_ties_but_still_refuses_a_true_tie() {
    let mut m = BRepModel::new();
    // Distinct dims: the two largest faces are the 40×30 top/bottom (area 1200).
    let s = box_solid(&mut m, 40.0, 30.0, 20.0);
    // "largest +Z planar face" → the top (unique once direction is fixed).
    let q = FaceQuery::new(SurfaceKind::Planar)
        .facing(Vector3::Z)
        .extremal(Extremal::LargestArea);
    assert!(
        resolve_face(&mut m, s, &q).is_ok(),
        "largest +Z planar face is unique"
    );

    // "largest planar face" WITHOUT a direction → top and bottom both area 1200
    // → a true tie → REFUSE (the kernel won't pick one).
    let q2 = FaceQuery::new(SurfaceKind::Planar).extremal(Extremal::LargestArea);
    match resolve_face(&mut m, s, &q2) {
        Err(SelectError::Ambiguous(c)) => assert!(c.len() >= 2, "top/bottom tie"),
        other => panic!("expected Ambiguous tie, got {other:?}"),
    }

    // A cube: every planar face ties → "largest planar face" refuses.
    let mut mc = BRepModel::new();
    let cube = box_solid(&mut mc, 10.0, 10.0, 10.0);
    let q3 = FaceQuery::new(SurfaceKind::Planar).extremal(Extremal::LargestArea);
    assert!(
        matches!(
            resolve_face(&mut mc, cube, &q3),
            Err(SelectError::Ambiguous(_))
        ),
        "a cube's faces all tie → refuse"
    );
}
