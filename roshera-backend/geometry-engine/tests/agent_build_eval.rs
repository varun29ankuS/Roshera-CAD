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
use geometry_engine::readable::cylindrical_diameters;
use geometry_engine::render::dimensioned::{coverage_report, render_dimensioned_multiview};
use geometry_engine::tessellation::TessellationParams;

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

/// Verify the part through the AGENT EYE — the visual perception channel — and
/// assert the eye AGREES with the sound B-Rep verdict. A part is only truly
/// verified when both channels concur (cf. EYE-SOUND #27: a verifier that judges
/// one channel and contradicts the other is worse than none). Two checks:
///   1. the eye MEASURES the part (dims/volume/centroid from the rendered mesh)
///      and its dims must match the sound B-Rep envelope — the agent sees the
///      same size the kernel reports;
///   2. the eye's face accounting (coverage across the 4 standard views) is an
///      EXACT partition — seen ∪ unseen = total, every fraction in [0,1] — so the
///      eye never silently double-counts or invents a face.
fn assert_eye_agrees(m: &BRepModel, sid: SolidId, part: &str) {
    let tess = TessellationParams::default();

    let frame = render_dimensioned_multiview(m, sid, &tess)
        .unwrap_or_else(|| panic!("[{part}] eye produced no frame"));
    let brep = world_dims(m, sid);
    let eye = [frame.dims.0, frame.dims.1, frame.dims.2];
    for k in 0..3 {
        let tol = 0.5 + 0.01 * brep[k];
        assert!(
            (eye[k] - brep[k]).abs() <= tol,
            "[{part}] eye dim[{k}]={:.3} disagrees with B-Rep {:.3} (tol {:.3})",
            eye[k],
            brep[k],
            tol
        );
    }
    assert!(
        frame.volume > 0.0 && frame.volume.is_finite(),
        "[{part}] eye volume not positive-finite: {}",
        frame.volume
    );
    assert!(
        frame.centroid.x.is_finite()
            && frame.centroid.y.is_finite()
            && frame.centroid.z.is_finite(),
        "[{part}] eye centroid not finite"
    );

    let cov =
        coverage_report(m, sid, &tess).unwrap_or_else(|| panic!("[{part}] eye coverage empty"));
    assert_eq!(
        cov.seen_faces.len() + cov.unseen_faces.len(),
        cov.total_faces,
        "[{part}] eye coverage is not an exact partition (seen {} + unseen {} != total {})",
        cov.seen_faces.len(),
        cov.unseen_faces.len(),
        cov.total_faces
    );
    assert!(
        (0.0..=1.0).contains(&cov.coverage_fraction),
        "[{part}] eye coverage_fraction out of range: {}",
        cov.coverage_fraction
    );
}

