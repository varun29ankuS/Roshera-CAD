//! #21 repro battery — primitive-face shared-edge watertightness.
//!
//! The EdgeSampleCache makes planar + curved-CDT faces sample shared edges
//! bit-exactly. The primitive GRID fallbacks (sphere / torus / cone-apex) are
//! suspected to re-sample rims at `arc_steps_for_quality` density, ignoring the
//! cache, so a trimmed primitive face sharing a rim with a cache-sampled
//! neighbour would T-junction. This battery tessellates standalone AND trimmed
//! primitive solids across chord tolerances and reports leak counts, so we see
//! WHICH configurations actually break before touching the tessellator.

use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

/// Tessellate at three chord tolerances and return `(label, chord, boundary,
/// nonmanifold, closed)` rows.
fn probe(model: &BRepModel, solid: SolidId, label: &str) -> Vec<(String, f64, usize, usize, bool)> {
    let mut rows = Vec::new();
    for &chord in &[0.5_f64, 0.1, 0.02] {
        match manifold_report(model, solid, chord, 1e-6) {
            Some(r) => rows.push((
                label.to_string(),
                chord,
                r.boundary_edges,
                r.nonmanifold_edges,
                r.closed,
            )),
            None => rows.push((label.to_string(), chord, usize::MAX, usize::MAX, false)),
        }
    }
    rows
}

