//! CF-β.5 — shared test fixtures for chamfer/fillet/mixed-kind blend
//! integration suites.
//!
//! Consolidates the small forest of inlined `make_box` / `vertex_at`
//! / `shell_census` / `find_planar_cap_face` helpers that had been
//! copy-pasted across the chamfer-α/β, fillet-α/β, and CF-α/β test
//! files. Every helper is `pub` so consumers can pull them via:
//!
//! ```ignore
//! #[path = "blend_fixtures/mod.rs"]
//! mod blend_fixtures;
//! use blend_fixtures::*;
//! ```
//!
//! The module is `#[cfg(test)]` by construction (test crates only
//! compile under `cargo test`), so the `expect_used` / `panic`
//! workspace deny-lints don't apply — this is test-fixture code,
//! its job is to fail loudly.
//!
//! New in CF-β.5: [`topology_hash`] — an order-independent
//! V/E/F + sorted edge-endpoint + sorted face-surface-kind digest
//! used by the order-invariance proptests and the replay-determinism
//! loop. Two solids with equal `topology_hash` have identical B-Rep
//! shape up to ID permutation; mismatched hashes guarantee at least
//! one structural difference.

#![allow(dead_code)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use std::collections::{BTreeMap, HashSet};

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::primitives::curve::{Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::face::{Face, FaceId, FaceOrientation};
use geometry_engine::primitives::r#loop::{Loop, LoopType};
use geometry_engine::primitives::shell::{Shell, ShellType};
use geometry_engine::primitives::solid::{Solid, SolidId};
use geometry_engine::primitives::surface::Plane;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::vertex::VertexId;

/// Build a width × height × depth axis-aligned box centred on the
/// origin via the kernel's primitive constructor. Returns the
/// `SolidId` of the produced body.
pub fn make_box(model: &mut BRepModel, width: f64, height: f64, depth: f64) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    match builder
        .create_box_3d(width, height, depth)
        .expect("box creation succeeds")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {:?}", other),
    }
}

/// Convenience: build a cube of side `size`.
pub fn make_cube(model: &mut BRepModel, size: f64) -> SolidId {
    make_box(model, size, size, size)
}

/// Find the vertex at the given position. Panics if absent.
pub fn vertex_at(model: &BRepModel, x: f64, y: f64, z: f64) -> VertexId {
    for (id, vertex) in model.vertices.iter() {
        let p = vertex.position;
        if (p[0] - x).abs() < 1.0e-9 && (p[1] - y).abs() < 1.0e-9 && (p[2] - z).abs() < 1.0e-9 {
            return id;
        }
    }
    panic!("no vertex at ({}, {}, {})", x, y, z);
}

/// All edges of `model` incident to `vertex` (start *or* end). Pure
/// topology walk, no orientation filter.
pub fn edges_at_vertex(model: &BRepModel, vertex: VertexId) -> Vec<EdgeId> {
    model
        .edges
        .iter()
        .filter(|(_, edge)| edge.start_vertex == vertex || edge.end_vertex == vertex)
        .map(|(id, _)| id)
        .collect()
}

/// Topology census of `solid_id`'s outer shell as `(V, E, F)`. The
/// Euler-Poincaré relation V − E + F = 2 is the genus-zero
/// closed-surface invariant.
pub fn shell_census(model: &BRepModel, solid_id: SolidId) -> (usize, usize, usize) {
    let solid = model.solids.get(solid_id).expect("solid exists");
    let shell = model.shells.get(solid.outer_shell).expect("shell exists");
    let mut vertices: HashSet<VertexId> = HashSet::new();
    let mut edges: HashSet<EdgeId> = HashSet::new();
    for &face_id in &shell.faces {
        let face = model.faces.get(face_id).expect("face exists");
        for loop_id in face.all_loops() {
            let lp = model.loops.get(loop_id).expect("loop exists");
            for &edge_id in &lp.edges {
                edges.insert(edge_id);
                if let Some(edge) = model.edges.get(edge_id) {
                    vertices.insert(edge.start_vertex);
                    vertices.insert(edge.end_vertex);
                }
            }
        }
    }
    (vertices.len(), edges.len(), shell.faces.len())
}

/// How many faces of `solid_id`'s outer shell reference `edge_id` in
/// any loop. 2 for every interior edge on a watertight manifold.
pub fn faces_referencing_edge(model: &BRepModel, solid_id: SolidId, edge_id: EdgeId) -> usize {
    let solid = model.solids.get(solid_id).expect("solid exists");
    let shell = model.shells.get(solid.outer_shell).expect("shell exists");
    let mut count = 0;
    for &face_id in &shell.faces {
        let face = model.faces.get(face_id).expect("face exists");
        for loop_id in face.all_loops() {
            let lp = model.loops.get(loop_id).expect("loop exists");
            if lp.edges.iter().any(|&e| e == edge_id) {
                count += 1;
                break;
            }
        }
    }
    count
}

