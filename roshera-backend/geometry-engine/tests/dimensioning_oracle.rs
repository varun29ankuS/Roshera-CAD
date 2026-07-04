//! Dimensioning oracle (campaign task #6) — the real measure of "amazing
//! perception": for a part built with KNOWN dimensions, the analytic dimension
//! table must recover every built dimension (built == perceived) AND every
//! record must be recoverable (finite world anchor; feature dims name real
//! faces of the solid). Values are read off analytic surfaces / exact curves,
//! never the mesh — so the table is ground truth, not a measurement.
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::Plane;
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

/// Raw PersistentId (u128) of the planar face of `s` whose plane normal is
/// parallel to `axis` and whose plane origin projects to `offset` along it.
/// Panics if no such face exists (test precondition).
fn plane_face_pid(m: &BRepModel, s: SolidId, axis: Vector3, offset: f64) -> Option<u128> {
    for fid in face_ids(m, s) {
        let face = match m.faces.get(fid) {
            Some(f) => f,
            None => continue,
        };
        let surf = match m.surfaces.get(face.surface_id) {
            Some(s) => s,
            None => continue,
        };
        let pl = match surf.as_any().downcast_ref::<Plane>() {
            Some(p) => p,
            None => continue,
        };
        let along = pl.normal.x * axis.x + pl.normal.y * axis.y + pl.normal.z * axis.z;
        if along.abs() < 0.999 {
            continue;
        }
        let origin_proj = pl.origin.x * axis.x + pl.origin.y * axis.y + pl.origin.z * axis.z;
        if (origin_proj - offset).abs() > 1e-6 {
            continue;
        }
        return m.face_pids.get(&fid).map(|p| p.as_u128());
    }
    panic!("no planar face with normal ∥ {axis:?} at offset {offset} on solid {s:?}");
}

/// A face the boolean geometrically ALTERED (the plate top face gains the
/// bore's inner loop → it is an annular face, a different entity) must get a
/// NEW pid — while a face the boolean never touched (a side wall) keeps its
/// pid (true passthrough). Guards against the corruption case where a
/// single-fragment split survivor inherits its parent's PID and labels/GD&T
/// bound to the pre-cut face silently re-bind to the trimmed one.
#[test]
fn trimmed_face_pid_changes_untouched_face_pid_survives() {
    let mut m = BRepModel::new();
    // Box 60×60×10 centred at origin: top plane z=+5, side plane x=+30.
    let plate = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(60.0, 60.0, 10.0)
        .expect("plate"));
    let top_before = plane_face_pid(&m, plate, Vector3::Z, 5.0).expect("plate top face has a pid");
    let side_before =
        plane_face_pid(&m, plate, Vector3::X, 30.0).expect("plate side face has a pid");

    // Bore through the middle: cuts top and bottom, never touches the sides.
    let bore = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -6.0), Vector3::Z, 4.0, 12.0)
        .expect("bore"));
    let part = diff(&mut m, plate, bore);

    let top_after = plane_face_pid(&m, part, Vector3::Z, 5.0).expect("bored top face has a pid");
    let side_after = plane_face_pid(&m, part, Vector3::X, 30.0).expect("side face still has a pid");

    assert_eq!(
        side_before, side_after,
        "untouched side face is a true passthrough — its pid must survive"
    );
    assert_ne!(
        top_before, top_after,
        "the bored (annular) top face is a DIFFERENT entity from the flat \
         pre-cut face — inheriting the old pid silently re-binds labels/GD&T"
    );
}

