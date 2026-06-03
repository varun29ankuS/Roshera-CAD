//! Universal watertightness oracle — the one correctness check every geometry
//! operation's output must pass.
//!
//! A solid is *watertight* when its boundary is a closed, consistently-oriented
//! surface enclosing a well-defined volume. The kernel can assert this cheaply
//! and universally: tessellate the solid and compare the mesh's enclosed volume
//! (the divergence-theorem sum over the triangles) against the analytic
//! mass-properties volume. A leak (open seam) or a flipped triangle makes the
//! divergence sum diverge wildly from the true volume, so agreement within the
//! faceting tolerance certifies the boundary is closed.
//!
//! Every op harness in this module — boolean, fillet, extrude, revolve, … — can
//! call [`is_watertight`] on its result; it is the shared, operation-agnostic
//! correctness primitive the whole geometry module is held to.

use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::tessellation::{tessellate_solid, TessellationParams};
use std::collections::HashMap;

/// The analytic (mass-properties) volume of a solid, or `None` if it can't be
/// computed.
pub fn analytic_volume(model: &mut BRepModel, solid: SolidId) -> Option<f64> {
    model.calculate_solid_volume(solid)
}

/// The volume enclosed by the solid's tessellated mesh at chord tolerance
/// `chord`, via the divergence theorem `V = (1/6) Σ p0·(p1×p2)`. `None` if the
/// solid is missing or tessellates to nothing.
pub fn mesh_volume(model: &BRepModel, solid: SolidId, chord: f64) -> Option<f64> {
    let solid_ref = model.solids.get(solid)?;
    let params = TessellationParams {
        chord_tolerance: chord,
        ..TessellationParams::default()
    };
    let mesh = tessellate_solid(solid_ref, model, &params);
    if mesh.triangles.is_empty() {
        return None;
    }
    let mut six_v = 0.0;
    for tri in &mesh.triangles {
        let p0 = mesh.vertices[tri[0] as usize].position;
        let p1 = mesh.vertices[tri[1] as usize].position;
        let p2 = mesh.vertices[tri[2] as usize].position;
        six_v += p0.dot(&p1.cross(&p2));
    }
    Some((six_v / 6.0).abs())
}

/// Is `solid` watertight? Its tessellated mesh must enclose the analytic volume
/// within the relative tolerance `rel_tol` (a few percent absorbs faceting; a
/// leak or flip produces a far larger discrepancy). `false` if either volume is
/// uncomputable (which is itself a failure).
pub fn is_watertight(model: &mut BRepModel, solid: SolidId, chord: f64, rel_tol: f64) -> bool {
    let Some(analytic) = model.calculate_solid_volume(solid) else {
        return false;
    };
    let Some(mesh) = mesh_volume(model, solid, chord) else {
        return false;
    };
    let scale = analytic.abs().max(mesh.abs()).max(1.0);
    (analytic - mesh).abs() / scale <= rel_tol
}

