//! STL file format support

use byteorder::{LittleEndian, WriteBytesExt};
use shared_types::*;
use std::io::Write;

use crate::formats::validate_mesh_for_export;

/// Generate binary STL content
pub fn generate_binary_stl(mesh: &Mesh, name: &str) -> Result<Vec<u8>, ExportError> {
    // AUDIT-H12: Validate mesh shape + size bounds before allocating /
    // iterating. Catches non-multiple-of-3 vertex or index arrays,
    // out-of-bounds indices, and oversized meshes as typed errors
    // instead of letting them surface as in-loop panics.
    let triangle_count_usize = validate_mesh_for_export(mesh)?;

    let mut buffer = Vec::new();

    // Write 80-byte header.
    //
    // AUDIT-L: binary STL fixes the header at exactly 80 bytes (per the
    // de-facto spec — see e.g. <https://www.fabbers.com/tech/STL_Format>).
    // `resize(80, 0)` will silently *truncate* any prefix longer than 80
    // bytes. The `"Binary STL - "` prefix consumes 13 bytes, leaving 67
    // bytes for the caller-supplied `name`. Warn (rather than fail) when
    // the formatted header overflows so callers see the loss in logs
    // without us synthesising an error on what is, contractually, a
    // valid binary STL.
    let header = format!("Binary STL - {}", name);
    let header_bytes_raw = header.as_bytes();
    if header_bytes_raw.len() > 80 {
        tracing::warn!(
            requested_len = header_bytes_raw.len(),
            name_len = name.len(),
            "STL binary header exceeds the 80-byte spec limit; \
             truncating (lost {} bytes from caller-supplied name)",
            header_bytes_raw.len() - 80
        );
    }
    let mut header_bytes = header_bytes_raw.to_vec();
    header_bytes.resize(80, 0);
    buffer
        .write_all(&header_bytes)
        .map_err(|_| ExportError::ExportFailed {
            reason: "Failed to write header".to_string(),
        })?;

    // Write triangle count. Bounded above by validate_mesh_for_export
    // to MAX_MESH_TRIANGLES (= 5,000,000), which fits comfortably in u32.
    let triangle_count = triangle_count_usize as u32;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_triangle() -> Mesh {
        Mesh {
            vertices: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            indices: vec![0, 1, 2],
            normals: Vec::new(),
            uvs: None,
            colors: None,
            face_map: None,
        }
    }

    #[test]
    fn generate_binary_stl_happy_path() {
        let bytes = generate_binary_stl(&unit_triangle(), "t").expect("valid mesh exports");
        // 80-byte header + 4-byte tri count + 50-byte triangle record.
        assert_eq!(bytes.len(), 80 + 4 + 50);
    }

    #[test]
    fn generate_binary_stl_rejects_oob_index() {
        let mut m = unit_triangle();
        m.indices = vec![0, 1, 7]; // 7 ≥ vertex_count (3)
        let err = generate_binary_stl(&m, "t").unwrap_err();
        assert!(format!("{err}").contains("out of bounds"));
    }

    #[test]
    fn generate_binary_stl_rejects_non_multiple_of_3_indices() {
        let mut m = unit_triangle();
        m.indices = vec![0, 1]; // length 2
        let err = generate_binary_stl(&m, "t").unwrap_err();
        assert!(format!("{err}").contains("indices"));
    }
}
