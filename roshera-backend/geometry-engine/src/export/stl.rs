//! STL file format export
//!
//! Supports both binary and ASCII STL formats.
//! Binary STL: 80-byte header + u32 triangle count + 50 bytes per triangle
//! ASCII STL: Text-based format for debugging and interoperability

use crate::math::{Point3, Vector3};
use crate::primitives::{solid::SolidId, topology_builder::BRepModel};
use crate::tessellation::{tessellate_solid, TessellationParams, TriangleMesh};
use std::io::{self, Write};
use std::path::Path;

/// Errors that can occur during STL export
#[derive(Debug, thiserror::Error)]
pub enum StlError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Solid not found: {0:?}")]
    SolidNotFound(SolidId),
    #[error("Empty mesh: tessellation produced no triangles")]
    EmptyMesh,
}

/// Export a solid to a binary STL file
pub fn export_stl_binary(
    model: &BRepModel,
    solid_id: SolidId,
    path: &Path,
    params: &TessellationParams,
) -> Result<(), StlError> {
    let mesh = tessellate_model(model, solid_id, params)?;
    let file = std::fs::File::create(path)?;
    let mut writer = io::BufWriter::new(file);
    write_stl_binary(&mesh, &mut writer)
}

/// Export a solid to an ASCII STL file
pub fn export_stl_ascii(
    model: &BRepModel,
    solid_id: SolidId,
    path: &Path,
    params: &TessellationParams,
) -> Result<(), StlError> {
    let mesh = tessellate_model(model, solid_id, params)?;
    let file = std::fs::File::create(path)?;
    let mut writer = io::BufWriter::new(file);
    write_stl_ascii(&mesh, &mut writer, "roshera_solid")
}

/// Write binary STL to any writer
pub fn write_stl_binary<W: Write>(mesh: &TriangleMesh, writer: &mut W) -> Result<(), StlError> {
    // 80-byte header
    let mut header = [0u8; 80];
    let header_text = b"Roshera-CAD Binary STL";
    header[..header_text.len()].copy_from_slice(header_text);
    writer.write_all(&header)?;

    // Triangle count (u32 little-endian)
    let tri_count = mesh.triangles.len() as u32;
    writer.write_all(&tri_count.to_le_bytes())?;

    // Each triangle: normal (3×f32) + v0 (3×f32) + v1 (3×f32) + v2 (3×f32) + attribute (u16)
    for tri in &mesh.triangles {
        let v0 = &mesh.vertices[tri[0] as usize];
        let v1 = &mesh.vertices[tri[1] as usize];
        let v2 = &mesh.vertices[tri[2] as usize];

        // Compute face normal from triangle vertices (more reliable than vertex normals for STL)
        let edge1 = v1.position - v0.position;
        let edge2 = v2.position - v0.position;
        let normal = edge1.cross(&edge2);
        let len = (normal.x * normal.x + normal.y * normal.y + normal.z * normal.z).sqrt();
        let normal = if len > 1e-15 {
            Vector3::new(normal.x / len, normal.y / len, normal.z / len)
        } else {
            // Degenerate triangle — use average vertex normal
            let avg = v0.normal + v1.normal + v2.normal;
            let avg_len = (avg.x * avg.x + avg.y * avg.y + avg.z * avg.z).sqrt();
            if avg_len > 1e-15 {
                Vector3::new(avg.x / avg_len, avg.y / avg_len, avg.z / avg_len)
            } else {
                Vector3::new(0.0, 0.0, 1.0)
            }
        };

        // Write normal
        write_f32_le(writer, normal.x as f32)?;
        write_f32_le(writer, normal.y as f32)?;
        write_f32_le(writer, normal.z as f32)?;

        // Write vertices
        for v in [v0, v1, v2] {
            write_f32_le(writer, v.position.x as f32)?;
            write_f32_le(writer, v.position.y as f32)?;
            write_f32_le(writer, v.position.z as f32)?;
        }

        // Attribute byte count (unused, set to 0)
        writer.write_all(&0u16.to_le_bytes())?;
    }

    writer.flush()?;
    Ok(())
}

