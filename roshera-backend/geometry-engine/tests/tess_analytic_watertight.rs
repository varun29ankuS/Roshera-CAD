//! Watertightness regression tests for analytic-surface tessellation
//! (CDT-γ.3).
//!
//! A closed analytic solid must tessellate to a closed 2-manifold: every
//! undirected mesh edge is shared by exactly two triangles, and the Euler
//! characteristic `V − E + F == 2` (sphere topology). This pins the
//! shared-edge coherence between a cylinder's lateral face and its planar
//! caps along the circular seam — the case the grid tessellator
//! historically left to the vertex-weld safety net (which collapses only
//! *coincident* vertices and cannot repair a T-junction where the lateral
//! and the cap sample the seam circle at different counts).

#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::indexing_slicing)]

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams, TriangleMesh};

/// Undirected-edge multiplicity histogram of a triangle mesh.
fn edge_count_histogram(mesh: &TriangleMesh) -> std::collections::HashMap<(u32, u32), u32> {
    let mut h: std::collections::HashMap<(u32, u32), u32> = std::collections::HashMap::new();
    for tri in &mesh.triangles {
        for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            let key = if a < b { (a, b) } else { (b, a) };
            *h.entry(key).or_insert(0) += 1;
        }
    }
    h
}

/// Build a closed cylinder solid (lateral + two planar caps) and return
/// its solid id.
fn cylinder_solid(model: &mut BRepModel, radius: f64, height: f64) -> u32 {
    let mut builder = TopologyBuilder::new(model);
    let geom = builder
        .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, radius, height)
        .expect("cylinder must construct for positive dimensions");
    match geom {
        GeometryId::Solid(id) => id,
        other => panic!("create_cylinder_3d must return a Solid, got {other:?}"),
    }
}

// CDT-γ.3: the cylinder lateral face is tessellated through the curved-CDT
// path (cache-based boundary), so the lateral and the planar caps agree on
// the shared circular seam bit-exactly. The previous grid tessellator
// sampled the lateral boundary independently of `EdgeSampleCache`, leaving
// 284 T-junction (count-1) edges the vertex-weld could not repair. The
// remaining periodic-u degeneracy (the circle's t=0 sweeping across the
// seam, which made the CDT reject a non-simple polygon) is resolved at the
// source: `create_cylinder_topology` now anchors the seam to the circles'
// `t=0` direction, so the lateral projects to a clean `[u₀, u₀+2π]`
// rectangle.
#[test]
fn cylinder_solid_tessellation_is_watertight() {
    let mut model = BRepModel::new();
    let solid_id = cylinder_solid(&mut model, 1.0, 2.0);
    let params = TessellationParams::default();
    let solid = model.solids.get(solid_id).expect("solid present");
    let mesh = tessellate_solid(solid, &model, &params);

    assert!(
        !mesh.triangles.is_empty(),
        "cylinder solid must tessellate to a non-empty mesh"
    );

    let hist = edge_count_histogram(&mesh);
    let non_manifold: Vec<((u32, u32), u32)> = hist
        .iter()
        .filter(|(_, &c)| c != 2)
        .map(|(&k, &c)| (k, c))
        .collect();
    assert!(
        non_manifold.is_empty(),
        "a closed cylinder solid must be a closed 2-manifold (every undirected \
         edge shared by exactly two triangles); found {} non-2 edges, e.g. {:?}",
        non_manifold.len(),
        non_manifold.iter().take(5).collect::<Vec<_>>()
    );

    // Closed orientable genus-0 surface: V - E + F == 2. Count only the
    // vertices actually referenced by triangles — the shared-edge weld
    // remaps indices but leaves the merged-away vertex slots in
    // `mesh.vertices`, so the raw length over-counts.
    let mut referenced = std::collections::HashSet::new();
    for tri in &mesh.triangles {
        referenced.insert(tri[0]);
        referenced.insert(tri[1]);
        referenced.insert(tri[2]);
    }
    let v = referenced.len() as i64;
    let e = hist.len() as i64;
    let f = mesh.triangles.len() as i64;
    assert_eq!(
        v - e + f,
        2,
        "closed cylinder Euler characteristic must be 2; got V={v}, E={e}, F={f}"
    );
}

/// Build a closed sphere solid (single closed face, no boundary edges)
/// and return its solid id.
fn sphere_solid(model: &mut BRepModel, radius: f64) -> u32 {
    let mut builder = TopologyBuilder::new(model);
    let geom = builder
        .create_sphere_3d(Point3::ORIGIN, radius)
        .expect("sphere must construct for positive radius");
    match geom {
        GeometryId::Solid(id) => id,
        other => panic!("create_sphere_3d must return a Solid, got {other:?}"),
    }
}

