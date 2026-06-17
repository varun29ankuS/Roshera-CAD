//! AGENT-BUILD EVAL — can the kernel build a correct COMPLEX part end-to-end?
//!
//! The real measure of an agent runtime for geometry isn't "does a box union a
//! box" — it's whether a multi-step build of a realistic part stays SOUND at
//! every step. Each scripted build below asserts, after EVERY operation:
//!   * B-Rep sound — `validate_solid_scoped` (exact, mesh-independent);
//!   * watertight at EXPORT density — `manifold_report` at the display/export
//!     default chord (which `tessellate_solid` floors size-relatively), so STL
//!     /FEA handoff is leak-free;
//!   * correct overall world dimensions.
//!
//! This is the harness-beats-model discipline made concrete: a sound verifier
//! plus a sound build pipeline, proven on the exact parts that exposed defects
//! this session (bored plate, box∪boss + coaxial bore, bell nozzle).

use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Matrix4, Point3, Tolerance, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::revolve::{revolve_profile, RevolveOptions};
use geometry_engine::operations::transform::{transform_solid, TransformOptions};
use geometry_engine::primitives::curve::{Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};

/// Assert a build STEP produced a sound, export-watertight solid.
fn assert_sound(m: &BRepModel, sid: SolidId, step: &str) {
    let v = validate_solid_scoped(m, sid, Tolerance::default(), ValidationLevel::Standard);
    assert!(v.is_valid, "[{step}] B-Rep INVALID: {:?}", v.errors);
    // Export density: pass the display/export default chord; tessellate_solid
    // floors it size-relatively, so this exercises the real STL/FEA path.
    let r = manifold_report(m, sid, 0.001, 1e-6).unwrap_or_else(|| panic!("[{step}] empty tess"));
    assert_eq!(
        (r.boundary_edges, r.nonmanifold_edges),
        (0, 0),
        "[{step}] NOT watertight at export density: open={} nm={}",
        r.boundary_edges,
        r.nonmanifold_edges
    );
}

fn world_dims(m: &BRepModel, sid: SolidId) -> [f64; 3] {
    let b = m.solid_world_bbox(sid).expect("bbox");
    let s = b.size();
    [s.x, s.y, s.z]
}

