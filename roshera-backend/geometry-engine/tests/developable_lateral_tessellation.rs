//! HARNESS GATE: a developable (cylinder/cone) lateral face must tessellate
//! WITHOUT a Ruppert refinement explosion, however its boundary is trimmed.
//!
//! Root cause this locks down (the user's long-standing inner-bore "scribble"):
//! a full-2π cylinder wall trimmed by a boolean (a bore interrupted by a pocket /
//! slot) is sampled on a coarse rim, so its developable-collapse skinny triangles
//! marginally fail the chord/normal fidelity gate. Interior Ruppert refinement
//! then CANNOT fix a boundary-arc-dominated error and instead CASCADES — the
//! per-pass chord + skinny scans both fire and DOUBLE the triangle count every
//! pass (instrumented on an imported part: 798 → 1834 → 4048 → 9618 triangles,
//! ~15k boundary-encroachment drops, 92 zero-area slivers). A *partial* (un-seamed)
//! wall happens to stay under tolerance and converges with zero additions, which
//! is why only the full-wrap trimmed wall scribbled.
//!
//! Fix: `Surface::is_developable()` takes the documented developable fast-path —
//! a zero-Gaussian-curvature lateral is already chord-faithful after the initial
//! CDT (developable-collapse grid + curvature-driven rim sampling), so it skips
//! interior refinement entirely. This gate sweeps the trimmed full-2π case and
//! fails if any cylinder face regresses to slivers or an exploded triangle count.

use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(id) => id,
        other => panic!("expected Solid, got {other:?}"),
    }
}

fn tri_area(p0: Point3, p1: Point3, p2: Point3) -> f64 {
    (p1 - p0).cross(&(p2 - p0)).magnitude() * 0.5
}

/// A solid cylinder (r=30, h=80) CROSS-DRILLED by a perpendicular through-hole
/// (r=12 along X). The main full-2π lateral wall is then trimmed by the two
/// windows where the cross-hole breaks through — the full-2π multi-edge cylinder
/// wall that triggered the explosion — and cylinder∖cylinder is a robust enough
/// boolean to keep the result watertight (unlike a slot-across-bore corefinement).
fn cross_drilled_cylinder() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let main = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 30.0, 80.0)
        .expect("main cylinder"));
    let hole = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::X, 12.0, 100.0)
        .expect("cross hole"));
    let result = boolean_operation(
        &mut m,
        main,
        hole,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("cross-drill difference");
    (m, result)
}

