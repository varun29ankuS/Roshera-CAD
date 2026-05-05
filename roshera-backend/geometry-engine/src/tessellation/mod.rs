//! B-Rep tessellation module
//!
//! Converts analytical B-Rep models to triangle meshes for visualization and export.

pub mod adaptive;
pub mod cache;
pub mod curve;
pub mod mesh;
pub mod parallel;
pub mod simple_box;
pub mod surface;

// Re-export main types
pub use adaptive::AdaptiveTessellator;
pub use curve::{tessellate_curve, tessellate_edge};
pub use mesh::{MeshVertex, ThreeJsMesh, TriangleMesh};
pub use surface::{tessellate_face, tessellate_surface};

use crate::primitives::{builder::BRepModel, shell::Shell, solid::Solid};

/// Tessellation parameters for controlling mesh quality
#[derive(Debug, Clone)]
pub struct TessellationParams {
    /// Maximum edge length in the mesh
    pub max_edge_length: f64,
    /// Maximum angle deviation from true surface (radians)
    pub max_angle_deviation: f64,
    /// Maximum distance from chord to curve
    pub chord_tolerance: f64,
    /// Minimum number of segments for curves
    pub min_segments: usize,
    /// Maximum number of segments for curves
    pub max_segments: usize,
}

impl Default for TessellationParams {
    fn default() -> Self {
        Self {
            max_edge_length: 0.1,
            max_angle_deviation: 0.1,
            chord_tolerance: 0.001,
            min_segments: 3,
            max_segments: 100,
        }
    }
}

impl TessellationParams {
    /// Create parameters for coarse tessellation (preview quality)
    pub fn coarse() -> Self {
        Self {
            max_edge_length: 0.5,
            max_angle_deviation: 0.3,
            chord_tolerance: 0.01,
            min_segments: 3,
            max_segments: 20,
        }
    }

    /// Create parameters for fine tessellation (high quality)
    pub fn fine() -> Self {
        Self {
            max_edge_length: 0.01,
            max_angle_deviation: 0.02,
            chord_tolerance: 0.0001,
            min_segments: 8,
            max_segments: 200,
        }
    }

    /// Create parameters for ultra-fast real-time preview
    pub fn realtime() -> Self {
        Self {
            max_edge_length: 1.0,
            max_angle_deviation: 0.5,
            chord_tolerance: 0.1,
            min_segments: 3,
            max_segments: 8, // Very low for speed
        }
    }
}

/// Tessellate a solid into a triangle mesh
pub fn tessellate_solid(
    solid: &Solid,
    model: &BRepModel,
    params: &TessellationParams,
) -> TriangleMesh {
    let mut mesh = TriangleMesh::new();

    // Tessellate outer shell
    if let Some(shell) = model.shells.get(solid.outer_shell) {
        tessellate_shell(shell, model, params, &mut mesh);
    }

    // Tessellate inner shells (voids)
    for &inner_shell_id in &solid.inner_shells {
        if let Some(shell) = model.shells.get(inner_shell_id) {
            tessellate_shell(shell, model, params, &mut mesh);
        }
    }

    mesh
}

/// Tessellate a shell and append to existing mesh.
/// Populates `mesh.face_map` so each triangle maps back to its B-Rep FaceId.
///
/// After all faces are tessellated, runs a vertex-welding pass so that
/// adjacent faces sharing a B-Rep edge collapse their 3D-coincident
/// boundary samples to a single mesh vertex. Without this pass the
/// resulting mesh has duplicate vertex indices on every shared edge —
/// which is invisible to the renderer but makes the mesh non-watertight
/// to STL export, BVH builders, and any topological analysis downstream.
///
/// Per-edge sampling in `surface::sample_loop_3d_polygon` is symmetric
/// across face traversal direction (both forward and reverse traversals
/// produce the same set of N+1 parameter values along the shared edge,
/// hence the same 3D points), so the welding pass collapses them
/// deterministically.
///
/// Welding tolerance is derived from the requested chord tolerance
/// floored at 1e-9; this matches the kernel's geometric tolerance
/// regime used by `Tolerance::from_distance` callers.
pub fn tessellate_shell(
    shell: &Shell,
    model: &BRepModel,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) {
    let weld_start_vertices = mesh.vertices.len();
    let weld_start_triangles = mesh.triangles.len();
    for &face_id in &shell.faces {
        if let Some(face) = model.faces.get(face_id) {
            let tri_start = mesh.triangles.len();
            surface::tessellate_face(face, model, params, mesh);
            let tri_end = mesh.triangles.len();
            // Record which B-Rep face each new triangle came from
            for _ in tri_start..tri_end {
                mesh.face_map.push(face_id);
            }
        }
    }

    // Run welding only over the vertices/triangles produced by THIS
    // shell call so that callers passing a pre-populated mesh (e.g.
    // multi-shell solids in `tessellate_solid`) don't have their
    // earlier shell's vertex indices invalidated.
    if mesh.vertices.len() > weld_start_vertices && mesh.triangles.len() > weld_start_triangles {
        surface::weld_mesh_watertight_range(
            mesh,
            params.chord_tolerance,
            weld_start_vertices,
            weld_start_triangles,
        );
    }
}

