//! The mesh a study will be solved on, shown before it is solved: both
//! meshes' sizes and counts, and the proving (fine) mesh's surface as
//! element edges for a wireframe over the part. Made with the same size rule
//! and the same mesher call as the run (`run::coarse_mesh_size`,
//! `mesh::mesh_solid`), so the mesh a person approves is the mesh solved.

use std::collections::BTreeSet;
use std::path::Path;

use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;
use serde::{Deserialize, Serialize};

use crate::engines::Engines;
use crate::mesh::{mesh_solid, MeshStats, VolumeMesh};
use crate::run::{coarse_mesh_size, StudyError, ENGINE_LIMIT};
use crate::study::FINE_RATIO;

/// What the mesh stage shows in numbers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeshPreview {
    pub coarse: MeshStats,
    /// The proving mesh: the stress is read off this one.
    pub fine: MeshStats,
}

/// The proving mesh's surface as element edges: positions (model
/// coordinates, mm) and index pairs. Each edge once.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MeshWire {
    pub positions: Vec<[f32; 3]>,
    pub edges: Vec<[u32; 2]>,
}

/// Mesh `solid` as a study would -- coarse, then fine -- without solving.
pub fn mesh_preview(
    model: &BRepModel,
    solid: SolidId,
    asked_size: Option<f64>,
    engines: &Engines,
    workdir: &Path,
) -> Result<(MeshPreview, MeshWire), StudyError> {
    let size = coarse_mesh_size(model, solid, asked_size)?;
    let (_, coarse) = mesh_solid(
        model,
        solid,
        size,
        &engines.mesher,
        &workdir.join("coarse"),
        ENGINE_LIMIT,
    )?;
    let (mesh, fine) = mesh_solid(
        model,
        solid,
        size * FINE_RATIO,
        &engines.mesher,
        &workdir.join("fine"),
        ENGINE_LIMIT,
    )?;
    Ok((MeshPreview { coarse, fine }, wire_of(&mesh)))
}

/// The boundary's element edges, corner to corner (the mid-side nodes lie
/// on them), each once.
pub(crate) fn wire_of(mesh: &VolumeMesh) -> MeshWire {
    let mut edges: BTreeSet<(u64, u64)> = BTreeSet::new();
    for b in &mesh.boundary {
        let [a, c, d] = [b.nodes[0], b.nodes[1], b.nodes[2]];
        for (x, y) in [(a, c), (c, d), (d, a)] {
            edges.insert((x.min(y), x.max(y)));
        }
    }
    let mut wire = MeshWire::default();
    let mut index = std::collections::HashMap::new();
    let mut at = |node: u64, wire: &mut MeshWire| -> u32 {
        *index.entry(node).or_insert_with(|| {
            let p = mesh.nodes.get(&node).copied().unwrap_or([0.0; 3]);
            wire.positions.push([p[0] as f32, p[1] as f32, p[2] as f32]);
            (wire.positions.len() - 1) as u32
        })
    };
    for (x, y) in edges {
        let e = [at(x, &mut wire), at(y, &mut wire)];
        wire.edges.push(e);
    }
    wire
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::mesh::BoundaryTri;

    /// Two boundary triangles sharing an edge give five edges, the shared
    /// one once, over their four corners -- the mid-side nodes are not
    /// vertices of the wireframe.
    ///
    /// Mutation: drop the (min, max) ordering -> the shared edge, walked in
    /// opposite directions by the two triangles, is drawn twice -> RED.
    #[test]
    fn a_shared_edge_is_drawn_once() {
        let mut mesh = VolumeMesh::default();
        for (id, p) in [
            (1, [0.0, 0.0, 0.0]),
            (2, [1.0, 0.0, 0.0]),
            (3, [0.0, 1.0, 0.0]),
            (4, [1.0, 1.0, 0.0]),
            (9, [0.5, 0.5, 0.0]),
        ] {
            mesh.nodes.insert(id, p);
        }
        mesh.boundary.push(BoundaryTri {
            nodes: [1, 2, 3, 9, 9, 9],
            face: 1,
        });
        mesh.boundary.push(BoundaryTri {
            nodes: [2, 4, 3, 9, 9, 9],
            face: 1,
        });
        let w = wire_of(&mesh);
        assert_eq!(w.edges.len(), 5);
        assert_eq!(w.positions.len(), 4, "corners only");
        let mut seen = BTreeSet::new();
        for e in &w.edges {
            assert!(
                seen.insert((e[0].min(e[1]), e[0].max(e[1]))),
                "edge {e:?} twice"
            );
        }
    }
}
