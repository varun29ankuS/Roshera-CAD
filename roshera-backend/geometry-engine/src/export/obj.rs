//! Wavefront OBJ file format export
//!
//! Exports B-Rep solids as OBJ files with vertex positions, normals, and face indices.
//! Supports vertex welding for smaller file sizes.

use crate::primitives::{solid::SolidId, topology_builder::BRepModel};
use crate::tessellation::{tessellate_solid, TessellationParams, TriangleMesh};
use std::io::{self, Write};
use std::path::Path;

/// Errors that can occur during OBJ export
#[derive(Debug, thiserror::Error)]
pub enum ObjError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Solid not found: {0:?}")]
    SolidNotFound(SolidId),
    #[error("Empty mesh: tessellation produced no triangles")]
    EmptyMesh,
}

/// Export a solid to an OBJ file
pub fn export_obj(
    model: &BRepModel,
    solid_id: SolidId,
    path: &Path,
    params: &TessellationParams,
) -> Result<(), ObjError> {
    let solid = model
        .solids
        .get(solid_id)
        .ok_or(ObjError::SolidNotFound(solid_id))?;
    let mesh = tessellate_solid(solid, model, params);
    if mesh.triangles.is_empty() {
        return Err(ObjError::EmptyMesh);
    }
    let file = std::fs::File::create(path)?;
    let mut writer = io::BufWriter::new(file);
    write_obj(&mesh, &mut writer)
}

/// Write OBJ format to any writer
pub fn write_obj<W: Write>(mesh: &TriangleMesh, writer: &mut W) -> Result<(), ObjError> {
    writeln!(writer, "# Roshera-CAD OBJ Export")?;
    writeln!(
        writer,
        "# Vertices: {}, Triangles: {}",
        mesh.vertices.len(),
        mesh.triangles.len()
    )?;
    writeln!(writer)?;

    // Write vertex positions
    for v in &mesh.vertices {
        writeln!(writer, "v {} {} {}", v.position.x, v.position.y, v.position.z)?;
    }
    writeln!(writer)?;

    // Write vertex normals
    for v in &mesh.vertices {
        writeln!(writer, "vn {} {} {}", v.normal.x, v.normal.y, v.normal.z)?;
    }
    writeln!(writer)?;

    // Write faces (OBJ uses 1-based indices)
    // Format: f v1//vn1 v2//vn2 v3//vn3
    for tri in &mesh.triangles {
        let i0 = tri[0] + 1;
        let i1 = tri[1] + 1;
        let i2 = tri[2] + 1;
        writeln!(writer, "f {i0}//{i0} {i1}//{i1} {i2}//{i2}")?;
    }

    writer.flush()?;
    Ok(())
}

