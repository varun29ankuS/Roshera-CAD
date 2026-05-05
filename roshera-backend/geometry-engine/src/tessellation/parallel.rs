//! Parallel tessellation.
//!
//! Uses Rayon for multi-threaded processing.
//!
//! Indexed access into mesh arrays is the canonical idiom — all `arr[i]`
//! sites use indices bounded by mesh dimensions. Matches the numerical-
//! kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::{surface, TessellationParams, ThreeJsMesh, TriangleMesh};
use crate::primitives::{face::Face, shell::Shell, solid::Solid, topology_builder::BRepModel};
use parking_lot::Mutex;
use rayon::prelude::*;
use std::sync::Arc;

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
    #[allow(clippy::expect_used)] // rayon thread pool failure is unrecoverable; surface loudly
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

                        let mut meshes_guard = meshes.lock();
                        meshes_guard.push(face_mesh.to_threejs());
                    }
                }
            });
        });

        // Merge results
        let mut final_mesh = ThreeJsMesh::new();
        let meshes_guard = meshes.lock();
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

/// Multi-threaded mesh optimization (vertex deduplication).
///
/// Builds a spatial hash keyed on quantized (position, normal) tuples
/// so two vertices that round to the same bucket within
/// `MERGE_TOLERANCE` collapse to one. The previous implementation
/// stamped every vertex as unique, which made this function a no-op
/// while still paying the cost of the parallel chunked walk.
pub fn optimize_mesh_parallel(mesh: &mut ThreeJsMesh) {
    use std::collections::HashMap;

    let vertex_count = mesh.vertex_count();
    if vertex_count == 0 {
        return;
    }

    // Quantization granularity. Vertices closer than ~1e-5 in any
    // component or whose normals differ by less than ~1e-4 (about
    // 0.006°) collapse into the same bucket. These thresholds match
    // the kernel's default mesh tolerance and produce visually
    // identical output while removing duplicates introduced by
    // adjacent face seams.
    const POS_QUANT: f32 = 1.0e5;
    const NRM_QUANT: f32 = 1.0e4;

    // Single-pass deduplication. The hash map is cheap to build
    // serially and avoids the lock contention that fooled the
    // previous parallel attempt into a silent no-op. We retain the
    // function name so callers stay unchanged.
    let mut bucket: HashMap<(i32, i32, i32, i32, i32, i32), u32> =
        HashMap::with_capacity(vertex_count);
    let mut new_positions: Vec<f32> = Vec::with_capacity(mesh.positions.len());
    let mut new_normals: Vec<f32> = Vec::with_capacity(mesh.normals.len());
    let has_uv = mesh
        .uvs
        .as_ref()
        .is_some_and(|u| u.len() == vertex_count * 2);
    let mut new_uvs: Vec<f32> = Vec::with_capacity(if has_uv { vertex_count * 2 } else { 0 });
    let has_color = mesh
        .colors
        .as_ref()
        .is_some_and(|c| c.len() == vertex_count * 3);
    let mut new_colors: Vec<f32> = Vec::with_capacity(if has_color { vertex_count * 3 } else { 0 });

    let mut remap = vec![0u32; vertex_count];
    let mut next_index: u32 = 0;

    for i in 0..vertex_count {
        let p_idx = i * 3;
        let n_idx = i * 3;
        let key = (
            (mesh.positions[p_idx] * POS_QUANT).round() as i32,
            (mesh.positions[p_idx + 1] * POS_QUANT).round() as i32,
            (mesh.positions[p_idx + 2] * POS_QUANT).round() as i32,
            (mesh.normals[n_idx] * NRM_QUANT).round() as i32,
            (mesh.normals[n_idx + 1] * NRM_QUANT).round() as i32,
            (mesh.normals[n_idx + 2] * NRM_QUANT).round() as i32,
        );
        if let Some(&existing) = bucket.get(&key) {
            remap[i] = existing;
        } else {
            bucket.insert(key, next_index);
            remap[i] = next_index;
            next_index += 1;
            new_positions.extend_from_slice(&mesh.positions[p_idx..p_idx + 3]);
            new_normals.extend_from_slice(&mesh.normals[n_idx..n_idx + 3]);
            if has_uv {
                if let Some(uvs) = mesh.uvs.as_ref() {
                    let uv_idx = i * 2;
                    new_uvs.extend_from_slice(&uvs[uv_idx..uv_idx + 2]);
                }
            }
            if has_color {
                if let Some(cs) = mesh.colors.as_ref() {
                    let c_idx = i * 3;
                    new_colors.extend_from_slice(&cs[c_idx..c_idx + 3]);
                }
            }
        }
    }

    // Parallel index remapping — this part of the original logic is
    // sound and benefits from rayon since indices.len() can be large.
    const CHUNK_SIZE: usize = 1000;
    let remap = Arc::new(remap);
    let indices_chunks: Vec<_> = mesh
        .indices
        .chunks(CHUNK_SIZE)
        .map(|chunk| chunk.to_vec())
        .collect();
    let remapped_indices: Vec<Vec<u32>> = indices_chunks
        .par_iter()
        .map(|chunk| {
            let remap = Arc::clone(&remap);
            chunk
                .iter()
                .map(|&idx| remap[idx as usize])
                .collect::<Vec<u32>>()
        })
        .collect();

    mesh.indices = remapped_indices.into_iter().flatten().collect();
    mesh.positions = new_positions;
    mesh.normals = new_normals;
    if has_uv {
        mesh.uvs = Some(new_uvs);
    }
    if has_color {
        mesh.colors = Some(new_colors);
    }
}