#[test]
fn trimmed_full_wrap_cylinder_has_no_refinement_explosion() {
    let (m, solid) = cross_drilled_cylinder();
    let params = TessellationParams::default();
    let solid_ref = m.solids.get(solid).expect("solid");
    let mesh = tessellate_solid(solid_ref, &m, &params);
    assert!(!mesh.triangles.is_empty(), "solid tessellated to nothing");
    assert_eq!(
        mesh.face_map.len(),
        mesh.triangles.len(),
        "every triangle must carry a face id"
    );

    // Per-face triangle areas, bucketed by face id.
    let mut per_face: std::collections::HashMap<u32, Vec<f64>> = std::collections::HashMap::new();
    for (i, tri) in mesh.triangles.iter().enumerate() {
        let a = mesh.vertices[tri[0] as usize].position;
        let b = mesh.vertices[tri[1] as usize].position;
        let c = mesh.vertices[tri[2] as usize].position;
        per_face
            .entry(mesh.face_map[i])
            .or_default()
            .push(tri_area(a, b, c));
    }

    let shell = m.shells.get(solid_ref.outer_shell).expect("shell");
    let mut checked_cyl = 0usize;
    let mut trimmed_cyl = 0usize;
    for &fid in &shell.faces {
        let face = match m.faces.get(fid) {
            Some(f) => f,
            None => continue,
        };
        let surface = match m.surfaces.get(face.surface_id) {
            Some(s) => s,
            None => continue,
        };
        if !surface.is_developable() {
            continue; // gate is about developable laterals
        }
        checked_cyl += 1;
        let edges = m
            .loops
            .get(face.outer_loop)
            .map(|l| l.edges.len())
            .unwrap_or(0);
        if edges > 4 {
            trimmed_cyl += 1; // a genuinely trimmed (multi-edge) wall
        }
        let areas = per_face.get(&fid).cloned().unwrap_or_default();
        assert!(
            !areas.is_empty(),
            "developable face {fid} ({} edges) emitted no triangles",
            edges
        );
        let total: f64 = areas.iter().sum();
        let mean = total / areas.len() as f64;
        assert!(
            total > 1e-6,
            "developable face {fid} collapsed to ~zero area ({total:.3e}) — degenerate tessellation"
        );

        // No slivers: a developable lateral with the fast-path emits a clean
        // developable grid; the refinement cascade produced near-zero-area
        // facets (min ~1e-7 vs a healthy ~0.3). Flag any facet < 1e-4 of mean.
        let slivers = areas.iter().filter(|&&a| a < mean * 1e-4).count();
        assert_eq!(
            slivers, 0,
            "developable face {fid} ({} edges, {} tris) has {} sliver facets — refinement explosion regressed",
            edges,
            areas.len(),
            slivers
        );

        // Bounded count: the developable grid is O(rim samples); the cascade
        // exploded a single wall to ~9.6k. A generous ceiling of 4000 catches a
        // regression while leaving ample headroom for a legitimately dense wall.
        assert!(
            areas.len() < 4000,
            "developable face {fid} ({} edges) emitted {} triangles — refinement-explosion regression (was 798 clean)",
            edges,
            areas.len()
        );
    }

    assert!(
        checked_cyl > 0,
        "fixture produced no developable faces to gate"
    );
    assert!(
        trimmed_cyl > 0,
        "fixture did not produce a TRIMMED (multi-edge) cylinder wall — the edge case is not being exercised"
    );

    // NOTE: this gate deliberately checks PER-FACE developable tessellation
    // quality, not whole-solid watertightness. A curved∖curved corefinement
    // (cross-drilled cylinder) does not yet weld fully watertight in this kernel
    // (the separate #35/#17 boolean-corefinement issue) — but each developable
    // face's tessellation is independent of that, and the explosion/sliver
    // regression this gate guards lives entirely in the per-face curved-CDT path.
    // Whole-solid watertightness is owned by the poke-matrix / watertight gates.
}

#[test]
fn clean_full_cylinder_is_unchanged_by_the_fast_path() {
    // A plain solid cylinder already converged with zero refinement additions,
    // so the developable fast-path must be a no-op: still clean, watertight.
    let mut m = BRepModel::new();
    let cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 25.0, 80.0)
        .expect("cylinder"));
    let params = TessellationParams::default();
    let solid_ref = m.solids.get(cyl).expect("solid");
    let mesh = tessellate_solid(solid_ref, &m, &params);
    let mut min_area = f64::INFINITY;
    let mut total = 0.0;
    let mut n = 0usize;
    for tri in &mesh.triangles {
        let a = mesh.vertices[tri[0] as usize].position;
        let b = mesh.vertices[tri[1] as usize].position;
        let c = mesh.vertices[tri[2] as usize].position;
        let ar = tri_area(a, b, c);
        min_area = min_area.min(ar);
        total += ar;
        n += 1;
    }
    assert!(n > 0 && total > 1.0, "cylinder tessellated to nothing");
    assert!(
        min_area > (total / n as f64) * 1e-4,
        "clean cylinder gained slivers (min {min_area:.3e}) — fast-path regressed a converging case"
    );
    let report = manifold_report(&m, cyl, 0.1, 1e-6).expect("mesh");
    assert_eq!(report.boundary_edges, 0, "clean cylinder leaked");
    assert_eq!(report.nonmanifold_edges, 0, "clean cylinder non-manifold");
}
