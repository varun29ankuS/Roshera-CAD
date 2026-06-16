//! B-Rep tessellation module
//!
//! Converts analytical B-Rep models to triangle meshes for visualization and export.

pub mod adaptive;
pub mod cache;
pub mod curve;
pub(crate) mod curved_cdt;
pub mod edge_cache;
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
use edge_cache::EdgeSampleCache;

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

/// Bridge from the wire-level `shared_types::DisplayQuality` enum
/// (carried in REST query strings and `ExportRequest` JSON) to the
/// kernel-level `TessellationParams`. This is the canonical place
/// callers translate the user-facing knob into something the
/// tessellator actually understands.
///
/// `Low` / `Medium` / `High` map to the existing `coarse` / `default`
/// / `fine` presets so behaviour is identical to a caller that
/// already constructs these directly. `Custom` carries the three
/// per-quality knobs over the wire (`max_edge_length`,
/// `max_angle_deviation`, `chord_tolerance`); `min_segments` and
/// `max_segments` aren't on the wire, so we derive sensible bounds
/// from `chord_tolerance`: tighter tolerance → more segments allowed.
/// This keeps the wire format minimal while letting the adaptive
/// quadtree (T-1/T-2) refine as much as the chord guard demands.
impl From<shared_types::DisplayQuality> for TessellationParams {
    fn from(quality: shared_types::DisplayQuality) -> Self {
        match quality {
            shared_types::DisplayQuality::Low => Self::coarse(),
            shared_types::DisplayQuality::Medium => Self::default(),
            shared_types::DisplayQuality::High => Self::fine(),
            shared_types::DisplayQuality::Custom {
                max_edge_length,
                max_angle_deviation,
                chord_tolerance,
            } => {
                // Derive segment bounds from chord_tolerance: a tighter
                // chord guard means a finer mesh is being requested, so
                // raise the segment ceiling proportionally. Floor stays
                // at 3 (minimum for a non-degenerate quad). Ceiling is
                // capped at 1000 to defend against pathological inputs
                // (`chord_tolerance = 1e-20`) that would otherwise let
                // a single face emit millions of triangles.
                let max_segments = if chord_tolerance > 0.0 {
                    ((0.1 / chord_tolerance).sqrt() * 64.0)
                        .ceil()
                        .clamp(20.0, 1000.0) as usize
                } else {
                    200
                };
                Self {
                    max_edge_length,
                    max_angle_deviation,
                    chord_tolerance,
                    min_segments: 3,
                    max_segments,
                }
            }
        }
    }
}

impl From<&shared_types::DisplayQuality> for TessellationParams {
    fn from(quality: &shared_types::DisplayQuality) -> Self {
        (*quality).into()
    }
}

/// Tessellate a solid into a triangle mesh
/// Largest distance between any two boundary vertices of a solid (a cheap
/// over-estimate of the bbox diagonal) — used to floor the chord tolerance
/// relative to part size.
fn solid_extent(solid: &Solid, model: &BRepModel) -> f64 {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    let mut any = false;
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    for sh in shells {
        let Some(shell) = model.shells.get(sh) else {
            continue;
        };
        for &fid in &shell.faces {
            let Some(face) = model.faces.get(fid) else {
                continue;
            };
            let mut loops = vec![face.outer_loop];
            loops.extend_from_slice(&face.inner_loops);
            for lid in loops {
                let Some(lp) = model.loops.get(lid) else {
                    continue;
                };
                for &eid in &lp.edges {
                    let Some(e) = model.edges.get(eid) else {
                        continue;
                    };
                    for vid in [e.start_vertex, e.end_vertex] {
                        if let Some(p) = model.vertices.get_position(vid) {
                            for i in 0..3 {
                                min[i] = min[i].min(p[i]);
                                max[i] = max[i].max(p[i]);
                            }
                            any = true;
                        }
                    }
                }
            }
        }
    }
    if !any {
        return 1.0;
    }
    let d = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6)
}

