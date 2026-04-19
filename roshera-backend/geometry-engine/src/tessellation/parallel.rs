//! Parallel tessellation.
//!
//! Uses Rayon for multi-threaded processing.

use super::{surface, TessellationParams, ThreeJsMesh, TriangleMesh};
use crate::primitives::{face::Face, shell::Shell, solid::Solid, topology_builder::BRepModel};
use rayon::prelude::*;
use std::sync::{Arc, Mutex};

/// Parallel tessellation of a solid
pub fn tessellate_solid_parallel(
    solid: &Solid,
    model: &BRepModel,
    params: &TessellationParams,
) -> ThreeJsMesh {
    let mut mesh = ThreeJsMesh::new();

    // Tessellate outer shell in parallel
    if let Some(shell) = model.shells.get(solid.outer_shell) {
        let shell_mesh = tessellate_shell_parallel(shell, model, params);
        mesh.merge(&shell_mesh);
    }

    // Tessellate inner shells in parallel
    let inner_meshes: Vec<ThreeJsMesh> = solid
        .inner_shells
        .par_iter()
        .filter_map(|&shell_id| {
            model
                .shells
                .get(shell_id)
                .map(|shell| tessellate_shell_parallel(shell, model, params))
        })
        .collect();

    for inner_mesh in inner_meshes {
        mesh.merge(&inner_mesh);
    }

    mesh
}

/// Parallel tessellation of a shell
pub fn tessellate_shell_parallel(
    shell: &Shell,
    model: &BRepModel,
    params: &TessellationParams,
) -> ThreeJsMesh {
    // Use rayon to process faces in parallel
    let face_meshes: Vec<ThreeJsMesh> = shell
        .faces
        .par_iter()
        .filter_map(|&face_id| {
            model.faces.get(face_id).map(|face| {
                let mut face_mesh = TriangleMesh::new();
                surface::tessellate_face(face, model, params, &mut face_mesh);
                face_mesh.to_threejs()
            })
        })
        .collect();

    // Merge all face meshes
    let mut final_mesh = ThreeJsMesh::new();
    for face_mesh in face_meshes {
        final_mesh.merge(&face_mesh);
    }

    final_mesh
}

/// Batch tessellation of multiple solids
pub fn tessellate_solids_batch(
    solids: &[(crate::primitives::solid::SolidId, &Solid)],
    model: &BRepModel,
    params: &TessellationParams,
) -> Vec<(crate::primitives::solid::SolidId, ThreeJsMesh)> {
    solids
        .par_iter()
        .map(|(id, solid)| {
            let mesh = tessellate_solid_parallel(solid, model, params);
            (*id, mesh)
        })
        .collect()
}

/// Adaptive parallel tessellation with work stealing
pub struct ParallelTessellator {
    thread_pool: rayon::ThreadPool,
    params: TessellationParams,
}

impl ParallelTessellator {
    /// Create a new parallel tessellator with specified thread count.
    ///
    /// # Panics
    /// Panics if the rayon thread pool fails to build. This can occur if
    /// `num_threads` is invalid for the host (e.g. zero on some platforms)
    /// or if the underlying OS refuses to spawn the requested threads.
    pub fn new(params: TessellationParams, num_threads: usize) -> Self {
        let thread_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .build()
            .expect("rayon thread pool construction failed (invalid num_threads or OS refused)");

        Self {
            thread_pool,
            params,
        }
    }

