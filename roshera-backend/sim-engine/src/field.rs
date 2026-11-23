//! The result on the part's surface, for the viewport: positions,
//! displacements and von Mises stress per vertex, triangles facing out of
//! the part, and the kernel face of each triangle -- so the app can colour
//! the part by stress, draw it deformed, and say which face a value is on.
//!
//! Each 6-node boundary triangle becomes four display triangles through its
//! mid-side nodes, so the colours follow the quadratic solution instead of
//! three corner values.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ccx::{tri6_on, von_mises, Solution};
use crate::mesh::VolumeMesh;

/// The solved field on the part's surface. `f32` keeps the payload small;
/// the report's own numbers stay `f64`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SurfaceField {
    pub positions: Vec<[f32; 3]>,
    /// Displacement of each vertex, mm.
    pub displacement: Vec<[f32; 3]>,
    /// Von Mises stress at each vertex, MPa.
    pub von_mises: Vec<f32>,
    /// Vertex indices, counter-clockwise seen from outside the part.
    pub triangles: Vec<[u32; 3]>,
    /// The kernel face each triangle lies on.
    pub faces: Vec<u32>,
    pub min_von_mises: f32,
    pub max_von_mises: f32,
}

/// The surface field of `mesh` solved as `solution`.
pub fn surface_field(mesh: &VolumeMesh, solution: &Solution) -> SurfaceField {
    let all_faces: Vec<u32> = {
        let mut f: Vec<u32> = mesh.boundary.iter().map(|b| b.face).collect();
        f.sort_unstable();
        f.dedup();
        f
    };
    let mut out = SurfaceField::default();
    let mut index: HashMap<u64, u32> = HashMap::new();
    let mut vertex = |node: u64, out: &mut SurfaceField| -> u32 {
        if let Some(&i) = index.get(&node) {
            return i;
        }
        let i = out.positions.len() as u32;
        let p = mesh.nodes.get(&node).copied().unwrap_or([0.0; 3]);
        let d = solution
            .displacement
            .get(&node)
            .copied()
            .unwrap_or([0.0; 3]);
        let s = solution.stress.get(&node).map(von_mises).unwrap_or(0.0);
        out.positions.push([p[0] as f32, p[1] as f32, p[2] as f32]);
        out.displacement
            .push([d[0] as f32, d[1] as f32, d[2] as f32]);
        out.von_mises.push(s as f32);
        index.insert(node, i);
        i
    };
    // tri6_on gives each boundary triangle with its outward normal, in the
    // same order as `mesh.boundary`.
    let tris = tri6_on(mesh, &all_faces);
    let faces_of: Vec<u32> = mesh
        .boundary
        .iter()
        .filter(|b| all_faces.contains(&b.face))
        .map(|b| b.face)
        .collect();
    for (t, face) in tris.iter().zip(faces_of) {
        let [a, b, c, ab, bc, ca] = t.nodes;
        // Corner winding's normal against the outward one: flip if inward.
        let [pa, pb, pc] = t.corners;
        let u = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let v = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
        let wn = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let outward = wn[0] * t.normal[0] + wn[1] * t.normal[1] + wn[2] * t.normal[2] >= 0.0;
        let quads = [[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]];
        for q in quads {
            let ids = [
                vertex(q[0], &mut out),
                vertex(q[1], &mut out),
                vertex(q[2], &mut out),
            ];
            out.triangles.push(if outward {
                ids
            } else {
                [ids[0], ids[2], ids[1]]
            });
            out.faces.push(face);
        }
    }
    out.min_von_mises = out.von_mises.iter().copied().fold(f32::INFINITY, f32::min);
    out.max_von_mises = out.von_mises.iter().copied().fold(0.0_f32, f32::max);
    if !out.min_von_mises.is_finite() {
        out.min_von_mises = 0.0;
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::mesh::BoundaryTri;

    /// One quadratic tet (corners 1-4, mid-sides 5-10 in Gmsh order) whose
    /// face 1-2-3 is on the boundary, written inward-wound on purpose.
    fn one_tet() -> (VolumeMesh, Solution) {
        let p = [
            (1, [0.0, 0.0, 0.0]),
            (2, [1.0, 0.0, 0.0]),
            (3, [0.0, 1.0, 0.0]),
            (4, [0.0, 0.0, 1.0]),
            (5, [0.5, 0.0, 0.0]),
            (6, [0.5, 0.5, 0.0]),
            (7, [0.0, 0.5, 0.0]),
            (8, [0.0, 0.0, 0.5]),
            (9, [0.0, 0.5, 0.5]),
            (10, [0.5, 0.0, 0.5]),
        ];
        let mut mesh = VolumeMesh::default();
        for (id, pos) in p {
            mesh.nodes.insert(id, pos);
        }
        mesh.tets.push([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        // Corners 1-2-3 wound so their normal points +z: INTO the tet.
        mesh.boundary.push(BoundaryTri {
            nodes: [1, 2, 3, 5, 6, 7],
            face: 42,
        });
        let mut sol = Solution::default();
        for (id, _) in p {
            sol.displacement.insert(id, [0.0, 0.0, -(id as f64)]);
            sol.stress
                .insert(id, [10.0 * id as f64, 0.0, 0.0, 0.0, 0.0, 0.0]);
        }
        (mesh, sol)
    }

    /// A quadratic boundary triangle becomes four display triangles over its
    /// six nodes, each wound to face OUT of the part (here -z, away from the
    /// tet's fourth corner), each on its kernel face; values come per node.
    ///
    /// Mutation: keep the corner winding without checking it against the
    /// outward normal -> the triangles face into the part -> RED.
    #[test]
    fn a_quadratic_triangle_becomes_four_outward_display_triangles() {
        let (mesh, sol) = one_tet();
        let f = surface_field(&mesh, &sol);
        assert_eq!(f.positions.len(), 6);
        assert_eq!(f.triangles.len(), 4);
        assert!(f.faces.iter().all(|&x| x == 42));
        for t in &f.triangles {
            let [a, b, c] = t.map(|i| f.positions[i as usize]);
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let nz = u[0] * v[1] - u[1] * v[0];
            assert!(nz < 0.0, "triangle {t:?} faces into the part");
        }
        assert_eq!(f.max_von_mises, 70.0, "node 7 carries 70 MPa");
        assert_eq!(f.min_von_mises, 10.0);
    }
}