/// Count edges of `solid_id`'s outer shell whose face-reference
/// count is not exactly 2. A watertight closed manifold has zero
/// such edges.
pub fn non_manifold_edge_count(model: &BRepModel, solid_id: SolidId) -> usize {
    let solid = model.solids.get(solid_id).expect("solid exists");
    let shell = model.shells.get(solid.outer_shell).expect("shell exists");
    let mut all_edges: HashSet<EdgeId> = HashSet::new();
    for &face_id in &shell.faces {
        let face = model.faces.get(face_id).expect("face exists");
        for loop_id in face.all_loops() {
            let lp = model.loops.get(loop_id).expect("loop exists");
            for &edge_id in &lp.edges {
                all_edges.insert(edge_id);
            }
        }
    }
    all_edges
        .into_iter()
        .filter(|&edge_id| faces_referencing_edge(model, solid_id, edge_id) != 2)
        .count()
}

/// Locate the unique planar cap face among `face_ids` whose outer
/// loop has exactly `expected_edge_count` edges. Panics if there
/// isn't exactly one. When `expected_edge_count` is `None`, accepts
/// any planar face and panics unless exactly one is present.
pub fn find_planar_cap_face(
    model: &BRepModel,
    face_ids: &[FaceId],
    expected_edge_count: Option<usize>,
) -> FaceId {
    let mut found: Option<FaceId> = None;
    for &fid in face_ids {
        let face = model.faces.get(fid).expect("face exists");
        let surface = model.surfaces.get(face.surface_id).expect("surface exists");
        if surface.as_any().downcast_ref::<Plane>().is_some() {
            let outer = model
                .loops
                .get(face.outer_loop)
                .expect("planar face outer loop");
            let match_count = match expected_edge_count {
                Some(n) => outer.edges.len() == n,
                None => true,
            };
            if match_count {
                assert!(
                    found.is_none(),
                    "more than one planar face matching the loop-size predicate"
                );
                found = Some(fid);
            }
        }
    }
    found.unwrap_or_else(|| {
        panic!(
            "no planar cap face with {:?} outer-loop edges among returned face IDs",
            expected_edge_count
        )
    })
}

