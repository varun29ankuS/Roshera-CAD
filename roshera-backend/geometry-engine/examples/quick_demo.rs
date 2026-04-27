//! Roshera kernel quick demo.
//!
//! End-to-end pipeline: primitives → tessellate → STL export, plus a
//! boolean step at the end to surface where the kernel still has gaps.
//! Run with `cargo run --release --example quick_demo`.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::{
    box_primitive::{BoxParameters, BoxPrimitive},
    builder::BRepModel,
    cylinder_primitive::{CylinderParameters, CylinderPrimitive},
    primitive_traits::Primitive,
    solid::SolidId,
    sphere_primitive::{SphereParameters, SpherePrimitive},
};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams, TriangleMesh};

fn main() {
    println!("=== Roshera kernel quick demo ===\n");

    let mut model = BRepModel::new();
    let params = TessellationParams::default();

    // ---- Step 1: box --------------------------------------------------
    let t = Instant::now();
    let box_id = BoxPrimitive::create(
        BoxParameters::new(50.0, 50.0, 50.0).expect("box params"),
        &mut model,
    )
    .expect("box create");
    println!(
        "[1] box     50x50x50          -> solid #{box_id}  (built in {:.2} ms)",
        t.elapsed().as_secs_f64() * 1e3
    );
    let box_stats = tess_and_write(&model, box_id, &params, "out_box.stl");

    // ---- Step 2: sphere ------------------------------------------------
    let t = Instant::now();
    let sphere_id = SpherePrimitive::create(
        SphereParameters::new(30.0, Point3::new(25.0, 25.0, 25.0)).expect("sphere params"),
        &mut model,
    )
    .expect("sphere create");
    println!(
        "[2] sphere  r=30 at (25,25,25) -> solid #{sphere_id}  (built in {:.2} ms)",
        t.elapsed().as_secs_f64() * 1e3
    );
    let sphere_stats = tess_and_write(&model, sphere_id, &params, "out_sphere.stl");

    // ---- Step 3: cylinder ---------------------------------------------
    let t = Instant::now();
    let cyl_params = CylinderParameters::new(15.0, 80.0)
        .expect("cyl params")
        .with_axis(Vector3::new(0.0, 0.0, 1.0))
        .expect("cyl axis");
    let cyl_id =
        CylinderPrimitive::create(cyl_params, &mut model).expect("cyl create");
    println!(
        "[3] cylinder r=15 h=80 axis=Z  -> solid #{cyl_id}  (built in {:.2} ms)",
        t.elapsed().as_secs_f64() * 1e3
    );
    let cyl_stats = tess_and_write(&model, cyl_id, &params, "out_cylinder.stl");

    // ---- Step 4: boolean (box - sphere) -------------------------------
    let t = Instant::now();
    let bool_result = boolean_operation(
        &mut model,
        box_id,
        sphere_id,
        BooleanOp::Difference,
        BooleanOptions::default(),
    );
    let dt_bool = t.elapsed();

    let bool_stats = match bool_result {
        Ok(id) => {
            println!(
                "[4] boolean (box - sphere)     -> solid #{id}  (computed in {:.2} ms)",
                dt_bool.as_secs_f64() * 1e3
            );
            Some(tess_and_write(&model, id, &params, "out_boolean.stl"))
        }
        Err(e) => {
            println!(
                "[4] boolean FAILED ({:.2} ms): {e:?}",
                dt_bool.as_secs_f64() * 1e3
            );
            None
        }
    };

    // ---- Summary table -------------------------------------------------
    println!("\n=== STL outputs ===");
    println!(
        "  {:<14} {:>8} verts  {:>8} tris  {:>8.2} ms",
        "box",       box_stats.verts, box_stats.tris, box_stats.tess_ms
    );
    println!(
        "  {:<14} {:>8} verts  {:>8} tris  {:>8.2} ms",
        "sphere",    sphere_stats.verts, sphere_stats.tris, sphere_stats.tess_ms
    );
    println!(
        "  {:<14} {:>8} verts  {:>8} tris  {:>8.2} ms",
        "cylinder",  cyl_stats.verts, cyl_stats.tris, cyl_stats.tess_ms
    );
    if let Some(s) = bool_stats {
        println!(
            "  {:<14} {:>8} verts  {:>8} tris  {:>8.2} ms  {}",
            "boolean diff", s.verts, s.tris, s.tess_ms,
            if s.tris == 0 { "<-- known gap: trimmed faces not triangulated" } else { "" }
        );
    }

    println!("\n=== model summary ===");
    println!("  solids:   {}", model.solids.len());
    println!("  shells:   {}", model.shells.len());
    println!("  faces:    {}", model.faces.len());
    println!("  edges:    {}", model.edges.len());
    println!("  vertices: {}", model.vertices.len());
    println!("  curves:   {}", model.curves.len());
    println!("  surfaces: {}", model.surfaces.len());
}