pub fn tessellate_solid(
    solid: &Solid,
    model: &BRepModel,
    params: &TessellationParams,
) -> TriangleMesh {
    // Size-relative chord FLOOR. An absolute chord (the 0.001 mm default) is a
    // size-blind deviation: on a 178 mm part it is 178000:1, which both wastes
    // wall-clock (build jitter) and pushes adjacent analytic bands into a
    // non-conforming fine-density regime (KNOWN_BUGS REVOLVE-TESS-SEAM). Floor
    // the chord at a small fraction of the part's size so it can only get
    // COARSER, never finer — coarse explicit chords (manifold_report's 0.5,
    // preview's 0.01) are above the floor and untouched; only pathological
    // over-tessellation is clamped. ~5e-4·diagonal ≈ a 6° facet on the part's
    // gross radius — smooth for display AND watertight for export.
    let floored;
    let params = {
        const REL_FLOOR: f64 = 5.0e-4;
        let floor = solid_extent(solid, model) * REL_FLOOR;
        if params.chord_tolerance > 0.0 && params.chord_tolerance < floor {
            floored = TessellationParams {
                chord_tolerance: floor,
                ..params.clone()
            };
            &floored
        } else {
            params
        }
    };
    let mut mesh = TriangleMesh::new();

    // A single canonical edge-sample cache is shared by every face of
    // every shell in this solid. This guarantees that any B-Rep edge
    // bounding two or more faces sees the SAME 3D sample sequence
    // along its length — which is the Parasolid-style invariant that
    // eliminates tessellation T-junctions across shared edges. See
    // `tessellation::edge_cache` for the full rationale.
    let cache = EdgeSampleCache::new(params);

    // Tessellate outer shell
    if let Some(shell) = model.shells.get(solid.outer_shell) {
        tessellate_shell(shell, model, params, &cache, &mut mesh);
    }

    // Tessellate inner shells (voids)
    for &inner_shell_id in &solid.inner_shells {
        if let Some(shell) = model.shells.get(inner_shell_id) {
            tessellate_shell(shell, model, params, &cache, &mut mesh);
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
    cache: &EdgeSampleCache,
    mesh: &mut TriangleMesh,
) {
    let weld_start_vertices = mesh.vertices.len();
    let weld_start_triangles = mesh.triangles.len();
    // Per-face timing trace, gated on ROSHERA_TESS_TRACE. Used to find
    // where wall-clock goes when a solid tessellates slower than its
    // triangle count justifies: prints surface type, triangle yield and
    // microseconds per face, plus the shell weld at the end.
    let trace = std::env::var("ROSHERA_TESS_TRACE").is_ok();
    for &face_id in &shell.faces {
        if let Some(face) = model.faces.get(face_id) {
            let tri_start = mesh.triangles.len();
            let t0 = trace.then(std::time::Instant::now);
            surface::tessellate_face(face, model, params, cache, mesh);
            let tri_end = mesh.triangles.len();
            if let Some(t0) = t0 {
                let surface_kind = model
                    .surfaces
                    .get(face.surface_id)
                    .map(|s| s.type_name())
                    .unwrap_or("?");
                eprintln!(
                    "[tess] face {} {} tris={} {}us",
                    face_id,
                    surface_kind,
                    tri_end - tri_start,
                    t0.elapsed().as_micros()
                );
            }
            // Record which B-Rep face each new triangle came from
            for _ in tri_start..tri_end {
                mesh.face_map.push(face_id);
            }
        }
    }
    let weld_t0 = trace.then(std::time::Instant::now);

    // Run welding only over the vertices/triangles produced by THIS
    // shell call so that callers passing a pre-populated mesh (e.g.
    // multi-shell solids in `tessellate_solid`) don't have their
    // earlier shell's vertex indices invalidated.
    if mesh.vertices.len() > weld_start_vertices && mesh.triangles.len() > weld_start_triangles {
        // The weld merges *coincident* seam vertices — shared B-Rep edges emit
        // bit-exact duplicate 3D points via the EdgeSampleCache — so its
        // threshold is a geometric-coincidence distance, NOT the visual chord
        // tolerance. Using the raw `chord_tolerance` over-merges at coarse
        // settings: when surface curvature drives the triangle density (e.g. a
        // sphere's angle-limited grid), the edge length can be ~`chord_tolerance`
        // itself, so welding at that distance collapses every vertex and deletes
        // the whole mesh (a coarse-LOD sphere came out invisible — found by the
        // tessellation ablation harness). Cap it well below any realistic edge
        // spacing; the cap equals the default chord so fine tessellation (the
        // production path) is byte-for-byte unchanged, and bit-exact seams still
        // merge at any positive distance.
        const MAX_WELD_DISTANCE: f64 = 1e-3;
        let weld_distance = params.chord_tolerance.min(MAX_WELD_DISTANCE);
        surface::weld_mesh_watertight_range(
            mesh,
            weld_distance,
            weld_start_vertices,
            weld_start_triangles,
        );
    }
    if let Some(t0) = weld_t0 {
        eprintln!(
            "[tess] shell weld: {} verts {} tris {}us",
            mesh.vertices.len() - weld_start_vertices,
            mesh.triangles.len() - weld_start_triangles,
            t0.elapsed().as_micros()
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

    /// Count how many times each undirected edge appears across the mesh's
    /// triangles, **welding vertices by quantised position first**. Tessellation
    /// intentionally keeps coincident-but-sharp-edged samples as distinct mesh
    /// vertices so each face retains its own shading normal (see
    /// `weld_mesh_watertight_range`'s normal-aware gate), so raw triangle indices
    /// do NOT share across a sharp seam. The meaningful watertightness invariant
    /// is geometric — every *position* edge borders exactly two triangles — so we
    /// re-weld by position here, exactly as the manifold oracle / STL export do.
    fn edge_use_counts(mesh: &TriangleMesh) -> HashMap<(u32, u32), u32> {
        // Position-weld to canonical indices (1e-6 lattice, well under any edge).
        let eps = 1e-6;
        let key3 = |p: crate::math::Point3| {
            (
                (p.x / eps).round() as i64,
                (p.y / eps).round() as i64,
                (p.z / eps).round() as i64,
            )
        };
        let mut canon: HashMap<(i64, i64, i64), u32> = HashMap::new();
        let mut remap: Vec<u32> = Vec::with_capacity(mesh.vertices.len());
        for v in &mesh.vertices {
            let next = canon.len() as u32;
            remap.push(*canon.entry(key3(v.position)).or_insert(next));
        }
        let mut counts: HashMap<(u32, u32), u32> = HashMap::new();
        for tri in &mesh.triangles {
            let (t0, t1, t2) = (
                remap[tri[0] as usize],
                remap[tri[1] as usize],
                remap[tri[2] as usize],
            );
            for &(a, b) in &[(t0, t1), (t1, t2), (t2, t0)] {
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

    /// Assert that no two distinct vertex indices used in any triangle share the
    /// same 3D position within `tol` **AND the same normal**. Two referenced
    /// vertices at one position with *different* normals are expected and correct
    /// — they are a sharp seam intentionally split so each face keeps its own
    /// shading normal (the normal-aware weld). A true welding failure is a
    /// position+normal duplicate (a smooth seam that should have collapsed to one
    /// vertex but didn't). Orphaned (un-referenced) vertices are ignored.
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
                let va = mesh.vertices[referenced[i] as usize];
                let vb = mesh.vertices[referenced[j] as usize];
                let d = va.position - vb.position;
                let d2 = d.x * d.x + d.y * d.y + d.z * d.z;
                // Same position AND near-identical normal ⇒ a genuine unwelded
                // smooth-seam duplicate. Same position, divergent normal ⇒ an
                // intended sharp-edge split (allowed).
                let same_normal = va.normal.dot(&vb.normal) >= 0.999;
                assert!(
                    d2 > tol_sq || !same_normal,
                    "referenced vertices {a} and {b} share position {pa:?} ≈ {pb:?} AND normal \
                     {na:?} ≈ {nb:?} (distance² = {d2:e}, tol² = {tol_sq:e}) — welding failed",
                    a = referenced[i],
                    b = referenced[j],
                    pa = va.position,
                    pb = vb.position,
                    na = va.normal,
                    nb = vb.normal,
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

    /// K14 — sharp-edge normal preservation. Two coincident vertices with
    /// normals 90° apart (the box-corner case) must NOT be welded into one — the
    /// normal-aware gate keeps them as distinct vertices so EACH face retains its
    /// own correct shading normal. (Welding them would force one shared vertex to
    /// carry a single normal, shading one face as if it faced the wrong way — the
    /// box-side-face bug the tessellation normal-agreement oracle catches.)
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

        // The two coincident sharp-edge vertices must stay DISTINCT, each keeping
        // its own face normal — the side triangle still references `v_side`
        // (normal +X), the top triangle still references `v_top` (normal +Z).
        assert!(
            mesh.triangles[1].contains(&v_side),
            "sharp-edge vertex must not weld into the other face: triangle {:?} \
             should still reference v_side {v_side}",
            mesh.triangles[1]
        );
        let n_top_kept = mesh.vertices[v_top as usize].normal;
        let n_side_kept = mesh.vertices[v_side as usize].normal;
        assert!(
            (n_top_kept - n_top).magnitude() < 1e-12,
            "top-face normal must be preserved (= {n_top:?}), got {n_top_kept:?}"
        );
        assert!(
            (n_side_kept - n_side).magnitude() < 1e-12,
            "side-face normal must be preserved (= {n_side:?}), got {n_side_kept:?}"
        );
    }
}