#[cfg(test)]
mod watertight_tests {
    //! Watertightness regression tests for the tessellation pipeline.
    //!
    //! A triangle mesh is **watertight** (in the manifold-edge sense)
    //! iff every undirected edge `{i, j}` is shared by exactly two
    //! triangles. Equivalently, every triangle edge has a unique
    //! "twin" on an adjacent triangle. Open boundaries (count = 1)
    //! mean the surface has a hole; non-manifold edges (count ≥ 3)
    //! mean three or more triangles meet at the same edge, which
    //! breaks downstream STL/CSG/BVH consumers.
    //!
    //! These tests exercise the box primitive (6 planar faces, 12
    //! shared B-Rep edges) end-to-end through `tessellate_solid`. Box
    //! tessellation is the smallest non-trivial test of shared-edge
    //! coherence: each B-Rep edge is shared by exactly two faces, so
    //! the fix in `tessellate_shell` (vertex welding) is the only
    //! thing standing between this test passing and failing.
    use super::*;
    use crate::math::Point3;
    use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
    use std::collections::HashMap;

    /// Build a (width × height × depth) box and return the mesh and
    /// the model. Centralised so test cases can mutate dimensions
    /// without copy-pasting the assembly.
    #[allow(clippy::panic)] // Reason: test diagnostic
    fn box_mesh(w: f64, h: f64, d: f64, params: &TessellationParams) -> (TriangleMesh, BRepModel) {
        let mut model = BRepModel::new();
        let solid_id = {
            let mut builder = TopologyBuilder::new(&mut model);
            let geom = builder
                .create_box_3d(w, h, d)
                .expect("create_box_3d must succeed for positive dimensions");
            match geom {
                GeometryId::Solid(id) => id,
                other => panic!("create_box_3d must return a Solid, got {other:?}"),
            }
        };
        let solid = model
            .solids
            .get(solid_id)
            .expect("solid must exist after create_box_3d")
            .clone();
        let mesh = tessellate_solid(&solid, &model, params);
        (mesh, model)
    }

