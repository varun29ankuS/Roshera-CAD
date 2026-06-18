//! NURBS CAR BODY — capability showcase for `operations::nurbs_loft`.
//!
//! A car body is, classically, a set of cross-section "stations" lofted along
//! the vehicle length — exactly the input `nurbs_loft` takes. Here the loft axis
//! is +X (nose→tail); each station is a closed YZ-plane ring (a rounded-rect
//! "superellipse" from floor to roof) whose half-width / height / floor vary to
//! sculpt the silhouette: low nose, hood, a raised cabin (windshield→roof→rear
//! glass), trunk deck, tapered tail. Degree-3 in V ⇒ the body shell is a single
//! G2 (curvature-continuous) NURBS surface. The test asserts the result is a
//! valid, watertight solid and reports its envelope + volume.

use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::Vector3;
use geometry_engine::math::{Point3, Tolerance};
use geometry_engine::operations::nurbs_loft::{nurbs_loft, NurbsLoftOptions};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};
use geometry_engine::render::dimensioned::render_dimensioned_multiview;
use geometry_engine::render::{render_solid_dir, RenderMode, RenderOptions};
use geometry_engine::tessellation::TessellationParams;

/// One cross-section station: a closed YZ-plane ring at length `x`, half-width
/// `w`, total height `h`, floor at `zf`. The ring is a rounded rectangle
/// (superellipse, exponent < 1 → fuller than an ellipse) sampled with `n` points
/// — flatter floor/roof/sides than an oval, so it reads as a car body, not a
/// torpedo. Points are ordered CCW about +X for a consistent loft.
fn station(x: f64, w: f64, h: f64, zf: f64, n: usize) -> Vec<Point3> {
    (0..n)
        .map(|i| {
            let t = i as f64 * std::f64::consts::TAU / n as f64;
            let (c, s) = (t.cos(), t.sin());
            let cy = c.signum() * c.abs().powf(0.6);
            let cz = s.signum() * s.abs().powf(0.6);
            let y = w * cy;
            let z = zf + 0.5 * h * (1.0 + cz);
            Point3::new(x, y, z)
        })
        .collect()
}

/// The 12 stations of the silhouette (mm), nose (x=0) → tail (x=440).
/// (x, half-width, height, floor-z)
fn car_stations(n: usize) -> Vec<Vec<Point3>> {
    let spec: &[(f64, f64, f64, f64)] = &[
        (0.0, 30.0, 40.0, 24.0),    // nose tip (small planar cap)
        (35.0, 70.0, 52.0, 16.0),   // front bumper
        (85.0, 86.0, 58.0, 16.0),   // hood front
        (140.0, 92.0, 66.0, 16.0),  // hood / cowl
        (185.0, 92.0, 96.0, 16.0),  // windshield base (greenhouse rising)
        (225.0, 90.0, 128.0, 16.0), // cabin front (roof)
        (270.0, 90.0, 138.0, 16.0), // cabin peak
        (315.0, 90.0, 126.0, 16.0), // rear glass
        (355.0, 88.0, 92.0, 16.0),  // trunk deck
        (400.0, 82.0, 66.0, 16.0),  // rear bumper
        (430.0, 66.0, 54.0, 18.0),  // tail
        (440.0, 34.0, 44.0, 22.0),  // tail tip (small planar cap)
    ];
    spec.iter()
        .map(|&(x, w, h, zf)| station(x, w, h, zf, n))
        .collect()
}

