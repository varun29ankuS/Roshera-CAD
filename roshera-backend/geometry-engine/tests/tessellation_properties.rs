// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Tessellation properties for the analytic primitives: every primitive
//! tessellates to a non-empty, finite, index-valid mesh; finer chord
//! tolerance never produces fewer triangles; and the mesh's divergence-
//! theorem volume matches the analytic volume (a watertightness witness).

use std::f64::consts::PI;

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams, TriangleMesh};

fn expect_solid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn box_solid(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    let mut b = TopologyBuilder::new(model);
    expect_solid(b.create_box_3d(w, h, d).expect("box"))
}
fn sphere_solid(model: &mut BRepModel, r: f64) -> SolidId {
    let mut b = TopologyBuilder::new(model);
    expect_solid(b.create_sphere_3d(Point3::ORIGIN, r).expect("sphere"))
}
fn cylinder_solid(model: &mut BRepModel, r: f64, h: f64) -> SolidId {
    let mut b = TopologyBuilder::new(model);
    expect_solid(
        b.create_cylinder_3d(Point3::ORIGIN, Vector3::Z, r, h)
            .expect("cyl"),
    )
}
fn cone_solid(model: &mut BRepModel, rb: f64, rt: f64, h: f64) -> SolidId {
    let mut b = TopologyBuilder::new(model);
    expect_solid(
        b.create_cone_3d(Point3::ORIGIN, Vector3::Z, rb, rt, h)
            .expect("cone"),
    )
}
fn torus_solid(model: &mut BRepModel, big: f64, small: f64) -> SolidId {
    let mut b = TopologyBuilder::new(model);
    expect_solid(
        b.create_torus_3d(Point3::ORIGIN, Vector3::Z, big, small)
            .expect("torus"),
    )
}

fn mesh_of(model: &BRepModel, id: SolidId, params: &TessellationParams) -> TriangleMesh {
    let solid = model.solids.get(id).expect("solid");
    tessellate_solid(solid, model, params)
}

fn params_chord(c: f64) -> TessellationParams {
    TessellationParams {
        chord_tolerance: c,
        ..TessellationParams::default()
    }
}

/// Divergence-theorem volume of a closed triangle mesh.
fn mesh_volume(mesh: &TriangleMesh) -> f64 {
    let mut v = 0.0;
    for t in &mesh.triangles {
        let a = mesh.vertices[t[0] as usize].position;
        let b = mesh.vertices[t[1] as usize].position;
        let c = mesh.vertices[t[2] as usize].position;
        v += (a.x * (b.y * c.z - b.z * c.y) - a.y * (b.x * c.z - b.z * c.x)
            + a.z * (b.x * c.y - b.y * c.x))
            / 6.0;
    }
    v.abs()
}

fn assert_mesh_sane(mesh: &TriangleMesh, label: &str) {
    assert!(!mesh.vertices.is_empty(), "{label}: no vertices");
    assert!(!mesh.triangles.is_empty(), "{label}: no triangles");
    let n = mesh.vertices.len() as u32;
    for v in &mesh.vertices {
        assert!(
            v.position.x.is_finite() && v.position.y.is_finite() && v.position.z.is_finite(),
            "{label}: non-finite vertex"
        );
    }
    for t in &mesh.triangles {
        for &idx in t {
            assert!(idx < n, "{label}: triangle index {idx} out of range {n}");
        }
        // Non-degenerate index triple (a real triangle references 3 distinct verts).
        assert!(
            t[0] != t[1] && t[1] != t[2] && t[0] != t[2],
            "{label}: degenerate index triple"
        );
    }
}

fn rel_close(a: f64, b: f64, tol: f64) -> bool {
    if b == 0.0 {
        a.abs() <= tol
    } else {
        ((a - b) / b).abs() <= tol
    }
}

// =====================================================================
// Mesh sanity per primitive.
// =====================================================================

macro_rules! mesh_sane_test {
    ($name:ident, $build:expr, $label:expr) => {
        #[test]
        fn $name() {
            let mut model = BRepModel::new();
            let id = $build(&mut model);
            let mesh = mesh_of(&model, id, &TessellationParams::default());
            assert_mesh_sane(&mesh, $label);
        }
    };
}

