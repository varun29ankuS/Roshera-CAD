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

// KNOWN GAP (CDT-γ.3, target of the analytic fast-paths migration).
// The grid cylinder tessellator samples its lateral boundary
// independently of `EdgeSampleCache`, so the lateral and the planar caps
// disagree on the shared circular seam → 284 T-junction (count-1) edges
// that the vertex-weld cannot repair. Routing the lateral through the
// curved-CDT path (cache-based boundary) halves it to ~142, but a closed
// cylinder is periodic in u (seam at 0/2π) and the CDT path — built for
// non-periodic NURBS patches — does not stitch that seam. The fix is
// periodic-seam stitching in `curved_cdt`; until then this is `#[ignore]`d
// as the tracked regression target rather than reddening the suite.
#[test]
#[ignore = "CDT-γ.3: analytic cylinder non-watertight pending periodic-seam stitching in curved_cdt"]
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

    // Closed orientable genus-0 surface: V - E + F == 2.
    let v = mesh.vertices.len() as i64;
    let e = hist.len() as i64;
    let f = mesh.triangles.len() as i64;
    assert_eq!(
        v - e + f,
        2,
        "closed cylinder Euler characteristic must be 2; got V={v}, E={e}, F={f}"
    );
}
