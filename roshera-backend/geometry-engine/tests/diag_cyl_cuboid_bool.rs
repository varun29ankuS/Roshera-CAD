//! DIAGNOSTIC: tessellation quality of cylinder ⊕ cuboid boolean results.
//!
//! Investigates whether the inner-bore "scribble" (degenerate / criss-crossed /
//! off-radial triangulation on a cylindrical wall) is rooted in the
//! cylinder+cuboid boolean (Difference = through-bore, Union = boss,
//! Intersection), and whether the defect is DENSITY-dependent (clean at the
//! certificate's chord 0.1, broken at the viewport's `default()` density).
//!
//! For each boolean result we isolate the CYLINDRICAL face(s) via `face_map`,
//! then for those triangles measure:
//!   - degenerate-triangle count (zero/near-zero area),
//!   - facet-normal vs STORED vertex-normal agreement (what the current cert can see),
//!   - facet-normal vs ANALYTIC cylinder-surface normal agreement (|n·radial|,
//!     the metric `score_mesh_tessellation` LACKS),
//!   - in-face self-overlap / criss-cross proxy (off-radial fraction).
//! Renders into the bore in Normals + Shaded for eyes-on confirmation.
//!
//! Run: cargo test -p geometry-engine --test diag_cyl_cuboid_bool -- --nocapture

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::extrude::{
    create_face_from_profile_with_plane, extrude_face, ExtrudeOptions,
};
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::curve::{Circle, Curve, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
use geometry_engine::primitives::r#loop::{Loop, LoopType};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::Cylinder;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::render::{render_solid_dir, CanonicalView, RenderMode, RenderOptions};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams, TriangleMesh};

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(id) => id,
        o => panic!("expected Solid, got {o:?}"),
    }
}

/// chord-0.1 params: what the certificate / audit oracle uses (coarse).
fn cert_params() -> TessellationParams {
    TessellationParams {
        max_edge_length: 0.5,
        max_angle_deviation: 0.3,
        chord_tolerance: 0.1,
        min_segments: 3,
        max_segments: 24,
    }
}

#[derive(Default, Debug)]
struct CylStats {
    cyl_faces: usize,
    tris: usize,
    degenerate: usize,
    /// stored-normal agreement: mean |facet · mean(stored vertex normals)|
    stored_agree_sum: f64,
    stored_bad: usize, // |dot| < 0.5
    /// analytic agreement: mean |facet · analytic radial normal at centroid|
    analytic_agree_sum: f64,
    analytic_bad: usize, // |dot| < 0.5  → off-radial / scribbled facet
    analytic_min: f64,
}

