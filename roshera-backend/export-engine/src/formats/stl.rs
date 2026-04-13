//! STL file format support

use byteorder::{LittleEndian, WriteBytesExt};
use shared_types::*;
use std::io::Write;

/// Generate binary STL content
pub fn generate_binary_stl(mesh: &Mesh, name: &str) -> Result<Vec<u8>, ExportError> {
    let mut buffer = Vec::new();

    // Write 80-byte header
    let header = format!("Binary STL - {}", name);
    let mut header_bytes = header.as_bytes().to_vec();
    header_bytes.resize(80, 0);
    buffer
        .write_all(&header_bytes)
        .map_err(|_| ExportError::ExportFailed {
            reason: "Failed to write header".to_string(),
        })?;

    // Write triangle count
    let triangle_count = mesh.triangle_count() as u32;
    buffer
        .write_u32::<LittleEndian>(triangle_count)
        .map_err(|_| ExportError::ExportFailed {
            reason: "Failed to write triangle count".to_string(),
        })?;

    // Write triangles
    for i in (0..mesh.indices.len()).step_by(3) {
        let i0 = mesh.indices[i] as usize * 3;
        let i1 = mesh.indices[i + 1] as usize * 3;
        let i2 = mesh.indices[i + 2] as usize * 3;

        let v0 = [
            mesh.vertices[i0],
            mesh.vertices[i0 + 1],
            mesh.vertices[i0 + 2],
        ];
        let v1 = [
            mesh.vertices[i1],
            mesh.vertices[i1 + 1],
            mesh.vertices[i1 + 2],
        ];
        let v2 = [
            mesh.vertices[i2],
            mesh.vertices[i2 + 1],
            mesh.vertices[i2 + 2],
        ];

        // Calculate normal
        let edge1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
        let edge2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];

        let normal = [
            edge1[1] * edge2[2] - edge1[2] * edge2[1],
            edge1[2] * edge2[0] - edge1[0] * edge2[2],
            edge1[0] * edge2[1] - edge1[1] * edge2[0],
        ];

        // Write normal
        for &n in &normal {
            buffer
                .write_f32::<LittleEndian>(n)
                .map_err(|_| ExportError::ExportFailed {
                    reason: "Failed to write normal".to_string(),
                })?;
        }

        // Write vertices
        for vertex in &[v0, v1, v2] {
            for &coord in vertex {
                buffer
                    .write_f32::<LittleEndian>(coord)
                    .map_err(|_| ExportError::ExportFailed {
                        reason: "Failed to write vertex".to_string(),
                    })?;
            }
        }

        // Write attribute byte count (always 0)
        buffer
            .write_u16::<LittleEndian>(0)
            .map_err(|_| ExportError::ExportFailed {
                reason: "Failed to write attributes".to_string(),
            })?;
    }

    Ok(buffer)
}

#[cfg(test)]
pub fn export_stl_ascii(mesh: &Mesh, name: &str) -> String {
    let mut content = format!("solid {}\n", name);

    for i in (0..mesh.indices.len()).step_by(3) {
        let i0 = mesh.indices[i] as usize * 3;
        let i1 = mesh.indices[i + 1] as usize * 3;
        let i2 = mesh.indices[i + 2] as usize * 3;

        let v0 = [
            mesh.vertices[i0],
            mesh.vertices[i0 + 1],
            mesh.vertices[i0 + 2],
        ];
        let v1 = [
            mesh.vertices[i1],
            mesh.vertices[i1 + 1],
            mesh.vertices[i1 + 2],
        ];
        let v2 = [
            mesh.vertices[i2],
            mesh.vertices[i2 + 1],
            mesh.vertices[i2 + 2],
        ];

        let normal = calculate_triangle_normal(&v0, &v1, &v2);

        content.push_str(&format!(
            "  facet normal {} {} {}\n",
            normal[0], normal[1], normal[2]
        ));
        content.push_str("    outer loop\n");
        content.push_str(&format!("      vertex {} {} {}\n", v0[0], v0[1], v0[2]));
        content.push_str(&format!("      vertex {} {} {}\n", v1[0], v1[1], v1[2]));
        content.push_str(&format!("      vertex {} {} {}\n", v2[0], v2[1], v2[2]));
        content.push_str("    endloop\n");
        content.push_str("  endfacet\n");
    }

    content.push_str(&format!("endsolid {}\n", name));
    content
}

/// Calculate the normal vector for a triangle
///
/// Given three vertices of a triangle, calculates the unit normal vector
/// using the cross product of two edges.
pub fn calculate_triangle_normal(v0: &[f32; 3], v1: &[f32; 3], v2: &[f32; 3]) -> [f32; 3] {
    let edge1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
    let edge2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];

    let mut normal = [
        edge1[1] * edge2[2] - edge1[2] * edge2[1],
        edge1[2] * edge2[0] - edge1[0] * edge2[2],
        edge1[0] * edge2[1] - edge1[1] * edge2[0],
    ];

    let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
    if length > 0.0 {
        normal[0] /= length;
        normal[1] /= length;
        normal[2] /= length;
    }

    normal
}