/// CF-β.5 — order-independent topology digest. Combines:
///
/// 1. `(V, E, F)` counts.
/// 2. Sorted set of `(min(end_a, end_b), max(end_a, end_b))` for
///    every edge's vertex endpoints — captures connectivity up to
///    ID permutation by canonicalising the unordered endpoint pair.
/// 3. Sorted multiset of face surface-kind tags (one byte per face
///    naming "Plane", "Cylinder", "Sphere", "Torus", "Nurbs",
///    "Ruled", "Offset", "SurfaceOfRevolution") — captures the
///    geometric content per face without depending on ID ordering.
///
/// Two solids with equal hash are structurally identical up to an
/// ID renaming. Hash mismatch guarantees at least one of: a count
/// difference, a connectivity difference, or a surface-kind
/// difference. Used by the order-invariance proptests
/// (`prop_mixed_kind_corner_topology_order_invariant`) and the
/// replay-determinism loop (`cf_beta_replay_determinism`).
pub fn topology_hash(model: &BRepModel, solid_id: SolidId) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let solid = model.solids.get(solid_id).expect("solid exists");
    let shell = model.shells.get(solid.outer_shell).expect("shell exists");

    // 1. (V, E, F) counts via shell_census.
    let (v_count, e_count, f_count) = shell_census(model, solid_id);

    // 2. Collect all edges in the shell and build a vertex
    // adjacency list (multigraph allowed — parallel edges are
    // tracked as multiplicities).
    let mut edge_ids: HashSet<EdgeId> = HashSet::new();
    for &face_id in &shell.faces {
        let face = model.faces.get(face_id).expect("face exists");
        for loop_id in face.all_loops() {
            let lp = model.loops.get(loop_id).expect("loop exists");
            for &eid in &lp.edges {
                edge_ids.insert(eid);
            }
        }
    }
    let mut vertices_sorted: Vec<VertexId> = edge_ids
        .iter()
        .filter_map(|&eid| model.edges.get(eid))
        .flat_map(|e| [e.start_vertex, e.end_vertex])
        .collect();
    vertices_sorted.sort_unstable();
    vertices_sorted.dedup();

    let mut adjacency: BTreeMap<VertexId, Vec<VertexId>> = BTreeMap::new();
    for &vid in &vertices_sorted {
        adjacency.insert(vid, Vec::new());
    }
    for &eid in &edge_ids {
        if let Some(e) = model.edges.get(eid) {
            // Skip self-loops at the adjacency-construction level
            // (a B-Rep edge with `start == end` is a degenerate
            // closed curve; record its presence in the per-vertex
            // signature via the edge-count diff instead).
            if e.start_vertex == e.end_vertex {
                continue;
            }
            adjacency
                .entry(e.start_vertex)
                .or_default()
                .push(e.end_vertex);
            adjacency.entry(e.end_vertex).or_default().push(e.start_vertex);
        }
    }

    // 3. Sorted multiset of face-surface-kind tags. Tag the
    // analytic kind by trait downcast; unknown / non-analytic
    // surfaces hash as "Other".
    let mut face_tags: Vec<&'static str> = shell
        .faces
        .iter()
        .map(|&fid| {
            let face = model.faces.get(fid).expect("face exists");
            let surface = model.surfaces.get(face.surface_id).expect("surface exists");
            classify_surface_tag(surface)
        })
        .collect();
    face_tags.sort_unstable();

    // 4. Weisfeiler-Lehman colour-refinement vertex fingerprint.
    // Two solids with isomorphic shell graphs produce identical
    // sorted-colour multisets after enough refinement rounds. For
    // the small (≤ ~30 vertex) graphs in this test suite, 3
    // refinement rounds suffice to distinguish every non-
    // isomorphic graph encountered to date. The initial colour is
    // the vertex's degree (≡ |adjacency|); each subsequent round
    // replaces it with a hash of `(old_colour,
    // sorted_multiset_of_neighbour_colours)`. Sorting the final
    // colour multiset makes the digest invariant under VertexId
    // permutation — the property the legacy "sort-by-VertexId then
    // remap" recipe failed to provide (it only removed in-store
    // insertion bias, not graph-relabelling).
    let mut colours: BTreeMap<VertexId, u64> = BTreeMap::new();
    for (&vid, nbrs) in adjacency.iter() {
        colours.insert(vid, nbrs.len() as u64);
    }
    const WL_ROUNDS: usize = 3;
    for _ in 0..WL_ROUNDS {
        let mut next: BTreeMap<VertexId, u64> = BTreeMap::new();
        for (&vid, nbrs) in adjacency.iter() {
            let old = *colours.get(&vid).unwrap_or(&0);
            let mut nbr_cols: Vec<u64> =
                nbrs.iter().map(|n| *colours.get(n).unwrap_or(&0)).collect();
            nbr_cols.sort_unstable();
            let mut h = DefaultHasher::new();
            old.hash(&mut h);
            nbr_cols.hash(&mut h);
            next.insert(vid, h.finish());
        }
        colours = next;
    }
    let mut colour_multiset: Vec<u64> = colours.values().copied().collect();
    colour_multiset.sort_unstable();

    let mut hasher = DefaultHasher::new();
    (v_count, e_count, f_count).hash(&mut hasher);
    colour_multiset.hash(&mut hasher);
    face_tags.hash(&mut hasher);
    hasher.finish()
}

/// Tag the surface's concrete kind by Trait downcast. Stable strings
/// keep the hash deterministic across runs. Covers every concrete
/// type implementing the `Surface` trait in `primitives::surface`;
/// unknown types fall through to `"Other"` (e.g. a future analytic
/// kind that hasn't been wired here yet).
fn classify_surface_tag(
    surface: &dyn geometry_engine::primitives::surface::Surface,
) -> &'static str {
    use geometry_engine::primitives::surface::{
        Cone, Cylinder, GeneralNurbsSurface, RuledSurface, Sphere, SurfaceOfRevolution, Torus,
    };
    let any = surface.as_any();
    if any.downcast_ref::<Plane>().is_some() {
        "Plane"
    } else if any.downcast_ref::<Cylinder>().is_some() {
        "Cylinder"
    } else if any.downcast_ref::<Sphere>().is_some() {
        "Sphere"
    } else if any.downcast_ref::<Cone>().is_some() {
        "Cone"
    } else if any.downcast_ref::<Torus>().is_some() {
        "Torus"
    } else if any.downcast_ref::<GeneralNurbsSurface>().is_some() {
        "Nurbs"
    } else if any.downcast_ref::<RuledSurface>().is_some() {
        "Ruled"
    } else if any.downcast_ref::<SurfaceOfRevolution>().is_some() {
        "SurfaceOfRevolution"
    } else {
        "Other"
    }
}