/// Topological verdict for a tessellated solid — far stricter than the
/// volume-agreement [`is_watertight`] check.
///
/// `is_watertight` only asserts the divergence-theorem volume of the mesh
/// matches the analytic volume; a solid can pass that while being
/// topologically broken in ways the signed volume happens to cancel out
/// (the sphere-winding `.abs()` class: two inverted patches whose flipped
/// contributions net to the right number). This report inspects the mesh's
/// *connectivity* instead:
///
/// * **closed** — no boundary edge (every undirected edge borders exactly two
///   triangles). A leak/open seam shows up as `boundary_edges > 0`.
/// * **manifold** — no edge shared by three or more triangles
///   (`nonmanifold_edges == 0`).
/// * **oriented** — every *directed* edge appears at most once. A consistently
///   wound closed surface traverses each edge once per direction, so a repeated
///   directed edge means two triangles wind the same way across it — a flipped
///   normal or a duplicated facet. This is the check `is_watertight` cannot make.
///
/// The mesh is welded by quantised position first: per-face tessellation emits
/// independent vertex indices even where faces share a boundary edge, so raw
/// triangle indices never share vertices across faces. Welding restores the
/// shared topology (shared-edge samples are bit-exact by the `EdgeSampleCache`
/// contract, so a tight epsilon suffices).
#[derive(Debug, Clone)]
pub struct ManifoldReport {
    pub triangles: usize,
    pub degenerate_triangles: usize,
    pub welded_vertices: usize,
    pub undirected_edges: usize,
    /// Undirected edges bordering exactly one triangle — a leak.
    pub boundary_edges: usize,
    /// Undirected edges bordering three or more triangles — non-manifold.
    pub nonmanifold_edges: usize,
    /// Directed edges traversed by more than one triangle — orientation flip
    /// or duplicated facet.
    pub inconsistent_directed_edges: usize,
    /// Connected components of the welded mesh (disjoint solids/shells).
    pub components: usize,
    /// V − E + F over the welded mesh. For `c` disjoint genus-0 shells this is
    /// `2c`; a single closed genus-0 solid is `2`.
    pub euler_characteristic: i64,
    pub closed: bool,
    pub manifold: bool,
    pub oriented: bool,
}

impl ManifoldReport {
    /// The result is a valid closed, oriented 2-manifold solid boundary.
    pub fn is_valid_solid(&self) -> bool {
        self.closed && self.manifold && self.oriented && self.triangles > 0
    }
}

/// Quantise a position to an integer lattice at spacing `eps` for welding.
fn weld_key(p: &crate::math::vector3::Point3, eps: f64) -> (i64, i64, i64) {
    (
        (p.x / eps).round() as i64,
        (p.y / eps).round() as i64,
        (p.z / eps).round() as i64,
    )
}

/// Tessellate `solid` and analyse the mesh's topological connectivity. `None`
/// if the solid is missing or tessellates to nothing.
///
/// `weld_eps` is the absolute distance below which two mesh vertices are treated
/// as the same point. Choose well under the chord length but comfortably above
/// f64 noise — `1e-6` works for the unit-to-ten-unit solids the harness builds.
pub fn manifold_report(
    model: &BRepModel,
    solid: SolidId,
    chord: f64,
    weld_eps: f64,
) -> Option<ManifoldReport> {
    let solid_ref = model.solids.get(solid)?;
    let params = TessellationParams {
        chord_tolerance: chord,
        ..TessellationParams::default()
    };
    let mesh = tessellate_solid(solid_ref, model, &params);
    if mesh.triangles.is_empty() {
        return None;
    }

    // Weld vertices by quantised position.
    let mut weld_map: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let mut welded_index: Vec<u32> = Vec::with_capacity(mesh.vertices.len());
    for v in &mesh.vertices {
        let key = weld_key(&v.position, weld_eps);
        let next = weld_map.len() as u32;
        let id = *weld_map.entry(key).or_insert(next);
        welded_index.push(id);
    }
    let welded_vertices = weld_map.len();

    // Directed-edge multiset over welded indices; skip degenerate triangles.
    let mut directed: HashMap<(u32, u32), u32> = HashMap::new();
    let mut degenerate_triangles = 0usize;
    let mut live_triangles = 0usize;
    // Union-find over welded vertices for component counting.
    let mut parent: Vec<u32> = (0..welded_vertices as u32).collect();
    fn find(parent: &mut Vec<u32>, mut x: u32) -> u32 {
        while parent[x as usize] != x {
            parent[x as usize] = parent[parent[x as usize] as usize];
            x = parent[x as usize];
        }
        x
    }
    let mut union = |parent: &mut Vec<u32>, a: u32, b: u32| {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent[ra as usize] = rb;
        }
    };

    for tri in &mesh.triangles {
        let a = welded_index[tri[0] as usize];
        let b = welded_index[tri[1] as usize];
        let c = welded_index[tri[2] as usize];
        if a == b || b == c || c == a {
            degenerate_triangles += 1;
            continue;
        }
        live_triangles += 1;
        for &(u, v) in &[(a, b), (b, c), (c, a)] {
            *directed.entry((u, v)).or_insert(0) += 1;
            union(&mut parent, u, v);
        }
    }

    // Aggregate undirected edges from the directed multiset.
    let mut undirected: HashMap<(u32, u32), u32> = HashMap::new();
    let mut inconsistent_directed_edges = 0usize;
    for (&(u, v), &count) in &directed {
        if count > 1 {
            inconsistent_directed_edges += 1;
        }
        let key = if u < v { (u, v) } else { (v, u) };
        *undirected.entry(key).or_insert(0) += count;
    }

    let mut boundary_edges = 0usize;
    let mut nonmanifold_edges = 0usize;
    for &incident in undirected.values() {
        if incident == 1 {
            boundary_edges += 1;
        } else if incident > 2 {
            nonmanifold_edges += 1;
        }
    }

    // Components over vertices actually referenced by a live triangle.
    let mut roots = std::collections::HashSet::new();
    for (&(u, v), _) in &directed {
        roots.insert(find(&mut parent, u));
        roots.insert(find(&mut parent, v));
    }
    let components = roots.len().max(1);

    let v_count = {
        let mut used = std::collections::HashSet::new();
        for (&(u, v), _) in &directed {
            used.insert(u);
            used.insert(v);
        }
        used.len() as i64
    };
    let e_count = undirected.len() as i64;
    let f_count = live_triangles as i64;
    let euler_characteristic = v_count - e_count + f_count;

    Some(ManifoldReport {
        triangles: mesh.triangles.len(),
        degenerate_triangles,
        welded_vertices,
        undirected_edges: undirected.len(),
        boundary_edges,
        nonmanifold_edges,
        inconsistent_directed_edges,
        components,
        euler_characteristic,
        closed: boundary_edges == 0,
        manifold: nonmanifold_edges == 0,
        oriented: inconsistent_directed_edges == 0,
    })
}