/// SEMANTIC eye check (the moat — perception depth ladder rung 3): the eye must
/// RECOGNIZE the holes the build actually drilled, not just see the silhouette.
/// `cylindrical_diameters` is the agent's "what bore sizes does this part have"
/// answer; assert it reports at least `min_count` cylindrical faces at the
/// expected Ø. This verifies built-feature == recognized-feature.
fn assert_recognizes_bore(
    m: &BRepModel,
    sid: SolidId,
    diameter: f64,
    min_count: usize,
    part: &str,
) {
    let dias = cylindrical_diameters(m, sid);
    let found: usize = dias
        .iter()
        .filter(|(d, _)| (d - diameter).abs() < 1e-3)
        .map(|(_, c)| *c)
        .sum();
    assert!(
        found >= min_count,
        "[{part}] eye recognized {found} Ø{diameter:.1} bore(s), expected ≥{min_count}; saw {dias:?}"
    );
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

/// GATE — BORE-TESS-VOLUME (FIXED 2026-06-17): a bored plate's tessellated MESH
/// must integrate to the correct volume. The bug was `annulus_radial_strip`
/// mis-classifying the square cap as a circular ring and radial-stripping it to
/// the bore, over-covering the annular cap (area 8320 vs 5948) and inflating the
/// mesh volume to 107817 vs 95162 — which is why the live viewport showed NO
/// hole. Fixed by the chord<radius guard in `circular` (surface.rs). This was a
/// FALSE GREEN: watertight + dims + bore-face-exists all passed; only volume
/// caught it. Now a running gate.
#[test]
fn bored_plate_mesh_volume_correct() {
    let mut m = BRepModel::new();
    let plate = box_solid(&mut m, 80.0, 80.0, 16.0); // centred z[-8,8], 102400
    let bore = cyl(&mut m, Point3::new(0.0, 0.0, -10.0), 12.0, 36.0); // through
    let holed = diff(&mut m, plate, bore);
    let frame =
        render_dimensioned_multiview(&m, holed, &TessellationParams::default()).expect("frame");
    let expected = 80.0 * 80.0 * 16.0 - std::f64::consts::PI * 144.0 * 16.0; // 95161.8
    let r = manifold_report(&m, holed, 0.001, 1e-6).expect("mesh");
    eprintln!(
        "bored plate: mesh_vol={:.1} expected~{:.1} (solid plate=102400) | watertight open={} nm={}",
        frame.volume, expected, r.boundary_edges, r.nonmanifold_edges
    );
    assert!(
        (frame.volume - expected).abs() < 0.02 * expected,
        "bored-plate mesh volume {:.1} != expected {:.1} (a SUBTRACTION must remove material; \
         inflated => bore wall mis-oriented / cap filled)",
        frame.volume,
        expected
    );
}

/// COUNTER-EVIDENCE: the KERNEL boolean difference is GEOMETRICALLY CORRECT — the
/// bore is a genuine B-Rep void (exact ray-parity) AND the tessellated mesh
/// contains the full bore wall. So the bored-plate trouble is NOT the kernel
/// boolean; it is (a) the volume integration above and (b) the extrude-path /
/// live tessellation that fills the cap (BORE-TESS-FILL in KNOWN_BUGS). This
/// test PASSES — it pins what is actually sound so future triage doesn't chase
/// the wrong layer.
#[test]
fn kernel_bored_plate_mesh_has_bore() {
    use geometry_engine::queries::point::{classify_point, PointClass};
    use geometry_engine::tessellation::tessellate_solid;
    let mut m = BRepModel::new();
    let plate = box_solid(&mut m, 80.0, 80.0, 16.0); // centred z[-8,8]
    let bore = cyl(&mut m, Point3::new(0.0, 0.0, -10.0), 12.0, 36.0); // z[-10,26] THROUGH
    let holed = diff(&mut m, plate, bore);
    // 1. B-Rep: bore centre is a void, plate body is solid.
    assert_eq!(
        classify_point(&m, holed, Point3::new(0.0, 0.0, 0.0), 1e-6),
        PointClass::Outside,
        "bore centre must be a B-Rep void"
    );
    assert_eq!(
        classify_point(&m, holed, Point3::new(30.0, 0.0, 0.0), 1e-6),
        PointClass::Inside,
        "plate body must be solid"
    );
    // 2. Mesh: the bore wall (r≈12) is actually tessellated, not filled.
    let solid = m.solids.get(holed).expect("s");
    let mesh = tessellate_solid(solid, &m, &TessellationParams::default());
    let on_bore = mesh
        .vertices
        .iter()
        .filter(|v| {
            let r = (v.position.x * v.position.x + v.position.y * v.position.y).sqrt();
            (r - 12.0).abs() < 0.5
        })
        .count();
    assert!(
        on_bore > 50,
        "kernel bored-plate mesh must contain the bore wall (verts at r≈12), got {on_bore}"
    );
}

#[test]
#[ignore = "diagnostic: per-face triangle-winding vs analytic outward normal on the bored plate"]
fn diag_bored_plate_face_winding() {
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    let mut m = BRepModel::new();
    let plate = box_solid(&mut m, 80.0, 80.0, 16.0);
    let bore = cyl(&mut m, Point3::new(0.0, 0.0, -10.0), 12.0, 36.0);
    let holed = diff(&mut m, plate, bore);
    let mesh = tessellate_solid(
        m.solids.get(holed).unwrap(),
        &m,
        &TessellationParams::default(),
    );
    // Per face: average dot(geometric triangle normal, analytic outward normal).
    // A negative average ⇒ that face's triangles are wound INWARD.
    // For the bore-wall cylinder, the CORRECT outward normal (it bounds a VOID,
    // material is outside r) points TOWARD the axis = −radial. So per triangle,
    // n_geo · radial_at_centroid should be ≈ −1 if correct, ≈ +1 if inverted.
    use std::collections::BTreeMap;
    let mut agree: BTreeMap<u32, (f64, usize, String)> = BTreeMap::new();
    for (ti, tri) in mesh.triangles.iter().enumerate() {
        let fid = mesh.face_map[ti];
        let v0 = mesh.vertices[tri[0] as usize].position.to_vec();
        let v1 = mesh.vertices[tri[1] as usize].position.to_vec();
        let v2 = mesh.vertices[tri[2] as usize].position.to_vec();
        let n_geo = (v1 - v0)
            .cross(&(v2 - v0))
            .normalize()
            .unwrap_or(Vector3::Z);
        let c = (v0 + v1 + v2) / 3.0;
        let radial = Vector3::new(c.x, c.y, 0.0)
            .normalize()
            .unwrap_or(Vector3::X);
        let kind = m
            .surfaces
            .get(m.faces.get(fid).unwrap().surface_id)
            .map(|s| s.type_name().to_string())
            .unwrap_or_default();
        let e = agree.entry(fid).or_insert((0.0, 0, kind));
        e.0 += n_geo.dot(&radial); // +1 = points away from axis (into material)
        e.1 += 1;
    }
    // Per-face signed-tet CONTRIBUTION to the mesh volume (Σ v0·(v1×v2)/6) and
    // surface area — the face whose mesh contribution deviates from analytic is
    // the culprit. (Total = 107817 vs correct 95162.)
    let mut contrib: BTreeMap<u32, (f64, f64)> = BTreeMap::new();
    for (ti, tri) in mesh.triangles.iter().enumerate() {
        let fid = mesh.face_map[ti];
        let v0 = mesh.vertices[tri[0] as usize].position.to_vec();
        let v1 = mesh.vertices[tri[1] as usize].position.to_vec();
        let v2 = mesh.vertices[tri[2] as usize].position.to_vec();
        let e = contrib.entry(fid).or_insert((0.0, 0.0));
        e.0 += v0.dot(&v1.cross(&v2)) / 6.0;
        e.1 += (v1 - v0).cross(&(v2 - v0)).magnitude() * 0.5;
    }
    for (fid, (sum, n, kind)) in &agree {
        let (vol, area) = contrib[fid];
        eprintln!(
            "face {fid} [{kind}]: tris={n} radial={:.2} signed_vol_contrib={vol:.1} area={area:.1}",
            sum / (*n as f64),
        );
    }
}

#[test]
#[ignore = "diagnostic: mesh signed-tet volume + orientation at default vs fine tess"]
fn diag_cylinder_mesh_orientation() {
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    let signed_vol = |mesh: &geometry_engine::tessellation::TriangleMesh| -> f64 {
        let mut six = 0.0;
        for t in &mesh.triangles {
            let v0 = mesh.vertices[t[0] as usize].position.to_vec();
            let v1 = mesh.vertices[t[1] as usize].position.to_vec();
            let v2 = mesh.vertices[t[2] as usize].position.to_vec();
            six += v0.dot(&v1.cross(&v2));
        }
        six / 6.0
    };
    for (label, params) in [
        ("default", TessellationParams::default()),
        ("fine", TessellationParams::fine()),
    ] {
        let mut m = BRepModel::new();
        let c = cyl(&mut m, Point3::new(0.0, 0.0, 0.0), 12.0, 26.0);
        let mesh = tessellate_solid(m.solids.get(c).unwrap(), &m, &params);
        let r = manifold_report(&m, c, 0.05, 1e-6).expect("mr");
        eprintln!(
            "CYL {label}: signed_tet_vol={:.1} (truth=11762.1) tris={} open={} nm={}",
            signed_vol(&mesh),
            mesh.triangles.len(),
            r.boundary_edges,
            r.nonmanifold_edges
        );
    }
    // Kernel bored plate: signed-tet (the CORRECT integrator) vs the render's
    // mesh_analytics frame.volume — which one is wrong?
    let mut m = BRepModel::new();
    let plate = box_solid(&mut m, 80.0, 80.0, 16.0);
    let bore = cyl(&mut m, Point3::new(0.0, 0.0, -10.0), 12.0, 36.0);
    let holed = diff(&mut m, plate, bore);
    let mesh = tessellate_solid(
        m.solids.get(holed).unwrap(),
        &m,
        &TessellationParams::default(),
    );
    let frame =
        render_dimensioned_multiview(&m, holed, &TessellationParams::default()).expect("frame");
    eprintln!(
        "BORED PLATE: signed_tet_vol={:.1}  mesh_analytics(frame)={:.1}  (truth=95161.8, solid=102400)",
        signed_vol(&mesh),
        frame.volume
    );
}

/// #41b — the KERNEL coaxial-bore-through-a-boss is SOUND (the corefinement is
/// NOT the bug). Base ∪ boss − a coaxial bore that EXITS the boss top: the result
/// must be a valid, watertight B-Rep whose boss-top cap (a Plane at z≈40, normal
/// +z) is OPENED by the bore (carries an inner loop). The live API once produced
/// `valid=false` here, but it could NOT be reproduced in-kernel — so that is a
/// live-pipeline artifact (store state / the slow GWN-tessellation path), not a
/// corefinement defect. (A curved-CDT panic, #24, fires during tessellation but is
/// caught and the cylinder walls fall back to a grid, so the mesh stays
/// watertight.)
#[test]
fn bearing_housing_coaxial_bore_is_sound() {
    let mut m = BRepModel::new();
    let base = box_solid(&mut m, 120.0, 120.0, 20.0); // centred z[-10,10]
    let boss = cyl(&mut m, Point3::new(0.0, 0.0, 0.0), 35.0, 40.0); // z[0,40]
    let body = union(&mut m, base, boss);
    let bore = cyl(&mut m, Point3::new(0.0, 0.0, -15.0), 20.0, 60.0); // coaxial z[-15,45]
    let holed = diff(&mut m, body, bore);
    let v = validate_solid_scoped(&m, holed, Tolerance::default(), ValidationLevel::Standard);
    assert!(v.is_valid, "bearing housing B-Rep invalid: {:?}", v.errors);
    let r = manifold_report(&m, holed, 0.5, 1e-6).expect("mr");
    assert_eq!(
        (r.boundary_edges, r.nonmanifold_edges),
        (0, 0),
        "bearing housing not watertight: open={} nm={}",
        r.boundary_edges,
        r.nonmanifold_edges
    );
    // The boss-top cap (Plane, normal +z, near z=40) must be OPENED by the bore.
    let solid = m.solids.get(holed).unwrap();
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    let mut boss_top_bored = false;
    for sh in shells {
        for &fid in &m.shells.get(sh).unwrap().faces {
            let face = m.faces.get(fid).unwrap();
            if m.surfaces.get(face.surface_id).unwrap().type_name() != "Plane" {
                continue;
            }
            let n = face.normal_at(0.5, 0.5, &m.surfaces).unwrap_or(Vector3::Z);
            let z = m
                .loops
                .get(face.outer_loop)
                .and_then(|lp| lp.edges.first())
                .and_then(|&e| m.edges.get(e))
                .and_then(|ed| m.vertices.get(ed.start_vertex))
                .map(|vtx| vtx.point().z)
                .unwrap_or(0.0);
            if n.z > 0.9 && z > 35.0 && !face.inner_loops.is_empty() {
                boss_top_bored = true;
            }
        }
    }
    assert!(
        boss_top_bored,
        "boss top cap was NOT opened by the coaxial bore (no inner loop)"
    );
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
    assert_eye_agrees(&m, holed, "bored plate");
    assert_recognizes_bore(&m, holed, 24.0, 1, "bored plate"); // Ø24 = r12 bore
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
    assert_eye_agrees(&m, holed, "bossed plate + coaxial bore");
    assert_recognizes_bore(&m, holed, 30.0, 1, "bossed plate"); // Ø30 = r15 coaxial bore
    assert_recognizes_bore(&m, holed, 52.0, 1, "bossed plate"); // Ø52 = r26 boss wall
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
    assert_eye_agrees(&m, sid, "bell nozzle");
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
    assert_eye_agrees(&m, acc, "gusseted L-bracket");
    assert_recognizes_bore(&m, acc, 8.0, 2, "gusseted L-bracket"); // two Ø8 = r4 mounting bores
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
    assert_eye_agrees(&m, acc, "flanged tube + bolt circle");
    assert_recognizes_bore(&m, acc, 6.0, 4, "flanged tube"); // four Ø6 = r3 bolt holes
    assert_recognizes_bore(&m, acc, 30.0, 1, "flanged tube"); // Ø30 = r15 inner bore
}

/// Build a solid hemispherical dome (radius R) by revolving a quarter-circle
/// profile whose APEX sits on the axis (r = 0). Returns the revolve Result so a
/// probe can observe rejection vs non-watertight tessellation.
fn try_dome(
    m: &mut BRepModel,
    r: f64,
    arc_segs: usize,
) -> geometry_engine::operations::OperationResult<SolidId> {
    // (0,0) base centre → (R,0) base edge → quarter arc up to the apex (0,R).
    // The implicit closing edge (0,R)→(0,0) runs ALONG the axis — the pole case.
    let mut pts = vec![(0.0_f64, 0.0_f64)];
    for k in 0..=arc_segs {
        let theta = (k as f64) / (arc_segs as f64) * std::f64::consts::FRAC_PI_2;
        pts.push((r * theta.cos(), r * theta.sin()));
    }
    let verts: Vec<_> = pts
        .iter()
        .map(|(rr, z)| m.vertices.add(*rr, 0.0, *z))
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
            segments: 64,
            ..Default::default()
        },
    )
}

