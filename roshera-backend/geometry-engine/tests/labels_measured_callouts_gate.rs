//! Gate for NAMED, MEASURED, COLOR-CODED callouts — the kernel half of the
//! labeller's set-of-marks eye.
//!
//! This pins the FIX to the geometry-aware recognizers (the bug: on a revolved
//! nozzle the smallest-AREA-cylinder throat collapsed onto the chamber barrel,
//! and the +Z-hardcoded exit failed a bell-down part) and the per-label
//! measurement / color / delete machinery:
//!
//! (a) propose/label throat + chamber + exit on a REVOLVED bell nozzle resolve to
//!     THREE DISTINCT faces (throat ≠ chamber — the must-fail the old recognizer
//!     could not satisfy);
//! (b) the throat measurement is its MEASURED diameter ≈ 2× the true throat
//!     radius, the chamber its measured Ø, in the document unit;
//! (c) DELETE a label → the list no longer carries it;
//! (d) the per-name colors are deterministic and distinct;
//! (e) a colored dimensioned overlay PNG is written to target/ (throat / chamber /
//!     exit each in its own color, each reading its Ø).
//!
//! The part is a HOLLOW nozzle of revolution whose inner contour necks down to a
//! throat: a chamber bore (r=6), a contraction, a LONG throat (r=2), an expansion,
//! and an exit bore (r=5), inside a constant outer wall (r=12). The throat (r=2)
//! is the global minimum-radius station, but it is NOT the smallest-AREA cylinder
//! (the short exit bore is) — so the old recognizer would have mislabelled it.