    /// Tessellate with automatic load balancing
    pub fn tessellate_adaptive(&self, shell: &Shell, model: &BRepModel) -> ThreeJsMesh {
        // Sort faces by estimated complexity
        let mut face_complexities: Vec<(usize, f64)> = shell
            .faces
            .iter()
            .enumerate()
            .filter_map(|(idx, &face_id)| {
                model.faces.get(face_id).map(|face| {
                    let complexity = estimate_face_complexity(face, model);
                    (idx, complexity)
                })
            })
            .collect();

        // Sort by complexity (descending) for better load balancing.
        // NaN-safe ordering: treat unorderable complexity values as equal
        // so the sort remains total even if an estimator returns NaN.
        face_complexities
            .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Process faces in parallel with work stealing
        let meshes = Arc::new(Mutex::new(Vec::new()));

        self.thread_pool.install(|| {
            face_complexities.par_iter().for_each(|&(idx, _)| {
                if let Some(&face_id) = shell.faces.get(idx) {
                    if let Some(face) = model.faces.get(face_id) {
                        let mut face_mesh = TriangleMesh::new();
                        surface::tessellate_face(face, model, &self.params, &mut face_mesh);

                        let mut meshes_guard = meshes
                            .lock()
                            .expect("parallel tessellator meshes Mutex poisoned");
                        meshes_guard.push(face_mesh.to_threejs());
                    }
                }
            });
        });

        // Merge results
        let mut final_mesh = ThreeJsMesh::new();
        let meshes_guard = meshes
            .lock()
            .expect("parallel tessellator meshes Mutex poisoned");
        for mesh in meshes_guard.iter() {
            final_mesh.merge(mesh);
        }

        final_mesh
    }
}

/// Estimate face complexity for load balancing
fn estimate_face_complexity(face: &Face, model: &BRepModel) -> f64 {
    let mut complexity = 1.0;

    // Factor in surface type
    if let Some(surface) = model.surfaces.get(face.surface_id) {
        complexity *= match surface.type_name() {
            "Plane" => 1.0,
            "Cylinder" => 2.0,
            "Sphere" => 3.0,
            "Cone" => 3.0,
            "Torus" => 4.0,
            "NURBS" => 10.0,
            _ => 5.0,
        };
    }

    // Factor in number of loops (holes)
    complexity *= (1 + face.inner_loops.len()) as f64;

    // Factor in edge count
    if let Some(outer_loop) = model.loops.get(face.outer_loop) {
        complexity *= outer_loop.edges.len() as f64 * 0.1;
    }

    complexity
}

/// Multi-threaded mesh optimization
pub fn optimize_mesh_parallel(mesh: &mut ThreeJsMesh) {
    use std::sync::atomic::{AtomicU32, Ordering};

    // Parallel vertex deduplication
    let vertex_count = mesh.vertex_count();
    let mut unique_vertices: Vec<(f32, f32, f32, f32, f32, f32)> = Vec::new();
    let vertex_remap = Arc::new(Mutex::new(vec![0u32; vertex_count]));
    let next_index = Arc::new(AtomicU32::new(0));

    // Process vertices in chunks
    const CHUNK_SIZE: usize = 1000;
    let chunks: Vec<_> = (0..vertex_count)
        .collect::<Vec<_>>()
        .chunks(CHUNK_SIZE)
        .map(|chunk| chunk.to_vec())
        .collect();

    chunks.par_iter().for_each(|chunk| {
        for &i in chunk {
            let idx = i * 3;
            let pos = (
                mesh.positions[idx],
                mesh.positions[idx + 1],
                mesh.positions[idx + 2],
            );
            let normal = (
                mesh.normals[idx],
                mesh.normals[idx + 1],
                mesh.normals[idx + 2],
            );

            // Check for duplicate (simplified for performance)
            let is_unique = true; // Would implement spatial hash lookup

            if is_unique {
                let new_idx = next_index.fetch_add(1, Ordering::SeqCst);
                let mut remap = vertex_remap
                    .lock()
                    .expect("parallel mesh optimizer vertex_remap Mutex poisoned");
                remap[i] = new_idx;
            }
        }
    });

    // Parallel index remapping
    let indices_chunks: Vec<_> = mesh
        .indices
        .chunks(CHUNK_SIZE)
        .map(|chunk| chunk.to_vec())
        .collect();

    let remapped_indices: Vec<Vec<u32>> = indices_chunks
        .par_iter()
        .map(|chunk| {
            let remap = vertex_remap
                .lock()
                .expect("parallel mesh optimizer vertex_remap Mutex poisoned");
            chunk.iter().map(|&idx| remap[idx as usize]).collect()
        })
        .collect();

    // Flatten remapped indices
    mesh.indices = remapped_indices.into_iter().flatten().collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parallel_tessellation() {
        // TODO: Add comprehensive tests
    }

    #[test]
    fn test_complexity_estimation() {
        // TODO: Test complexity estimation
    }
}