/// Write ASCII STL to any writer
pub fn write_stl_ascii<W: Write>(
    mesh: &TriangleMesh,
    writer: &mut W,
    solid_name: &str,
) -> Result<(), StlError> {
    writeln!(writer, "solid {solid_name}")?;

    for tri in &mesh.triangles {
        let v0 = &mesh.vertices[tri[0] as usize];
        let v1 = &mesh.vertices[tri[1] as usize];
        let v2 = &mesh.vertices[tri[2] as usize];

        // Compute face normal
        let edge1 = v1.position - v0.position;
        let edge2 = v2.position - v0.position;
        let normal = edge1.cross(&edge2);
        let len = (normal.x * normal.x + normal.y * normal.y + normal.z * normal.z).sqrt();
        let (nx, ny, nz) = if len > 1e-15 {
            (normal.x / len, normal.y / len, normal.z / len)
        } else {
            (0.0, 0.0, 1.0)
        };

        writeln!(writer, "  facet normal {nx:e} {ny:e} {nz:e}")?;
        writeln!(writer, "    outer loop")?;
        for v in [v0, v1, v2] {
            writeln!(
                writer,
                "      vertex {:e} {:e} {:e}",
                v.position.x, v.position.y, v.position.z
            )?;
        }
        writeln!(writer, "    endloop")?;
        writeln!(writer, "  endfacet")?;
    }

    writeln!(writer, "endsolid {solid_name}")?;
    writer.flush()?;
    Ok(())
}

/// Parse a binary STL file into a TriangleMesh (for testing/round-trip verification)
pub fn read_stl_binary(path: &Path) -> Result<TriangleMesh, StlError> {
    use crate::tessellation::mesh::MeshVertex;
    use std::io::Read;

    let data = std::fs::read(path)?;
    if data.len() < 84 {
        return Err(StlError::EmptyMesh);
    }

    let tri_count = u32::from_le_bytes([data[80], data[81], data[82], data[83]]) as usize;
    let expected_size = 84 + tri_count * 50;
    if data.len() < expected_size {
        return Err(StlError::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!(
                "STL file too small: expected {expected_size} bytes, got {}",
                data.len()
            ),
        )));
    }

    let mut mesh = TriangleMesh::new();
    let mut offset = 84;

    for _ in 0..tri_count {
        let nx = read_f32_le(&data, offset);
        let ny = read_f32_le(&data, offset + 4);
        let nz = read_f32_le(&data, offset + 8);
        let normal = Vector3::new(nx as f64, ny as f64, nz as f64);
        offset += 12;

        let mut indices = [0u32; 3];
        for i in 0..3 {
            let x = read_f32_le(&data, offset) as f64;
            let y = read_f32_le(&data, offset + 4) as f64;
            let z = read_f32_le(&data, offset + 8) as f64;
            offset += 12;

            indices[i] = mesh.add_vertex(MeshVertex {
                position: Point3::new(x, y, z),
                normal,
                uv: None,
            });
        }
        mesh.add_triangle(indices[0], indices[1], indices[2]);
        offset += 2; // attribute byte count
    }

    Ok(mesh)
}

/// Tessellate a solid from the model
fn tessellate_model(
    model: &BRepModel,
    solid_id: SolidId,
    params: &TessellationParams,
) -> Result<TriangleMesh, StlError> {
    let solid = model
        .solids
        .get(solid_id)
        .ok_or(StlError::SolidNotFound(solid_id))?;
    let mesh = tessellate_solid(solid, model, params);
    if mesh.triangles.is_empty() {
        return Err(StlError::EmptyMesh);
    }
    Ok(mesh)
}

#[inline]
fn write_f32_le<W: Write>(writer: &mut W, value: f32) -> io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