#[test]
fn primitive_faces_are_watertight_across_tolerances() {
    let mut all: Vec<(String, f64, usize, usize, bool)> = Vec::new();

    // --- Standalone primitives (regression guard: these must stay watertight) ---
    {
        let mut m = BRepModel::new();
        let s = sid(TopologyBuilder::new(&mut m)
            .create_sphere_3d(Point3::ZERO, 10.0)
            .expect("sphere"));
        all.extend(probe(&m, s, "sphere_full"));
    }
    {
        let mut m = BRepModel::new();
        let c = sid(TopologyBuilder::new(&mut m)
            .create_cone_3d(Point3::ZERO, Vector3::Z, 10.0, 0.0, 20.0)
            .expect("cone_apex"));
        all.extend(probe(&m, c, "cone_apex_full"));
    }
    {
        let mut m = BRepModel::new();
        let c = sid(TopologyBuilder::new(&mut m)
            .create_cone_3d(Point3::ZERO, Vector3::Z, 10.0, 5.0, 20.0)
            .expect("cone_frustum"));
        all.extend(probe(&m, c, "cone_frustum_full"));
    }
    {
        let mut m = BRepModel::new();
        let t = sid(TopologyBuilder::new(&mut m)
            .create_torus_3d(Point3::ZERO, Vector3::Z, 20.0, 6.0)
            .expect("torus"));
        all.extend(probe(&m, t, "torus_full"));
    }

    // --- Trimmed: sphere cut flat by a box (hemisphere) → spherical cap meets a
    // planar disc on the equator rim. ---
    {
        let mut m = BRepModel::new();
        let s = sid(TopologyBuilder::new(&mut m)
            .create_sphere_3d(Point3::ZERO, 10.0)
            .expect("sphere"));
        let cutter = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(40.0, 40.0, 20.0)
            .expect("box"));
        // Box centred at z=-10 removes the lower half → top hemisphere.
        let m2 = move_box(&mut m, cutter, Point3::new(0.0, 0.0, -10.0));
        match try_bool(&mut m, s, m2, BooleanOp::Difference) {
            Some(part) => all.extend(probe(&m, part, "hemisphere_sphere_minus_box")),
            None => all.push(bool_fail("hemisphere_sphere_minus_box")),
        }
    }

    // --- Trimmed: box with a hemispherical pocket (box minus a sphere poking a
    // face). The spherical face is interior, sharing a rim circle with the box
    // face it broke through. ---
    {
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(40.0, 40.0, 20.0)
            .expect("box"));
        let s = sid(TopologyBuilder::new(&mut m)
            .create_sphere_3d(Point3::new(0.0, 0.0, 10.0), 12.0)
            .expect("sphere"));
        match try_bool(&mut m, b, s, BooleanOp::Difference) {
            Some(part) => all.extend(probe(&m, part, "box_minus_sphere_pocket")),
            None => all.push(bool_fail("box_minus_sphere_pocket")),
        }
    }

    // --- Trimmed: torus cut flat by a box (half torus) → toroidal face meets
    // two planar half-discs. ---
    {
        let mut m = BRepModel::new();
        let t = sid(TopologyBuilder::new(&mut m)
            .create_torus_3d(Point3::ZERO, Vector3::Z, 20.0, 6.0)
            .expect("torus"));
        let cutter = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(80.0, 80.0, 20.0)
            .expect("box"));
        let m2 = move_box(&mut m, cutter, Point3::new(0.0, 0.0, -10.0));
        match try_bool(&mut m, t, m2, BooleanOp::Difference) {
            Some(part) => all.extend(probe(&m, part, "half_torus_minus_box")),
            None => all.push(bool_fail("half_torus_minus_box")),
        }
    }

    // --- Trimmed: cone (apex) cut flat by a box near the apex → smaller frustum
    // with a planar top sharing the cut circle. ---
    {
        let mut m = BRepModel::new();
        let c = sid(TopologyBuilder::new(&mut m)
            .create_cone_3d(Point3::ZERO, Vector3::Z, 10.0, 0.0, 20.0)
            .expect("cone"));
        let cutter = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(40.0, 40.0, 20.0)
            .expect("box"));
        let m2 = move_box(&mut m, cutter, Point3::new(0.0, 0.0, 25.0));
        match try_bool(&mut m, c, m2, BooleanOp::Difference) {
            Some(part) => all.extend(probe(&m, part, "cone_apex_minus_top_box")),
            None => all.push(bool_fail("cone_apex_minus_top_box")),
        }
    }

    // Report everything; fail listing every leaking configuration. A BOOL-FAIL
    // row (chord < 0) means the boolean itself produced no solid — a boolean
    // limitation, NOT a tessellation watertightness leak — so it is reported but
    // does not fail this (tessellation) gate.
    let mut failures = Vec::new();
    let mut report = String::from("\n#21 primitive watertightness battery:\n");
    for (label, chord, boundary, nonman, closed) in &all {
        if *chord < 0.0 {
            report.push_str(&format!("  {label:<32} BOOL-FAIL (no solid produced)\n"));
            continue;
        }
        let bstr = if *boundary == usize::MAX {
            "TESS-FAIL".to_string()
        } else {
            boundary.to_string()
        };
        report.push_str(&format!(
            "  {label:<32} chord={chord:<5} boundary={bstr:<10} nonmanifold={nonman} closed={closed}\n"
        ));
        if *boundary != 0 || !*closed {
            failures.push(format!("{label}@chord{chord}"));
        }
    }
    eprintln!("{report}");
    assert!(
        failures.is_empty(),
        "{report}\nLEAKING configurations: {failures:?}"
    );
}

/// A row marking that a boolean produced no solid (chord = −1 sentinel).
fn bool_fail(label: &str) -> (String, f64, usize, usize, bool) {
    (label.to_string(), -1.0, 0, 0, false)
}

/// Translate a box's geometry by `delta` (the builder always centres at origin).
/// `transform_solid` mutates the solid in place, so the id is unchanged.
fn move_box(model: &mut BRepModel, solid: SolidId, delta: Point3) -> SolidId {
    use geometry_engine::math::Matrix4;
    use geometry_engine::operations::transform::{transform_solid, TransformOptions};
    let t = Matrix4::translation(delta.x, delta.y, delta.z);
    let _ = transform_solid(model, solid, t, TransformOptions::default());
    solid
}

fn try_bool(model: &mut BRepModel, a: SolidId, b: SolidId, op: BooleanOp) -> Option<SolidId> {
    boolean_operation(model, a, b, op, BooleanOptions::default()).ok()
}