mesh_sane_test!(
    mesh_box,
    |m: &mut BRepModel| box_solid(m, 2.0, 3.0, 4.0),
    "box"
);
mesh_sane_test!(
    mesh_sphere,
    |m: &mut BRepModel| sphere_solid(m, 3.0),
    "sphere"
);
mesh_sane_test!(
    mesh_cylinder,
    |m: &mut BRepModel| cylinder_solid(m, 2.0, 6.0),
    "cylinder"
);
mesh_sane_test!(
    mesh_cone,
    |m: &mut BRepModel| cone_solid(m, 2.0, 0.0, 5.0),
    "cone"
);
mesh_sane_test!(
    mesh_frustum,
    |m: &mut BRepModel| cone_solid(m, 3.0, 1.0, 5.0),
    "frustum"
);
mesh_sane_test!(
    mesh_torus,
    |m: &mut BRepModel| torus_solid(m, 4.0, 1.0),
    "torus"
);
mesh_sane_test!(
    mesh_thin_box,
    |m: &mut BRepModel| box_solid(m, 10.0, 0.5, 8.0),
    "thin box"
);
mesh_sane_test!(
    mesh_small_sphere,
    |m: &mut BRepModel| sphere_solid(m, 0.5),
    "small sphere"
);

// =====================================================================
// Refinement monotonicity: finer chord tolerance ⇒ ≥ as many triangles.
// =====================================================================

macro_rules! refinement_monotone_test {
    ($name:ident, $build:expr) => {
        #[test]
        fn $name() {
            let coarse = {
                let mut m = BRepModel::new();
                let id = $build(&mut m);
                mesh_of(&m, id, &params_chord(0.5)).triangles.len()
            };
            let fine = {
                let mut m = BRepModel::new();
                let id = $build(&mut m);
                mesh_of(&m, id, &params_chord(0.02)).triangles.len()
            };
            assert!(
                fine >= coarse,
                "finer tolerance produced fewer triangles: coarse={coarse} fine={fine}"
            );
        }
    };
}

refinement_monotone_test!(refine_sphere, |m: &mut BRepModel| sphere_solid(m, 3.0));
refinement_monotone_test!(refine_cylinder, |m: &mut BRepModel| cylinder_solid(
    m, 2.0, 6.0
));
refinement_monotone_test!(refine_cone, |m: &mut BRepModel| cone_solid(
    m, 2.0, 0.0, 5.0
));
refinement_monotone_test!(refine_torus, |m: &mut BRepModel| torus_solid(m, 4.0, 1.0));

// =====================================================================
// Mesh volume matches analytic volume (watertightness witness).
// =====================================================================

#[test]
fn box_mesh_volume_matches_analytic() {
    let mut m = BRepModel::new();
    let id = box_solid(&mut m, 2.0, 3.0, 4.0);
    let vol = mesh_volume(&mesh_of(&m, id, &TessellationParams::default()));
    assert!(rel_close(vol, 24.0, 0.03), "box mesh volume {vol} vs 24");
}

#[test]
fn cylinder_mesh_volume_matches_analytic() {
    let mut m = BRepModel::new();
    let id = cylinder_solid(&mut m, 2.0, 6.0);
    let vol = mesh_volume(&mesh_of(&m, id, &params_chord(0.05)));
    assert!(
        rel_close(vol, PI * 4.0 * 6.0, 0.05),
        "cylinder mesh volume {vol}"
    );
}

#[test]
fn sphere_mesh_volume_matches_analytic() {
    let mut m = BRepModel::new();
    let id = sphere_solid(&mut m, 3.0);
    let vol = mesh_volume(&mesh_of(&m, id, &params_chord(0.05)));
    assert!(
        rel_close(vol, 4.0 / 3.0 * PI * 27.0, 0.05),
        "sphere mesh volume {vol}"
    );
}

#[test]
fn cone_mesh_volume_matches_analytic() {
    let mut m = BRepModel::new();
    let id = cone_solid(&mut m, 2.0, 0.0, 6.0);
    // Pointed cones undershoot a little more than other primitives because
    // the apex singularity is sampled coarsely; 8% absorbs that.
    let vol = mesh_volume(&mesh_of(&m, id, &params_chord(0.05)));
    assert!(
        rel_close(vol, 1.0 / 3.0 * PI * 4.0 * 6.0, 0.08),
        "cone mesh volume {vol}"
    );
}

#[test]
fn torus_mesh_volume_matches_analytic() {
    let mut m = BRepModel::new();
    let id = torus_solid(&mut m, 4.0, 1.0);
    // Torus volume = 2π²·R·r².
    let vol = mesh_volume(&mesh_of(&m, id, &params_chord(0.05)));
    assert!(
        rel_close(vol, 2.0 * PI * PI * 4.0 * 1.0, 0.06),
        "torus mesh volume {vol}"
    );
}

#[test]
fn box_mesh_volume_stable_across_tolerances() {
    // A box is planar; its mesh volume must be tolerance-independent.
    let mut m = BRepModel::new();
    let id = box_solid(&mut m, 2.0, 2.0, 2.0);
    let coarse = mesh_volume(&mesh_of(&m, id, &params_chord(1.0)));
    let fine = mesh_volume(&mesh_of(&m, id, &params_chord(0.01)));
    assert!(
        rel_close(coarse, fine, 0.01),
        "box volume drifted with tolerance: {coarse} vs {fine}"
    );
}