#[test]
#[ignore = "REVOLVE axis-touch (🔴): apex-on-axis revolve rejected SelfIntersection; un-ignore when the pole case lands"]
fn eval_revolved_dome() {
    // A hemispherical pressure-vessel dome — apex on the axis (r=0). DESIRED end
    // state: a sound, export-watertight solid of revolution. OBSERVED 2026-06-17:
    // `revolve_profile` REJECTS the profile with `SelfIntersection` (the implicit
    // closing edge runs along the axis), so the dome can't be built at all — the
    // REVOLVE axis-touch pole bug (KNOWN_BUGS → "REVOLVE axis-touch"). Pinned as a
    // forward-looking eval — un-ignore when the pole case is handled (admit the
    // single axis segment + single-apex fan tessellation). Workaround today: a
    // small pole vent bore keeps r_min > 0 (see eval_flanged_tube's annulus).
    let mut m = BRepModel::new();
    let dome = try_dome(&mut m, 40.0, 16).expect("dome revolve (apex on axis)");
    assert_sound(&m, dome, "hemispherical dome (pole)");
    let d = world_dims(&m, dome);
    assert!(
        (d[0] - 80.0).abs() < 1.0 && (d[2] - 40.0).abs() < 0.5,
        "dome envelope wrong: {d:?}"
    );
    assert_eye_agrees(&m, dome, "hemispherical dome");
}