// CDT-γ.3 baseline (sphere). A sphere is a single closed face with no
// boundary edges, so it stays on the full-domain grid path (the
// curved-CDT path needs a trim loop to project and is N/A here). This
// pins whether the grid tessellator + pole handling already produces a
// closed 2-manifold; the count in the failure message is the baseline if
// it does not.
#[test]
fn sphere_solid_tessellation_is_watertight() {
    let mut model = BRepModel::new();
    let solid_id = sphere_solid(&mut model, 1.0);
    let params = TessellationParams::default();
    let solid = model.solids.get(solid_id).expect("solid present");
    let mesh = tessellate_solid(solid, &model, &params);

    assert!(
        !mesh.triangles.is_empty(),
        "sphere solid must tessellate to a non-empty mesh"
    );

    let hist = edge_count_histogram(&mesh);
    let non_manifold: Vec<((u32, u32), u32)> = hist
        .iter()
        .filter(|(_, &c)| c != 2)
        .map(|(&k, &c)| (k, c))
        .collect();
    assert!(
        non_manifold.is_empty(),
        "a closed sphere solid must be a closed 2-manifold (every undirected \
         edge shared by exactly two triangles); found {} non-2 edges",
        non_manifold.len()
    );

    let mut referenced = std::collections::HashSet::new();
    for tri in &mesh.triangles {
        referenced.insert(tri[0]);
        referenced.insert(tri[1]);
        referenced.insert(tri[2]);
    }
    let v = referenced.len() as i64;
    let e = hist.len() as i64;
    let f = mesh.triangles.len() as i64;
    assert_eq!(
        v - e + f,
        2,
        "closed sphere Euler characteristic must be 2; got V={v}, E={e}, F={f}"
    );
}

/// Shared closed-2-manifold assertion: every undirected edge shared by
/// exactly two triangles AND Euler `V − E + F == expected_euler`
/// (counting only referenced vertices — the weld leaves merged-away slots
/// in `mesh.vertices`). `expected_euler` is `2` for a genus-0 boundary
/// (sphere/cylinder/cone) and `0` for a genus-1 boundary (torus).
fn assert_watertight_closed_manifold(mesh: &TriangleMesh, label: &str, expected_euler: i64) {
    assert!(
        !mesh.triangles.is_empty(),
        "{label} must tessellate to a non-empty mesh"
    );
    let hist = edge_count_histogram(mesh);
    let non_manifold = hist.values().filter(|&&c| c != 2).count();
    assert_eq!(
        non_manifold, 0,
        "{label} must be a closed 2-manifold; found {non_manifold} non-2 edges"
    );
    let mut referenced = std::collections::HashSet::new();
    for tri in &mesh.triangles {
        referenced.insert(tri[0]);
        referenced.insert(tri[1]);
        referenced.insert(tri[2]);
    }
    let v = referenced.len() as i64;
    let e = hist.len() as i64;
    let f = mesh.triangles.len() as i64;
    assert_eq!(
        v - e + f,
        expected_euler,
        "{label} Euler characteristic must be {expected_euler}; got V={v}, E={e}, F={f}"
    );
}

// CDT-γ.3 (cone with apex). A true cone (top_radius = 0) has an apex
// degeneracy on the lateral plus a planar base cap. The lateral's outer
// loop is a single edge — the base circle (the apex is a point, not an
// edge) — so its boundary does not enclose the UV domain and curved-CDT
// is N/A. Instead the grid `tessellate_conical_with_apex` now drives its
// base row from the base edge's `EdgeSampleCache` samples, bit-exact with
// the base cap (which samples the same edge via the cache). Was 135 non-2
// edges; now watertight.
#[test]
fn cone_solid_tessellation_is_watertight() {
    let mut model = BRepModel::new();
    let solid_id = {
        let mut b = TopologyBuilder::new(&mut model);
        match b
            .create_cone_3d(Point3::ORIGIN, Vector3::Z, 1.0, 0.0, 2.0)
            .expect("cone must construct for valid dimensions")
        {
            GeometryId::Solid(id) => id,
            other => panic!("create_cone_3d must return a Solid, got {other:?}"),
        }
    };
    let params = TessellationParams::default();
    let solid = model.solids.get(solid_id).expect("solid present");
    let mesh = tessellate_solid(solid, &model, &params);
    assert_watertight_closed_manifold(&mesh, "closed cone solid", 2);
}

// CDT-γ.2 (cone frustum). A truncated cone (both radii > 0) has a
// u-periodic lateral with NO seam edge: outer loop = bottom circle, inner
// loop = top circle, joined by u-periodicity. The two circles have
// different radii → different cache sample counts, so a uniform grid
// cannot match both caps. The curved-CDT periodic-wrap path synthesises a
// virtual seam and bridges the differing ring counts.
#[test]
fn cone_frustum_solid_tessellation_is_watertight() {
    let mut model = BRepModel::new();
    let solid_id = {
        let mut b = TopologyBuilder::new(&mut model);
        match b
            .create_cone_3d(Point3::ORIGIN, Vector3::Z, 1.0, 0.5, 2.0)
            .expect("frustum must construct for valid dimensions")
        {
            GeometryId::Solid(id) => id,
            other => panic!("create_cone_3d must return a Solid, got {other:?}"),
        }
    };
    let params = TessellationParams::default();
    let solid = model.solids.get(solid_id).expect("solid present");
    let mesh = tessellate_solid(solid, &model, &params);
    assert_watertight_closed_manifold(&mesh, "closed cone frustum solid", 2);
}

// CDT-γ.3 baseline (torus). A torus is a single closed, doubly-periodic
// face. Pins whether the grid path closes both the major and minor seams.
#[test]
fn torus_solid_tessellation_is_watertight() {
    let mut model = BRepModel::new();
    let solid_id = {
        let mut b = TopologyBuilder::new(&mut model);
        match b
            .create_torus_3d(Point3::ORIGIN, Vector3::Z, 2.0, 0.5)
            .expect("torus must construct for valid radii")
        {
            GeometryId::Solid(id) => id,
            other => panic!("create_torus_3d must return a Solid, got {other:?}"),
        }
    };
    let params = TessellationParams::default();
    let solid = model.solids.get(solid_id).expect("solid present");
    let mesh = tessellate_solid(solid, &model, &params);
    assert_watertight_closed_manifold(&mesh, "closed torus solid", 0);
}
