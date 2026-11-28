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
pub(crate) mod trim_index;

#[cfg(test)]
mod planar_collinear_run_tests;

// Re-export main types
pub use adaptive::AdaptiveTessellator;
pub use curve::{tessellate_curve, tessellate_edge};
pub use mesh::{MeshVertex, ThreeJsMesh, TriangleMesh, WeldRefusals};
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

    /// Parameters for the COARSE internal self-intersection certificate — a hard
    /// segment ceiling (`max_segments 24`) bounding the triangle count that the
    /// O(n²) mesh self-intersection pair scan
    /// ([`crate::harness::self_intersection::mesh_self_intersects`]) runs over.
    ///
    /// The certificate's self-overlap check was the dominant cost of the per-op
    /// verification audit: a curved-Boolean arrangement-cell fragment sampled at
    /// `default()`/`fine()` density produces thousands of rim triangles, and the
    /// quadratic pair scan over them ran for tens of seconds per op. Gross self-
    /// overlap is a coarse geometric fault, visible at low density (the check's
    /// own stated contract), so a 24-segment mesh detects it just as reliably
    /// while keeping `n²` small. The spherical-fan triangle budget also tightens
    /// for sub-100-segment presets (see `surface.rs`), reinforcing the bound.
    ///
    /// Coarse-grade — between `coarse()` and `realtime()`. DISPLAY/EXPORT
    /// tessellation, the agent-facing `mass_properties_for`, and the watertight /
    /// audit mass-properties oracles all stay at `fine()`/`default()` — only the
    /// self-intersection scan uses this. Precedent: `solid_gwn_triangles` already
    /// uses `coarse()` for GWN classification.
    pub fn audit() -> Self {
        Self {
            max_edge_length: 0.5,
            max_angle_deviation: 0.3,
            chord_tolerance: 0.05,
            min_segments: 6,
            max_segments: 24,
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

    /// Parameters for the mesh that mass properties integrate over:
    /// [`Self::fine`] with its maximum edge length floored at
    /// [`MASS_PROPERTIES_EDGE_FLOOR`] of the part's size.
    ///
    /// `fine()`'s 0.01 mm edge length is absolute, so on any part much
    /// larger than a millimetre it pins every curved direction at the
    /// segment cap. A 20 mm gear from the robot-arm assembly integrated
    /// over 1.7 million triangles (1006 s, debug); floored, 69 thousand
    /// (20 s). Chord tolerance, angle and segment limits stay `fine()`'s.
    /// The edge length only sets element size; the 0.02 rad angle limit,
    /// which every curved direction honours on its boundary and in its
    /// interior grid, bounds each feature's error relative to that feature.
    /// (`tessellate_solid` floors the chord to the part's size, so the
    /// chord cannot do that on a small feature of a large part.)
    pub fn mass_properties(solid: &Solid, model: &BRepModel) -> Self {
        let fine = Self::fine();
        let floor = solid_extent(solid, model) * MASS_PROPERTIES_EDGE_FLOOR;
        Self {
            max_edge_length: fine.max_edge_length.max(floor),
            ..fine
        }
    }

    /// Parameters for the LIVE viewport broadcast — the mesh the server
    /// tessellates after every interactive geometry op and pushes to the
    /// frontend. `default()`'s 1-micron chord (and up to 100 segments per
    /// curve) is export-grade overkill for a screen-space preview and is
    /// the dominant per-op latency. This preset is ~10× lighter on the
    /// chord guard while keeping curves visibly smooth (up to 64 segments,
    /// ≈0.15 rad facets) so a part still reads cleanly on camera.
    ///
    /// Safe to coarsen because the live perception verdict (`watertight`,
    /// `sound`) is computed from the EXACT B-Rep (`validate_solid_scoped`),
    /// not from this display mesh — coarsening the preview can never flip
    /// the soundness signal. The fine/`default()` mesh is still used for
    /// export and the analytic agent-eye render.
    pub fn display() -> Self {
        Self {
            max_edge_length: 0.5,
            max_angle_deviation: 0.15,
            chord_tolerance: 0.01,
            min_segments: 8,
            max_segments: 64,
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
    shells.extend_from_slice(&solid.peer_shells);
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

/// Floor on the mass-properties mesh's edge length, as a fraction of the
/// part's size (see [`TessellationParams::mass_properties`]). Measured on
/// the robot arm's gear: 0.2 % meshed in 44 s, 0.5 % in 20 s, 1 % in 40 s
/// (more refinement), each within 1e-4 of the unfloored volume.
pub const MASS_PROPERTIES_EDGE_FLOOR: f64 = 5.0e-3;

/// Most worker threads [`tessellate_solids`] uses. Each worker holds one
/// solid's mesh while it builds, so this bounds peak memory as well as CPU.
const TESSELLATE_BATCH_THREADS: usize = 4;

/// [`tessellate_solid`] for many solids, in parallel on a small bounded
/// pool — the display meshes of an imported assembly or a scene snapshot,
/// which are independent per solid. Each mesh is exactly the one
/// `tessellate_solid` builds alone. Ids not in the model are skipped; the
/// rest come back in input order.
pub fn tessellate_solids(
    model: &BRepModel,
    solid_ids: &[crate::primitives::solid::SolidId],
    params: &TessellationParams,
) -> Vec<(crate::primitives::solid::SolidId, TriangleMesh)> {
    let work = || {
        use rayon::prelude::*;
        solid_ids
            .par_iter()
            .filter_map(|&id| {
                model
                    .solids
                    .get(id)
                    .map(|solid| (id, tessellate_solid(solid, model, params)))
            })
            .collect::<Vec<_>>()
    };
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, TESSELLATE_BATCH_THREADS);
    match rayon::ThreadPoolBuilder::new().num_threads(threads).build() {
        Ok(pool) => pool.install(work),
        Err(_) => work(),
    }
}

/// Fraction of a part's size below which [`effective_params`] will not cap a
/// mesh edge's length. Half a percent: 0.65 mm on a 130 mm part, 5 mm on a
/// metre -- far below any feature a drawing or a certificate reads, while
/// the chord (curvature) and angle limits still shape every curved face.
pub(crate) const REL_EDGE_FLOOR: f64 = 5.0e-3;

/// The parameters [`tessellate_solid`] actually meshes `solid` with.
///
/// Size-relative chord FLOOR. An absolute chord (the 0.001 mm default) is a
/// size-blind deviation: on a 178 mm part it is 178000:1, which both wastes
/// wall-clock (build jitter) and pushes adjacent analytic bands into a
/// non-conforming fine-density regime (KNOWN_BUGS REVOLVE-TESS-SEAM). The
/// chord is floored at a small fraction of the part's size so it can only get
/// COARSER, never finer -- coarse explicit chords (manifold_report's 0.5,
/// preview's 0.01) are above the floor and untouched; only pathological
/// over-tessellation is clamped. ~5e-4 * diagonal is about a 6 deg facet on the
/// part's gross radius -- smooth for display AND watertight for export. Two
/// requests with equal effective parameters mesh to the same mesh.
///
/// The edge-length cap is floored the same way, at [`REL_EDGE_FLOOR`] of the
/// part's size. An absolute cap (the 0.1 mm default) cuts a long, thin face
/// into pieces of that length whatever its curvature: every fillet of a
/// 130 mm turned part became hundreds of 0.1 mm strips, 1.17 million
/// triangles for 779 faces. The cap can only get coarser; a curved face is
/// still held to the chord and angle limits.
pub(crate) fn effective_params(
    solid: &Solid,
    model: &BRepModel,
    params: &TessellationParams,
) -> TessellationParams {
    const REL_FLOOR: f64 = 5.0e-4;
    let extent = solid_extent(solid, model);
    let floor = extent * REL_FLOOR;
    let edge_floor = extent * REL_EDGE_FLOOR;
    let mut effective = params.clone();
    if params.chord_tolerance > 0.0 && params.chord_tolerance < floor {
        effective.chord_tolerance = floor;
    }
    if params.max_edge_length > 0.0 && params.max_edge_length < edge_floor {
        effective.max_edge_length = edge_floor;
    }
    effective
}

/// `params` as comparable bits: equal keys, equal meshes.
pub(crate) fn params_key(params: &TessellationParams) -> [u64; 5] {
    [
        params.max_edge_length.to_bits(),
        params.max_angle_deviation.to_bits(),
        params.chord_tolerance.to_bits(),
        params.min_segments as u64,
        params.max_segments as u64,
    ]
}

#[cfg(test)]
thread_local! {
    /// Every `tessellate_solid` call on this thread: the solid and the
    /// effective parameters (test instrumentation -- a caller must not mesh
    /// the same solid twice at the same parameters).
    pub(crate) static TESSELLATIONS: std::cell::RefCell<Vec<(u32, [u64; 5])>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

pub fn tessellate_solid(
    solid: &Solid,
    model: &BRepModel,
    params: &TessellationParams,
) -> TriangleMesh {
    // Size-relative chord FLOOR: see `effective_params`.
    let params = &effective_params(solid, model, params);
    #[cfg(test)]
    TESSELLATIONS.with(|t| t.borrow_mut().push((solid.id, params_key(params))));
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

    // Tessellate the remaining shells — voids AND peer bodies. A peer is
    // material, so its surface belongs in the mesh exactly like the outer
    // shell's; this path already summed peers positively while they were
    // mis-filed as voids, and that must not change.
    for &inner_shell_id in solid.inner_shells.iter().chain(solid.peer_shells.iter()) {
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
    tessellate_shell_with(shell, model, params, cache, mesh, true);
}

/// Rounds of [`resample_clashing_boundaries`].
const BOUNDARY_CLASH_ROUNDS: usize = 6;

/// One face's mesh as [`tessellate_shell_with`] collects it: the face, its
/// surface, its triangles and how long they took.
type FacePart = (u32, u32, TriangleMesh, std::time::Duration);

/// Where a face could not be meshed soundly because the CHORDS of two of its
/// boundary edges clash although the edges do not (see
/// [`edge_cache::BoundaryClash`]), sample the offending edge twice as finely
/// and mesh again every face of the shell that uses it -- the face itself
/// and its neighbour across the edge, which must read the same samples.
///
/// Only a face that was left with nothing (its outline refused as crossing
/// itself) or with facets turned against its surface is a reason to
/// resample; every other face of the shell keeps the mesh it has, bit for
/// bit, and a shell with no such face is not touched at all. Repeats while
/// faces still report clashes, at most [`BOUNDARY_CLASH_ROUNDS`] times; an
/// edge is doubled at most a fixed number of times, after which a face that
/// still cannot be meshed stays refused, as before.
fn resample_clashing_boundaries(
    shell: &Shell,
    model: &BRepModel,
    cache: &EdgeSampleCache,
    parts: &mut [FacePart],
    mesh_faces: &dyn Fn(&[u32]) -> Vec<FacePart>,
    trace: bool,
) {
    use edge_cache::BoundaryClashKind;
    for round in 0..BOUNDARY_CLASH_ROUNDS {
        let clashes: Vec<_> = cache
            .take_boundary_clashes()
            .into_iter()
            .filter(|clash| {
                parts.iter().any(|part| {
                    part.0 == clash.face
                        && match clash.kind {
                            // A crossing outline that some other path still
                            // meshed is not a face left with nothing.
                            BoundaryClashKind::Crossing => part.2.triangles.is_empty(),
                            BoundaryClashKind::TurnedFacet => true,
                        }
                })
            })
            .collect();
        if clashes.is_empty() {
            return;
        }
        let refined = cache.refine_clashing_edges(model, &clashes);
        if refined.is_empty() {
            return;
        }
        let uses_refined = |face_id: u32| {
            model.faces.get(face_id).is_some_and(|face| {
                std::iter::once(face.outer_loop)
                    .chain(face.inner_loops.iter().copied())
                    .filter_map(|l| model.loops.get(l))
                    .any(|l| l.edges.iter().any(|e| refined.contains(e)))
            })
        };
        let again: Vec<u32> = shell
            .faces
            .iter()
            .copied()
            .filter(|&f| uses_refined(f))
            .collect();
        if trace {
            eprintln!(
                "[tess] boundary clash round {round}: {} clash(es), edges {refined:?} resampled, faces {again:?} meshed again",
                clashes.len()
            );
        }
        for redone in mesh_faces(&again) {
            if let Some(slot) = parts.iter_mut().find(|part| part.0 == redone.0) {
                *slot = redone;
            }
        }
    }
    // Clashes of the last round stay unanswered; they are not the next
    // shell's.
    let _ = cache.take_boundary_clashes();
}

/// Faces of a shell are meshed independently -- a face reads only the model
/// and the edge-sample cache (a concurrent map; an edge's samples are the
/// same whichever face asks first) -- so they mesh in parallel, each into its
/// own mesh, appended in shell order. The result is identical to meshing them
/// one after another (`parallel = false`), which a test compares bit for bit.
/// The weld then runs over the whole shell as before.
pub(crate) fn tessellate_shell_with(
    shell: &Shell,
    model: &BRepModel,
    params: &TessellationParams,
    cache: &EdgeSampleCache,
    mesh: &mut TriangleMesh,
    parallel: bool,
) {
    let weld_start_vertices = mesh.vertices.len();
    let weld_start_triangles = mesh.triangles.len();
    // Per-face timing trace, gated on ROSHERA_TESS_TRACE. Used to find
    // where wall-clock goes when a solid tessellates slower than its
    // triangle count justifies: prints surface type, triangle yield and
    // microseconds per face, plus the shell weld at the end.
    let trace = std::env::var("ROSHERA_TESS_TRACE").is_ok();
    let mesh_one = |face_id: u32| {
        let face = model.faces.get(face_id)?;
        let t0 = std::time::Instant::now();
        let mut part = TriangleMesh::new();
        surface::tessellate_face(face, model, params, cache, &mut part);
        Some((face_id, face.surface_id, part, t0.elapsed()))
    };
    let mesh_faces = |faces: &[u32]| -> Vec<_> {
        if parallel && faces.len() > 1 {
            use rayon::prelude::*;
            faces.par_iter().filter_map(|&f| mesh_one(f)).collect()
        } else {
            faces.iter().filter_map(|&f| mesh_one(f)).collect()
        }
    };
    let mut parts = mesh_faces(&shell.faces);
    resample_clashing_boundaries(shell, model, cache, &mut parts, &mesh_faces, trace);
    for (face_id, surface_id, part, elapsed) in parts {
        if trace {
            let surface_kind = model
                .surfaces
                .get(surface_id)
                .map(|s| s.type_name())
                .unwrap_or("?");
            eprintln!(
                "[tess] face {} {} tris={} {}us",
                face_id,
                surface_kind,
                part.triangles.len(),
                elapsed.as_micros()
            );
        }
        let base = mesh.vertices.len() as u32;
        let added = part.triangles.len();
        mesh.vertices.extend(part.vertices);
        mesh.triangles.extend(
            part.triangles
                .into_iter()
                .map(|t| [t[0] + base, t[1] + base, t[2] + base]),
        );
        // Record which B-Rep face each new triangle came from.
        mesh.face_map.extend(std::iter::repeat(face_id).take(added));
        mesh.weld_refusals.non_finite += part.weld_refusals.non_finite;
        mesh.weld_refusals.unaddressable += part.weld_refusals.unaddressable;
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
        // tessellation ablation harness).
        //
        // Slice 5 (tolerance authority): the cap is `authority::MESH_WELD_CAP`
        // (= 4·τ_weld), NOT the former 1e-3. Seam duplicates are bit-exact by
        // the EdgeSampleCache contract (plus ≤~1e-9 evaluation noise on the
        // few non-cache stitched paths), so 4e-6 still welds every genuine
        // seam — while a REAL sub-millimetre feature (a 1e-4 ledge from a
        // near-coincident union, a 1e-5 sliver wall) no longer has its
        // vertices collapsed pairwise, which deleted its triangles and tore a
        // hole in the certification mesh (`watertight=false` on a VALID
        // B-Rep — the flush-upstand ε∈[1e-6,1e-4] danger band's mesh half).
        // Features thinner than MESH_WELD_CAP are below the certification
        // mesh's resolution; the plane-unification pre-pass (τ_coincide =
        // 10·τ_weld > this cap) guarantees boolean output cannot carry
        // cross-operand near-coincidence in that blind zone.
        let weld_distance = params
            .chord_tolerance
            .min(crate::math::tolerance::authority::MESH_WELD_CAP);
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

    /// The refusal path, at the production entry point rather than at
    /// `weld_cell`: a mesh that mixes a vertex the grid cannot address with a
    /// genuinely coincident pair must weld the pair, keep the un-addressable
    /// vertex as its own canonical, and COUNT it - on the returned report and
    /// on the mesh, which is the only channel a downstream certificate has.
    #[test]
    fn weld_mesh_watertight_counts_refused_vertices_and_still_welds_the_rest() {
        use crate::math::Vector3;
        let mut mesh = TriangleMesh::new();
        // The weldable pair: two faces sharing edge (a1,a2), emitted twice.
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
        // The un-addressable vertex: a NaN coordinate has no cell at all. Its
        // triangle's other two corners are distinct and finite, so nothing
        // else about this triangle can drop it.
        let nan = mesh.add_vertex(MeshVertex {
            position: Point3::new(f64::NAN, 0.0, 0.0),
            normal: Vector3::Z,
            uv: None,
        });
        let c1 = mesh.add_vertex(MeshVertex {
            position: Point3::new(5.0, 0.0, 0.0),
            normal: Vector3::Z,
            uv: None,
        });
        let c2 = mesh.add_vertex(MeshVertex {
            position: Point3::new(5.0, 1.0, 0.0),
            normal: Vector3::Z,
            uv: None,
        });
        mesh.add_triangle(a0, a1, a2);
        mesh.add_triangle(b0, b1, b2);
        mesh.add_triangle(nan, c1, c2);

        let report = surface::weld_mesh_watertight(&mut mesh, 1e-6);

        assert_eq!(
            report.refusals.non_finite, 1,
            "the NaN vertex must be COUNTED as refused, not silently skipped"
        );
        assert_eq!(report.refusals.unaddressable, 0);
        assert_eq!(
            mesh.weld_refusals, report.refusals,
            "the counts must ride out on the mesh - the certificate reads them there"
        );
        assert_eq!(
            report.welded, 2,
            "the coincident pair must still weld: {report:?}"
        );

        assert_eq!(mesh.triangles.len(), 3, "no triangle should be dropped");
        assert!(
            mesh.triangles
                .iter()
                .any(|t| t.contains(&a1) && t.contains(&a2) && t.contains(&b2)),
            "face B's triangle must collapse onto the welded {a1}/{a2}: {:?}",
            mesh.triangles
        );
        assert!(
            mesh.triangles.iter().any(|t| t.contains(&nan)),
            "the refused vertex stays its own canonical: {:?}",
            mesh.triangles
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

        // Second-shell triangles have q3 collapsed to q0, so the second
        // shell triangle (q3, q1, q2) becomes (q0, q1, q2) — an exact,
        // same-winding duplicate of the first shell triangle. The weld
        // pass's doubled-facet removal (#65) collapses same-winding exact
        // duplicates to a single representative (an emitted-twice facet is
        // a non-manifold fin over an already-covered patch), so exactly ONE
        // of the two survives. The head triangle, being outside the weld
        // range, is untouched. `face_map` shrinks in lock-step with the
        // surviving triangles.
        assert_eq!(mesh.triangles.len(), 2);
        assert_eq!(mesh.triangles[1], [q0, q1, q2]); // q3 → q0; duplicate dropped
        assert_eq!(mesh.face_map, vec![0, 1]);
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

/// Meshing a shell's faces in parallel must give exactly the mesh that
/// meshing them one after another gives: same vertices (bit for bit), same
/// triangles, same face map. Repeated, so a scheduling-dependent difference
/// (an edge's samples depending on which face asked first) shows up.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod parallel_shell_tests {
    use super::*;
    use crate::math::{Point3, Vector3};
    use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use crate::operations::nurbs_loft::{nurbs_loft, NurbsLoftOptions};
    use crate::primitives::topology_builder::TopologyBuilder;

    fn mesh_of(model: &BRepModel, solid: &Solid, parallel: bool) -> TriangleMesh {
        let params = TessellationParams::default();
        let cache = EdgeSampleCache::new(&params);
        let mut mesh = TriangleMesh::new();
        for sh in solid.all_shells() {
            if let Some(shell) = model.shells.get(sh) {
                tessellate_shell_with(shell, model, &params, &cache, &mut mesh, parallel);
            }
        }
        mesh
    }

    fn fingerprint(m: &TriangleMesh) -> (Vec<[u64; 8]>, Vec<[u32; 3]>, Vec<u32>) {
        let verts = m
            .vertices
            .iter()
            .map(|v| {
                let (u, w) = v.uv.unwrap_or((f64::NAN, f64::NAN));
                [
                    v.position.x.to_bits(),
                    v.position.y.to_bits(),
                    v.position.z.to_bits(),
                    v.normal.x.to_bits(),
                    v.normal.y.to_bits(),
                    v.normal.z.to_bits(),
                    u.to_bits(),
                    w.to_bits(),
                ]
            })
            .collect();
        (verts, m.triangles.clone(), m.face_map.clone())
    }

    fn ring(n: usize, r: f64, z: f64) -> Vec<Point3> {
        (0..n)
            .map(|i| {
                let a = i as f64 * std::f64::consts::TAU / n as f64;
                Point3::new(r * a.cos(), r * a.sin(), z)
            })
            .collect()
    }

    fn fixtures() -> Vec<(&'static str, BRepModel)> {
        let mut out = Vec::new();
        // A block with a bore: planes, a trimmed cylinder, holes.
        let mut m = BRepModel::new();
        {
            let mut b = TopologyBuilder::new(&mut m);
            b.create_box_3d(40.0, 30.0, 20.0).expect("box");
            b.create_cylinder_3d(Point3::new(0.0, 0.0, -15.0), Vector3::Z, 6.0, 30.0)
                .expect("cylinder");
        }
        let ids: Vec<_> = m.solids.iter().map(|(id, _)| id).collect();
        boolean_operation(
            &mut m,
            ids[0],
            ids[1],
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("bore");
        out.push(("bored block", m));
        let mut m = BRepModel::new();
        TopologyBuilder::new(&mut m)
            .create_sphere_3d(Point3::ZERO, 12.0)
            .expect("sphere");
        out.push(("sphere", m));
        let mut m = BRepModel::new();
        nurbs_loft(
            &mut m,
            vec![
                ring(20, 20.0, 0.0),
                ring(20, 30.0, 15.0),
                ring(20, 35.0, 30.0),
                ring(20, 20.0, 60.0),
            ],
            NurbsLoftOptions::default(),
        )
        .expect("barrel");
        out.push(("NURBS barrel", m));
        out
    }

    #[test]
    fn parallel_face_meshing_is_bit_identical_to_serial() {
        for (name, model) in fixtures() {
            for (_, solid) in model.solids.iter() {
                let serial = fingerprint(&mesh_of(&model, solid, false));
                assert!(!serial.1.is_empty(), "{name}: meshed nothing");
                for round in 0..3 {
                    let parallel = fingerprint(&mesh_of(&model, solid, true));
                    assert_eq!(
                        serial.0.len(),
                        parallel.0.len(),
                        "{name} round {round}: vertex count"
                    );
                    assert!(
                        serial == parallel,
                        "{name} round {round}: parallel mesh differs from serial"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod certificate_mesh_reuse_tests {
    use super::*;
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    /// The self-certificate meshes a solid for its topology checks and again
    /// for its display-quality checks. On a part large enough that its size
    /// floor lifts the display chord to the certification chord, the two
    /// meshes are the same mesh, and it was built twice (CADGenBench part 211:
    /// 46 s + 27 s). No two meshings of one solid at equal effective
    /// parameters.
    #[test]
    fn certifying_a_large_part_meshes_it_once_per_parameter_set() {
        let mut model = BRepModel::new();
        let solid = match TopologyBuilder::new(&mut model)
            .create_box_3d(300.0, 300.0, 300.0)
            .expect("box")
        {
            GeometryId::Solid(s) => s,
            other => panic!("a box is a solid: {other:?}"),
        };
        TESSELLATIONS.with(|t| t.borrow_mut().clear());
        let cert = model.certify_solid(solid);
        assert!(
            cert.is_sound(),
            "the box certifies sound: {:?}",
            cert.errors
        );
        let calls = TESSELLATIONS.with(|t| t.borrow().clone());
        let mut seen = std::collections::HashSet::new();
        let repeats: Vec<_> = calls.iter().filter(|c| !seen.insert(**c)).collect();
        assert!(
            repeats.is_empty(),
            "the certificate meshed one solid twice at the same parameters: {repeats:?} (all: {calls:?})"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod size_relative_edge_tests {
    use super::*;
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn block(model: &mut BRepModel, size: f64) -> crate::primitives::solid::SolidId {
        match TopologyBuilder::new(model)
            .create_box_3d(size, size / 2.0, size / 4.0)
            .expect("box")
        {
            GeometryId::Solid(s) => s,
            other => panic!("a box is a solid: {other:?}"),
        }
    }

    /// The edge-length cap scales with the part, as the chord floor does: an
    /// absolute 0.1 mm cap cut every long, thin fillet of a 130 mm turned
    /// part (CADGenBench 109) into 0.1 mm pieces -- 1.17 million triangles
    /// for 779 faces and 21 s of certificate meshing. A small part keeps the
    /// requested cap; a large part's cap is a fixed fraction of its size; and
    /// a cap already coarser than that is left alone.
    #[test]
    fn the_edge_length_cap_scales_with_the_part() {
        let mut model = BRepModel::new();
        let small = block(&mut model, 10.0);
        let large = block(&mut model, 200.0);
        let fine = TessellationParams::default();
        let get = |id| model.solids.get(id).expect("solid");

        let small_p = effective_params(get(small), &model, &fine);
        assert_eq!(
            small_p.max_edge_length, fine.max_edge_length,
            "a 10 mm part keeps 0.1 mm"
        );

        let large_p = effective_params(get(large), &model, &fine);
        let extent = solid_extent(get(large), &model);
        assert!(
            (large_p.max_edge_length - extent * REL_EDGE_FLOOR).abs() < 1e-12,
            "a {extent:.0} mm part's cap is {} mm",
            large_p.max_edge_length
        );
        assert!(large_p.max_edge_length > 1.0, "well above 0.1 mm");

        let coarse = TessellationParams {
            max_edge_length: 50.0,
            ..fine.clone()
        };
        assert_eq!(
            effective_params(get(large), &model, &coarse).max_edge_length,
            50.0,
            "a coarser cap is untouched"
        );
        let none = TessellationParams {
            max_edge_length: 0.0,
            ..fine
        };
        assert_eq!(
            effective_params(get(large), &model, &none).max_edge_length,
            0.0,
            "no cap stays no cap"
        );
    }
}

#[cfg(test)]
mod boundary_clash_tests {
    //! Two edges of a face that leave a vertex at a shallow angle never
    //! cross, but their CHORDS do when the outer one is sampled in longer
    //! steps than the inner: the long chord cuts the corner and passes
    //! through the short ones. The outline is then not a polygon, the face
    //! was refused and left a hole in the shell (CADGenBench part 134).
    use super::*;
    use crate::math::nurbs::{
        interpolate_nurbs_curve, NurbsSurface as MathNurbs, ParameterizationType,
    };
    use crate::math::{Point3, Vector3};
    use crate::primitives::curve::{NurbsCurve, ParameterRange};
    use crate::primitives::edge::{Edge, EdgeOrientation};
    use crate::primitives::face::{Face, FaceOrientation};
    use crate::primitives::r#loop::{Loop, LoopType};
    use crate::primitives::shell::{Shell, ShellType};
    use crate::primitives::surface::{GeneralNurbsSurface, Plane};
    use crate::primitives::topology_builder::BRepModel;
    use std::f64::consts::PI;

    const OUTER: f64 = 1.0;
    const INNER: f64 = 0.97;

    /// The curve through `points`, parameterised uniformly over them.
    fn curve_through(points: &[Point3]) -> (NurbsCurve, ParameterRange) {
        let fit = interpolate_nurbs_curve(points, 3, ParameterizationType::Uniform).expect("fit");
        let knots = fit.knots.values().to_vec();
        let range = ParameterRange::new(knots[0], knots[knots.len() - 1]);
        let curve = NurbsCurve::new(
            fit.degree,
            fit.control_points.clone(),
            fit.weights.clone(),
            knots,
        )
        .expect("curve");
        (curve, range)
    }

    /// A lens in the plane z = 0 (on a flat NURBS patch, or with `plane` on a
    /// plane proper, which is meshed by the planar path), between
    /// y = 0.97 sin x below and y = sin x above, for x in [0, pi]. The upper
    /// edge runs at even speed; the lower one starts slowly (x = pi t^2), so
    /// equal steps of its parameter are short near x = 0. Returns the model,
    /// the shell, and the (lower, upper) edges.
    fn lens(plane: bool) -> (BRepModel, Shell, u32, u32) {
        let mut model = BRepModel::new();
        let cp = vec![
            vec![Point3::new(-1.0, -2.0, 0.0), Point3::new(-1.0, 2.0, 0.0)],
            vec![Point3::new(5.0, -2.0, 0.0), Point3::new(5.0, 2.0, 0.0)],
        ];
        // u runs along +X and v along +Y, so the patch's normal is +Z.
        let patch = MathNurbs::new(
            cp,
            vec![vec![1.0, 1.0], vec![1.0, 1.0]],
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
            1,
            1,
        )
        .expect("patch");
        let surface = if plane {
            model.surfaces.add(Box::new(
                Plane::new(Point3::new(0.0, 0.0, 0.0), Vector3::Z, Vector3::X).expect("plane"),
            ))
        } else {
            model
                .surfaces
                .add(Box::new(GeneralNurbsSurface { nurbs: patch }))
        };
        let n = 64;
        let upper_pts: Vec<Point3> = (0..=n)
            .map(|k| {
                let x = PI * k as f64 / n as f64;
                Point3::new(x, OUTER * x.sin(), 0.0)
            })
            .collect();
        let lower_pts: Vec<Point3> = (0..=n)
            .map(|k| {
                let t = k as f64 / n as f64;
                let x = PI * t * t;
                Point3::new(x, INNER * x.sin(), 0.0)
            })
            .collect();
        let start = model.vertices.add(0.0, 0.0, 0.0);
        let end = model.vertices.add(PI, 0.0, 0.0);
        let (upper, upper_range) = curve_through(&upper_pts);
        let (lower, lower_range) = curve_through(&lower_pts);
        let upper_curve = model.curves.add(Box::new(upper));
        let lower_curve = model.curves.add(Box::new(lower));
        let e_upper = model.edges.add(Edge::new(
            0,
            start,
            end,
            upper_curve,
            EdgeOrientation::Forward,
            upper_range,
        ));
        let e_lower = model.edges.add(Edge::new(
            0,
            start,
            end,
            lower_curve,
            EdgeOrientation::Forward,
            lower_range,
        ));
        // Anticlockwise about +Z: out along the lower edge, back along the
        // upper one.
        let mut outer = Loop::new(0, LoopType::Outer);
        outer.add_edge(e_lower, true);
        outer.add_edge(e_upper, false);
        let outer = model.loops.add(outer);
        let face = model
            .faces
            .add(Face::new(0, surface, outer, FaceOrientation::Forward));
        let mut shell = Shell::new(0, ShellType::Open);
        shell.add_face(face);
        (model, shell, e_lower, e_upper)
    }

    /// Coarse enough that each edge is first sampled in four steps.
    fn coarse() -> TessellationParams {
        TessellationParams {
            max_edge_length: 10.0,
            max_angle_deviation: 0.6,
            chord_tolerance: 0.05,
            min_segments: 3,
            max_segments: 100,
        }
    }

    fn area(mesh: &TriangleMesh) -> f64 {
        mesh.triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| mesh.vertices[i as usize].position);
                0.5 * (b - a).cross(&(c - a)).magnitude()
            })
            .sum()
    }

    /// The fixture is what it claims: meshed by itself with the first
    /// samples, the face's outline crosses and it gets no triangles.
    #[test]
    fn the_lens_outline_crosses_at_the_first_sampling() {
        let (model, shell, e_lower, e_upper) = lens(false);
        let params = coarse();
        let cache = EdgeSampleCache::new(&params);
        let face = model.faces.get(shell.faces[0]).expect("face");
        let mut alone = TriangleMesh::new();
        surface::tessellate_face(face, &model, &params, &cache, &mut alone);
        assert!(
            alone.triangles.is_empty(),
            "the outline crosses, so the face alone is refused ({} triangles)",
            alone.triangles.len()
        );
        assert_eq!(cache.get_or_compute(e_upper, &model).len(), 5);
        assert_eq!(cache.get_or_compute(e_lower, &model).len(), 5);
        let clashes = cache.take_boundary_clashes();
        assert!(
            clashes
                .iter()
                .any(|c| c.kind == edge_cache::BoundaryClashKind::Crossing),
            "and the refusal names the crossing segments: {clashes:?}"
        );
    }

    /// Meshed as part of its shell, the edges whose chords clash are sampled
    /// more finely and the face is meshed: all of it, facing up. (Near x = 0
    /// the upper edge's first chord cuts the corner; at the crest each
    /// edge's chord sags farther than the two edges stand apart.)
    #[test]
    fn a_face_whose_boundary_chords_cross_is_meshed_after_resampling() {
        let (model, shell, e_lower, e_upper) = lens(false);
        let params = coarse();
        let cache = EdgeSampleCache::new(&params);
        let mut mesh = TriangleMesh::new();
        tessellate_shell(&shell, &model, &params, &cache, &mut mesh);
        assert!(!mesh.triangles.is_empty(), "the lens is meshed");
        let upper = cache.get_or_compute(e_upper, &model);
        let lower = cache.get_or_compute(e_lower, &model);
        assert!(
            upper.len() > 5,
            "the upper edge, whose chord cut the corner, was resampled ({} samples)",
            upper.len()
        );
        // The doubled stations contain the first ones, on both edges.
        let first = EdgeSampleCache::new(&params);
        for (edge, now) in [(e_upper, &upper), (e_lower, &lower)] {
            assert_eq!((now.len() - 1) % 4, 0, "four steps, doubled: {}", now.len());
            for p in first.get_or_compute(edge, &model).iter() {
                assert!(
                    now.iter().any(|q| (*q - *p).magnitude() < 1e-12),
                    "a first sample {p:?} of edge {edge} is still a sample"
                );
            }
        }
        for t in &mesh.triangles {
            let [a, b, c] = t.map(|i| mesh.vertices[i as usize].position);
            assert!(
                (b - a).cross(&(c - a)).z > 0.0,
                "every facet faces up: {a:?} {b:?} {c:?}"
            );
        }
        // The lens between the two sampled outlines; the true lens is
        // 2 (1 - 0.97) = 0.06.
        let polygon: Vec<Point3> = lower
            .iter()
            .copied()
            .chain(upper.iter().rev().skip(1).copied())
            .collect();
        let outline_area = 0.5
            * (0..polygon.len() - 1)
                .map(|i| polygon[i].x * polygon[i + 1].y - polygon[i + 1].x * polygon[i].y)
                .sum::<f64>();
        assert!(
            (area(&mesh) - outline_area).abs() < 1e-9,
            "the mesh fills its outline exactly: {} against {outline_area}",
            area(&mesh)
        );
        assert!(
            outline_area > 0.0 && (outline_area - 0.06).abs() < 0.03,
            "and the outline is the lens: {outline_area}"
        );
    }

    /// The same lens as a face of a PLANE, which the planar triangulator
    /// meshes: it refuses the crossing outline too (CADGenBench part 225),
    /// and is answered the same way.
    #[test]
    fn a_flat_face_whose_boundary_chords_cross_is_meshed_after_resampling() {
        let (model, shell, _, e_upper) = lens(true);
        let params = coarse();
        let face = model.faces.get(shell.faces[0]).expect("face");
        let first = EdgeSampleCache::new(&params);
        let mut alone = TriangleMesh::new();
        surface::tessellate_face(face, &model, &params, &first, &mut alone);
        assert!(
            alone.triangles.is_empty(),
            "the outline crosses, so the face alone is refused ({} triangles)",
            alone.triangles.len()
        );

        let cache = EdgeSampleCache::new(&params);
        let mut mesh = TriangleMesh::new();
        tessellate_shell(&shell, &model, &params, &cache, &mut mesh);
        assert!(!mesh.triangles.is_empty(), "the lens is meshed");
        assert!(cache.get_or_compute(e_upper, &model).len() > 5);
        for t in &mesh.triangles {
            let [a, b, c] = t.map(|i| mesh.vertices[i as usize].position);
            assert!(
                (b - a).cross(&(c - a)).z > 0.0,
                "every facet faces up: {a:?} {b:?} {c:?}"
            );
        }
        assert!(
            (area(&mesh) - 0.06).abs() < 0.03,
            "and fills the lens: {}",
            area(&mesh)
        );
    }
}

#[cfg(test)]
mod spherical_crescent_tests {
    //! A sphere face bounded by arcs is filled by a fan from the centroid of
    //! its rim, the rim first sorted by azimuth about that centroid. A
    //! crescent's rim centroid lies in its hollow: both arcs span the same
    //! azimuths from there, and the sort deals their samples together. The
    //! fan then joined samples that are not neighbours on the boundary, and
    //! the face did not meet the faces across its edges (CADGenBench part
    //! 202).
    use super::*;
    use crate::math::nurbs::{interpolate_nurbs_curve, ParameterizationType};
    use crate::math::{Point3, Vector3};
    use crate::primitives::curve::{NurbsCurve, ParameterRange};
    use crate::primitives::edge::{Edge, EdgeOrientation};
    use crate::primitives::face::{Face, FaceOrientation};
    use crate::primitives::r#loop::{Loop, LoopType};
    use crate::primitives::shell::{Shell, ShellType};
    use crate::primitives::surface::Sphere;
    use crate::primitives::topology_builder::BRepModel;
    use std::collections::{BTreeMap, BTreeSet};

    const RADIUS: f64 = 10.0;
    /// Both arcs end on the chord from (8, 0, 6) to (-8, 0, 6).
    const CHORD_HEIGHT: f64 = 6.0;

    /// Points along the sphere's section by the plane through that chord
    /// tilted `tilt` about it, from (8, 0, 6) over the far side to (-8, 0, 6).
    fn arc(tilt: f64, n: usize) -> Vec<Point3> {
        let normal = Vector3::new(0.0, tilt.sin(), tilt.cos());
        let centre = normal * (CHORD_HEIGHT * tilt.cos());
        let across = Vector3::new(0.0, tilt.cos(), -tilt.sin());
        let rho = (RADIUS * RADIUS - (CHORD_HEIGHT * tilt.cos()).powi(2)).sqrt();
        let t0 = (-CHORD_HEIGHT * tilt.sin() / rho).asin();
        (0..=n)
            .map(|k| {
                let t = t0 + (std::f64::consts::PI - 2.0 * t0) * k as f64 / n as f64;
                let p = centre + Vector3::X * (rho * t.cos()) + across * (rho * t.sin());
                Point3::new(p.x, p.y, p.z)
            })
            .collect()
    }

    fn curve_through(points: &[Point3]) -> (NurbsCurve, ParameterRange) {
        let fit =
            interpolate_nurbs_curve(points, 3, ParameterizationType::ChordLength).expect("fit");
        let knots = fit.knots.values().to_vec();
        let range = ParameterRange::new(knots[0], knots[knots.len() - 1]);
        let curve = NurbsCurve::new(
            fit.degree,
            fit.control_points.clone(),
            fit.weights.clone(),
            knots,
        )
        .expect("curve");
        (curve, range)
    }

    /// The crescent on a radius-10 sphere between the sections tilted 0.3
    /// and 0.45 rad, the sphere parameterised about `north`: the model, its
    /// shell, and the (far, near) edges.
    fn crescent(north: Vector3) -> (BRepModel, Shell, u32, u32) {
        let mut model = BRepModel::new();
        let sphere =
            Sphere::with_orientation(Point3::new(0.0, 0.0, 0.0), RADIUS, Vector3::X, north)
                .expect("sphere");
        let surface = model.surfaces.add(Box::new(sphere));
        let right = model.vertices.add(8.0, 0.0, CHORD_HEIGHT);
        let left = model.vertices.add(-8.0, 0.0, CHORD_HEIGHT);
        let (far, far_range) = curve_through(&arc(-0.3, 64));
        let (near, near_range) = curve_through(&arc(-0.45, 64));
        let far_curve = model.curves.add(Box::new(far));
        let near_curve = model.curves.add(Box::new(near));
        let e_far = model.edges.add(Edge::new(
            0,
            right,
            left,
            far_curve,
            EdgeOrientation::Forward,
            far_range,
        ));
        let e_near = model.edges.add(Edge::new(
            0,
            right,
            left,
            near_curve,
            EdgeOrientation::Forward,
            near_range,
        ));
        // Anticlockwise seen from outside: along the far arc, back along
        // the near one.
        let mut outer = Loop::new(0, LoopType::Outer);
        outer.add_edge(e_far, true);
        outer.add_edge(e_near, false);
        let outer = model.loops.add(outer);
        let face = model
            .faces
            .add(Face::new(0, surface, outer, FaceOrientation::Forward));
        let mut shell = Shell::new(0, ShellType::Open);
        shell.add_face(face);
        (model, shell, e_far, e_near)
    }

    fn key(p: Point3) -> (i64, i64, i64) {
        (
            (p.x * 1e7).round() as i64,
            (p.y * 1e7).round() as i64,
            (p.z * 1e7).round() as i64,
        )
    }

    /// The point of the sphere at `colatitude` from +Z and `azimuth` from +X.
    fn on_sphere(colatitude: f64, azimuth: f64) -> Vector3 {
        Vector3::new(
            colatitude.sin() * azimuth.cos(),
            colatitude.sin() * azimuth.sin(),
            colatitude.cos(),
        ) * RADIUS
    }

    /// Points along the great arc from `a` to `b`.
    fn great_arc(a: Vector3, b: Vector3, n: usize) -> Vec<Point3> {
        let (ua, ub) = (a * (1.0 / RADIUS), b * (1.0 / RADIUS));
        let angle = ua.dot(&ub).clamp(-1.0, 1.0).acos();
        (0..=n)
            .map(|k| {
                let t = k as f64 / n as f64;
                let p = (ua * ((1.0 - t) * angle).sin() + ub * (t * angle).sin())
                    * (RADIUS / angle.sin());
                Point3::new(p.x, p.y, p.z)
            })
            .collect()
    }

    /// A sliver of the sphere: a triangle 10 mm long and 0.5 mm high whose
    /// long side runs straight over the pole. Returns the model, its shell
    /// and its three edges.
    fn sliver_over_the_pole() -> (BRepModel, Shell, [u32; 3]) {
        let mut model = BRepModel::new();
        let sphere = Sphere::new(Point3::new(0.0, 0.0, 0.0), RADIUS).expect("sphere");
        let surface = model.surfaces.add(Box::new(sphere));
        let corners = [
            on_sphere(0.5, 0.0),
            on_sphere(0.05, std::f64::consts::FRAC_PI_2),
            on_sphere(0.5, std::f64::consts::PI),
        ];
        let vertices: Vec<u32> = corners
            .iter()
            .map(|c| model.vertices.add(c.x, c.y, c.z))
            .collect();
        // Anticlockwise seen from outside: +X corner, the low apex on +Y,
        // -X corner, and back over the pole.
        let mut outer = Loop::new(0, LoopType::Outer);
        let mut edges = [0u32; 3];
        for k in 0..3 {
            let (curve, range) = curve_through(&great_arc(corners[k], corners[(k + 1) % 3], 64));
            let curve = model.curves.add(Box::new(curve));
            edges[k] = model.edges.add(Edge::new(
                0,
                vertices[k],
                vertices[(k + 1) % 3],
                curve,
                EdgeOrientation::Forward,
                range,
            ));
            outer.add_edge(edges[k], true);
        }
        let outer = model.loops.add(outer);
        let face = model
            .faces
            .add(Face::new(0, surface, outer, FaceOrientation::Forward));
        let mut shell = Shell::new(0, ShellType::Open);
        shell.add_face(face);
        (model, shell, edges)
    }

    /// The mesh's boundary (edges one facet uses) against the edges' own
    /// sample segments, and every facet facing out of the sphere.
    fn assert_conforms(
        mesh: &TriangleMesh,
        model: &BRepModel,
        cache: &EdgeSampleCache,
        edges: &[u32],
    ) {
        let mut uses: BTreeMap<((i64, i64, i64), (i64, i64, i64)), usize> = BTreeMap::new();
        for t in &mesh.triangles {
            for k in 0..3 {
                let (a, b) = (
                    key(mesh.vertices[t[k] as usize].position),
                    key(mesh.vertices[t[(k + 1) % 3] as usize].position),
                );
                *uses.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        let open: BTreeSet<_> = uses
            .iter()
            .filter(|(_, &n)| n == 1)
            .map(|(e, _)| *e)
            .collect();
        let mut wanted = BTreeSet::new();
        for &edge in edges {
            let samples = cache.get_or_compute(edge, model);
            for pair in samples.windows(2) {
                let (a, b) = (key(pair[0]), key(pair[1]));
                wanted.insert((a.min(b), a.max(b)));
            }
        }
        assert_eq!(
            open,
            wanted,
            "the mesh's boundary is the edges' samples, each joined to the next: {} mesh \
             boundary edges against {} sample segments",
            open.len(),
            wanted.len()
        );
        for t in &mesh.triangles {
            let [a, b, c] = t.map(|i| mesh.vertices[i as usize].position);
            let centre = (a + b.to_vec() + c.to_vec()) * (1.0 / 3.0);
            assert!(
                (b - a).cross(&(c - a)).dot(&centre.to_vec()) >= 0.0,
                "a facet faces into the sphere: {a:?} {b:?} {c:?}"
            );
        }
    }

    /// Near each sharp corner of a sliver, the samples of its two long sides
    /// have nearly one azimuth from the centroid, and sorting by azimuth
    /// dealt them together.
    #[test]
    fn a_spherical_sliver_over_the_pole_meets_its_edges_sample_for_sample() {
        let (model, shell, edges) = sliver_over_the_pole();
        let params = TessellationParams::coarse();
        let cache = EdgeSampleCache::new(&params);
        let mut mesh = TriangleMesh::new();
        tessellate_shell(&shell, &model, &params, &cache, &mut mesh);
        assert!(!mesh.triangles.is_empty(), "the sliver is meshed");
        assert_conforms(&mesh, &model, &cache, &edges);
    }

    /// The same crescent on a sphere whose pole lies INSIDE it (midway
    /// between the two arcs): the sphere's own chart has no outline of such
    /// a region, so it is triangulated in the chart of the sphere turned to
    /// face it.
    #[test]
    fn a_spherical_crescent_holding_a_pole_meets_its_edges_sample_for_sample() {
        let (model, shell, e_far, e_near) = crescent(Vector3::new(0.0, 0.568, 0.823));
        let params = TessellationParams::coarse();
        let cache = EdgeSampleCache::new(&params);
        let mut mesh = TriangleMesh::new();
        tessellate_shell(&shell, &model, &params, &cache, &mut mesh);
        assert!(!mesh.triangles.is_empty(), "the crescent is meshed");
        assert_conforms(&mesh, &model, &cache, &[e_far, e_near]);
        assert!(
            mesh.triangles.iter().all(|t| {
                let [a, b, c] = t.map(|i| mesh.vertices[i as usize].position);
                let centre = (a + b.to_vec() + c.to_vec()) * (1.0 / 3.0);
                (b - a).cross(&(c - a)).dot(&centre.to_vec()) > 0.0
            }),
            "every facet faces out of the sphere"
        );
    }

    #[test]
    fn a_spherical_crescent_meets_its_edges_sample_for_sample() {
        let (model, shell, e_far, e_near) = crescent(Vector3::Z);
        let params = TessellationParams::coarse();
        let cache = EdgeSampleCache::new(&params);
        let mut mesh = TriangleMesh::new();
        tessellate_shell(&shell, &model, &params, &cache, &mut mesh);
        assert!(!mesh.triangles.is_empty(), "the crescent is meshed");

        // The mesh's own boundary: the edges one facet uses.
        let mut uses: BTreeMap<((i64, i64, i64), (i64, i64, i64)), usize> = BTreeMap::new();
        for t in &mesh.triangles {
            for k in 0..3 {
                let (a, b) = (
                    key(mesh.vertices[t[k] as usize].position),
                    key(mesh.vertices[t[(k + 1) % 3] as usize].position),
                );
                *uses.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        let open: BTreeSet<_> = uses
            .iter()
            .filter(|(_, &n)| n == 1)
            .map(|(e, _)| *e)
            .collect();
        assert!(
            uses.values().all(|&n| n <= 2),
            "no edge is used by more than two facets"
        );
        // The boundary's own segments: each edge's samples, one to the next.
        let mut wanted = BTreeSet::new();
        for edge in [e_far, e_near] {
            let samples = cache.get_or_compute(edge, &model);
            for pair in samples.windows(2) {
                let (a, b) = (key(pair[0]), key(pair[1]));
                wanted.insert((a.min(b), a.max(b)));
            }
        }
        assert_eq!(
            open,
            wanted,
            "the mesh's boundary is the edges' samples, each joined to the next: {} mesh \
             boundary edges against {} sample segments",
            open.len(),
            wanted.len()
        );
        // Every facet faces out of the sphere.
        for t in &mesh.triangles {
            let [a, b, c] = t.map(|i| mesh.vertices[i as usize].position);
            let centre = (a + b.to_vec() + c.to_vec()) * (1.0 / 3.0);
            assert!(
                (b - a).cross(&(c - a)).dot(&centre.to_vec()) > 0.0,
                "a facet faces into the sphere: {a:?} {b:?} {c:?}"
            );
        }
    }
}
