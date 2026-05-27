//! File format modules

pub mod obj;
pub mod ros;
pub mod ros_snapshot;
pub mod step;
pub mod stl;
pub mod timeline_chunk;

use shared_types::{ExportError, Mesh};

/// Hard cap on vertex count for STL/OBJ export.
///
/// 15,000,000 vertices × 12 bytes (3 × f32) ≈ 180 MB of input, well
/// above any realistic CAD tessellation density. Exceeding this is
/// almost certainly a malformed mesh, not a legitimate export.
pub(crate) const MAX_MESH_VERTICES: usize = 15_000_000;

/// Hard cap on triangle count for STL/OBJ export.
///
/// 5,000,000 triangles × 50 bytes per binary-STL record ≈ 250 MB
/// output. Bounded so a runaway tessellation or adversarial input
/// cannot exhaust memory inside the serializer's `Vec` allocation.
pub(crate) const MAX_MESH_TRIANGLES: usize = 5_000_000;

/// AUDIT-H12: Validate a [`Mesh`] is structurally well-formed and within
/// the export-engine's size budget. Returns the triangle count on
/// success.
///
/// This is the single gate between caller-supplied mesh data and the
/// STL / OBJ serializers, which otherwise rely on implicit `Vec`
/// indexing invariants. Without it, a `Mesh` with
/// `vertices.len() % 3 != 0` or with an `indices[i] * 3 >= vertices.len()`
/// would panic inside the triangle loop (denied by workspace lint
/// policy, and a 500-class failure mode at the engine boundary).
///
/// Checks performed:
/// 1. `vertices.len()` is a multiple of 3.
/// 2. `indices.len()` is a multiple of 3.
/// 3. `normals.len() == vertices.len()` when normals are present.
/// 4. Vertex count ≤ [`MAX_MESH_VERTICES`].
/// 5. Triangle count ≤ [`MAX_MESH_TRIANGLES`].
/// 6. Every index < `vertex_count` (no out-of-bounds vertex access
///    will occur in the serializer).
pub(crate) fn validate_mesh_for_export(mesh: &Mesh) -> Result<usize, ExportError> {
    if mesh.vertices.len() % 3 != 0 {
        return Err(ExportError::ExportFailed {
            reason: format!(
                "mesh.vertices.len() ({}) is not a multiple of 3",
                mesh.vertices.len()
            ),
        });
    }
    if mesh.indices.len() % 3 != 0 {
        return Err(ExportError::ExportFailed {
            reason: format!(
                "mesh.indices.len() ({}) is not a multiple of 3",
                mesh.indices.len()
            ),
        });
    }
    if !mesh.normals.is_empty() && mesh.normals.len() != mesh.vertices.len() {
        return Err(ExportError::ExportFailed {
            reason: format!(
                "mesh.normals.len() ({}) must equal mesh.vertices.len() ({}) \
                 when normals are present",
                mesh.normals.len(),
                mesh.vertices.len()
            ),
        });
    }
    let vertex_count = mesh.vertices.len() / 3;
    if vertex_count > MAX_MESH_VERTICES {
        return Err(ExportError::ExportFailed {
            reason: format!(
                "mesh vertex count {vertex_count} exceeds MAX_MESH_VERTICES={MAX_MESH_VERTICES}"
            ),
        });
    }
    let triangle_count = mesh.indices.len() / 3;
    if triangle_count > MAX_MESH_TRIANGLES {
        return Err(ExportError::ExportFailed {
            reason: format!(
                "mesh triangle count {triangle_count} exceeds \
                 MAX_MESH_TRIANGLES={MAX_MESH_TRIANGLES}"
            ),
        });
    }
    // O(indices.len()) but each access is already O(1) per index in the
    // serializer hot loop — this is the same work, hoisted upfront so a
    // bad index surfaces as a typed `ExportError::ExportFailed` rather
    // than an index-out-of-bounds panic (denied by workspace lints).
    for (k, &idx) in mesh.indices.iter().enumerate() {
        if (idx as usize) >= vertex_count {
            return Err(ExportError::ExportFailed {
                reason: format!(
                    "mesh.indices[{k}] = {idx} is out of bounds for vertex_count={vertex_count}"
                ),
            });
        }
    }
    Ok(triangle_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_mesh() -> Mesh {
        Mesh {
            vertices: Vec::new(),
            indices: Vec::new(),
            normals: Vec::new(),
            uvs: None,
            colors: None,
            face_map: None,
        }
    }

    fn unit_triangle_mesh() -> Mesh {
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
    fn empty_mesh_validates_with_zero_triangles() {
        let m = empty_mesh();
        assert_eq!(validate_mesh_for_export(&m).unwrap(), 0);
    }

    #[test]
    fn unit_triangle_validates() {
        let m = unit_triangle_mesh();
        assert_eq!(validate_mesh_for_export(&m).unwrap(), 1);
    }

    #[test]
    fn vertices_len_not_multiple_of_3_rejected() {
        let mut m = unit_triangle_mesh();
        m.vertices.pop(); // 8 floats — not a multiple of 3
        let err = validate_mesh_for_export(&m).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("vertices"), "got: {msg}");
        assert!(msg.contains("multiple of 3"), "got: {msg}");
    }

    #[test]
    fn indices_len_not_multiple_of_3_rejected() {
        let mut m = unit_triangle_mesh();
        m.indices.pop(); // 2 indices — not a multiple of 3
        let err = validate_mesh_for_export(&m).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("indices"), "got: {msg}");
        assert!(msg.contains("multiple of 3"), "got: {msg}");
    }

    #[test]
    fn normals_length_mismatch_rejected() {
        let mut m = unit_triangle_mesh();
        m.normals = vec![0.0, 0.0, 1.0]; // 3 floats, vertices has 9
        let err = validate_mesh_for_export(&m).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("normals"), "got: {msg}");
    }

    #[test]
    fn normals_matching_length_accepted() {
        let mut m = unit_triangle_mesh();
        m.normals = vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0];
        assert_eq!(validate_mesh_for_export(&m).unwrap(), 1);
    }

    #[test]
    fn index_out_of_bounds_rejected() {
        let mut m = unit_triangle_mesh();
        m.indices = vec![0, 1, 99]; // 99 ≥ vertex_count (3)
        let err = validate_mesh_for_export(&m).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("out of bounds"), "got: {msg}");
        assert!(msg.contains("99"), "got: {msg}");
    }

    #[test]
    fn vertex_cap_boundary_accepts_exact() {
        // The cap itself is inclusive (≤, not <). Build a vertex array
        // exactly at the cap with no indices so the per-index loop is
        // O(0); this keeps the test cheap.
        let m = Mesh {
            vertices: vec![0.0; MAX_MESH_VERTICES * 3],
            indices: Vec::new(),
            normals: Vec::new(),
            uvs: None,
            colors: None,
            face_map: None,
        };
        assert_eq!(validate_mesh_for_export(&m).unwrap(), 0);
    }

    #[test]
    fn vertex_cap_exceeded_rejected() {
        // Same shape as above but one vertex over the cap.
        let m = Mesh {
            vertices: vec![0.0; (MAX_MESH_VERTICES + 1) * 3],
            indices: Vec::new(),
            normals: Vec::new(),
            uvs: None,
            colors: None,
            face_map: None,
        };
        let err = validate_mesh_for_export(&m).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("vertex count"), "got: {msg}");
        assert!(msg.contains("MAX_MESH_VERTICES"), "got: {msg}");
    }

    #[test]
    fn triangle_cap_exceeded_rejected() {
        // 3 valid vertices + indices long enough to exceed the triangle
        // cap, all pointing at vertex 0 (so the per-index bounds check
        // is satisfied and we hit the triangle-count gate first).
        let m = Mesh {
            vertices: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            indices: vec![0u32; (MAX_MESH_TRIANGLES + 1) * 3],
            normals: Vec::new(),
            uvs: None,
            colors: None,
            face_map: None,
        };
        let err = validate_mesh_for_export(&m).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("triangle count"), "got: {msg}");
        assert!(msg.contains("MAX_MESH_TRIANGLES"), "got: {msg}");
    }
}