/// Find all Cylinder faces on the result solid and analyze their triangles.
/// `radius_filter` keeps only cylinder faces whose radius is < `radius_filter`
/// (so we target the BORE on a box−cyl, or the boss wall otherwise). Pass
/// f64::INFINITY to keep every cylinder face.
fn analyze_cyl(model: &BRepModel, s: SolidId, mesh: &TriangleMesh, radius_filter: f64) -> CylStats {
    let solid = model.solids.get(s).expect("solid");
    let shell = model.shells.get(solid.outer_shell).expect("shell");

    // Map FaceId -> (cylinder copy) for cylinder faces under the radius filter.
    let mut cyl_faces: std::collections::HashMap<u32, Cylinder> = Default::default();
    for &fid in &shell.faces {
        let Some(face) = model.faces.get(fid) else {
            continue;
        };
        let Some(surf) = model.surfaces.get(face.surface_id) else {
            continue;
        };
        if let Some(cyl) = surf.as_any().downcast_ref::<Cylinder>() {
            if cyl.radius < radius_filter {
                cyl_faces.insert(fid, *cyl);
            }
        }
    }

    let mut st = CylStats {
        cyl_faces: cyl_faces.len(),
        analytic_min: 1.0,
        ..Default::default()
    };

    for (ti, tri) in mesh.triangles.iter().enumerate() {
        let fid = mesh.face_map.get(ti).copied().unwrap_or(u32::MAX);
        let Some(cyl) = cyl_faces.get(&fid) else {
            continue;
        };
        st.tris += 1;

        let p0 = mesh.vertices[tri[0] as usize].position;
        let p1 = mesh.vertices[tri[1] as usize].position;
        let p2 = mesh.vertices[tri[2] as usize].position;
        let e1 = p1 - p0;
        let e2 = p2 - p0;
        let cross = e1.cross(&e2);
        let area2 = cross.magnitude();
        if area2 < 1e-9 {
            st.degenerate += 1;
            continue;
        }
        let facet_n = cross * (1.0 / area2);

        // STORED normal: mean of the three stored vertex normals.
        let n0 = mesh.vertices[tri[0] as usize].normal;
        let n1 = mesh.vertices[tri[1] as usize].normal;
        let n2 = mesh.vertices[tri[2] as usize].normal;
        let stored = (n0 + n1 + n2).normalize().unwrap_or(facet_n);
        let stored_dot = facet_n.dot(&stored).abs();
        st.stored_agree_sum += stored_dot;
        if stored_dot < 0.5 {
            st.stored_bad += 1;
        }

        // ANALYTIC normal: radial direction at the triangle centroid.
        // For a point P on a cylinder, the surface normal is the component of
        // (P - origin) perpendicular to the axis, normalized. Sense-independent
        // via |dot| so a flipped bore face is not falsely flagged.
        let c = (p0 + p1 + p2) * (1.0 / 3.0);
        let rel = c - cyl.origin;
        let along = cyl.axis * rel.dot(&cyl.axis);
        let radial = rel - along;
        if let Ok(radial_n) = radial.normalize() {
            let adot = facet_n.dot(&radial_n).abs();
            st.analytic_agree_sum += adot;
            if adot < 0.5 {
                st.analytic_bad += 1;
            }
            if adot < st.analytic_min {
                st.analytic_min = adot;
            }
        }
    }
    st
}

fn report(label: &str, st: &CylStats) {
    let n = st.tris.max(1) as f64;
    let nd = (st.tris - st.degenerate).max(1) as f64;
    eprintln!("  [{label}]");
    eprintln!(
        "    cyl_faces={} cyl_tris={} degenerate={}",
        st.cyl_faces, st.tris, st.degenerate
    );
    eprintln!(
        "    STORED-normal agree (mean|dot|)={:.4}  bad(<0.5)={} ({:.1}%)",
        st.stored_agree_sum / nd,
        st.stored_bad,
        100.0 * st.stored_bad as f64 / n
    );
    eprintln!(
        "    ANALYTIC-radial agree (mean|dot|)={:.4} min={:.4}  bad(<0.5)={} ({:.1}%)",
        st.analytic_agree_sum / nd,
        st.analytic_min,
        st.analytic_bad,
        100.0 * st.analytic_bad as f64 / n
    );
}

fn save(model: &BRepModel, s: SolidId, dir: Vector3, up: Vector3, mode: RenderMode, name: &str) {
    let opts = RenderOptions {
        width: 640,
        height: 640,
        view: CanonicalView::Isometric,
        mode,
        tessellation: TessellationParams::default(),
    };
    let frame = render_solid_dir(model, s, dir, up, &opts).expect("render");
    let png = frame.to_png().expect("png");
    let outdir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("cyl_cuboid_diag");
    std::fs::create_dir_all(&outdir).unwrap();
    let path = outdir.join(format!("{name}.png"));
    std::fs::write(&path, png).unwrap();
    eprintln!("    wrote {} ({} bytes)", path.display(), png_len(&path));
}

fn png_len(p: &std::path::Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// Build BOX − CYLINDER : a coaxial cylinder fully through a box = through-bore.
/// Box 40×40×20 centered at origin (z in [-10,10]); cylinder r=8 along +Z from
/// z=-15..+15 (fully through). Returns (model, result_solid, bore_radius_filter).
fn box_minus_cyl() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let box_s = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(40.0, 40.0, 20.0)
        .unwrap());
    let cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -15.0), Vector3::Z, 8.0, 30.0)
        .unwrap());
    let res = boolean_operation(
        &mut m,
        box_s,
        cyl,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("box - cyl");
    (m, sid_of(res))
}