/// Identity follows the ENTITY, not the shape: recreating a geometrically
/// identical bore (fresh plate, fresh cutter, same numbers) is a new feature
/// instance and must yield a DIFFERENT dimension pid.
#[test]
fn recreated_identical_bore_gets_new_pid() {
    let mut m = BRepModel::new();

    let bore_pid = |m: &mut BRepModel| -> String {
        let plate = sid(TopologyBuilder::new(m)
            .create_box_3d(60.0, 60.0, 10.0)
            .expect("plate"));
        let cutter = sid(TopologyBuilder::new(m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, -6.0), Vector3::Z, 4.0, 12.0)
            .expect("cutter"));
        let part = diff(m, plate, cutter);
        extract_dimensions(m, part)
            .into_iter()
            .find(|d| d.kind == "diameter" && (d.value - 8.0).abs() < 1e-6)
            .and_then(|d| d.pid)
            .expect("bore diameter must carry a pid")
    };

    // Build the bored plate, then "delete and recreate": build an identical
    // one from scratch in the same model — same geometry, new entities.
    let pid_first = bore_pid(&mut m);
    let pid_recreated = bore_pid(&mut m);

    assert_ne!(
        pid_first, pid_recreated,
        "a recreated identical bore is a NEW feature instance — identity \
         follows the entity, not the shape"
    );
}

// ── Amendment A2: position dimensions (Task 8) ────────────────────────────────

/// Helper: find position records with a given axis tag ("x" or "y").
fn position_records<'a>(dims: &'a [DimensionRecord], axis: &str) -> Vec<&'a DimensionRecord> {
    let prefix = format!("{} ", axis.to_uppercase());
    dims.iter()
        .filter(|d| d.kind == "position" && d.label.starts_with(&prefix))
        .collect()
}

/// A 60×40 plate with one Z-axis bore at world centre (−15, 5):
///   - plate runs from x=−30..+30, y=−20..+20 (centred at world origin via
///     TopologyBuilder — 60×40 box is origin-centred).
///   - bore axis at (−15, 5): X offset from min corner (−30) = 15.00;
///                             Y offset from min corner (−20) = 25.00.
///   - datum origin should be (−30, −20, <z_of_top_face>).
///   - datum kind = "part_corner".
#[test]
fn bore_position_from_part_corner_exact() {
    let mut m = BRepModel::new();
    // 60×40×10 box — TopologyBuilder centres it, so spans x=−30..+30, y=−20..+20, z=−5..+5.
    let plate = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(60.0, 40.0, 10.0)
        .expect("plate"));
    // Bore axis at (−15, 5), drilling all the way through along Z.
    let cutter = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(-15.0, 5.0, -20.0), Vector3::Z, 4.0, 80.0)
        .expect("bore cutter"));
    let part = diff(&mut m, plate, cutter);

    let dims = extract_dimensions(&m, part);

    // Exactly one X-position and one Y-position record.
    let x_recs = position_records(&dims, "x");
    assert_eq!(x_recs.len(), 1, "expected 1 X position record: {dims:?}");
    let y_recs = position_records(&dims, "y");
    assert_eq!(y_recs.len(), 1, "expected 1 Y position record: {dims:?}");

    let x_rec = x_recs[0];
    let y_rec = y_recs[0];

    // X offset: bore axis x (−15) − AABB min_x (−30) = 15.00
    assert!(
        (x_rec.value - 15.0).abs() < 1e-3,
        "X offset should be 15.00, got {}: {x_rec:?}",
        x_rec.value
    );
    // Y offset: bore axis y (5) − AABB min_y (−20) = 25.00
    assert!(
        (y_rec.value - 25.0).abs() < 1e-3,
        "Y offset should be 25.00, got {}: {y_rec:?}",
        y_rec.value
    );

    // Labels use drawing-style format: axis prefix + formatted value + unit suffix.
    // Model default is Millimetre (2 dp) so: "X 15.00mm" / "Y 25.00mm".
    assert_eq!(x_rec.label, "X 15.00mm", "label mismatch: {x_rec:?}");
    assert_eq!(y_rec.label, "Y 25.00mm", "label mismatch: {y_rec:?}");

    // Datum must be Some with kind "part_corner".
    let x_datum = x_rec.datum.as_ref().expect("X position must carry a datum");
    assert_eq!(
        x_datum.kind, "part_corner",
        "datum kind must be part_corner: {x_datum:?}"
    );
    let y_datum = y_rec.datum.as_ref().expect("Y position must carry a datum");
    assert_eq!(
        y_datum.kind, "part_corner",
        "datum kind must be part_corner: {y_datum:?}"
    );

    // Datum origin: (min_x, min_y, z_at_drilled_face).
    // For a Z-axis bore the "drilled face height" = AABB min_z (= −5 for our 60×40×10 box).
    let ox = x_datum.origin[0];
    let oy = x_datum.origin[1];
    assert!(
        (ox - (-30.0)).abs() < 1e-3,
        "datum origin x should be −30 (AABB min_x), got {ox}"
    );
    assert!(
        (oy - (-20.0)).abs() < 1e-3,
        "datum origin y should be −20 (AABB min_y), got {oy}"
    );
    assert_eq!(
        x_datum.name, "part corner",
        "datum name must be 'part corner'"
    );

    // Entities must reference the cylindrical face.
    assert!(
        !x_rec.entities.is_empty(),
        "X position must reference the bore face"
    );
    assert!(
        !y_rec.entities.is_empty(),
        "Y position must reference the bore face"
    );

    // Anchors must be finite.
    assert!(
        x_rec.anchor.iter().all(|c: &f64| c.is_finite()),
        "X position anchor must be finite"
    );
    assert!(
        y_rec.anchor.iter().all(|c: &f64| c.is_finite()),
        "Y position anchor must be finite"
    );
}