    /// Count how many times each undirected edge appears across the
    /// mesh's triangles. Returns a map from sorted (u32, u32) to count.
    fn edge_use_counts(mesh: &TriangleMesh) -> HashMap<(u32, u32), u32> {
        let mut counts: HashMap<(u32, u32), u32> = HashMap::new();
        for tri in &mesh.triangles {
            for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                let key = if a < b { (a, b) } else { (b, a) };
                *counts.entry(key).or_insert(0) += 1;
            }
        }
        counts
    }

    /// Assert that every triangle edge is shared by exactly two
    /// triangles (the standard manifold-watertight invariant). Returns
    /// the (open_edges, non_manifold_edges) tuple for diagnostic reporting.
    fn assert_watertight(mesh: &TriangleMesh) {
        let counts = edge_use_counts(mesh);
        let open: Vec<_> = counts.iter().filter(|(_, &c)| c == 1).collect();
        let non_manifold: Vec<_> = counts.iter().filter(|(_, &c)| c > 2).collect();
        assert!(
            open.is_empty() && non_manifold.is_empty(),
            "mesh not watertight: {open_count} open edges, {nm_count} non-manifold edges \
             (e.g. {first_open:?}, {first_nm:?}); total triangles = {tris}, vertices = {vs}",
            open_count = open.len(),
            nm_count = non_manifold.len(),
            first_open = open.first(),
            first_nm = non_manifold.first(),
            tris = mesh.triangles.len(),
            vs = mesh.vertices.len(),
        );
    }

    /// Assert that no two distinct vertex indices used in any triangle
    /// share the same 3D position within `tol`. Orphaned vertices in
    /// `mesh.vertices` (un-referenced by any triangle) are ignored —
    /// the welding pass leaves them in place by design.
    fn assert_no_referenced_duplicates(mesh: &TriangleMesh, tol: f64) {
        use std::collections::HashSet;
        let mut referenced: HashSet<u32> = HashSet::new();
        for tri in &mesh.triangles {
            referenced.insert(tri[0]);
            referenced.insert(tri[1]);
            referenced.insert(tri[2]);
        }
        let referenced: Vec<u32> = referenced.into_iter().collect();
        let tol_sq = tol * tol;
        for i in 0..referenced.len() {
            for j in i + 1..referenced.len() {
                let pa = mesh.vertices[referenced[i] as usize].position;
                let pb = mesh.vertices[referenced[j] as usize].position;
                let d = pa - pb;
                let d2 = d.x * d.x + d.y * d.y + d.z * d.z;
                assert!(
                    d2 > tol_sq,
                    "referenced vertices {a} and {b} share position {pa:?} ≈ {pb:?} \
                     (distance² = {d2:e}, tol² = {tol_sq:e}) — welding failed",
                    a = referenced[i],
                    b = referenced[j],
                );
            }
        }
    }

    #[test]
    fn box_tessellation_is_watertight_default_params() {
        // 2×1×3 box, default tessellation params. Every B-Rep edge is
        // shared by exactly two faces; after welding, every mesh edge
        // must be shared by exactly two triangles.
        let (mesh, _model) = box_mesh(2.0, 1.0, 3.0, &TessellationParams::default());
        assert!(
            mesh.triangles.len() >= 12,
            "box should produce ≥12 triangles, got {}",
            mesh.triangles.len()
        );
        assert_watertight(&mesh);
    }

    #[test]
    fn box_tessellation_is_watertight_coarse_params() {
        // Coarse params produce far fewer triangles per face but the
        // shared-edge invariant must still hold.
        let (mesh, _model) = box_mesh(10.0, 5.0, 7.0, &TessellationParams::coarse());
        assert_watertight(&mesh);
    }

    #[test]
    fn box_tessellation_is_watertight_fine_params() {
        // Fine params stress the welding tolerance — chord_tolerance
        // is 1e-4, so the welder must use a matching tolerance to
        // catch coincident boundary samples.
        let (mesh, _model) = box_mesh(1.0, 1.0, 1.0, &TessellationParams::fine());
        assert_watertight(&mesh);
    }

    #[test]
    fn box_tessellation_has_no_referenced_duplicates_after_welding() {
        // After welding, every vertex referenced by a triangle should
        // be unique in 3-space. Orphans from the welding pass are
        // allowed (they're harmless to consumers).
        let (mesh, _model) = box_mesh(2.0, 3.0, 4.0, &TessellationParams::default());
        // Tolerance for "distinct" is 10× the welding tolerance to
        // catch any near-misses that escaped the cell-neighbourhood scan.
        assert_no_referenced_duplicates(&mesh, 1e-5);
    }

    #[test]
    fn weld_mesh_watertight_collapses_coincident_vertices() {
        use crate::math::Vector3;
        // Hand-build a "two faces sharing an edge" pattern with
        // duplicate boundary vertices and confirm welding collapses
        // them.
        let mut mesh = TriangleMesh::new();
        // Face A: (0,0,0)-(1,0,0)-(0,1,0) (CCW from +Z)
        let a0 = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 0.0, 0.0),
            normal: Vector3::Z,
            uv: None,
        });
        let a1 = mesh.add_vertex(MeshVertex {
            position: Point3::new(1.0, 0.0, 0.0),
            normal: Vector3::Z,
            uv: None,
        });
        let a2 = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 1.0, 0.0),
            normal: Vector3::Z,
            uv: None,
        });
        // Face B: (1,0,0)-(0,1,0)-(1,1,0) — shares edge (a1,a2) with A,
        // emitted as duplicate vertices b0/b1 to simulate per-face
        // independent tessellation.
        let b0 = mesh.add_vertex(MeshVertex {
            position: Point3::new(1.0, 0.0, 0.0),
            normal: Vector3::Z,
            uv: None,
        });
        let b1 = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 1.0, 0.0),
            normal: Vector3::Z,
            uv: None,
        });
        let b2 = mesh.add_vertex(MeshVertex {
            position: Point3::new(1.0, 1.0, 0.0),
            normal: Vector3::Z,
            uv: None,
        });
        mesh.add_triangle(a0, a1, a2);
        mesh.add_triangle(b0, b1, b2);
        mesh.face_map.push(0);
        mesh.face_map.push(1);

        surface::weld_mesh_watertight(&mut mesh, 1e-6);

        // After welding, the shared edge (a1,a2) ↔ (b0,b1) must collapse:
        // triangle 1 should reference a1 and a2 directly.
        assert_eq!(mesh.triangles.len(), 2, "no triangles should be dropped");
        let t1 = mesh.triangles[1];
        assert!(
            t1.contains(&a1) && t1.contains(&a2),
            "second triangle {t1:?} should reference welded indices {a1} and {a2} \
             after collapsing duplicate boundary vertices"
        );
        // face_map must remain in lock-step with triangles
        assert_eq!(mesh.face_map.len(), 2);
        assert_eq!(mesh.face_map, vec![0, 1]);

        // The shared edge (a1, a2) (sorted) appears in BOTH triangles ⇒
        // count == 2, which is the watertightness invariant.
        let counts = edge_use_counts(&mesh);
        let edge = if a1 < a2 { (a1, a2) } else { (a2, a1) };
        assert_eq!(
            counts.get(&edge).copied().unwrap_or(0),
            2,
            "shared edge ({a1},{a2}) should be referenced by exactly 2 triangles \
             after welding; got counts = {counts:?}"
        );
    }

    #[test]
    fn weld_mesh_watertight_drops_degenerate_triangles() {
        use crate::math::Vector3;
        // Three vertices, two coincident → the triangle collapses to
        // a sliver after welding and must be dropped, with face_map
        // shrinking accordingly.
        let mut mesh = TriangleMesh::new();
        let v0 = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 0.0, 0.0),
            normal: Vector3::Z,
            uv: None,
        });
        let v1 = mesh.add_vertex(MeshVertex {
            position: Point3::new(1.0, 0.0, 0.0),
            normal: Vector3::Z,
            uv: None,
        });
        let v2 = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 0.0, 0.0), // duplicate of v0
            normal: Vector3::Z,
            uv: None,
        });
        mesh.add_triangle(v0, v1, v2);
        mesh.face_map.push(42);

        surface::weld_mesh_watertight(&mut mesh, 1e-6);

        assert!(
            mesh.triangles.is_empty(),
            "degenerate triangle should be dropped, got {:?}",
            mesh.triangles
        );
        assert!(
            mesh.face_map.is_empty(),
            "face_map should shrink in lock-step, got {:?}",
            mesh.face_map
        );
    }

    #[test]
    fn weld_mesh_watertight_handles_empty_mesh() {
        // No vertices, no triangles — the welder must be a no-op.
        let mut mesh = TriangleMesh::new();
        surface::weld_mesh_watertight(&mut mesh, 1e-6);
        assert!(mesh.vertices.is_empty());
        assert!(mesh.triangles.is_empty());
    }

    #[test]
    fn weld_mesh_watertight_range_preserves_pre_existing_vertices() {
        use crate::math::Vector3;
        // Pre-populate with two vertices and a triangle outside the
        // weld range — they must survive untouched.
        let mut mesh = TriangleMesh::new();
        let p0 = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 0.0, 0.0),
            normal: Vector3::Z,
            uv: None,
        });
        let p1 = mesh.add_vertex(MeshVertex {
            position: Point3::new(10.0, 0.0, 0.0),
            normal: Vector3::Z,
            uv: None,
        });
        let p2 = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 10.0, 0.0),
            normal: Vector3::Z,
            uv: None,
        });
        mesh.add_triangle(p0, p1, p2);
        mesh.face_map.push(0);
        let v_start = mesh.vertices.len();
        let t_start = mesh.triangles.len();

        // Now add a "second shell" with duplicate boundary vertices.
        let q0 = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 0.0, 1.0),
            normal: Vector3::Z,
            uv: None,
        });
        let q1 = mesh.add_vertex(MeshVertex {
            position: Point3::new(1.0, 0.0, 1.0),
            normal: Vector3::Z,
            uv: None,
        });
        let q2 = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 1.0, 1.0),
            normal: Vector3::Z,
            uv: None,
        });
        let q3 = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 0.0, 1.0), // duplicate of q0
            normal: Vector3::Z,
            uv: None,
        });
        mesh.add_triangle(q0, q1, q2);
        mesh.add_triangle(q3, q1, q2);
        mesh.face_map.push(1);
        mesh.face_map.push(2);

        surface::weld_mesh_watertight_range(&mut mesh, 1e-6, v_start, t_start);

        // First triangle (head) must remain intact and reference its
        // original indices.
        assert_eq!(mesh.triangles[0], [p0, p1, p2]);
        assert_eq!(mesh.face_map[0], 0);

        // Second-shell triangles must have q3 collapsed to q0 — but
        // since q3 ↔ q0 collapse makes both triangles share the same
        // 3-vertex set, the second triangle becomes (q0, q1, q2) which
        // is a duplicate of the first; both must survive (welding does
        // not deduplicate identical triangles, only collapses indices).
        assert_eq!(mesh.triangles.len(), 3);
        assert_eq!(mesh.triangles[1], [q0, q1, q2]);
        assert_eq!(mesh.triangles[2], [q0, q1, q2]); // q3 → q0
        assert_eq!(mesh.face_map, vec![0, 1, 2]);
    }

    /// K13 — winding consistency. For a closed convex solid centred at
    /// the origin, every triangle's geometric normal `(b - a) × (c - a)`
    /// must point AWAY from the centre — i.e. dot-product with the
    /// triangle centroid (from origin) is positive. This is the
    /// classical "outward winding" test for closed solids and is
    /// independent of the welding pass (vertex normals can be
    /// overwritten by canonicals; positions cannot).
    ///
    /// A negative result means the triangle is wound clockwise from
    /// the outside view, which would make a back-face-culling renderer
    /// drop it (and a normal-from-cross-product BVH builder flip its
    /// sign). This is the bug class K13 closes.
    #[test]
    fn box_tessellation_winding_is_outward_for_closed_solid() {
        // Centred box (origin = centroid). `create_box_3d` builds a box
        // centred at the origin (vertices at ±hw, ±hh, ±hd).
        let (mesh, _model) = box_mesh(2.0, 3.0, 4.0, &TessellationParams::default());

        let mut bad = 0usize;
        let mut total = 0usize;
        for tri in &mesh.triangles {
            total += 1;
            let a = mesh.vertices[tri[0] as usize].position;
            let b = mesh.vertices[tri[1] as usize].position;
            let c = mesh.vertices[tri[2] as usize].position;

            // Geometric normal from CCW cross product.
            let ab = b - a;
            let ac = c - a;
            let geom = ab.cross(&ac);

            // Centroid as a vector from origin (the solid's centre).
            let cx = (a.x + b.x + c.x) / 3.0;
            let cy = (a.y + b.y + c.y) / 3.0;
            let cz = (a.z + b.z + c.z) / 3.0;
            let radial = crate::math::Vector3::new(cx, cy, cz);

            // Outward agreement: cross-product normal pointing away
            // from origin must have positive dot with the centroid
            // radial vector.
            let agreement = geom.dot(&radial);
            if agreement <= 0.0 {
                bad += 1;
            }
        }
        assert_eq!(
            bad, 0,
            "{bad}/{total} triangles wound inward (geom·centroid ≤ 0) — \
             K13 winding consistency violated for closed-solid invariant"
        );
    }

    /// K14 — G1 smoothing. Two coincident vertices with normals only
    /// 5° apart should be averaged into a single canonical normal
    /// (smooth seam case — e.g. closed cylinder wraparound, NURBS
    /// tangent-continuous join).
    #[test]
    fn weld_mesh_watertight_g1_smoothes_close_normals() {
        use crate::math::Vector3;
        let mut mesh = TriangleMesh::new();
        // Two faces, each with one vertex at (0,0,0) but normals at 5°.
        let theta: f64 = 5.0_f64.to_radians();
        let n_a = Vector3::new(theta.sin(), 0.0, theta.cos()); // tilted +x by 5°
        let n_b = Vector3::new(-theta.sin(), 0.0, theta.cos()); // tilted -x by 5°
        let v_a = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 0.0, 0.0),
            normal: n_a,
            uv: None,
        });
        let _v_a1 = mesh.add_vertex(MeshVertex {
            position: Point3::new(1.0, 0.0, 0.0),
            normal: n_a,
            uv: None,
        });
        let _v_a2 = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 1.0, 0.0),
            normal: n_a,
            uv: None,
        });
        let v_b = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 0.0, 0.0), // coincident with v_a
            normal: n_b,
            uv: None,
        });
        let _v_b1 = mesh.add_vertex(MeshVertex {
            position: Point3::new(-1.0, 0.0, 0.0),
            normal: n_b,
            uv: None,
        });
        let _v_b2 = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, -1.0, 0.0),
            normal: n_b,
            uv: None,
        });
        mesh.add_triangle(v_a, _v_a1, _v_a2);
        mesh.add_triangle(v_b, _v_b1, _v_b2);
        mesh.face_map.push(0);
        mesh.face_map.push(1);

        surface::weld_mesh_watertight(&mut mesh, 1e-6);

        // After welding, v_b → v_a. The canonical's normal should be
        // the unit-length average ≈ (0, 0, cos 5°) → renormalised to Z.
        let nv = mesh.vertices[v_a as usize].normal;
        let len = nv.dot(&nv).sqrt();
        assert!(
            (len - 1.0).abs() < 1e-9,
            "averaged normal must remain unit-length, got len = {len}"
        );
        // Average should point essentially +Z (both contributors were
        // symmetric around +Z).
        assert!(nv.z > 0.999, "averaged normal should be ≈ +Z, got {nv:?}");
        assert!(
            nv.x.abs() < 1e-9,
            "averaged normal x-component should cancel, got x = {}",
            nv.x
        );
    }

    /// K14 — G1 sharp-edge preservation. Two coincident vertices
    /// with normals 90° apart (the box-corner case) must NOT be
    /// averaged — averaging would smear the shading discontinuity
    /// the renderer needs at sharp B-Rep edges.
    #[test]
    fn weld_mesh_watertight_g1_preserves_sharp_normals() {
        use crate::math::Vector3;
        let mut mesh = TriangleMesh::new();
        let n_top = Vector3::new(0.0, 0.0, 1.0); // +Z (top of box)
        let n_side = Vector3::new(1.0, 0.0, 0.0); // +X (side of box)
        let v_top = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 0.0, 0.0),
            normal: n_top,
            uv: None,
        });
        let _v_top1 = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 1.0, 0.0),
            normal: n_top,
            uv: None,
        });
        let _v_top2 = mesh.add_vertex(MeshVertex {
            position: Point3::new(-1.0, 0.0, 0.0),
            normal: n_top,
            uv: None,
        });
        let v_side = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 0.0, 0.0), // coincident with v_top
            normal: n_side,
            uv: None,
        });
        let _v_side1 = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 1.0, 0.0),
            normal: n_side,
            uv: None,
        });
        let _v_side2 = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 0.0, -1.0),
            normal: n_side,
            uv: None,
        });
        mesh.add_triangle(v_top, _v_top1, _v_top2);
        mesh.add_triangle(v_side, _v_side1, _v_side2);
        mesh.face_map.push(0);
        mesh.face_map.push(1);

        surface::weld_mesh_watertight(&mut mesh, 1e-6);

        // Canonical normal should remain `n_top` (the lower-index
        // contributor) — averaging two 90° normals gives |avg| ≈
        // 0.707 < 0.95 threshold → no overwrite.
        let nv = mesh.vertices[v_top as usize].normal;
        assert!(
            (nv.x - n_top.x).abs() < 1e-12
                && (nv.y - n_top.y).abs() < 1e-12
                && (nv.z - n_top.z).abs() < 1e-12,
            "sharp-edge canonical normal should be preserved (= {n_top:?}), got {nv:?}"
        );
    }
}
