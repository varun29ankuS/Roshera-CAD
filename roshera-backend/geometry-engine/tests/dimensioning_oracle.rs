//! Dimensioning oracle (campaign task #6) — the real measure of "amazing
//! perception": for a part built with KNOWN dimensions, the analytic dimension
//! table must recover every built dimension (built == perceived) AND every
//! record must be recoverable (finite world anchor; feature dims name real
//! faces of the solid). Values are read off analytic surfaces / exact curves,
//! never the mesh — so the table is ground truth, not a measurement.
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::readable::{extract_dimensions, DimensionRecord};

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

fn diff(m: &mut BRepModel, a: SolidId, b: SolidId) -> SolidId {
    boolean_operation(m, a, b, BooleanOp::Difference, BooleanOptions::default())
        .expect("difference")
}

fn has(dims: &[DimensionRecord], kind: &str, value: f64) -> bool {
    dims.iter()
        .any(|d| d.kind == kind && (d.value - value).abs() < 2e-3)
}

fn count(dims: &[DimensionRecord], kind: &str, value: f64) -> usize {
    dims.iter()
        .filter(|d| d.kind == kind && (d.value - value).abs() < 2e-3)
        .count()
}

fn face_ids(m: &BRepModel, s: SolidId) -> Vec<u32> {
    let solid = m.solids.get(s).unwrap_or_else(|| panic!("solid"));
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    let mut out = Vec::new();
    for shid in shells {
        if let Some(shell) = m.shells.get(shid) {
            out.extend_from_slice(&shell.faces);
        }
    }
    out
}

/// Every record must be recoverable: a finite world anchor, and any face it
/// claims to span must actually exist on the solid.
fn assert_recoverable(m: &BRepModel, s: SolidId, dims: &[DimensionRecord], label: &str) {
    let faces = face_ids(m, s);
    for d in dims {
        assert!(
            d.anchor.iter().all(|c| c.is_finite()),
            "{label}: non-finite anchor on {}: {:?}",
            d.id,
            d.anchor
        );
        for &e in &d.entities {
            assert!(
                faces.contains(&e),
                "{label}: dim {} names face {e} not on the solid (faces={faces:?})",
                d.id
            );
        }
    }
    // Every feature (diameter/length/angle) dim must name at least one face.
    for d in dims {
        if d.kind == "diameter" || d.kind == "length" || d.kind == "angle" {
            assert!(
                !d.entities.is_empty(),
                "{label}: feature dim {} ({}) names no face",
                d.id,
                d.label
            );
        }
    }
}

#[test]
fn box_extents_match_built() {
    let mut m = BRepModel::new();
    let b = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(40.0, 30.0, 20.0)
        .expect("box"));
    let dims = extract_dimensions(&m, b);
    assert!(has(&dims, "extent", 40.0), "X 40: {dims:?}");
    assert!(has(&dims, "extent", 30.0), "Y 30");
    assert!(has(&dims, "extent", 20.0), "Z 20");
    assert_recoverable(&m, b, &dims, "box");
}

#[test]
fn cylinder_diameter_length_extents_match_built() {
    let mut m = BRepModel::new();
    let c = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::ZERO, Vector3::Z, 15.0, 50.0)
        .expect("cyl"));
    let dims = extract_dimensions(&m, c);
    assert!(has(&dims, "diameter", 30.0), "Ø30: {dims:?}");
    assert!(has(&dims, "length", 50.0), "L50");
    assert!(has(&dims, "extent", 30.0), "extent Ø30 (curve-sampled)");
    assert!(has(&dims, "extent", 50.0), "extent H50");
    assert_recoverable(&m, c, &dims, "cylinder");
}

#[test]
fn sphere_diameter_and_extents_match_built() {
    let mut m = BRepModel::new();
    let s = sid(TopologyBuilder::new(&mut m)
        .create_sphere_3d(Point3::ZERO, 22.0)
        .expect("sphere"));
    let dims = extract_dimensions(&m, s);
    assert!(has(&dims, "diameter", 44.0), "SØ44: {dims:?}");
    // A sphere's full ±radius envelope on ALL THREE axes (sparse seam edges
    // must not under-report it).
    assert_eq!(
        count(&dims, "extent", 44.0),
        3,
        "sphere should be Ø44 on X, Y AND Z: {dims:?}"
    );
    assert_recoverable(&m, s, &dims, "sphere");
}

#[test]
fn cone_reports_angle_and_base_diameter() {
    let mut m = BRepModel::new();
    // base_r 20, apex (top_r 0), height 30 → half-angle atan(20/30) ≈ 33.69°.
    let c = sid(TopologyBuilder::new(&mut m)
        .create_cone_3d(Point3::ZERO, Vector3::Z, 20.0, 0.0, 30.0)
        .expect("cone"));
    let dims = extract_dimensions(&m, c);
    let want_deg = (20.0_f64 / 30.0).atan().to_degrees();
    assert!(
        dims.iter()
            .any(|d| d.kind == "angle" && (d.value - want_deg).abs() < 0.5),
        "cone half-angle ~{want_deg:.2}°: {dims:?}"
    );
    assert!(has(&dims, "diameter", 40.0), "cone base Ø40: {dims:?}");
    assert_recoverable(&m, c, &dims, "cone");
}

#[test]
fn bored_plate_matches_built() {
    let mut m = BRepModel::new();
    let plate = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(50.0, 50.0, 16.0)
        .expect("plate"));
    let cutter = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -20.0), Vector3::Z, 10.0, 80.0)
        .expect("cutter"));
    let part = diff(&mut m, plate, cutter);
    let dims = extract_dimensions(&m, part);
    assert!(has(&dims, "diameter", 20.0), "bore Ø20: {dims:?}");
    assert!(
        has(&dims, "length", 16.0),
        "bore length = plate thickness 16"
    );
    assert!(has(&dims, "extent", 50.0), "extent 50");
    assert!(has(&dims, "extent", 16.0), "extent 16");
    assert_recoverable(&m, part, &dims, "bored plate");
}

