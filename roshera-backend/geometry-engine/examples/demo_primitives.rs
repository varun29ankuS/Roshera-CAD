//! Roshera kernel demo — every primitive.
//!
//! Builds one of each primitive (box, sphere, cylinder, cone, torus),
//! tessellates it, writes a binary STL into `target/demos/primitives/`,
//! and asserts that the tessellator emits a non-empty, sane mesh for each.
//!
//! Run with `cargo run --release --example demo_primitives`.

#[path = "common/mod.rs"]
mod common;

use std::time::Instant;

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::{
    box_primitive::{BoxParameters, BoxPrimitive},
    builder::BRepModel,
    cone_primitive::{ConeParameters, ConePrimitive},
    cylinder_primitive::{CylinderParameters, CylinderPrimitive},
    primitive_traits::Primitive,
    sphere_primitive::{SphereParameters, SpherePrimitive},
    torus_primitive::{TorusParameters, TorusPrimitive},
};
use geometry_engine::tessellation::TessellationParams;

use common::{header, model_summary, tess_and_write, MeshStats};

const SUBDIR: &str = "primitives";

/// Acceptance bounds for each primitive's mesh after Phase 2 lands.
/// Lower bound `> 0` simply asserts the tessellator produces output.
/// Upper bound is a sanity check against runaway over-tessellation.
struct Bounds {
    name: &'static str,
    min_tris: usize,
    max_tris: usize,
}

const BOX: Bounds = Bounds {
    name: "box",
    min_tris: 12, // 6 faces × 2 tris minimum
    max_tris: 5_000,
};
const SPHERE: Bounds = Bounds {
    name: "sphere",
    min_tris: 100,
    max_tris: 50_000,
};
const CYLINDER: Bounds = Bounds {
    name: "cylinder",
    min_tris: 60,      // ~16 segments × ~2 tris × 2 faces + caps
    max_tris: 100_000, // hard ceiling — Phase 2.2 must hold this
};
const CONE: Bounds = Bounds {
    name: "cone",
    min_tris: 30,
    max_tris: 50_000,
};
const TORUS: Bounds = Bounds {
    name: "torus",
    min_tris: 200,
    max_tris: 100_000,
};

fn main() {
    header("primitives — one of each");

    let mut model = BRepModel::new();
    let params = TessellationParams::default();

    let box_stats = make_box(&mut model, &params);
    let sphere_stats = make_sphere(&mut model, &params);
    let cyl_stats = make_cylinder(&mut model, &params);
    let cone_stats = make_cone(&mut model, &params);
    let torus_stats = make_torus(&mut model, &params);

    println!("\n=== STL outputs ===");
    print_row(&BOX, &box_stats);
    print_row(&SPHERE, &sphere_stats);
    print_row(&CYLINDER, &cyl_stats);
    print_row(&CONE, &cone_stats);
    print_row(&TORUS, &torus_stats);

    model_summary(&model);

    // Assertions — failing any of these means a regression. CI picks them up
    // as a non-zero exit when this demo is run via `cargo test --examples`.
    check(&BOX, &box_stats);
    check(&SPHERE, &sphere_stats);
    check(&CYLINDER, &cyl_stats);
    check(&CONE, &cone_stats);
    check(&TORUS, &torus_stats);

    println!("\nAll primitives within acceptance bounds.");
}

fn make_box(model: &mut BRepModel, params: &TessellationParams) -> MeshStats {
    let t = Instant::now();
    let id = BoxPrimitive::create(
        BoxParameters::new(50.0, 50.0, 50.0).expect("box params"),
        model,
    )
    .expect("box create");
    println!(
        "[1] box       50x50x50            -> solid #{id}  (built in {:.2} ms)",
        t.elapsed().as_secs_f64() * 1e3
    );
    tess_and_write(model, id, params, SUBDIR, "box.stl")
}

fn make_sphere(model: &mut BRepModel, params: &TessellationParams) -> MeshStats {
    let t = Instant::now();
    let id = SpherePrimitive::create(
        SphereParameters::new(30.0, Point3::new(0.0, 0.0, 0.0)).expect("sphere params"),
        model,
    )
    .expect("sphere create");
    println!(
        "[2] sphere    r=30                 -> solid #{id}  (built in {:.2} ms)",
        t.elapsed().as_secs_f64() * 1e3
    );
    tess_and_write(model, id, params, SUBDIR, "sphere.stl")
}

fn make_cylinder(model: &mut BRepModel, params: &TessellationParams) -> MeshStats {
    let t = Instant::now();
    let cyl_params = CylinderParameters::new(15.0, 80.0)
        .expect("cyl params")
        .with_axis(Vector3::new(0.0, 0.0, 1.0))
        .expect("cyl axis");
    let id = CylinderPrimitive::create(cyl_params, model).expect("cyl create");
    println!(
        "[3] cylinder  r=15 h=80 axis=Z     -> solid #{id}  (built in {:.2} ms)",
        t.elapsed().as_secs_f64() * 1e3
    );
    tess_and_write(model, id, params, SUBDIR, "cylinder.stl")
}

fn make_cone(model: &mut BRepModel, params: &TessellationParams) -> MeshStats {
    let t = Instant::now();
    // Apex at top, axis pointing down → base radius = h * tan(half_angle).
    // half_angle = π/6 (30°), height = 50 → base radius ≈ 28.87.
    let cone_params = ConeParameters::new(
        Point3::new(0.0, 0.0, 50.0),
        Vector3::new(0.0, 0.0, -1.0),
        std::f64::consts::FRAC_PI_6,
        50.0,
    )
    .expect("cone params");
    let id = ConePrimitive::create(&cone_params, model).expect("cone create");
    println!(
        "[4] cone      half=30° h=50        -> solid #{id}  (built in {:.2} ms)",
        t.elapsed().as_secs_f64() * 1e3
    );
    tess_and_write(model, id, params, SUBDIR, "cone.stl")
}

fn make_torus(model: &mut BRepModel, params: &TessellationParams) -> MeshStats {
    let t = Instant::now();
    let torus_params = TorusParameters::new(
        Point3::new(0.0, 0.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
        30.0,
        10.0,
    )
    .expect("torus params");
    let id = TorusPrimitive::create(&torus_params, model).expect("torus create");
    println!(
        "[5] torus     R=30 r=10 axis=Z     -> solid #{id}  (built in {:.2} ms)",
        t.elapsed().as_secs_f64() * 1e3
    );
    tess_and_write(model, id, params, SUBDIR, "torus.stl")
}

fn print_row(b: &Bounds, s: &MeshStats) {
    println!(
        "  {:<10} {:>10} verts  {:>10} tris  {:>8.2} ms",
        b.name, s.verts, s.tris, s.tess_ms
    );
}

fn check(b: &Bounds, s: &MeshStats) {
    assert!(
        s.tris >= b.min_tris,
        "{} tessellation under-produced: got {} tris, expected >= {} (defect surfaced — see Phase 2 of the kernel hardening plan)",
        b.name, s.tris, b.min_tris
    );
    assert!(
        s.tris <= b.max_tris,
        "{} tessellation over-produced: got {} tris, expected <= {} (defect surfaced — see Phase 2 of the kernel hardening plan)",
        b.name, s.tris, b.max_tris
    );
}
