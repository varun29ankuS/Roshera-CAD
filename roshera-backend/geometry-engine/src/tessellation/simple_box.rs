//! Simple box tessellation for debugging
//!
//! Creates a non-indexed box mesh with per-face normals for sharp edges

use super::ThreeJsMesh;
use crate::math::{Point3, Vector3};

/// Create a simple box mesh with per-face normals
pub fn create_simple_box_mesh(width: f64, height: f64, depth: f64) -> ThreeJsMesh {
    let mut mesh = ThreeJsMesh::new();

    let hw = width / 2.0;
    let hh = height / 2.0;
    let hd = depth / 2.0;

    // Define vertices for each face (24 vertices total - 4 per face)
    // This allows us to have per-face normals for sharp edges

    // Front face (+Z)
    add_quad(
        &mut mesh,
        Point3::new(-hw, -hh, hd),
        Point3::new(hw, -hh, hd),
        Point3::new(hw, hh, hd),
        Point3::new(-hw, hh, hd),
        Vector3::new(0.0, 0.0, 1.0),
    );

    // Back face (-Z)
    add_quad(
        &mut mesh,
        Point3::new(hw, -hh, -hd),
        Point3::new(-hw, -hh, -hd),
        Point3::new(-hw, hh, -hd),
        Point3::new(hw, hh, -hd),
        Vector3::new(0.0, 0.0, -1.0),
    );

    // Top face (+Y)
    add_quad(
        &mut mesh,
        Point3::new(-hw, hh, hd),
        Point3::new(hw, hh, hd),
        Point3::new(hw, hh, -hd),
        Point3::new(-hw, hh, -hd),
        Vector3::new(0.0, 1.0, 0.0),
    );

    // Bottom face (-Y)
    add_quad(
        &mut mesh,
        Point3::new(-hw, -hh, -hd),
        Point3::new(hw, -hh, -hd),
        Point3::new(hw, -hh, hd),
        Point3::new(-hw, -hh, hd),
        Vector3::new(0.0, -1.0, 0.0),
    );

    // Right face (+X)
    add_quad(
        &mut mesh,
        Point3::new(hw, -hh, -hd),
        Point3::new(hw, -hh, hd),
        Point3::new(hw, hh, hd),
        Point3::new(hw, hh, -hd),
        Vector3::new(1.0, 0.0, 0.0),
    );

    // Left face (-X)
    add_quad(
        &mut mesh,
        Point3::new(-hw, -hh, hd),
        Point3::new(-hw, -hh, -hd),
        Point3::new(-hw, hh, -hd),
        Point3::new(-hw, hh, hd),
        Vector3::new(-1.0, 0.0, 0.0),
    );

    // No indices needed - we're using direct triangles
    // This gives us 36 vertices (6 faces × 4 vertices × 1.5 triangles/quad)

    tracing::info!(
        "Created simple box mesh: {} vertices, {} triangles",
        mesh.positions.len() / 3,
        mesh.positions.len() / 9
    );

    mesh
}

/// Add a quad (two triangles) with a specific normal
fn add_quad(
    mesh: &mut ThreeJsMesh,
    v0: Point3,
    v1: Point3,
    v2: Point3,
    v3: Point3,
    normal: Vector3,
) {
    // First triangle: v0, v1, v2
    add_vertex(mesh, &v0, &normal);
    add_vertex(mesh, &v1, &normal);
    add_vertex(mesh, &v2, &normal);

    // Second triangle: v0, v2, v3
    add_vertex(mesh, &v0, &normal);
    add_vertex(mesh, &v2, &normal);
    add_vertex(mesh, &v3, &normal);
}

/// Add a vertex with its normal
fn add_vertex(mesh: &mut ThreeJsMesh, point: &Point3, normal: &Vector3) {
    mesh.positions.push(point.x as f32);
    mesh.positions.push(point.y as f32);
    mesh.positions.push(point.z as f32);

    mesh.normals.push(normal.x as f32);
    mesh.normals.push(normal.y as f32);
    mesh.normals.push(normal.z as f32);
}