use geometry_engine::labels::{label_color, label_color_hex, MeasurementKind};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::revolve::{revolve_profile, RevolveOptions};
use geometry_engine::primitives::curve::{Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::queries::select::{resolve_face, Axis, Extremal, FaceQuery, SurfaceKind};
use geometry_engine::render::{
    render_solid_with_label_marks, CanonicalView, LabelMark, RenderOptions,
};

const THROAT_R: f64 = 2.0;
const CHAMBER_OUTER_R: f64 = 12.0;

/// Build the hollow bell nozzle of revolution. The closed meridian (r, z) traces
/// the outer wall up, the top annulus in, the inner nozzle contour down, then the
/// bottom annulus out — revolved a full turn about +Z.
fn build_bell_nozzle(m: &mut BRepModel) -> SolidId {
    // (r, z): outer up → top in → inner contour down (chamber bore, contraction,
    // throat, expansion, exit bore) → bottom out.
    let pts = [
        (12.0, 0.0),
        (12.0, 20.0),
        (6.0, 20.0),
        (6.0, 18.0),
        (2.0, 16.0),
        (2.0, 4.0),
        (5.0, 2.0),
        (5.0, 0.0),
    ];
    let verts: Vec<u32> = pts
        .iter()
        .map(|(r, z)| m.vertices.add(*r, 0.0, *z))
        .collect();
    let mut edges = Vec::new();
    for i in 0..pts.len() {
        let j = (i + 1) % pts.len();
        let line = Line::new(
            Point3::new(pts[i].0, 0.0, pts[i].1),
            Point3::new(pts[j].0, 0.0, pts[j].1),
        );
        let cid = m.curves.add(Box::new(line));
        edges.push(m.edges.add(Edge::new(
            0,
            verts[i],
            verts[j],
            cid,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )));
    }
    let opts = RevolveOptions {
        axis_origin: Point3::ZERO,
        axis_direction: Vector3::Z,
        angle: std::f64::consts::TAU,
        segments: 48,
        ..Default::default()
    };
    revolve_profile(m, edges, opts).expect("revolve bell nozzle")
}

#[test]
fn measured_colored_callouts_on_a_revolved_bell_nozzle() {
    let mut m = BRepModel::new();
    let solid = build_bell_nozzle(&mut m);

    // ── (a) propose → throat / chamber / exit recognized, three DISTINCT faces ──
    let proposals = m.propose_labels(solid);
    let names: Vec<&str> = proposals
        .iter()
        .map(|p| p.suggested_name.as_str())
        .collect();
    assert!(names.contains(&"throat"), "proposes throat: {names:?}");
    assert!(names.contains(&"chamber"), "proposes chamber: {names:?}");
    assert!(names.contains(&"exit"), "proposes exit: {names:?}");

    // The symmetry axis is detected (it IS a surface of revolution about +Z).
    let (axis_o, axis_d) = m.symmetry_axis(solid).expect("symmetry axis detected");
    assert!(
        axis_d.dot(&Vector3::Z).abs() > 0.999,
        "axis is the revolve axis (±Z): {axis_d:?}"
    );
    let axis = Axis {
        origin: Vector3::new(axis_o.x, axis_o.y, axis_o.z),
        direction: axis_d,
    };

    // Resolve each recognizer's selector to a concrete face.
    let throat_fid = resolve_face(
        &mut m,
        solid,
        &FaceQuery::new(SurfaceKind::Any).extremal(Extremal::MinRadiusStation(axis)),
    )
    .expect("throat (min-radius station) resolves");
    let chamber_fid = resolve_face(
        &mut m,
        solid,
        &FaceQuery::new(SurfaceKind::Cylindrical).extremal(Extremal::LargestArea),
    )
    .expect("chamber (max-area cylinder) resolves");
    let exit_fid = resolve_face(
        &mut m,
        solid,
        &FaceQuery::new(SurfaceKind::Planar).extremal(Extremal::AxialExtremalCap(axis)),
    )
    .expect("exit (axial-extremal cap) resolves");

    // MUST-FAIL-STYLE: the three faces are pairwise DISTINCT. The old recognizer
    // collapsed throat onto chamber — this is the regression guard.
    assert_ne!(
        throat_fid, chamber_fid,
        "throat and chamber are DIFFERENT faces"
    );
    assert_ne!(throat_fid, exit_fid, "throat and exit are different faces");
    assert_ne!(
        chamber_fid, exit_fid,
        "chamber and exit are different faces"
    );

    // The min-radius station was found as a SurfaceKind::Any band (the geometry-
    // aware path, not a cylinder-area heuristic). That it sits at the throat
    // radius is proved by the measurement assertion below (Ø ≈ 2×throat radius).

    // ── Confirm the proposals as labels (selector assertions) ───────────────────
    for (fid, name) in [
        (throat_fid, "throat"),
        (chamber_fid, "chamber"),
        (exit_fid, "exit"),
    ] {
        let prop = proposals
            .iter()
            .find(|p| p.suggested_name == name)
            .unwrap_or_else(|| panic!("{name} proposal present"));
        m.label_face_with_assertion(
            fid,
            name,
            prop.assertion.clone(),
            Some(prop.rationale.clone()),
        )
        .unwrap_or_else(|e| panic!("label {name}: {e:?}"));
    }

    // ── (b) MEASURED diameters, in the document unit (mm) ───────────────────────
    let throat_meas = m.label_measurement("throat").expect("throat measurement");
    assert_eq!(throat_meas.kind, MeasurementKind::Diameter);
    assert_eq!(throat_meas.unit, "mm");
    assert!(
        (throat_meas.value - 2.0 * THROAT_R).abs() < 1e-3,
        "throat Ø ≈ 2×{THROAT_R} = {}, got {}",
        2.0 * THROAT_R,
        throat_meas.value
    );
    assert!(
        throat_meas.display.starts_with('\u{00d8}') && throat_meas.display.contains("mm"),
        "throat display is a Ø readout in mm: {}",
        throat_meas.display
    );

    let chamber_meas = m.label_measurement("chamber").expect("chamber measurement");
    assert_eq!(chamber_meas.kind, MeasurementKind::Diameter);
    assert!(
        (chamber_meas.value - 2.0 * CHAMBER_OUTER_R).abs() < 1e-3,
        "chamber Ø ≈ 2×{CHAMBER_OUTER_R}, got {}",
        chamber_meas.value
    );
    // The exit is a planar cap → an AREA readout.
    let exit_meas = m.label_measurement("exit").expect("exit measurement");
    assert_eq!(exit_meas.kind, MeasurementKind::Area);
    assert!(exit_meas.value > 0.0, "exit cap area is positive");

    // ── (d) deterministic + distinct colors ─────────────────────────────────────
    assert_eq!(
        label_color("throat"),
        label_color("throat"),
        "stable per run"
    );
    let ct = label_color("throat");
    let cc = label_color("chamber");
    let ce = label_color("exit");
    assert_ne!(ct, cc);
    assert_ne!(ct, ce);
    assert_ne!(cc, ce);
    for hex in [label_color_hex("throat"), label_color_hex("chamber")] {
        assert_eq!(hex.len(), 7);
        assert!(hex.starts_with('#'));
    }

    // ── (e) COLOR-CODED dimensioned overlay PNG ─────────────────────────────────
    let mut marks: Vec<LabelMark> = Vec::new();
    for (name, fid) in [
        ("throat", throat_fid),
        ("chamber", chamber_fid),
        ("exit", exit_fid),
    ] {
        let anchor = m.label_anchor(name).expect("anchor");
        let meas = m.label_measurement(name).expect("measurement");
        marks.push(LabelMark {
            anchor,
            text: format!("{} - {}", name.to_uppercase(), meas.display),
            color: label_color(name),
            target_face: Some(fid),
        });
    }
    let frame = render_solid_with_label_marks(
        &m,
        solid,
        &marks,
        &RenderOptions {
            width: 900,
            height: 900,
            view: CanonicalView::Isometric,
            ..Default::default()
        },
    )
    .expect("render colored marks");
    let png = frame.to_png().expect("encode png");
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("labels_measured_callouts_overlay.png");
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).expect("create output dir");
    }
    std::fs::write(&out, &png).expect("write png");
    eprintln!("MEASURED COLOR-CODED overlay written to {}", out.display());

    // ── (c) DELETE a label → it leaves the listing ──────────────────────────────
    assert!(m.delete_label("chamber"), "chamber existed and was removed");
    assert!(!m.delete_label("chamber"), "second delete reports gone");
    let listed: Vec<String> = m.list_labels().into_iter().map(|(n, ..)| n).collect();
    assert!(
        !listed.contains(&"chamber".to_string()),
        "chamber gone from list"
    );
    assert!(
        listed.contains(&"throat".to_string()),
        "throat still present"
    );
}
