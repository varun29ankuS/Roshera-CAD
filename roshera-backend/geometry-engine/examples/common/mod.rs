//! Shared helpers for the Roshera kernel demo harness.
//!
//! This module is included by every example file in `examples/` via
//! `#[path = "common/mod.rs"] mod common;`. Cargo only treats files
//! directly under `examples/` (or `examples/<name>/main.rs`) as example
//! binaries, so a `common/` subdirectory without a `main.rs` is safely
//! ignored by the auto-discovery rules.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::builder::BRepModel;
use geometry_engine::primitives::curve::{Circle, Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::vertex::VertexId;
use geometry_engine::tessellation::{tessellate_solid, TessellationParams, TriangleMesh};

/// Stats produced by [`tess_and_write`].
pub struct MeshStats {
    pub verts: usize,
    pub tris: usize,
    pub tess_ms: f64,
    pub stl_path: PathBuf,
}

/// Tessellate a solid and write the resulting mesh to a binary STL file.
///
/// `subdir` is the demo's working directory under `target/demos/`.
pub fn tess_and_write(
    model: &BRepModel,
    solid_id: SolidId,
    params: &TessellationParams,
    subdir: &str,
    filename: &str,
) -> MeshStats {
    let solid = model.solids.get(solid_id).expect("solid exists");

    let t = Instant::now();
    let mesh = tessellate_solid(solid, model, params);
    let tess_dt = t.elapsed();

    let dir = PathBuf::from("target").join("demos").join(subdir);
    let _ = fs::create_dir_all(&dir);
    let path = dir.join(filename);

    let stl_dt = match write_stl_binary(&mesh, &path) {
        Ok(d) => d,
        Err(e) => {
            println!("    !! STL write failed for {}: {e}", path.display());
            Duration::ZERO
        }
    };

    let abs = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
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
        stl_path: abs,
    }
}

/// Minimal binary-STL writer. Inlined here so demos compile without the
/// gated `export` feature.
pub fn write_stl_binary(mesh: &TriangleMesh, path: &Path) -> std::io::Result<Duration> {
    let t = Instant::now();
    let file = File::create(path)?;
    let mut w = BufWriter::new(file);

    // 80-byte header
    let mut header = [0u8; 80];
    let tag = b"Roshera demo binary STL";
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

/// Add a straight-line edge between two existing vertices in `model`.
///
/// Looks up vertex coordinates, builds a [`Line`] curve, registers it,
/// then creates an [`Edge`] with `EdgeOrientation::Forward` and the unit
/// parameter range. Returns the freshly-allocated [`EdgeId`].
pub fn add_line_edge(model: &mut BRepModel, v_start: VertexId, v_end: VertexId) -> EdgeId {
    let s = model.vertices.get(v_start).expect("start vertex exists");
    let e = model.vertices.get(v_end).expect("end vertex exists");
    let line = Line::new(
        Point3::new(s.position[0], s.position[1], s.position[2]),
        Point3::new(e.position[0], e.position[1], e.position[2]),
    );
    let curve_id = model.curves.add(Box::new(line));
    let edge = Edge::new_auto_range(0, v_start, v_end, curve_id, EdgeOrientation::Forward);
    model.edges.add(edge)
}

/// Build a closed rectangular profile in the XY plane (constant z).
///
/// Returns the four edges in CCW order: bottom, right, top, left. Suitable
/// to feed straight into `extrude_profile` or `revolve_profile`.
pub fn make_rectangle_profile(
    model: &mut BRepModel,
    origin: Point3,
    width: f64,
    height: f64,
) -> Vec<EdgeId> {
    let z = origin.z;
    let v0 = model.vertices.add(origin.x,           origin.y,           z);
    let v1 = model.vertices.add(origin.x + width,   origin.y,           z);
    let v2 = model.vertices.add(origin.x + width,   origin.y + height,  z);
    let v3 = model.vertices.add(origin.x,           origin.y + height,  z);
    vec![
        add_line_edge(model, v0, v1),
        add_line_edge(model, v1, v2),
        add_line_edge(model, v2, v3),
        add_line_edge(model, v3, v0),
    ]
}

/// Build a closed circle profile as a single self-closing edge.
///
/// `axis` is the circle's normal vector. Returns one edge whose start and
/// end vertex are identical (the circle's seam point at angle 0).
pub fn make_circle_profile(
    model: &mut BRepModel,
    center: Point3,
    axis: Vector3,
    radius: f64,
) -> Vec<EdgeId> {
    // Pick a seam point on the circle. The agent's recipe places it at
    // (center.x + radius, center.y, center.z) — works for axis = +Z. For
    // arbitrary axes we'd need a tangent frame; demos use axis-aligned
    // circles, so this suffices.
    let seam = Point3::new(center.x + radius, center.y, center.z);
    let v = model.vertices.add_or_find(seam.x, seam.y, seam.z, 1e-6);
    let circle = Circle::new(center, axis, radius).expect("circle params");
    let curve_id = model.curves.add(Box::new(circle));
    let edge = Edge::new(0, v, v, curve_id, EdgeOrientation::Forward, ParameterRange::unit());
    vec![model.edges.add(edge)]
}

/// Print a header banner for a demo.
pub fn header(title: &str) {
    println!("=== Roshera demo: {title} ===\n");
}

/// Print a one-line model summary for a [`BRepModel`].
pub fn model_summary(model: &BRepModel) {
    println!("\n=== model summary ===");
    println!("  solids:   {}", model.solids.len());
    println!("  shells:   {}", model.shells.len());
    println!("  faces:    {}", model.faces.len());
    println!("  edges:    {}", model.edges.len());
    println!("  vertices: {}", model.vertices.len());
    println!("  curves:   {}", model.curves.len());
    println!("  surfaces: {}", model.surfaces.len());
}