/// Write OBJ format with vertex welding for smaller files.
/// Merges vertices that are within `tolerance` distance of each other.
pub fn write_obj_welded<W: Write>(
    mesh: &TriangleMesh,
    writer: &mut W,
    tolerance: f64,
) -> Result<(), ObjError> {
    // Build welding map: for each vertex, find its canonical index
    let tol_sq = tolerance * tolerance;
    let n = mesh.vertices.len();
    let mut canonical = vec![0u32; n];
    let mut unique_positions = Vec::new();
    let mut unique_normals = Vec::new();

    for i in 0..n {
        let v = &mesh.vertices[i];
        let mut found = false;

        for (j, upos) in unique_positions.iter().enumerate() {
            let uv: &crate::math::Point3 = upos;
            let dx = v.position.x - uv.x;
            let dy = v.position.y - uv.y;
            let dz = v.position.z - uv.z;
            if dx * dx + dy * dy + dz * dz < tol_sq {
                canonical[i] = j as u32;
                found = true;
                break;
            }
        }

        if !found {
            canonical[i] = unique_positions.len() as u32;
            unique_positions.push(v.position);
            unique_normals.push(v.normal);
        }
    }

    writeln!(writer, "# Roshera-CAD OBJ Export (welded)")?;
    writeln!(
        writer,
        "# Unique vertices: {}, Triangles: {}",
        unique_positions.len(),
        mesh.triangles.len()
    )?;
    writeln!(writer)?;

    for p in &unique_positions {
        writeln!(writer, "v {} {} {}", p.x, p.y, p.z)?;
    }
    writeln!(writer)?;

    for n in &unique_normals {
        writeln!(writer, "vn {} {} {}", n.x, n.y, n.z)?;
    }
    writeln!(writer)?;

    for tri in &mesh.triangles {
        let i0 = canonical[tri[0] as usize] + 1;
        let i1 = canonical[tri[1] as usize] + 1;
        let i2 = canonical[tri[2] as usize] + 1;

        // Skip degenerate triangles after welding
        if i0 == i1 || i1 == i2 || i0 == i2 {
            continue;
        }

        writeln!(writer, "f {i0}//{i0} {i1}//{i1} {i2}//{i2}")?;
    }

    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Point3, Vector3};
    use crate::tessellation::mesh::MeshVertex;

    fn make_box_mesh() -> TriangleMesh {
        let mut mesh = TriangleMesh::new();
        let hw = 5.0;

        let faces: &[([f64; 3], [f64; 3], [f64; 3], [f64; 3], [f64; 3])] = &[
            ([-hw, -hw, hw], [hw, -hw, hw], [hw, hw, hw], [-hw, hw, hw], [0.0, 0.0, 1.0]),
            ([hw, -hw, -hw], [-hw, -hw, -hw], [-hw, hw, -hw], [hw, hw, -hw], [0.0, 0.0, -1.0]),
            ([-hw, hw, hw], [hw, hw, hw], [hw, hw, -hw], [-hw, hw, -hw], [0.0, 1.0, 0.0]),
            ([-hw, -hw, -hw], [hw, -hw, -hw], [hw, -hw, hw], [-hw, -hw, hw], [0.0, -1.0, 0.0]),
            ([hw, -hw, hw], [hw, -hw, -hw], [hw, hw, -hw], [hw, hw, hw], [1.0, 0.0, 0.0]),
            ([-hw, -hw, -hw], [-hw, -hw, hw], [-hw, hw, hw], [-hw, hw, -hw], [-1.0, 0.0, 0.0]),
        ];

        for &(p0, p1, p2, p3, n) in faces {
            let normal = Vector3::new(n[0], n[1], n[2]);
            let i0 = mesh.add_vertex(MeshVertex { position: Point3::new(p0[0], p0[1], p0[2]), normal, uv: None });
            let i1 = mesh.add_vertex(MeshVertex { position: Point3::new(p1[0], p1[1], p1[2]), normal, uv: None });
            let i2 = mesh.add_vertex(MeshVertex { position: Point3::new(p2[0], p2[1], p2[2]), normal, uv: None });
            let i3 = mesh.add_vertex(MeshVertex { position: Point3::new(p3[0], p3[1], p3[2]), normal, uv: None });
            mesh.add_triangle(i0, i1, i2);
            mesh.add_triangle(i0, i2, i3);
        }

        mesh
    }

    #[test]
    fn test_obj_format() {
        let mesh = make_box_mesh();
        let mut buf = Vec::new();
        write_obj(&mesh, &mut buf).unwrap();

        let output = String::from_utf8(buf).unwrap();

        let v_count = output.lines().filter(|l| l.starts_with("v ")).count();
        assert_eq!(v_count, 24, "Box mesh has 24 vertices (4 per face)");

        let vn_count = output.lines().filter(|l| l.starts_with("vn ")).count();
        assert_eq!(v_count, vn_count, "Vertex and normal counts should match");

        let f_count = output.lines().filter(|l| l.starts_with("f ")).count();
        assert_eq!(f_count, 12, "Box should have 12 triangle faces");

        // Verify face indices are 1-based
        for line in output.lines().filter(|l| l.starts_with("f ")) {
            for part in line.split_whitespace().skip(1) {
                let idx: u32 = part.split("//").next().unwrap().parse().unwrap();
                assert!(idx >= 1, "OBJ indices must be 1-based, got {idx}");
                assert!(
                    idx <= v_count as u32,
                    "Index {idx} out of range (max {v_count})"
                );
            }
        }
    }

    #[test]
    fn test_obj_welded_reduces_vertices() {
        let mesh = make_box_mesh();
        let original_verts = mesh.vertices.len();
        assert_eq!(original_verts, 24);

        let mut buf = Vec::new();
        write_obj_welded(&mesh, &mut buf, 0.001).unwrap();

        let output = String::from_utf8(buf).unwrap();
        let welded_verts = output.lines().filter(|l| l.starts_with("v ")).count();

        // 24 vertices → 8 unique positions (4 per face, 3 faces share each corner)
        assert_eq!(welded_verts, 8, "Box should weld to 8 unique vertices");
    }
}