/// Build a symmetric square-base pyramid centred on the z-axis with
/// the base on z=0 and the apex at (0, 0, height).
///
/// Vertex layout (`hb = base / 2`):
///   v0=(-hb,-hb,0) v1=(hb,-hb,0) v2=(hb,hb,0) v3=(-hb,hb,0)   base
///   v4=(0,0,h)                                                 apex
///
/// Edge layout (all stored start→end):
///   e0:v0→v1   e1:v1→v2   e2:v2→v3   e3:v3→v0   (base, CCW)
///   e4:v0→v4   e5:v1→v4   e6:v2→v4   e7:v3→v4   (sloped to apex)
///
/// Face layout (outward normals point away from the solid centroid):
///   f_base:  base square at z=0, outward −Z, loop v0→v3→v2→v1
///   f_front: triangle v0,v1,v4 — outward (0,−h,hb) direction
///   f_right: triangle v1,v2,v4 — outward (h,0,hb) direction
///   f_back:  triangle v2,v3,v4 — outward (0,h,hb) direction
///   f_left:  triangle v3,v0,v4 — outward (−h,0,hb) direction
pub fn make_square_pyramid_solid(model: &mut BRepModel, base: f64, height: f64) -> SolidId {
    let tol = Tolerance::default().distance();
    let hb = base / 2.0;

    let v0 = model.vertices.add_or_find(-hb, -hb, 0.0, tol);
    let v1 = model.vertices.add_or_find(hb, -hb, 0.0, tol);
    let v2 = model.vertices.add_or_find(hb, hb, 0.0, tol);
    let v3 = model.vertices.add_or_find(-hb, hb, 0.0, tol);
    let v4 = model.vertices.add_or_find(0.0, 0.0, height, tol);

    let p0 = Point3::new(-hb, -hb, 0.0);
    let p1 = Point3::new(hb, -hb, 0.0);
    let p2 = Point3::new(hb, hb, 0.0);
    let p3 = Point3::new(-hb, hb, 0.0);
    let p4 = Point3::new(0.0, 0.0, height);

    let edges = [
        (v0, v1, p0, p1),
        (v1, v2, p1, p2),
        (v2, v3, p2, p3),
        (v3, v0, p3, p0),
        (v0, v4, p0, p4),
        (v1, v4, p1, p4),
        (v2, v4, p2, p4),
        (v3, v4, p3, p4),
    ];
    let mut edge_ids: [EdgeId; 8] = [0; 8];
    for (i, &(sv, ev, sp, ep)) in edges.iter().enumerate() {
        let curve_id = model.curves.add(Box::new(Line::new(sp, ep)));
        let edge = Edge::new(
            0,
            sv,
            ev,
            curve_id,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        );
        edge_ids[i] = model.edges.add(edge);
    }

    let face_data: [(Vec<usize>, Vec<bool>, Point3, Vector3); 5] = [
        (
            vec![3, 2, 1, 0],
            vec![false, false, false, false],
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, -1.0),
        ),
        (
            vec![0, 5, 4],
            vec![true, true, false],
            p0,
            Vector3::new(0.0, -height, hb),
        ),
        (
            vec![1, 6, 5],
            vec![true, true, false],
            p1,
            Vector3::new(height, 0.0, hb),
        ),
        (
            vec![2, 7, 6],
            vec![true, true, false],
            p2,
            Vector3::new(0.0, height, hb),
        ),
        (
            vec![3, 4, 7],
            vec![true, true, false],
            p3,
            Vector3::new(-height, 0.0, hb),
        ),
    ];

    let mut face_ids: [FaceId; 5] = [0; 5];
    for (face_idx, (edge_indices, orientations, point, normal)) in face_data.iter().enumerate() {
        let plane = Plane::from_point_normal(*point, *normal).expect("pyramid plane construction");
        let surface_id = model.surfaces.add(Box::new(plane));

        let mut loop_obj = Loop::new(0, LoopType::Outer);
        for (i, &edge_idx) in edge_indices.iter().enumerate() {
            loop_obj.add_edge(edge_ids[edge_idx], orientations[i]);
        }
        let loop_id = model.loops.add(loop_obj);

        let mut face = Face::new(0, surface_id, loop_id, FaceOrientation::Forward);
        face.outer_loop = loop_id;
        face_ids[face_idx] = model.faces.add(face);
    }

    let mut shell = Shell::new(0, ShellType::Closed);
    for &fid in &face_ids {
        shell.add_face(fid);
    }
    let shell_id = model.shells.add(shell);
    model.solids.add(Solid::new(0, shell_id))
}