#[test]
fn nurbs_car_body_is_a_valid_watertight_solid() {
    let mut m = BRepModel::new();
    let sections = car_stations(28);
    let s = nurbs_loft(&mut m, sections, NurbsLoftOptions::default()).expect("car body nurbs_loft");

    let v = validate_solid_scoped(&m, s, Tolerance::default(), ValidationLevel::Standard);
    assert!(v.is_valid, "car body B-Rep invalid: {:?}", v.errors);

    for defl in [1.0_f64, 0.25] {
        let r = manifold_report(&m, s, defl, 1e-6)
            .unwrap_or_else(|| panic!("car body: empty tess @defl {defl}"));
        assert_eq!(
            (r.boundary_edges, r.nonmanifold_edges),
            (0, 0),
            "car body not watertight @defl {defl}: open={} nm={}",
            r.boundary_edges,
            r.nonmanifold_edges
        );
    }

    let b = m.solid_world_bbox(s).expect("bbox");
    let sz = b.size();
    let frame = render_dimensioned_multiview(&m, s, &TessellationParams::default()).expect("frame");
    eprintln!(
        "CAR BODY: envelope L×W×H = {:.0} × {:.0} × {:.0} mm | volume ≈ {:.0} mm³ ({:.1} L)",
        sz.x,
        sz.y,
        sz.z,
        frame.volume,
        frame.volume / 1.0e6
    );
    // Sanity: ~440 long, ~180 wide (2×~90), ~140 tall.
    assert!((sz.x - 440.0).abs() < 5.0, "length wrong: {}", sz.x);
    assert!(sz.y > 150.0 && sz.y < 200.0, "width wrong: {}", sz.y);
    assert!(sz.z > 120.0 && sz.z < 170.0, "height wrong: {}", sz.z);
    // A solid body must enclose a sensible fraction of its bbox (not a sliver).
    let bbox_vol = sz.x * sz.y * sz.z;
    assert!(
        frame.volume > 0.25 * bbox_vol && frame.volume < bbox_vol,
        "car body volume {:.0} implausible vs bbox {:.0}",
        frame.volume,
        bbox_vol
    );
}

/// Emit the car-body sections as the `/api/geometry/nurbs_loft` request payload
/// (run explicitly) so it can be POSTed to the live server to show in the
/// frontend viewport. Writes `_car_body.json` to the crate dir.
#[test]
#[ignore = "writes the live POST payload; run explicitly"]
fn emit_car_body_json() {
    let sections = car_stations(40);
    let secs: Vec<Vec<[f64; 3]>> = sections
        .iter()
        .map(|s| s.iter().map(|p| [p.x, p.y, p.z]).collect())
        .collect();
    let payload = serde_json::json!({
        "sections": secs,
        "degree_u": 3,
        "degree_v": 3,
        "name": "NURBS Car Body"
    });
    let dir = env!("CARGO_MANIFEST_DIR");
    std::fs::write(
        format!("{dir}/_car_body.json"),
        serde_json::to_string(&payload).expect("json"),
    )
    .expect("write");
    eprintln!("wrote _car_body.json to {dir}");
}

/// Render shaded views of the NURBS car body to PNG (run explicitly:
/// `cargo test --test nurbs_car_body render_car_body_views -- --ignored`).
/// Writes to the crate dir so an agent can read them back.
#[test]
#[ignore = "writes PNGs; run explicitly to view the car body"]
fn render_car_body_views() {
    let mut m = BRepModel::new();
    let s = nurbs_loft(&mut m, car_stations(40), NurbsLoftOptions::default()).expect("car");
    let opts = RenderOptions {
        width: 900,
        height: 480,
        mode: RenderMode::Shaded,
        tessellation: TessellationParams::default(),
        ..Default::default()
    };
    let dir = env!("CARGO_MANIFEST_DIR");
    // Side (profile, looking +Y): the recognizable silhouette.
    let side =
        render_solid_dir(&m, s, Vector3::new(0.0, 1.0, 0.0), Vector3::Z, &opts).expect("side");
    std::fs::write(format!("{dir}/_car_side.png"), side.to_png().expect("png")).expect("w");
    // Front-3/4 from above: camera front-left-top → looking back-right-down.
    let iso = render_solid_dir(
        &m,
        s,
        Vector3::new(1.0, 0.7, -0.5).normalize().unwrap(),
        Vector3::Z,
        &opts,
    )
    .expect("iso");
    std::fs::write(format!("{dir}/_car_iso.png"), iso.to_png().expect("png")).expect("w");
    // Top (plan, looking -Z).
    let top =
        render_solid_dir(&m, s, Vector3::new(0.0, 0.0, -1.0), Vector3::Y, &opts).expect("top");
    std::fs::write(format!("{dir}/_car_top.png"), top.to_png().expect("png")).expect("w");
    eprintln!("wrote _car_side.png _car_iso.png _car_top.png to {dir}");
}
