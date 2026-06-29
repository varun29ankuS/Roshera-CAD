//! Revolve analytic-bands gate (#19).
//!
//! A FULL revolution of a closed cylinder/plane line profile must emit ONE
//! analytic face per band (Cylinder / annular Plane) — NOT one
//! `SurfaceOfRevolution` patch per (segment × band). A 48-segment tube must be
//! 4 analytic faces, not 192. The analytic path self-verifies watertightness
//! and rolls back to the per-segment path on any failure, so this also pins the
//! zero-regression contract: cone/stepped profiles still produce a watertight
//! solid (via fallback), just not the minimal analytic face set yet (v2).
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::revolve::{
    get_revolve_meridian, revolve_meridian, revolve_profile, revolve_smooth_nozzle,
    revolve_spline_meridian, RevolveOptions,
};
use geometry_engine::primitives::curve::{Arc, Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::{Cylinder, SurfaceType};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};

fn revolve(m: &mut BRepModel, pts: &[(f64, f64)], segments: u32) -> SolidId {
    let verts: Vec<_> = pts
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
        segments,
        ..Default::default()
    };
    revolve_profile(m, edges, opts).unwrap_or_else(|e| panic!("revolve: {e:?}"))
}

fn face_kinds(m: &BRepModel, sid: SolidId) -> Vec<SurfaceType> {
    let solid = m.solids.get(sid).unwrap_or_else(|| panic!("solid"));
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    let mut out = Vec::new();
    for shid in shells {
        if let Some(shell) = m.shells.get(shid) {
            for &fid in &shell.faces {
                if let Some(f) = m.faces.get(fid) {
                    if let Some(s) = m.surfaces.get(f.surface_id) {
                        out.push(s.surface_type());
                    }
                }
            }
        }
    }
    out
}

fn count(k: &[SurfaceType], want: SurfaceType) -> usize {
    k.iter().filter(|&&x| x == want).count()
}