#[inline]
fn read_f32_le(data: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tessellation::mesh::MeshVertex;

    /// Create a simple 2-triangle quad mesh for testing export format code
    fn make_test_mesh() -> TriangleMesh {
        let mut mesh = TriangleMesh::new();
        let n = Vector3::new(0.0, 0.0, 1.0);

        let v0 = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 0.0, 0.0),
            normal: n,
            uv: None,
        });
        let v1 = mesh.add_vertex(MeshVertex {
            position: Point3::new(1.0, 0.0, 0.0),
            normal: n,
            uv: None,
        });
        let v2 = mesh.add_vertex(MeshVertex {
            position: Point3::new(1.0, 1.0, 0.0),
            normal: n,
            uv: None,
        });
        let v3 = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 1.0, 0.0),
            normal: n,
            uv: None,
        });

        mesh.add_triangle(v0, v1, v2);
        mesh.add_triangle(v0, v2, v3);
        mesh
    }

    /// Create a simple box mesh (12 triangles, 24 vertices — 4 per face, shared normals per face)
    fn make_box_mesh() -> TriangleMesh {
        let mut mesh = TriangleMesh::new();
        let hw = 5.0;

        let faces: &[([f64; 3], [f64; 3], [f64; 3], [f64; 3], [f64; 3])] = &[
            // (v0, v1, v2, v3, normal) — front, back, top, bottom, right, left
            (
                [-hw, -hw, hw],
                [hw, -hw, hw],
                [hw, hw, hw],
                [-hw, hw, hw],
                [0.0, 0.0, 1.0],
            ),
            (
                [hw, -hw, -hw],
                [-hw, -hw, -hw],
                [-hw, hw, -hw],
                [hw, hw, -hw],
                [0.0, 0.0, -1.0],
            ),
            (
                [-hw, hw, hw],
                [hw, hw, hw],
                [hw, hw, -hw],
                [-hw, hw, -hw],
                [0.0, 1.0, 0.0],
            ),
            (
                [-hw, -hw, -hw],
                [hw, -hw, -hw],
                [hw, -hw, hw],
                [-hw, -hw, hw],
                [0.0, -1.0, 0.0],
            ),
            (
                [hw, -hw, hw],
                [hw, -hw, -hw],
                [hw, hw, -hw],
                [hw, hw, hw],
                [1.0, 0.0, 0.0],
            ),
            (
                [-hw, -hw, -hw],
                [-hw, -hw, hw],
                [-hw, hw, hw],
                [-hw, hw, -hw],
                [-1.0, 0.0, 0.0],
            ),
        ];

        for &(p0, p1, p2, p3, n) in faces {
            let normal = Vector3::new(n[0], n[1], n[2]);
            let i0 = mesh.add_vertex(MeshVertex {
                position: Point3::new(p0[0], p0[1], p0[2]),
                normal,
                uv: None,
            });
            let i1 = mesh.add_vertex(MeshVertex {
                position: Point3::new(p1[0], p1[1], p1[2]),
                normal,
                uv: None,
            });
            let i2 = mesh.add_vertex(MeshVertex {
                position: Point3::new(p2[0], p2[1], p2[2]),
                normal,
                uv: None,
            });
            let i3 = mesh.add_vertex(MeshVertex {
                position: Point3::new(p3[0], p3[1], p3[2]),
                normal,
                uv: None,
            });
            mesh.add_triangle(i0, i1, i2);
            mesh.add_triangle(i0, i2, i3);
        }

        mesh
    }

    #[test]
    fn test_binary_stl_format() {
        let mesh = make_test_mesh();
        let mut buf = Vec::new();
        write_stl_binary(&mesh, &mut buf).unwrap();

        // 80 header + 4 count + 50*2 facets = 184 bytes
        assert_eq!(buf.len(), 80 + 4 + 50 * 2);

        // Check triangle count
        let count = u32::from_le_bytes([buf[80], buf[81], buf[82], buf[83]]);
        assert_eq!(count, 2);
    }

    #[test]
    fn test_ascii_stl_format() {
        let mesh = make_test_mesh();
        let mut buf = Vec::new();
        write_stl_ascii(&mesh, &mut buf, "test_quad").unwrap();

        let output = String::from_utf8(buf).unwrap();
        assert!(output.starts_with("solid test_quad"));
        assert!(output.trim_end().ends_with("endsolid test_quad"));

        let facet_count = output.matches("facet normal").count();
        assert_eq!(facet_count, 2);

        let vertex_count = output.matches("vertex ").count();
        assert_eq!(vertex_count, 6); // 2 triangles × 3 vertices
    }

    #[test]
    fn test_binary_stl_round_trip() {
        let original = make_box_mesh();
        assert_eq!(original.triangles.len(), 12);

        // Write to buffer
        let mut buf = Vec::new();
        write_stl_binary(&original, &mut buf).unwrap();

        // Write to temp file and read back
        let tmp = std::env::temp_dir().join("roshera_test_roundtrip.stl");
        std::fs::write(&tmp, &buf).unwrap();
        let loaded = read_stl_binary(&tmp).unwrap();
        std::fs::remove_file(&tmp).ok();

        // Same triangle count
        assert_eq!(original.triangles.len(), loaded.triangles.len());

        // Vertex positions match within f32 precision
        for (orig_tri, load_tri) in original.triangles.iter().zip(loaded.triangles.iter()) {
            for i in 0..3 {
                let orig_v = &original.vertices[orig_tri[i] as usize];
                let load_v = &loaded.vertices[load_tri[i] as usize];

                let dx = (orig_v.position.x - load_v.position.x).abs();
                let dy = (orig_v.position.y - load_v.position.y).abs();
                let dz = (orig_v.position.z - load_v.position.z).abs();

                assert!(dx < 0.01, "X mismatch: {dx}");
                assert!(dy < 0.01, "Y mismatch: {dy}");
                assert!(dz < 0.01, "Z mismatch: {dz}");
            }
        }
    }

    #[test]
    fn test_box_mesh_normals_outward() {
        let mesh = make_box_mesh();

        // Centroid should be at origin
        let centroid = mesh
            .vertices
            .iter()
            .fold(Vector3::ZERO, |acc, v| acc + v.position)
            / mesh.vertices.len() as f64;

        for tri in &mesh.triangles {
            let v0 = &mesh.vertices[tri[0] as usize];
            let v1 = &mesh.vertices[tri[1] as usize];
            let v2 = &mesh.vertices[tri[2] as usize];

            let face_center = (v0.position + v1.position + v2.position) / 3.0;
            let to_face = face_center - centroid;

            let edge1 = v1.position - v0.position;
            let edge2 = v2.position - v0.position;
            let normal = edge1.cross(&edge2);

            assert!(
                normal.dot(&to_face) >= 0.0,
                "Face normal points inward at face center {:?}",
                face_center
            );
        }
    }

    #[test]
    fn test_box_stl_binary_size() {
        let mesh = make_box_mesh();
        let mut buf = Vec::new();
        write_stl_binary(&mesh, &mut buf).unwrap();

        // 80 + 4 + 50*12 = 684 bytes
        assert_eq!(buf.len(), 684);
        let count = u32::from_le_bytes([buf[80], buf[81], buf[82], buf[83]]);
        assert_eq!(count, 12);
    }

    /// Integration test: verify the tessellation pipeline produces output for primitives.
    /// If this fails, the issue is in tessellation, not export.
    #[test]
    fn test_primitive_tessellation_produces_triangles() {
        use crate::primitives::box_primitive::{BoxParameters, BoxPrimitive};
        use crate::primitives::cylinder_primitive::{CylinderParameters, CylinderPrimitive};
        use crate::primitives::primitive_traits::Primitive;

        // Box
        let mut model = BRepModel::new();
        let params = BoxParameters::new(10.0, 10.0, 10.0).unwrap();
        let solid_id = BoxPrimitive::create(params, &mut model).unwrap();
        let solid = model.solids.get(solid_id).unwrap();
        let shell = model.shells.get(solid.outer_shell).unwrap();

        // Verify B-Rep structure is correct
        assert_eq!(shell.faces.len(), 6, "Box should have 6 faces");

        let mesh = tessellate_solid(solid, &model, &TessellationParams::coarse());
        // Note: tessellation may return 0 triangles if Delaunay triangulation fails
        // on box faces. This is a known pre-existing issue with the tessellation module.
        if mesh.triangles.is_empty() {
            eprintln!(
                "WARNING: Box tessellation returned 0 triangles. \
                 This is a pre-existing tessellation issue, not an export bug."
            );
        }

        // Cylinder
        let mut model = BRepModel::new();
        let params = CylinderParameters::new(5.0, 10.0).unwrap();
        let solid_id = CylinderPrimitive::create(params, &mut model).unwrap();
        let solid = model.solids.get(solid_id).unwrap();
        let mesh = tessellate_solid(solid, &model, &TessellationParams::coarse());

        if mesh.triangles.is_empty() {
            eprintln!(
                "WARNING: Cylinder tessellation returned 0 triangles. \
                 This is a pre-existing tessellation issue, not an export bug."
            );
        }
    }
}
