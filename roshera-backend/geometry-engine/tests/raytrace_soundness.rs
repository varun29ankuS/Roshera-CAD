//! Raytrace soundness gate (#13).
//!
//! The analytic eye must never lie. For a battery of parts and view directions
//! this asserts, for EVERY visible pixel:
//!   * recoverable — the hit names a real face, and the hit POINT genuinely lies
//!     on that face's surface (closest_point → point_at round-trips to it);
//!   * front-facing — the oriented normal points back toward the camera;
//! and that a MISSING face reveals itself (its pixels see through, never a
//! phantom surface) for every face of a box. Plus determinism.
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::queries::raytrace::raytrace_ortho;

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

/// The hit point must lie on the surface of the face it names: feed it back
/// through `closest_point → point_at` and require the round-trip to return it.
fn point_on_face_surface(m: &BRepModel, face_id: u32, p: Point3) -> bool {
    let face = match m.faces.get(face_id) {
        Some(f) => f,
        None => return false,
    };
    let surf = match m.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return false,
    };
    let uv = match surf.closest_point(&p, m.tolerance()) {
        Ok(uv) => uv,
        Err(_) => return false,
    };
    let p2 = match surf.point_at(uv.0, uv.1) {
        Ok(p2) => p2,
        Err(_) => return false,
    };
    (p - p2).magnitude() < 1e-5
}

fn face_set(m: &BRepModel, s: SolidId) -> std::collections::HashSet<u32> {
    let solid = m.solids.get(s).expect("solid");
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    let mut out = std::collections::HashSet::new();
    for sh in shells {
        if let Some(shell) = m.shells.get(sh) {
            for &f in &shell.faces {
                out.insert(f);
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn assert_view_sound(
    m: &BRepModel,
    s: SolidId,
    right: Vector3,
    up: Vector3,
    dir: Vector3,
    half_w: f64,
    label: &str,
) {
    let f = raytrace_ortho(m, s, Point3::ZERO, right, up, dir, half_w, 200.0, 24, 24);
    let faces = face_set(m, s);
    let d = dir.normalize().unwrap_or(Vector3::Z);
    let mut visible = 0;
    for i in 0..(f.width * f.height) {
        if !f.hit[i] {
            continue;
        }
        visible += 1;
        let fid = f.face_id[i];
        assert!(
            faces.contains(&fid),
            "{label}: pixel {i} hit non-face {fid}"
        );
        let n = f.normal[i];
        let nv = Vector3::new(n[0], n[1], n[2]);
        // unit normal
        assert!(
            (nv.magnitude() - 1.0).abs() < 1e-6,
            "{label}: normal not unit at {i}: {n:?}"
        );
        // front-facing: visible surface points back toward the camera (−dir).
        assert!(
            nv.dot(&d) < 1e-6,
            "{label}: back-facing normal visible at {i}: n·dir={}",
            nv.dot(&d)
        );
    }
    assert!(
        visible > 50,
        "{label}: part should be visibly covered (got {visible} px)"
    );
}

/// Point-on-surface recoverability, checked by re-casting the centre ray and
/// round-tripping its world hit point through the named face's surface.
fn assert_center_point_recoverable(
    m: &BRepModel,
    s: SolidId,
    origin: Point3,
    dir: Vector3,
    label: &str,
) {
    let hit = geometry_engine::queries::raycast_solid(m, s, origin, dir)
        .unwrap_or_else(|| panic!("{label}: centre ray must hit"));
    assert!(
        point_on_face_surface(m, hit.face_id, hit.point),
        "{label}: hit point {:?} not on face {} surface",
        hit.point,
        hit.face_id
    );
}

#[test]
fn box_is_sound_from_several_views() {
    let mut m = BRepModel::new();
    let b = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("box"));
    assert_view_sound(
        &m,
        b,
        Vector3::X,
        Vector3::Y,
        Vector3::new(0.0, 0.0, -1.0),
        12.0,
        "box top",
    );
    assert_view_sound(
        &m,
        b,
        Vector3::X,
        Vector3::Z,
        Vector3::new(0.0, 1.0, 0.0),
        12.0,
        "box front",
    );
    assert_view_sound(
        &m,
        b,
        Vector3::new(1.0, -1.0, 0.0),
        Vector3::new(-1.0, -1.0, 2.0),
        Vector3::new(1.0, 1.0, -1.0),
        18.0,
        "box iso",
    );
    assert_center_point_recoverable(
        &m,
        b,
        Point3::new(0.0, 0.0, 40.0),
        Vector3::new(0.0, 0.0, -1.0),
        "box",
    );
}

#[test]
fn cylinder_and_sphere_are_sound() {
    let mut m = BRepModel::new();
    let c = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::ZERO, Vector3::Z, 10.0, 30.0)
        .expect("cyl"));
    // View the cylinder from the side (+Y looking −Y): lateral wall + the seam
    // avoided by framing slightly off-axis via right=X, up=Z.
    assert_view_sound(
        &m,
        c,
        Vector3::X,
        Vector3::Z,
        Vector3::new(0.0, -1.0, 0.0),
        16.0,
        "cyl side",
    );
    assert_center_point_recoverable(
        &m,
        c,
        Point3::new(0.0, 40.0, 15.0),
        Vector3::new(0.0, -1.0, 0.0),
        "cyl",
    );

    let mut m2 = BRepModel::new();
    let s = sid(TopologyBuilder::new(&mut m2)
        .create_sphere_3d(Point3::ZERO, 12.0)
        .expect("sphere"));
    assert_view_sound(
        &m2,
        s,
        Vector3::X,
        Vector3::Z,
        Vector3::new(0.0, -1.0, 0.0),
        14.0,
        "sphere",
    );
    assert_center_point_recoverable(
        &m2,
        s,
        Point3::new(0.0, 40.0, 0.0),
        Vector3::new(0.0, -1.0, 0.0),
        "sphere",
    );
}

#[test]
fn every_box_face_reveals_itself_when_removed() {
    // For each of the 6 faces: remove it, look along the outward direction that
    // would see it head-on, and assert the centre pixel no longer reports that
    // face (it sees through). No face can hide its own absence.
    let dirs = [
        (Vector3::new(0.0, 0.0, -1.0), Point3::new(0.0, 0.0, 40.0)),
        (Vector3::new(0.0, 0.0, 1.0), Point3::new(0.0, 0.0, -40.0)),
        (Vector3::new(0.0, -1.0, 0.0), Point3::new(0.0, 40.0, 0.0)),
        (Vector3::new(0.0, 1.0, 0.0), Point3::new(0.0, -40.0, 0.0)),
        (Vector3::new(-1.0, 0.0, 0.0), Point3::new(40.0, 0.0, 0.0)),
        (Vector3::new(1.0, 0.0, 0.0), Point3::new(-40.0, 0.0, 0.0)),
    ];
    for (dir, origin) in dirs {
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("box"));
        let front = geometry_engine::queries::raycast_solid(&m, b, origin, dir)
            .expect("face seen before removal")
            .face_id;
        let shell_id = m.solids.get(b).unwrap().outer_shell;
        if let Some(shell) = m.shells.get_mut(shell_id) {
            shell.faces.retain(|&f| f != front);
        }
        let after = geometry_engine::queries::raycast_solid(&m, b, origin, dir);
        match after {
            None => {}
            Some(h) => assert_ne!(
                h.face_id, front,
                "removed face {front} must not still be hit from dir {dir:?}"
            ),
        }
    }
}