fn box_solid(m: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    match TopologyBuilder::new(m).create_box_3d(w, h, d).unwrap() {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}

fn cyl(m: &mut BRepModel, base: Point3, r: f64, h: f64) -> SolidId {
    match TopologyBuilder::new(m)
        .create_cylinder_3d(base, Vector3::Z, r, h)
        .unwrap()
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}

fn diff(m: &mut BRepModel, a: SolidId, b: SolidId) -> SolidId {
    boolean_operation(m, a, b, BooleanOp::Difference, BooleanOptions::default())
        .expect("difference")
}
fn union(m: &mut BRepModel, a: SolidId, b: SolidId) -> SolidId {
    boolean_operation(m, a, b, BooleanOp::Union, BooleanOptions::default()).expect("union")
}

/// Revolve a closed (r, z) profile a full turn about +Z → a solid of revolution.
fn revolve_ring(m: &mut BRepModel, pts: &[(f64, f64)], segments: u32) -> SolidId {
    let verts: Vec<_> = pts
        .iter()
        .map(|(r, z)| m.vertices.add(*r, 0.0, *z))
        .collect();
    let mut edges = Vec::new();
    for i in 0..pts.len() {
        let j = (i + 1) % pts.len();
        let cid = m.curves.add(Box::new(Line::new(
            Point3::new(pts[i].0, 0.0, pts[i].1),
            Point3::new(pts[j].0, 0.0, pts[j].1),
        )));
        edges.push(m.edges.add(Edge::new(
            0,
            verts[i],
            verts[j],
            cid,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )));
    }
    revolve_profile(
        m,
        edges,
        RevolveOptions {
            axis_origin: Point3::ZERO,
            axis_direction: Vector3::Z,
            angle: std::f64::consts::TAU,
            segments,
            ..Default::default()
        },
    )
    .expect("revolve_ring")
}

fn translate(m: &mut BRepModel, sid: SolidId, dx: f64, dy: f64, dz: f64) {
    transform_solid(
        m,
        sid,
        Matrix4::from_translation(&Vector3::new(dx, dy, dz)),
        TransformOptions::default(),
    )
    .expect("translate");
}

#[test]
fn eval_bored_plate() {
    let mut m = BRepModel::new();
    let plate = box_solid(&mut m, 80.0, 80.0, 16.0);
    assert_sound(&m, plate, "plate");
    let bore = cyl(&mut m, Point3::new(0.0, 0.0, -5.0), 12.0, 26.0);
    let holed = diff(&mut m, plate, bore);
    assert_sound(&m, holed, "plate − bore");
    let d = world_dims(&m, holed);
    assert!(
        (d[0] - 80.0).abs() < 0.5 && (d[1] - 80.0).abs() < 0.5 && (d[2] - 16.0).abs() < 0.5,
        "bored-plate envelope wrong: {d:?}"
    );
}

#[test]
fn eval_bossed_plate_with_coaxial_bore() {
    // box ∪ coaxial cylinder boss (interpenetrating) − coaxial through-bore —
    // the exact build that exposed #41 (outer wall dropped) and the #65
    // doubled-facet seam mesh. Must stay sound + export-watertight at each step.
    let mut m = BRepModel::new();
    let plate = box_solid(&mut m, 120.0, 80.0, 16.0); // centred z −8..8
    assert_sound(&m, plate, "plate");
    let boss = cyl(&mut m, Point3::new(0.0, 0.0, 4.0), 26.0, 45.0); // base buried in plate
    let body = union(&mut m, plate, boss);
    assert_sound(&m, body, "plate ∪ boss");
    let bore = cyl(&mut m, Point3::new(0.0, 0.0, -10.0), 15.0, 70.0); // through everything
    let holed = diff(&mut m, body, bore);
    assert_sound(&m, holed, "boss − coaxial bore");
    // Envelope: outer plate 120×80, boss rises to z=49 → height 49−(−8)=57.
    let d = world_dims(&m, holed);
    assert!(
        (d[0] - 120.0).abs() < 0.5 && (d[1] - 80.0).abs() < 0.5,
        "bossed-plate envelope wrong: {d:?}"
    );
}

#[test]
fn eval_bell_nozzle() {
    // A hollow de Laval nozzle by revolve — chamber → throat → flared bell +
    // injector flange. Must be a sound, export-watertight solid of revolution.
    let pts: Vec<(f64, f64)> = vec![
        (36.0, 0.0),
        (36.0, 45.0),
        (30.0, 58.0),
        (18.0, 72.0),
        (22.0, 90.0),
        (30.0, 112.0),
        (42.0, 138.0),
        (56.0, 162.0),
        (68.0, 178.0),
        (75.0, 178.0),
        (63.0, 162.0),
        (49.0, 138.0),
        (37.0, 112.0),
        (28.0, 90.0),
        (24.0, 72.0),
        (34.0, 58.0),
        (42.0, 45.0),
        (42.0, 10.0),
        (58.0, 10.0),
        (58.0, 0.0),
    ];
    let mut m = BRepModel::new();
    let verts: Vec<_> = pts
        .iter()
        .map(|(r, z)| m.vertices.add(*r, 0.0, *z))
        .collect();
    let mut edges = Vec::new();
    for i in 0..pts.len() {
        let j = (i + 1) % pts.len();
        let cid = m.curves.add(Box::new(Line::new(
            Point3::new(pts[i].0, 0.0, pts[i].1),
            Point3::new(pts[j].0, 0.0, pts[j].1),
        )));
        edges.push(m.edges.add(Edge::new(
            0,
            verts[i],
            verts[j],
            cid,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )));
    }
    let sid = revolve_profile(
        &mut m,
        edges,
        RevolveOptions {
            axis_origin: Point3::ZERO,
            axis_direction: Vector3::Z,
            angle: std::f64::consts::TAU,
            segments: 120,
            ..Default::default()
        },
    )
    .expect("nozzle revolve");
    assert_sound(&m, sid, "bell nozzle");
    // Envelope: exit Ø150 (outer lip r75), height 178.
    let d = world_dims(&m, sid);
    assert!(
        (d[2] - 178.0).abs() < 0.5 && (d[0] - 150.0).abs() < 1.0,
        "nozzle envelope wrong: {d:?}"
    );
}

#[test]
fn eval_gusseted_l_bracket() {
    // The hardest agent-build probe in the eval: THREE chained box unions —
    // horizontal plate ∪ vertical plate ∪ an interpenetrating gusset web —
    // then two mounting bores, asserting sound + watertight at EXPORT density
    // after EVERY step. The parts_invariant_sweep L-bracket only checks at the
    // coarse chord 0.5; this verifies the same family of seams at the real
    // STL/FEA density (the floored default chord), where box∪box seams are most
    // likely to leak.
    let mut m = BRepModel::new();

    let horiz = box_solid(&mut m, 80.0, 50.0, 12.0); // x[-40,40] y[-25,25] z[-6,6]
    assert_sound(&m, horiz, "horiz plate");

    let vert = box_solid(&mut m, 80.0, 12.0, 50.0); // centred → stand it up at the back
    translate(&mut m, vert, 0.0, -19.0, 19.0); // y[-25,-13] z[-6,44]
    let l = union(&mut m, horiz, vert);
    assert_sound(&m, l, "horiz ∪ vert");

    // Gusset web bridging the inside corner — interpenetrates BOTH plates.
    let rib = box_solid(&mut m, 10.0, 24.0, 24.0); // x[-5,5] y[-12,12] z[-12,12]
    translate(&mut m, rib, 0.0, -7.0, 11.0); // y[-19,5] z[-1,23] — buried in both
    let gusseted = union(&mut m, l, rib);
    assert_sound(&m, gusseted, "L ∪ gusset web");

    // Two mounting bores through the horizontal plate.
    let mut acc = gusseted;
    for bx in [-25.0, 25.0] {
        let bore = cyl(&mut m, Point3::new(bx, 10.0, -10.0), 4.0, 32.0);
        acc = diff(&mut m, acc, bore);
        assert_sound(&m, acc, "mounting bore");
    }

    // Envelope: x 80, y 50 (−25..25), z 50 (−6..44).
    let d = world_dims(&m, acc);
    assert!(
        (d[0] - 80.0).abs() < 0.6 && (d[1] - 50.0).abs() < 0.6 && (d[2] - 50.0).abs() < 0.6,
        "gusseted-bracket envelope wrong: {d:?}"
    );
}

#[test]
fn eval_flanged_tube() {
    // Probes the #35-family path at EXPORT density: a hollow flanged tube
    // (revolved annular profile) with a bolt-circle of bores chained-differenced
    // into the FLANGE — i.e. several holes through one annular cap, the exact
    // topology that #35/#84 corefinement fixed (commits 98c20c5 + d4b5113). Here
    // we assert it stays sound + watertight at the floored default (STL/FEA)
    // chord after EACH bolt, not just the coarse density the flanged_body test
    // checks. A revolved annulus (r_min = 15 > 0) never touches the axis, so the
    // REVOLVE axis-touch pole bug is deliberately avoided.
    let mut m = BRepModel::new();
    // Hollow flanged tube, cross-section (r, z): inner bore r15 the full height,
    // a foot flange r20→40 at z0–10, tube wall r15–20 up to z60.
    let body = revolve_ring(
        &mut m,
        &[
            (15.0, 0.0),
            (40.0, 0.0),
            (40.0, 10.0),
            (20.0, 10.0),
            (20.0, 60.0),
            (15.0, 60.0),
        ],
        96,
    );
    assert_sound(&m, body, "flanged tube (revolve)");

    // Bolt circle: four Ø6 holes at radius 30, through the 10 mm flange foot.
    let mut acc = body;
    for (bx, by) in [(30.0, 0.0), (0.0, 30.0), (-30.0, 0.0), (0.0, -30.0)] {
        let bore = cyl(&mut m, Point3::new(bx, by, -5.0), 3.0, 20.0);
        acc = diff(&mut m, acc, bore);
        assert_sound(&m, acc, "flange bolt bore");
    }

    // Envelope: OD 80 (flange r40), height 60.
    let d = world_dims(&m, acc);
    assert!(
        (d[0] - 80.0).abs() < 1.0 && (d[1] - 80.0).abs() < 1.0 && (d[2] - 60.0).abs() < 0.5,
        "flanged-tube envelope wrong: {d:?}"
    );
}