fn cyl_radii(m: &BRepModel, sid: SolidId) -> Vec<f64> {
    let solid = m.solids.get(sid).unwrap_or_else(|| panic!("solid"));
    let shell = m
        .shells
        .get(solid.outer_shell)
        .unwrap_or_else(|| panic!("shell"));
    let mut out = Vec::new();
    for &fid in &shell.faces {
        if let Some(f) = m.faces.get(fid) {
            if let Some(s) = m.surfaces.get(f.surface_id) {
                if let Some(c) = s.as_any().downcast_ref::<Cylinder>() {
                    out.push(c.radius);
                }
            }
        }
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out
}

#[test]
fn tube_is_four_analytic_faces_19() {
    // Tube r6..10, z0..20: outer cyl r10 + inner cyl r6 + 2 annular plane caps.
    let mut m = BRepModel::new();
    let s = revolve(
        &mut m,
        &[(10.0, 0.0), (10.0, 20.0), (6.0, 20.0), (6.0, 0.0)],
        48,
    );
    let k = face_kinds(&m, s);
    assert_eq!(k.len(), 4, "tube must be 4 faces, not 192 (kinds={k:?})");
    assert_eq!(count(&k, SurfaceType::Cylinder), 2, "2 cylinder walls");
    assert_eq!(count(&k, SurfaceType::Plane), 2, "2 annular plane caps");
    assert_eq!(
        count(&k, SurfaceType::SurfaceOfRevolution),
        0,
        "no faceted SurfaceOfRevolution patches"
    );
    let radii = cyl_radii(&m, s);
    assert!(
        radii.len() == 2 && (radii[0] - 6.0).abs() < 1e-6 && (radii[1] - 10.0).abs() < 1e-6,
        "cylinder radii recoverable as 6 and 10: {radii:?}"
    );
    let v = validate_solid_scoped(&m, s, Tolerance::default(), ValidationLevel::Standard);
    assert!(v.is_valid, "tube B-Rep invalid: {:?}", v.errors);
}

#[test]
fn frustum_tube_is_four_analytic_faces_19() {
    // A hollow conical frustum (sloped outer + inner walls): outer cone
    // (10,0)→(6,20), top annulus, inner cone (4,20)→(8,0), bottom annulus.
    // Must be 2 Cone + 2 Plane analytic faces, not segments × SurfaceOfRevolution.
    let mut m = BRepModel::new();
    let s = revolve(
        &mut m,
        &[(10.0, 0.0), (6.0, 20.0), (4.0, 20.0), (8.0, 0.0)],
        48,
    );
    let k = face_kinds(&m, s);
    assert_eq!(k.len(), 4, "frustum tube must be 4 faces (kinds={k:?})");
    assert_eq!(count(&k, SurfaceType::Cone), 2, "2 cone walls");
    assert_eq!(count(&k, SurfaceType::Plane), 2, "2 annular plane caps");
    assert_eq!(
        count(&k, SurfaceType::SurfaceOfRevolution),
        0,
        "no faceted SurfaceOfRevolution patches"
    );
    let v = validate_solid_scoped(&m, s, Tolerance::default(), ValidationLevel::Standard);
    assert!(v.is_valid, "frustum tube B-Rep invalid: {:?}", v.errors);
}

#[test]
fn open_washer_is_four_analytic_faces_19() {
    // A flat washer (short tube): outer r20, inner r8, thin (z0..2). Still 4.
    let mut m = BRepModel::new();
    let s = revolve(
        &mut m,
        &[(8.0, 0.0), (20.0, 0.0), (20.0, 2.0), (8.0, 2.0)],
        64,
    );
    let k = face_kinds(&m, s);
    assert_eq!(k.len(), 4, "washer must be 4 faces (kinds={k:?})");
    assert_eq!(count(&k, SurfaceType::Cylinder), 2);
    assert_eq!(count(&k, SurfaceType::Plane), 2);
}

/// The annular-cap hole must be SUBTRACTED, not filled. The general hole-CDT
/// mishandles a concentric-circle washer (chevron mesh + spanning triangles that
/// fill the bore); the annulus radial-strip fast path fixes it. The bore of a
/// hollow revolve is empty, so NO tessellation triangle may have its centroid
/// inside the bore radius.
#[test]
fn revolved_washer_bore_is_not_filled() {
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

    let mut m = BRepModel::new();
    // Flat washer: bore radius 8, outer radius 20, thin (z 0..2).
    let s = revolve(
        &mut m,
        &[(8.0, 0.0), (20.0, 0.0), (20.0, 2.0), (8.0, 2.0)],
        64,
    );
    let solid = m.solids.get(s).expect("solid");
    let mesh = tessellate_solid(solid, &m, &TessellationParams::default());

    let mut filling = 0usize;
    for tri in &mesh.triangles {
        let a = mesh.vertices[tri[0] as usize].position;
        let b = mesh.vertices[tri[1] as usize].position;
        let c = mesh.vertices[tri[2] as usize].position;
        let cx = (a.x + b.x + c.x) / 3.0;
        let cy = (a.y + b.y + c.y) / 3.0;
        if (cx * cx + cy * cy).sqrt() < 7.0 {
            filling += 1;
        }
    }
    assert_eq!(
        filling, 0,
        "{filling} tessellation triangle(s) fill the bore (centroid inside the Ø16 bore)"
    );
}

/// #21 — a CURVED meridian edge revolves to ONE `SurfaceOfRevolution` face for
/// the whole 360°, not `segments` patches. An annular barrel: the outer wall is
/// an ARC (r bulges 8→13→8 over z 0→10), inner wall straight (r=5), annular caps
/// — all radii > 0 so the analytic-band path is eligible. Proves the curved arm
/// engaged: a grid fallback would emit 48 SurfaceOfRevolution patches just for
/// the outer wall.
#[test]
fn curved_meridian_revolves_to_one_surface_of_revolution() {
    let mut m = BRepModel::new();
    let v_bo = m.vertices.add(8.0, 0.0, 0.0); // bottom outer
    let v_to = m.vertices.add(8.0, 0.0, 10.0); // top outer
    let v_ti = m.vertices.add(5.0, 0.0, 10.0); // top inner
    let v_bi = m.vertices.add(5.0, 0.0, 0.0); // bottom inner

    // Outer wall arc: center (8,0,5), r=5, normal +Y; start π = (8,0,0),
    // sweep -π ends at (8,0,10) bulging through (13,0,5).
    let arc = Arc::new(
        Point3::new(8.0, 0.0, 5.0),
        Vector3::Y,
        5.0,
        std::f64::consts::PI,
        -std::f64::consts::PI,
    )
    .expect("arc");
    let arc_cid = m.curves.add(Box::new(arc));
    let e_outer = m.edges.add(Edge::new(
        0,
        v_bo,
        v_to,
        arc_cid,
        EdgeOrientation::Forward,
        ParameterRange::new(0.0, 1.0),
    ));
    let mut line = |m: &mut BRepModel, a: (f64, f64), b: (f64, f64), s, e| {
        let l = Line::new(Point3::new(a.0, 0.0, a.1), Point3::new(b.0, 0.0, b.1));
        let cid = m.curves.add(Box::new(l));
        m.edges.add(Edge::new(
            0,
            s,
            e,
            cid,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ))
    };
    let e_top = line(&mut m, (8.0, 10.0), (5.0, 10.0), v_to, v_ti);
    let e_inner = line(&mut m, (5.0, 10.0), (5.0, 0.0), v_ti, v_bi);
    let e_bot = line(&mut m, (5.0, 0.0), (8.0, 0.0), v_bi, v_bo);

    let opts = RevolveOptions {
        axis_origin: Point3::ZERO,
        axis_direction: Vector3::Z,
        angle: std::f64::consts::TAU,
        segments: 48,
        ..Default::default()
    };
    let sid = revolve_profile(&mut m, vec![e_outer, e_top, e_inner, e_bot], opts)
        .unwrap_or_else(|e| panic!("revolve: {e:?}"));

    let kinds = face_kinds(&m, sid);
    assert_eq!(
        count(&kinds, SurfaceType::SurfaceOfRevolution),
        1,
        "curved outer wall must be ONE SurfaceOfRevolution face (analytic path), got {kinds:?}"
    );
    assert!(
        kinds.len() <= 6,
        "barrel must be a handful of analytic faces, not 48× patches: {} faces {kinds:?}",
        kinds.len()
    );
    assert!(
        validate_solid_scoped(&m, sid, Tolerance::default(), ValidationLevel::Standard).is_valid,
        "curved revolve must be a valid solid"
    );
}

/// #25.1 — a PARAMETRIC revolve (`revolve_meridian`) builds a valid solid AND
/// RETAINS its generating meridian as construction geometry, so the part
/// remembers how it was made (the foundation of the edit→regenerate workflow:
/// the profile is recoverable for editing).
#[test]
fn revolve_meridian_retains_its_generating_profile() {
    let mut m = BRepModel::new();
    // Solid cylinder meridian (CCW in r-z): axis-bottom → outer-bottom →
    // outer-top → axis-top.
    let profile = [(0.0, 0.0), (5.0, 0.0), (5.0, 10.0), (0.0, 10.0)];
    let opts = RevolveOptions {
        axis_origin: Point3::ZERO,
        axis_direction: Vector3::Z,
        angle: std::f64::consts::TAU,
        segments: 48,
        ..Default::default()
    };
    let sid = revolve_meridian(&mut m, &profile, opts)
        .unwrap_or_else(|e| panic!("revolve_meridian: {e:?}"));

    // The part REMEMBERS its meridian (all 4 points, recoverable for editing).
    let cg = m
        .solid_construction(sid)
        .expect("revolve_meridian must retain the generating meridian");
    assert_eq!(
        cg.profile_points.len(),
        4,
        "all 4 meridian points retained as construction geometry"
    );

    // And it is a valid solid of revolution.
    assert!(
        validate_solid_scoped(&m, sid, Tolerance::default(), ValidationLevel::Standard).is_valid,
        "revolve_meridian must build a valid solid"
    );
}

/// #25.2 — the kernel edit→regenerate loop: RECOVER a part's meridian, EDIT it,
/// and re-revolve to a new part. Widening the profile must yield a larger solid,
/// and the regenerated part must retain the edited meridian.
#[test]
fn revolve_meridian_edit_regenerate_loop() {
    let opts = || RevolveOptions {
        axis_origin: Point3::ZERO,
        axis_direction: Vector3::Z,
        angle: std::f64::consts::TAU,
        segments: 48,
        ..Default::default()
    };
    let mut m = BRepModel::new();
    let profile = [(0.0, 0.0), (5.0, 0.0), (5.0, 10.0), (0.0, 10.0)];
    let sid = revolve_meridian(&mut m, &profile, opts()).expect("revolve");

    // RECOVER the editable meridian (matches the original).
    let recovered = get_revolve_meridian(&m, sid).expect("recover meridian");
    assert_eq!(recovered.len(), 4);
    for (got, want) in recovered.iter().zip(profile.iter()) {
        assert!(
            (got.0 - want.0).abs() < 1e-9 && (got.1 - want.1).abs() < 1e-9,
            "recovered {got:?} != original {want:?}"
        );
    }
    let v_orig = m.mass_properties_for(sid).expect("mp").volume;

    // EDIT: widen the outer radius 5 → 8, then REGENERATE.
    let edited: Vec<(f64, f64)> = recovered
        .iter()
        .map(|&(r, z)| (if r > 0.0 { 8.0 } else { r }, z))
        .collect();
    let mut m2 = BRepModel::new();
    let sid2 = revolve_meridian(&mut m2, &edited, opts()).expect("regenerate");
    let v_new = m2.mass_properties_for(sid2).expect("mp2").volume;

    // π·8²·10 ≈ 2.56× π·5²·10 — the edit regenerated a substantially larger part.
    assert!(
        v_new > v_orig * 1.5,
        "widened profile must regenerate a larger part: {v_new} vs {v_orig}"
    );
    // The regenerated part retains the EDITED meridian (r = 8).
    let re2 = get_revolve_meridian(&m2, sid2).expect("recover2");
    assert!(
        (re2[1].0 - 8.0).abs() < 1e-9,
        "regenerated part must retain the edit: {re2:?}"
    );
}

/// #9 — a SMOOTH (NURBS-spline) wall revolves to ONE `SurfaceOfRevolution`, not a
/// faceted polyline of `P × segments` tiny faces (the original nozzle complaint).
/// A curved bell wall → one revolution surface + plane caps, valid, and the wall
/// profile stays recoverable for editing.
#[test]
fn revolve_spline_meridian_is_one_smooth_wall() {
    let mut m = BRepModel::new();
    // A bell/vase outer wall: a smooth curve through r = 5→3→4→7 over z = 0→12,
    // hollowed by a Ø4 bore (radius 2) so it is a tube (no axis-touching).
    let wall = [(5.0, 0.0), (3.0, 4.0), (4.0, 8.0), (7.0, 12.0)];
    let opts = RevolveOptions {
        axis_origin: Point3::ZERO,
        axis_direction: Vector3::Z,
        angle: std::f64::consts::TAU,
        segments: 48,
        ..Default::default()
    };
    let sid = revolve_spline_meridian(&mut m, &wall, 2.0, opts)
        .unwrap_or_else(|e| panic!("revolve_spline_meridian: {e:?}"));

    let k = face_kinds(&m, sid);
    // The smooth outer wall is ONE revolution surface (NOT planar facets) …
    assert!(
        count(&k, SurfaceType::SurfaceOfRevolution) >= 1,
        "the smooth wall must be a SurfaceOfRevolution: {k:?}"
    );
    // … and there is NO per-segment explosion: a faceted/grid wall would be ~48+
    // faces; the analytic collapse gives wall + bore + 2 caps.
    assert!(
        k.len() <= 5,
        "smooth wall must not explode into per-segment faces: {} faces {k:?}",
        k.len()
    );
    assert!(
        validate_solid_scoped(&m, sid, Tolerance::default(), ValidationLevel::Standard).is_valid,
        "spline revolve must be a valid solid"
    );
    // The editable wall profile is retained.
    assert_eq!(
        get_revolve_meridian(&m, sid).expect("recover").len(),
        4,
        "the wall profile is recoverable for editing"
    );
}

/// HARNESS — face economy + smoothness (the invariant that was MISSING when the
/// faceted "so many circles" revolve shipped): a smooth axisymmetric nozzle must
/// revolve to O(1) analytic faces — ONE `SurfaceOfRevolution` per wall + 2 rims
/// = 4 — NOT O(points) faceted bands. A valid, watertight, 90-face solid passes
/// every OTHER check; only this gate fails it.
#[test]
fn smooth_nozzle_is_two_revolution_walls_not_faceted() {
    let mut m = BRepModel::new();
    // A bell inner/flow contour: chamber r12 → throat r4 @ z12 → exit r10, sampled
    // at 15 stations (a faceted revolve would explode to ~15 bands per wall).
    let inner: Vec<(f64, f64)> = (0..15)
        .map(|i| {
            let z = i as f64 * 3.0;
            let r = if z <= 12.0 {
                12.0 - 8.0 * (z / 12.0)
            } else {
                4.0 + 6.0 * ((z - 12.0) / 30.0)
            };
            (r, z)
        })
        .collect();
    let opts = RevolveOptions {
        axis_origin: Point3::ZERO,
        axis_direction: Vector3::Z,
        angle: std::f64::consts::TAU,
        segments: 64,
        ..Default::default()
    };
    let sid = revolve_smooth_nozzle(&mut m, &inner, 2.0, opts)
        .unwrap_or_else(|e| panic!("revolve_smooth_nozzle: {e:?}"));

    let k = face_kinds(&m, sid);
    // The inner flow wall and the outer wall are EACH one smooth SurfaceOfRevolution.
    assert_eq!(
        count(&k, SurfaceType::SurfaceOfRevolution),
        2,
        "inner + outer must each be ONE SurfaceOfRevolution (face economy): {k:?}"
    );
    // O(1) faces total — 2 walls + 2 annular rims — never O(points) bands.
    assert!(
        k.len() <= 4,
        "smooth nozzle must be 4 faces (2 walls + 2 rims), not faceted: {} {k:?}",
        k.len()
    );
    assert!(
        validate_solid_scoped(&m, sid, Tolerance::default(), ValidationLevel::Standard).is_valid,
        "smooth nozzle must be a valid solid"
    );
    // And the flow contour is recoverable for editing.
    assert_eq!(get_revolve_meridian(&m, sid).expect("recover").len(), 15);
}

/// HARNESS — ROTATIONAL SYMMETRY (the automatic visual-verification invariant,
/// geometric form): a Z-axis revolve is axisymmetric, so the tessellated mesh's
/// X-extent must equal its Y-extent. A VALID, watertight nozzle can still be
/// DEFORMED (non-circular) — exactly what shipped (cert said sound, dims were
/// 114≠104). This invariant catches that class. Uses a Rao-like contour with a
/// throat + a sharp chamber corner (the case that warped live).
///
/// This gate guards the SurfaceOfRevolution seam-tessellation fix: the wedge
/// path (`tessellate_revolution_wedge`) used to Coons-blend interior nodes, which
/// overshoots OFF a curved meridian and spiked the seam to r≈60 on a wall of true
/// radius ≤52 (octant_max_r=[60,52,…,52,60], x=112≠y=104). The fix rotates the
/// cached seam about the surface's true axis instead — exact, so the mesh is a
/// flat 52 in every octant and x==y. Regression marker: if the Coons path (or any
/// non-surface interior) returns, this fails first.
#[test]
fn smooth_nozzle_revolve_is_rotationally_symmetric() {
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    let mut m = BRepModel::new();
    let inner: Vec<(f64, f64)> = vec![
        (30.0, 0.0),
        (30.0, 6.0),
        (30.0, 12.0),
        (24.0, 17.0),
        (18.0, 21.0),
        (13.0, 24.0),
        (10.0, 27.0),
        (10.5, 29.0),
        (13.0, 35.0),
        (18.0, 45.0),
        (24.0, 60.0),
        (30.0, 80.0),
        (38.0, 105.0),
        (45.0, 130.0),
        (50.0, 156.0),
    ];
    let opts = RevolveOptions {
        axis_origin: Point3::ZERO,
        axis_direction: Vector3::Z,
        angle: std::f64::consts::TAU,
        segments: 96,
        ..Default::default()
    };
    let sid = revolve_smooth_nozzle(&mut m, &inner, 2.0, opts)
        .unwrap_or_else(|e| panic!("revolve_smooth_nozzle: {e:?}"));
    let solid = m.solids.get(sid).expect("solid");
    let mesh = tessellate_solid(solid, &m, &TessellationParams::default());

    let (mut xmin, mut xmax, mut ymin, mut ymax) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for v in &mesh.vertices {
        let p = v.position;
        xmin = xmin.min(p.x);
        xmax = xmax.max(p.x);
        ymin = ymin.min(p.y);
        ymax = ymax.max(p.y);
    }
    let (xext, yext) = (xmax - xmin, ymax - ymin);

    assert!(
        (xext - yext).abs() < 0.01 * xext.max(yext),
        "a Z-axis revolve must be rotationally symmetric (X-extent ≈ Y-extent); \
         deformed: x={xext:.3} y={yext:.3}"
    );
}

/// Fraction of non-degenerate facets whose winding normal sits in the SAME
/// hemisphere as the TRUE surface normal (oriented by the owning face's sense),
/// plus the worst deviation in degrees. This is the cheap, coarse-mesh form of
/// the `tessellation_quality` / `mesh_quality` certificate's ground-truth check:
/// an inverted wall (the B3 bug — every facet wound against its own surface)
/// scores ~0.0 and ~180°. Coarse params keep the per-facet `closest_point` count
/// in the hundreds (the `fine()` cert runs ~40k facets → minutes), while gross
/// inversion is a coarse fault visible at any density.
fn revolve_facet_soundness(m: &BRepModel, sid: SolidId) -> (f64, f64) {
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    let solid = m.solids.get(sid).expect("solid");
    let mesh = tessellate_solid(solid, m, &TessellationParams::coarse());
    let has_faces = mesh.face_map.len() == mesh.triangles.len();
    let tol = Tolerance::default();
    let (mut agree, mut comparable) = (0usize, 0usize);
    let mut max_dev = 0.0_f64;
    for (i, tri) in mesh.triangles.iter().enumerate() {
        let p0 = mesh.vertices[tri[0] as usize].position;
        let p1 = mesh.vertices[tri[1] as usize].position;
        let p2 = mesh.vertices[tri[2] as usize].position;
        let geo = (p1 - p0).cross(&(p2 - p0));
        if geo.magnitude() <= 1e-12 {
            continue;
        }
        comparable += 1;
        let fid = if has_faces { mesh.face_map[i] } else { 0 };
        let face = match m.faces.get(fid) {
            Some(f) => f,
            None => {
                agree += 1;
                continue;
            }
        };
        let surface = match m.surfaces.get(face.surface_id) {
            Some(s) => s,
            None => {
                agree += 1;
                continue;
            }
        };
        let centroid = Point3::new(
            (p0.x + p1.x + p2.x) / 3.0,
            (p0.y + p1.y + p2.y) / 3.0,
            (p0.z + p1.z + p2.z) / 3.0,
        );
        let oriented = match surface
            .closest_point(&centroid, tol)
            .ok()
            .and_then(|(u, v)| surface.normal_at(u, v).ok())
        {
            Some(n) => n * face.orientation.sign(),
            None => {
                agree += 1;
                continue;
            }
        };
        if let (Ok(g), Ok(o)) = (geo.normalize(), oriented.normalize()) {
            let dot = g.dot(&o).clamp(-1.0, 1.0);
            if dot > 0.0 {
                agree += 1;
            }
            max_dev = max_dev.max(dot.acos().to_degrees());
        } else {
            agree += 1;
        }
    }
    let frac = if comparable == 0 {
        1.0
    } else {
        agree as f64 / comparable as f64
    };
    (frac, max_dev)
}

/// SOUNDNESS GATE — `revolve` + `wall_thickness` (smooth-nozzle) must tessellate
/// to facets that AGREE with their true surface normals, not just be watertight.
///
/// The B3 bug: the NACELLE flow contour (a near-CONSTANT-radius body section —
/// r = 2.6 at z = 4 AND z = 8 — plus a shallow bulge) built a watertight, valid,
/// rotationally-symmetric wall whose ENTIRE tessellation was INVERTED (every
/// facet wound against its own `SurfaceOfRevolution`: normal_agreement ≈ 0.01,
/// max deviation ≈ 179°, ~40 000/40 400 off-surface facets) — `sound = false`
/// despite passing every watertight/bbox check. Root cause: the full-360°
/// revolution band's structured-grid tessellation keyed its triangle winding on
/// `face.orientation` under the assumption `(i+)×(j+) = +natural`, but for a
/// revolution wall the grid index runs (angle, profile), so the cross product is
/// `dv×du = −natural` — the winding inverted. The fix winds each facet to match
/// the TRUE surface normal (× face sign), independent of the i/j convention.
///
/// This gate asserts BOTH the failing nacelle and the originally-sound nozzle are
/// facet-sound. Watertight ≠ sound — `revolve_watertight.rs` checks the former;
/// this checks the latter. Regression marker: if the grid winding reverts, the
/// nacelle agreement collapses to ~0 and this fails first.
#[test]
fn smooth_nozzle_wall_tessellation_is_facet_sound() {
    let cases: &[(&str, &[(f64, f64)], f64)] = &[
        (
            "nacelle",
            &[
                (2.2, 0.0),
                (2.5, 1.5),
                (2.6, 4.0),
                (2.6, 8.0),
                (2.3, 10.5),
                (2.0, 12.0),
            ],
            0.4,
        ),
        (
            "nozzle",
            &[
                (6.0, 0.0),
                (5.2, 2.0),
                (4.0, 3.5),
                (2.8, 5.0),
                (2.1, 6.0),
                (2.0, 6.5),
                (2.5, 7.2),
                (3.2, 8.4),
                (4.0, 10.2),
                (4.8, 13.0),
                (5.5, 16.0),
                (5.9, 18.5),
            ],
            0.3,
        ),
    ];
    for &(name, inner, wt) in cases {
        for (label, origin) in [
            ("origin", Point3::ZERO),
            ("offset", Point3::new(5.0, 0.0, 0.0)),
        ] {
            let mut m = BRepModel::new();
            let opts = RevolveOptions {
                axis_origin: origin,
                axis_direction: Vector3::Z,
                angle: std::f64::consts::TAU,
                segments: 32,
                ..Default::default()
            };
            let sid = revolve_smooth_nozzle(&mut m, inner, wt, opts)
                .unwrap_or_else(|e| panic!("{name}@{label}: build {e:?}"));
            assert!(
                validate_solid_scoped(&m, sid, Tolerance::default(), ValidationLevel::Standard)
                    .is_valid,
                "{name}@{label}: must be a valid solid"
            );
            let (agree, max_dev) = revolve_facet_soundness(&m, sid);
            assert!(
                agree >= 0.999,
                "{name}@{label}: revolve+wall_thickness tessellation must be facet-sound \
                 (winding agrees with the true surface normal); got agreement={agree:.4} \
                 max_deviation={max_dev:.1}° — the wall is INVERTED"
            );
            assert!(
                max_dev < 40.0,
                "{name}@{label}: worst facet normal deviation {max_dev:.1}° exceeds 40° \
                 (off-surface / inverted facets)"
            );
        }
    }
}

/// FUNDAMENTAL FIX — a solid of revolution that CLOSES TO THE AXIS (an apex /
/// pole) must be O(profile) analytic faces, NOT profile×`segments`. This is the
/// nose-cone / dome / vessel-cap class that fell to the grid path and exploded
/// (a live nose cone = 3,840 faces), bloating the STEP export until FreeCAD
/// crashed. The apex band is ONE cone-to-apex face, the base ONE disc, and the
/// pure axis segment bounds no face.
#[test]
fn solid_cone_to_apex_is_two_analytic_faces() {
    let mut m = BRepModel::new();
    // base centre → base rim → apex (auto-closes apex→centre along the axis).
    let s = revolve(&mut m, &[(0.0, 0.0), (10.0, 0.0), (0.0, 20.0)], 64);
    let k = face_kinds(&m, s);
    assert!(
        k.len() <= 3,
        "solid cone must be ~2 analytic faces, not 64× bands: {} {k:?}",
        k.len()
    );
    assert_eq!(count(&k, SurfaceType::Cone), 1, "one cone-to-apex lateral");
    assert_eq!(count(&k, SurfaceType::Plane), 1, "one disc base");
    assert_eq!(
        count(&k, SurfaceType::SurfaceOfRevolution),
        0,
        "no faceted patches"
    );
    let v = validate_solid_scoped(&m, s, Tolerance::default(), ValidationLevel::Standard);
    assert!(v.is_valid, "solid cone invalid: {:?}", v.errors);
}

/// The exact failing case: a tangent-ogive NOSE CONE (polyline meridian to the
/// apex). 20 profile points → ~19 cone bands + apex + disc — O(profile). Before
/// the fix this hit the grid path and emitted profile×`segments` ≈ 1,800+ faces
/// (the live nose was 3,840), which bloated the STEP and crashed FreeCAD. Pin it:
/// under 60 faces, no faceted patches, and a valid solid.
#[test]
fn nose_cone_ogive_is_o_profile_faces_not_band_explosion() {
    let (r, l) = (30.0_f64, 78.0_f64);
    let rho = (r * r + l * l) / (2.0 * r);
    let mut pts = vec![(0.0_f64, 0.0_f64)];
    for i in 0..20 {
        let z = l * i as f64 / 19.0;
        let rr = ((rho * rho - z * z).sqrt() - (rho - r)).max(0.0);
        pts.push((rr, z));
    }
    *pts.last_mut().expect("nonempty") = (0.0, l); // exact apex on the axis
    let mut m = BRepModel::new();
    let s = revolve(&mut m, &pts, 96);
    let k = face_kinds(&m, s);
    assert!(
        k.len() < 60,
        "nose cone must be O(profile) faces (~22), not O(profile×segments) ≈ 1800: {} faces",
        k.len()
    );
    assert_eq!(
        count(&k, SurfaceType::SurfaceOfRevolution),
        0,
        "polyline ogive bands are Cones, not faceted SurfaceOfRevolution patches: {k:?}"
    );
    let v = validate_solid_scoped(&m, s, Tolerance::default(), ValidationLevel::Standard);
    assert!(v.is_valid, "nose cone invalid: {:?}", v.errors);
}