fn box_union_cyl() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let box_s = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(40.0, 40.0, 20.0)
        .unwrap());
    // Boss rising out of the top face (box top at z=+10), cyl base at z=0 so it
    // overlaps the box body and protrudes above.
    let cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 8.0, 25.0)
        .unwrap());
    let res = boolean_operation(
        &mut m,
        box_s,
        cyl,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("box ∪ cyl");
    (m, sid_of(res))
}

fn box_intersect_cyl() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let box_s = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(40.0, 40.0, 20.0)
        .unwrap());
    // Cylinder taller than the box so ∩ = a disc of height 20, r=8 (cyl walls
    // trimmed by box top/bottom planes).
    let cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -15.0), Vector3::Z, 8.0, 30.0)
        .unwrap());
    let res = boolean_operation(
        &mut m,
        box_s,
        cyl,
        BooleanOp::Intersection,
        BooleanOptions::default(),
    )
    .expect("box ∩ cyl");
    (m, sid_of(res))
}

fn sid_of(g: SolidId) -> SolidId {
    g
}

fn run_case(name: &str, model: &BRepModel, s: SolidId, radius_filter: f64) {
    eprintln!("\n========== {name} ==========");
    let def = TessellationParams::default();
    let cert = cert_params();

    let solid = model.solids.get(s).expect("solid");
    let mesh_def = tessellate_solid(solid, model, &def);
    let mesh_cert = tessellate_solid(solid, model, &cert);

    eprintln!(
        "  total tris: default={} cert(0.1)={}",
        mesh_def.triangles.len(),
        mesh_cert.triangles.len()
    );

    let st_def = analyze_cyl(model, s, &mesh_def, radius_filter);
    let st_cert = analyze_cyl(model, s, &mesh_cert, radius_filter);

    eprintln!("  --- DISPLAY default() (chord 0.001, viewport) ---");
    report("default", &st_def);
    eprintln!("  --- CERT chord 0.1 (coarse) ---");
    report("cert-0.1", &st_cert);
}

fn circle_edge(m: &mut BRepModel, center: Point3, axis: Vector3, radius: f64) -> u32 {
    let circle = Circle::new(center, axis, radius).unwrap();
    let seam = circle.point_at(0.0).unwrap();
    let v = m
        .vertices
        .add_or_find(seam.x, seam.y, seam.z, Tolerance::default().distance());
    let cid = m.curves.add(Box::new(circle));
    m.edges.add(Edge::new(
        0,
        v,
        v,
        cid,
        EdgeOrientation::Forward,
        ParameterRange::new(0.0, 1.0),
    ))
}

/// Build a tube whose INNER bore cylinder seam (`ref_dir`) is rotated by
/// `offset_deg`, mimicking what STEP import produces (arbitrary ref_dir not
/// aligned to canonical +X). This is the IMPORT-TOPOLOGY path — NOT a boolean.
fn import_like_tube(offset_deg: f64) -> (BRepModel, SolidId) {
    use std::f64::consts::PI;
    let mut m = BRepModel::new();
    let outer = circle_edge(&mut m, Point3::ZERO, Vector3::Z, 30.0);
    let face =
        create_face_from_profile_with_plane(&mut m, vec![outer], Point3::ZERO, Vector3::Z).unwrap();
    let he = circle_edge(&mut m, Point3::ZERO, Vector3::Z, 18.0);
    let mut il = Loop::new(0, LoopType::Inner);
    il.add_edge(he, true);
    let ilid = m.loops.add(il);
    m.faces.get_mut(face).unwrap().add_inner_loop(ilid);
    let opts = ExtrudeOptions {
        direction: Vector3::Z,
        distance: 20.0,
        ..ExtrudeOptions::default()
    };
    let s = extrude_face(&mut m, face, opts).unwrap();
    let solid = m.solids.get(s).unwrap();
    let shell = m.shells.get(solid.outer_shell).unwrap();
    let inner_fid = *shell
        .faces
        .iter()
        .find(|&&fid| {
            let f = m.faces.get(fid).unwrap();
            m.surfaces
                .get(f.surface_id)
                .unwrap()
                .as_any()
                .downcast_ref::<Cylinder>()
                .map(|c| c.radius < 25.0)
                .unwrap_or(false)
        })
        .unwrap();
    let surf_id = m.faces.get(inner_fid).unwrap().surface_id;
    let mut rotated = *m
        .surfaces
        .get(surf_id)
        .unwrap()
        .as_any()
        .downcast_ref::<Cylinder>()
        .unwrap();
    let a = offset_deg * PI / 180.0;
    rotated.ref_dir = Vector3::new(a.cos(), a.sin(), 0.0);
    m.surfaces.replace(surf_id, Box::new(rotated));
    (m, s)
}