#[test]
fn flange_bolt_circle_matches_built() {
    // Disc Ø80 × 12, central Ø24 bore, 4× Ø6 bolt holes on a Ø56 circle.
    let mut m = BRepModel::new();
    let mut part = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::ZERO, Vector3::Z, 40.0, 12.0)
        .expect("disc"));
    let central = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -10.0), Vector3::Z, 12.0, 40.0)
        .expect("central"));
    part = diff(&mut m, part, central);
    for i in 0..4 {
        let a = std::f64::consts::TAU * i as f64 / 4.0;
        let bolt = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(
                Point3::new(28.0 * a.cos(), 28.0 * a.sin(), -10.0),
                Vector3::Z,
                3.0,
                40.0,
            )
            .expect("bolt"));
        part = diff(&mut m, part, bolt);
    }
    let dims = extract_dimensions(&m, part);
    assert!(has(&dims, "diameter", 80.0), "outer Ø80: {dims:?}");
    assert!(has(&dims, "diameter", 24.0), "central bore Ø24");
    assert_eq!(
        count(&dims, "diameter", 6.0),
        4,
        "4× Ø6 bolt holes: {dims:?}"
    );
    assert!(has(&dims, "extent", 80.0), "extent Ø80");
    assert!(has(&dims, "extent", 12.0), "extent thickness 12");
    assert_recoverable(&m, part, &dims, "flange");
}

#[test]
fn boss_on_plate_matches_built() {
    let mut m = BRepModel::new();
    let plate = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(60.0, 60.0, 20.0)
        .expect("plate"));
    let boss = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 5.0), Vector3::Z, 12.0, 20.0)
        .expect("boss"));
    let part = boolean_operation(
        &mut m,
        plate,
        boss,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("union");
    let dims = extract_dimensions(&m, part);
    assert!(has(&dims, "diameter", 24.0), "boss Ø24: {dims:?}");
    assert!(has(&dims, "extent", 60.0), "plate extent 60");
    assert_recoverable(&m, part, &dims, "boss on plate");
}

// ── Persistent dimension identity (Task 1: Viewport Dimensions campaign) ──────

/// A dimension's PID must be stable across unrelated edits.  Bore A is drilled
/// first; then an independent bore B is added.  Bore A's diameter record must
/// keep the exact same `pid`; bore B gets a different one.
#[test]
fn bore_diameter_pid_survives_unrelated_edit() {
    let mut m = BRepModel::new();
    let plate = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(60.0, 60.0, 10.0)
        .expect("plate"));
    let bore_a_cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(-15.0, 0.0, -6.0), Vector3::Z, 4.0, 12.0)
        .expect("bore_a_cyl"));
    let part = diff(&mut m, plate, bore_a_cyl);

    // Capture bore A's diameter pid.
    let pid_a = extract_dimensions(&m, part)
        .into_iter()
        .find(|d| d.kind == "diameter" && (d.value - 8.0).abs() < 1e-6)
        .and_then(|d| d.pid.clone())
        .expect("bore A diameter must carry a pid");

    // Drill an unrelated bore B.
    let bore_b_cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(15.0, 0.0, -6.0), Vector3::Z, 2.0, 12.0)
        .expect("bore_b_cyl"));
    let part2 = diff(&mut m, part, bore_b_cyl);

    let dims2 = extract_dimensions(&m, part2);

    // Bore A's pid must be unchanged.
    let pid_a2 = dims2
        .iter()
        .find(|d| d.kind == "diameter" && (d.value - 8.0).abs() < 1e-6)
        .and_then(|d| d.pid.clone())
        .expect("bore A diameter must still carry a pid after bore B was added");
    assert_eq!(
        pid_a, pid_a2,
        "bore A identity follows its entity across unrelated edits"
    );

    // Bore B has a distinct pid.
    let pid_b = dims2
        .iter()
        .find(|d| d.kind == "diameter" && (d.value - 4.0).abs() < 1e-6)
        .and_then(|d| d.pid.clone())
        .expect("bore B diameter must carry a pid");
    assert_ne!(pid_a, pid_b, "bore A and bore B must have distinct pids");
}

/// Extents for a box must each carry a pid; all three must be distinct; and
/// re-extracting the same model must return identical pids (determinism).
#[test]
fn extent_pid_stable_and_axis_distinct() {
    let mut m = BRepModel::new();
    let b1 = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(40.0, 30.0, 20.0)
        .expect("box"));
    let dims = extract_dimensions(&m, b1);

    let pids: Vec<Option<String>> = dims
        .iter()
        .filter(|d| d.kind == "extent")
        .map(|d| d.pid.clone())
        .collect();
    assert_eq!(
        pids.len(),
        3,
        "expected 3 extent records, got {}",
        pids.len()
    );
    assert!(
        pids.iter().all(|p| p.is_some()),
        "all extent records must carry a pid: {pids:?}"
    );

    // All three axis pids must be distinct.
    let uniq: std::collections::HashSet<_> = pids.iter().collect();
    assert_eq!(
        uniq.len(),
        3,
        "X/Y/Z extents must have distinct pids: {pids:?}"
    );

    // Re-extract → identical pids (deterministic).
    let again: Vec<Option<String>> = extract_dimensions(&m, b1)
        .into_iter()
        .filter(|d| d.kind == "extent")
        .map(|d| d.pid)
        .collect();
    assert_eq!(
        pids, again,
        "pids must be deterministic across re-extraction"
    );
}