/// Convenience: is `solid` a valid closed, oriented 2-manifold at the given
/// chord tolerance? Uses a `1e-6` weld epsilon.
pub fn is_manifold(model: &BRepModel, solid: SolidId, chord: f64) -> bool {
    manifold_report(model, solid, chord, 1e-6)
        .map(|r| r.is_valid_solid())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::vector3::Vector3;
    use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use crate::operations::transform::translate;
    use crate::primitives::topology_builder::TopologyBuilder;

    fn last_solid(model: &BRepModel) -> SolidId {
        model.solids.iter().last().map(|(id, _)| id).expect("solid")
    }

    #[test]
    fn primitives_are_watertight() {
        // Box (exact), sphere and cylinder (curved, faceted) must all enclose
        // their analytic volume within the faceting tolerance.
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("box");
        let box_solid = last_solid(&model);
        assert!(
            is_watertight(&mut model, box_solid, 0.01, 1e-6),
            "box leaks"
        );

        let mut m2 = BRepModel::new();
        TopologyBuilder::new(&mut m2)
            .create_sphere_3d(Vector3::new(0.0, 0.0, 0.0), 3.0)
            .expect("sphere");
        let sphere = last_solid(&m2);
        assert!(is_watertight(&mut m2, sphere, 0.01, 0.03), "sphere leaks");

        let mut m3 = BRepModel::new();
        TopologyBuilder::new(&mut m3)
            .create_cylinder_3d(Vector3::new(0.0, 0.0, 0.0), Vector3::Z, 2.0, 5.0)
            .expect("cylinder");
        let cyl = last_solid(&m3);
        assert!(is_watertight(&mut m3, cyl, 0.01, 0.03), "cylinder leaks");
    }

    #[test]
    fn boolean_result_is_watertight() {
        // A union of two overlapping boxes must itself be a closed solid.
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_box_3d(4.0, 4.0, 4.0)
            .expect("a");
        let a = last_solid(&model);
        TopologyBuilder::new(&mut model)
            .create_box_3d(4.0, 4.0, 4.0)
            .expect("b");
        let b = last_solid(&model);
        translate(&mut model, vec![b], Vector3::X, 2.0, Default::default()).expect("translate");

        let result = boolean_operation(
            &mut model,
            a,
            b,
            BooleanOp::Union,
            BooleanOptions::default(),
        )
        .expect("union");
        assert!(
            is_watertight(&mut model, result, 0.01, 1e-3),
            "boolean union result is not watertight"
        );
    }

    #[test]
    fn mesh_volume_matches_analytic_for_a_box() {
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_box_3d(2.0, 3.0, 4.0)
            .expect("box");
        let solid = last_solid(&model);
        let analytic = analytic_volume(&mut model, solid).expect("analytic");
        let mesh = mesh_volume(&model, solid, 0.01).expect("mesh");
        assert!((analytic - 24.0).abs() < 1e-6, "analytic {analytic}");
        assert!((mesh - 24.0).abs() < 1e-6, "mesh {mesh}");
    }

    // ── Manifold oracle ─────────────────────────────────────────────────

    #[test]
    #[ignore = "diagnostic: print manifold reports for all primitives"]
    fn diag_primitive_manifold_reports() {
        let cases: Vec<(&str, Box<dyn Fn(&mut BRepModel)>)> = vec![
            (
                "box",
                Box::new(|m: &mut BRepModel| {
                    TopologyBuilder::new(m)
                        .create_box_3d(2.0, 2.0, 2.0)
                        .unwrap();
                }),
            ),
            (
                "sphere",
                Box::new(|m: &mut BRepModel| {
                    TopologyBuilder::new(m)
                        .create_sphere_3d(Vector3::ZERO, 3.0)
                        .unwrap();
                }),
            ),
            (
                "cylinder",
                Box::new(|m: &mut BRepModel| {
                    TopologyBuilder::new(m)
                        .create_cylinder_3d(Vector3::ZERO, Vector3::Z, 2.0, 5.0)
                        .unwrap();
                }),
            ),
            (
                "cone",
                Box::new(|m: &mut BRepModel| {
                    TopologyBuilder::new(m)
                        .create_cone_3d(Vector3::ZERO, Vector3::Z, 2.0, 0.0, 5.0)
                        .unwrap();
                }),
            ),
            (
                "cone-frustum",
                Box::new(|m: &mut BRepModel| {
                    TopologyBuilder::new(m)
                        .create_cone_3d(Vector3::ZERO, Vector3::Z, 2.0, 1.0, 5.0)
                        .unwrap();
                }),
            ),
            (
                "torus",
                Box::new(|m: &mut BRepModel| {
                    TopologyBuilder::new(m)
                        .create_torus_3d(Vector3::ZERO, Vector3::Z, 3.0, 1.0)
                        .unwrap();
                }),
            ),
        ];
        for (name, build) in cases {
            let mut m = BRepModel::new();
            build(&mut m);
            let s = last_solid(&m);
            match manifold_report(&m, s, 0.05, 1e-6) {
                Some(r) => eprintln!(
                    "{name:>14}: valid={} closed={} manifold={} oriented={} \
                     bnd={} nonman={} inconsistent={} euler={} comp={} tris={}",
                    r.is_valid_solid(),
                    r.closed,
                    r.manifold,
                    r.oriented,
                    r.boundary_edges,
                    r.nonmanifold_edges,
                    r.inconsistent_directed_edges,
                    r.euler_characteristic,
                    r.components,
                    r.triangles,
                ),
                None => eprintln!("{name:>14}: NO MESH"),
            }
        }
    }

    #[test]
    fn primitives_are_valid_manifolds() {
        // Each primitive's tessellation must be a closed, oriented 2-manifold
        // with the genus-0 Euler characteristic 2.
        let mut m = BRepModel::new();
        TopologyBuilder::new(&mut m)
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("box");
        let b = last_solid(&m);
        let r = manifold_report(&m, b, 0.05, 1e-6).expect("box mesh");
        assert!(r.is_valid_solid(), "box not a valid manifold: {r:?}");
        assert_eq!(r.euler_characteristic, 2, "box euler: {r:?}");
        assert_eq!(r.components, 1, "box components: {r:?}");

        let mut m2 = BRepModel::new();
        TopologyBuilder::new(&mut m2)
            .create_sphere_3d(Vector3::ZERO, 3.0)
            .expect("sphere");
        let s = last_solid(&m2);
        let r = manifold_report(&m2, s, 0.05, 1e-6).expect("sphere mesh");
        assert!(r.is_valid_solid(), "sphere not a valid manifold: {r:?}");
        assert_eq!(r.euler_characteristic, 2, "sphere euler: {r:?}");

        let mut m3 = BRepModel::new();
        TopologyBuilder::new(&mut m3)
            .create_cylinder_3d(Vector3::ZERO, Vector3::Z, 2.0, 5.0)
            .expect("cylinder");
        let c = last_solid(&m3);
        let r = manifold_report(&m3, c, 0.05, 1e-6).expect("cyl mesh");
        assert!(r.is_valid_solid(), "cylinder not a valid manifold: {r:?}");
        assert_eq!(r.euler_characteristic, 2, "cylinder euler: {r:?}");
    }

    #[test]
    fn box_union_is_a_valid_manifold() {
        // Overlapping-box union must close into a single valid genus-0 manifold.
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_box_3d(4.0, 4.0, 4.0)
            .expect("a");
        let a = last_solid(&model);
        TopologyBuilder::new(&mut model)
            .create_box_3d(4.0, 4.0, 4.0)
            .expect("b");
        let b = last_solid(&model);
        translate(&mut model, vec![b], Vector3::X, 2.0, Default::default()).expect("translate");
        let result = boolean_operation(
            &mut model,
            a,
            b,
            BooleanOp::Union,
            BooleanOptions::default(),
        )
        .expect("union");
        let r = manifold_report(&model, result, 0.05, 1e-6).expect("union mesh");
        assert!(r.is_valid_solid(), "box union not a valid manifold: {r:?}");
        assert_eq!(r.euler_characteristic, 2, "union euler: {r:?}");
    }

    /// The oracle is strict enough to FAIL on a known-broken result: the
    /// sphere-poke-through (sphere r=2.5 through a 4-box) still mis-partitions
    /// the spherical face (tracked #53/#54). `is_watertight` is fooled because
    /// the mis-stitched mesh's signed volume is self-consistent, but the
    /// manifold oracle catches the open/duplicated edges. When #53/#54 land,
    /// flip this to `assert!(r.is_valid_solid())` and drop the `#[ignore]`.
    #[test]
    #[ignore = "#53/#54: sphere poke-through still mis-stitches; oracle correctly rejects it"]
    fn sphere_poke_through_is_not_yet_manifold() {
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_box_3d(4.0, 4.0, 4.0)
            .expect("box");
        let a = last_solid(&model);
        TopologyBuilder::new(&mut model)
            .create_sphere_3d(Vector3::ZERO, 2.5)
            .expect("sphere");
        let b = last_solid(&model);
        let result = boolean_operation(
            &mut model,
            a,
            b,
            BooleanOp::Intersection,
            BooleanOptions::default(),
        )
        .expect("intersection");
        let r = manifold_report(&model, result, 0.05, 1e-6).expect("mesh");
        eprintln!("sphere-poke ∩ manifold report: {r:?}");
        assert!(
            !r.is_valid_solid(),
            "sphere poke-through unexpectedly became a valid manifold — \
             if #53/#54 are fixed, invert this assertion: {r:?}"
        );
    }
}