/// Position pids must be stable across an unrelated second bore.
#[test]
fn position_pid_survives_unrelated_bore() {
    let mut m = BRepModel::new();
    let plate = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(60.0, 60.0, 10.0)
        .expect("plate"));
    let bore_a = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(-10.0, 0.0, -20.0), Vector3::Z, 4.0, 80.0)
        .expect("bore_a"));
    let part = diff(&mut m, plate, bore_a);

    // Capture bore A's X-position pid.
    let pid_x_a = extract_dimensions(&m, part)
        .into_iter()
        .find(|d| d.kind == "position" && d.label.starts_with("X "))
        .and_then(|d| d.pid.clone())
        .expect("bore A X-position must carry a pid");

    // Drill an unrelated bore B.
    let bore_b = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(10.0, 0.0, -20.0), Vector3::Z, 3.0, 80.0)
        .expect("bore_b"));
    let part2 = diff(&mut m, part, bore_b);

    let dims2 = extract_dimensions(&m, part2);
    // The X-position record from bore A (value ~ 20.00: axis x=−10, min_x=−30) must survive.
    let pid_x_a2 = dims2
        .iter()
        .filter(|d| d.kind == "position" && d.label.starts_with("X "))
        .find(|d| (d.value - 20.0).abs() < 1e-3)
        .and_then(|d| d.pid.clone())
        .expect("bore A X-position pid must survive after bore B added");

    assert_eq!(
        pid_x_a, pid_x_a2,
        "bore A X-position pid must be stable across unrelated edits"
    );
}

/// A diagonal-axis cylinder (axis not aligned with X, Y, or Z beyond the
/// degenerate-threshold) must NOT emit any position records.
#[test]
fn diagonal_axis_cylinder_emits_no_position_records() {
    let mut m = BRepModel::new();
    // Axis = (1,1,1) normalised — all three components equal → no dominant axis
    // → two perpendicular axes cannot be unambiguously chosen → no position records.
    let diagonal_axis = Vector3::new(1.0, 1.0, 1.0);
    let cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::ZERO, diagonal_axis, 5.0, 20.0)
        .expect("diagonal cyl"));
    let dims = extract_dimensions(&m, cyl);
    let pos_records: Vec<_> = dims.iter().filter(|d| d.kind == "position").collect();
    assert!(
        pos_records.is_empty(),
        "diagonal-axis cylinder must NOT emit position records (honest absence): {pos_records:?}"
    );
}

// ── Task 1 (ui-units campaign): unit-aware labels + coincident-row dedupe ──────