/// Stats produced by `tess_and_write`.
struct MeshStats {
    verts: usize,
    tris: usize,
    tess_ms: f64,
}

fn tess_and_write(
    model: &BRepModel,
    solid_id: SolidId,
    params: &TessellationParams,
    filename: &str,
) -> MeshStats {
    let solid = model.solids.get(solid_id).expect("solid exists");

    let t = Instant::now();
    let mesh = tessellate_solid(solid, model, params);
    let tess_dt = t.elapsed();

    let path = PathBuf::from(filename);
    let stl_dt = match write_stl_binary(&mesh, &path) {
        Ok(d) => d,
        Err(e) => {
            println!("    !! STL write failed for {filename}: {e}");
            Duration::ZERO
        }
    };

    let abs = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    println!(
        "    -> {} ({} verts / {} tris, tess {:.2} ms, stl {:.2} ms)",
        abs.display(),
        mesh.vertices.len(),
        mesh.triangles.len(),
        tess_dt.as_secs_f64() * 1e3,
        stl_dt.as_secs_f64() * 1e3,
    );

    MeshStats {
        verts: mesh.vertices.len(),
        tris: mesh.triangles.len(),
        tess_ms: tess_dt.as_secs_f64() * 1e3,
    }
}

/// Minimal binary-STL writer (inlined so the example compiles without the
/// gated `export` feature).
fn write_stl_binary(mesh: &TriangleMesh, path: &Path) -> std::io::Result<Duration> {
    let t = Instant::now();
    let file = File::create(path)?;
    let mut w = BufWriter::new(file);

    // 80-byte header
    let mut header = [0u8; 80];
    let tag = b"Roshera quick_demo binary STL";
    header[..tag.len()].copy_from_slice(tag);
    w.write_all(&header)?;

    // u32 LE triangle count
    let tri_count = mesh.triangles.len() as u32;
    w.write_all(&tri_count.to_le_bytes())?;

    for tri in &mesh.triangles {
        let v0 = &mesh.vertices[tri[0] as usize];
        let v1 = &mesh.vertices[tri[1] as usize];
        let v2 = &mesh.vertices[tri[2] as usize];

        // Face normal from triangle edges
        let e1 = v1.position - v0.position;
        let e2 = v2.position - v0.position;
        let nx = e1.y * e2.z - e1.z * e2.y;
        let ny = e1.z * e2.x - e1.x * e2.z;
        let nz = e1.x * e2.y - e1.y * e2.x;
        let len = (nx * nx + ny * ny + nz * nz).sqrt();
        let (nx, ny, nz) = if len > 1e-15 {
            (nx / len, ny / len, nz / len)
        } else {
            (0.0, 0.0, 1.0)
        };

        for f in [nx as f32, ny as f32, nz as f32] {
            w.write_all(&f.to_le_bytes())?;
        }
        for v in [v0, v1, v2] {
            for f in [v.position.x as f32, v.position.y as f32, v.position.z as f32] {
                w.write_all(&f.to_le_bytes())?;
            }
        }
        w.write_all(&0u16.to_le_bytes())?;
    }

    w.flush()?;
    Ok(t.elapsed())
}