#[test]
fn diag_import_like_bore() {
    // CONTROL: the import-topology path (rotated inner-bore seam), NOT a boolean.
    // This is the case the user attributes the scribble to. r_in=18 < 25.
    let (m, s) = import_like_tube(137.0);
    run_case("IMPORT-LIKE TUBE (rotated bore seam 137°)", &m, s, 25.0);
    let dir = Vector3::new(-0.25, -0.25, -1.0).normalize().unwrap();
    let up = Vector3::new(0.0, 1.0, 0.0);
    save(&m, s, dir, up, RenderMode::Normals, "import_bore_normals");
    save(&m, s, dir, up, RenderMode::Shaded, "import_bore_shaded");
    save(&m, s, dir, up, RenderMode::Diagnostic, "import_bore_diag");
}

#[test]
fn diag_box_minus_cyl_bore() {
    let (m, s) = box_minus_cyl();
    // bore radius is 8; keep cyl faces with radius < 1000 (the only cyl face).
    run_case("BOX − CYLINDER (through-bore)", &m, s, 1000.0);

    // Render INTO the bore from above.
    let dir = Vector3::new(-0.3, -0.3, -1.0).normalize().unwrap();
    let up = Vector3::new(0.0, 1.0, 0.0);
    save(&m, s, dir, up, RenderMode::Normals, "box_minus_cyl_normals");
    save(&m, s, dir, up, RenderMode::Shaded, "box_minus_cyl_shaded");
    save(&m, s, dir, up, RenderMode::Diagnostic, "box_minus_cyl_diag");
}

#[test]
fn diag_box_union_cyl_boss() {
    let (m, s) = box_union_cyl();
    run_case("BOX ∪ CYLINDER (boss)", &m, s, 1000.0);
    let dir = Vector3::new(-0.4, -0.4, -0.6).normalize().unwrap();
    let up = Vector3::new(0.0, 0.0, 1.0);
    save(&m, s, dir, up, RenderMode::Normals, "box_union_cyl_normals");
    save(&m, s, dir, up, RenderMode::Shaded, "box_union_cyl_shaded");
    save(&m, s, dir, up, RenderMode::Diagnostic, "box_union_cyl_diag");
}

#[test]
fn diag_box_intersect_cyl() {
    let (m, s) = box_intersect_cyl();
    run_case("BOX ∩ CYLINDER (disc)", &m, s, 1000.0);
    let dir = Vector3::new(-0.4, -0.4, -0.6).normalize().unwrap();
    let up = Vector3::new(0.0, 0.0, 1.0);
    save(
        &m,
        s,
        dir,
        up,
        RenderMode::Normals,
        "box_intersect_cyl_normals",
    );
    save(
        &m,
        s,
        dir,
        up,
        RenderMode::Shaded,
        "box_intersect_cyl_shaded",
    );
    save(
        &m,
        s,
        dir,
        up,
        RenderMode::Diagnostic,
        "box_intersect_cyl_diag",
    );
}