/// Labels are formatted via `document_unit`. With the default Millimetre unit
/// a Ø20 bore must label "Ø20.00mm" and its length must label "L 20.00mm".
#[test]
fn labels_include_unit_suffix_in_default_mm() {
    let mut m = BRepModel::new();
    // Model default is Millimetre.
    let plate = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(40.0, 40.0, 20.0)
        .expect("plate"));
    let bore_cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -30.0), Vector3::Z, 10.0, 80.0)
        .expect("bore"));
    let part = diff(&mut m, plate, bore_cyl);
    let dims = extract_dimensions(&m, part);

    // Diameter label must carry the "mm" suffix.
    let dia = dims
        .iter()
        .find(|d| d.kind == "diameter" && (d.value - 20.0).abs() < 1e-3)
        .expect("Ø20 diameter record must exist");
    assert_eq!(
        dia.label, "Ø20.00mm",
        "diameter label must include mm suffix: got {:?}",
        dia.label
    );
    assert_eq!(dia.unit, "mm", "unit field must be 'mm'");

    // Length label must carry the "mm" suffix.
    let len = dims
        .iter()
        .find(|d| d.kind == "length" && (d.value - 20.0).abs() < 1e-3)
        .expect("bore length 20mm record must exist");
    assert!(
        len.label.starts_with("L ") && len.label.ends_with("mm"),
        "length label must be 'L <value>mm': got {:?}",
        len.label
    );

    // Extent labels carry the unit suffix too.
    let ext_x = dims
        .iter()
        .find(|d| d.kind == "extent" && (d.value - 40.0).abs() < 1e-3)
        .expect("X/Y 40mm extent record must exist");
    assert!(
        ext_x.label.ends_with("mm"),
        "extent label must end with 'mm': got {:?}",
        ext_x.label
    );
}

/// With document_unit set to Inch, labels show the converted value + "in".
#[test]
fn labels_reflect_inch_document_unit() {
    use geometry_engine::units::LengthUnit;
    let mut m = BRepModel::new();
    m.set_document_unit(LengthUnit::Inch);
    // 25.4mm bore → Ø1.000in.
    let plate = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(50.8, 50.8, 25.4)
        .expect("plate"));
    let bore_cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -30.0), Vector3::Z, 12.7, 80.0)
        .expect("bore"));
    let part = diff(&mut m, plate, bore_cyl);
    let dims = extract_dimensions(&m, part);

    // Ø25.4mm → "Ø1.000in".
    let dia = dims
        .iter()
        .find(|d| d.kind == "diameter" && (d.value - 25.4).abs() < 1e-3)
        .expect("Ø25.4mm bore must exist");
    assert_eq!(
        dia.label, "Ø1.000in",
        "diameter label must be in inches: got {:?}",
        dia.label
    );
    assert_eq!(dia.unit, "in", "unit field must be 'in'");
}

/// Coincident-row dedupe: the flange bolt-circle fixture has 4× Ø6mm through-
/// bores. Each bore may produce one or more lateral face entries (the boolean
/// may seam-split a cylindrical wall). After dedupe, there must be exactly 4
/// distinct diameter records for Ø6, not more — the dedupe merges any
/// geometrically-coincident rows that arise from seam-split faces while
/// preserving the per-bore distinction.
///
/// This is also the regression guard for the A2 wire finding: "the
/// viewport showed duplicate Ø6 callouts in the bolt-circle view".
#[test]
fn flange_bolt_bores_exactly_four_diameter_records_after_dedupe() {
    let mut m = BRepModel::new();
    let disc = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 40.0, 12.0)
        .expect("disc"));
    let central = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -10.0), Vector3::Z, 12.0, 40.0)
        .expect("central"));
    let mut part = diff(&mut m, disc, central);
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

    // After dedupe: exactly 4 Ø6 diameter records (one per bolt bore, no
    // seam-split duplicates).
    let dia6: Vec<_> = dims
        .iter()
        .filter(|d| d.kind == "diameter" && (d.value - 6.0).abs() < 1e-3)
        .collect();
    assert_eq!(
        dia6.len(),
        4,
        "4× Ø6mm bolt bores: expected exactly 4 diameter records after dedupe, \
         got {}: {dia6:?}",
        dia6.len()
    );
}
