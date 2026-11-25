// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A STEP SURFACE_OF_REVOLUTION's normal is fixed by ISO 10303-42: u is the
//! ROTATION angle, v the swept curve's parameter, and the normal is
//! dσ/du × dσ/dv. A face's `same_sense` flag is relative to THAT normal. The
//! kernel's surface of revolution parameterises the other way round (u along
//! the profile, v the angle), which reverses du × dv, so an imported face
//! has to come in with the outward side STEP meant -- not the opposite one.
//! One CADGenBench part's hole walls (revolution faces) all read as
//! inconsistently oriented against the plates around them.

use export_engine::ExportEngine;

fn envelope(body: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('rev'),'2;1');\n\
         FILE_NAME('rev.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n{body}\nENDSEC;\n\
         END-ISO-10303-21;\n"
    )
}

/// A quarter of a radius-10 wall about +Z, 5 tall, as a revolution of the
/// straight profile (10,0,0) -> (10,0,5). ISO 10303-42: dσ/du (rotation) at
/// the profile is +Y, dσ/dv (up the profile) is +Z, so the surface normal
/// is +Y x +Z = +X: AWAY from the axis. With `same_sense = .T.` the face's
/// outward side is away from the axis.
const WALL: &str = "#1=CARTESIAN_POINT('',(10.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#3=VECTOR('',#2,1.);
#4=LINE('',#1,#3);
#5=CARTESIAN_POINT('',(0.,0.,0.));
#6=AXIS1_PLACEMENT('',#5,#2);
#7=SURFACE_OF_REVOLUTION('',#4,#6);
#11=CARTESIAN_POINT('',(10.,0.,0.));
#12=CARTESIAN_POINT('',(0.,10.,0.));
#13=CARTESIAN_POINT('',(0.,10.,5.));
#14=CARTESIAN_POINT('',(10.,0.,5.));
#21=VERTEX_POINT('',#11);
#22=VERTEX_POINT('',#12);
#23=VERTEX_POINT('',#13);
#24=VERTEX_POINT('',#14);
#30=DIRECTION('',(1.,0.,0.));
#31=AXIS2_PLACEMENT_3D('',#5,#2,#30);
#32=CIRCLE('',#31,10.);
#33=CARTESIAN_POINT('',(0.,0.,5.));
#34=AXIS2_PLACEMENT_3D('',#33,#2,#30);
#35=CIRCLE('',#34,10.);
#36=CARTESIAN_POINT('',(0.,10.,0.));
#37=VECTOR('',#2,1.);
#38=LINE('',#36,#37);
#51=EDGE_CURVE('',#21,#22,#32,.T.);
#52=EDGE_CURVE('',#22,#23,#38,.T.);
#53=EDGE_CURVE('',#24,#23,#35,.T.);
#54=EDGE_CURVE('',#21,#24,#4,.T.);
#61=ORIENTED_EDGE('',*,*,#51,.T.);
#62=ORIENTED_EDGE('',*,*,#52,.T.);
#63=ORIENTED_EDGE('',*,*,#53,.F.);
#64=ORIENTED_EDGE('',*,*,#54,.F.);
#70=EDGE_LOOP('',(#61,#62,#63,#64));
#71=FACE_OUTER_BOUND('',#70,.T.);
#80=ADVANCED_FACE('',(#71),#7,.T.);
#90=OPEN_SHELL('',(#80));
#91=SHELL_BASED_SURFACE_MODEL('',(#90));";

#[test]
fn an_imported_revolution_face_faces_the_way_step_says() {
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(WALL), "rev")
        .expect("imports");
    let faces: Vec<_> = model.faces.iter().collect();
    assert_eq!(faces.len(), 1, "one face: {:#?}", report.warnings);
    let (_, face) = faces[0];
    let surface = model.surfaces.get(face.surface_id).expect("surface");
    // The face's outward normal at a point of the wall, 45 degrees round.
    let p = geometry_engine::math::Point3::new(
        10.0 * std::f64::consts::FRAC_1_SQRT_2,
        10.0 * std::f64::consts::FRAC_1_SQRT_2,
        2.5,
    );
    let (u, v) = surface
        .closest_point(&p, geometry_engine::math::Tolerance::default())
        .expect("projects");
    let n = surface.normal_at(u, v).expect("normal") * face.orientation.sign();
    let away = geometry_engine::math::Vector3::new(p.x, p.y, 0.0);
    assert!(
        n.dot(&away) > 0.0,
        "{}: the face's outward normal {n:?} points toward the axis; STEP says away",
        surface.type_name()
    );
}

/// The writer is the importer's inverse: a kernel vase (one smooth revolution
/// wall, a bore, two caps) exported to STEP and read back keeps its outer
/// wall facing AWAY from the axis. Together with the test above (the
/// importer reads ISO's convention), this pins the WRITER to ISO too: a
/// writer that emitted the kernel's own axis would round-trip a wall that
/// OCCT and every other reader see inside out.
#[tokio::test]
async fn an_exported_revolution_wall_reads_back_facing_out() {
    use geometry_engine::math::{Point3, Tolerance, Vector3};
    use geometry_engine::operations::revolve::{revolve_spline_meridian, RevolveOptions};
    use geometry_engine::primitives::topology_builder::BRepModel;

    let mut model = BRepModel::new();
    let wall = [(5.0, 0.0), (3.0, 4.0), (4.0, 8.0), (7.0, 12.0)];
    let opts = RevolveOptions {
        axis_origin: Point3::ZERO,
        axis_direction: Vector3::Z,
        angle: std::f64::consts::TAU,
        segments: 48,
        ..Default::default()
    };
    revolve_spline_meridian(&mut model, &wall, 2.0, opts).expect("vase");

    let temp = tempfile::TempDir::new().expect("tmp");
    let engine = ExportEngine::with_output_directory(temp.path().to_string_lossy().to_string());
    let filename = engine.export_step(&model, "vase").await.expect("export");
    let text = std::fs::read_to_string(temp.path().join(&filename)).expect("read");
    let (back, _report) = ExportEngine::new()
        .import_step_content(&text, "vase")
        .expect("re-imports");

    let mut walls = 0;
    for (_, face) in back.faces.iter() {
        let Some(surface) = back.surfaces.get(face.surface_id) else {
            continue;
        };
        if surface.type_name() != "SurfaceOfRevolution" {
            continue;
        }
        walls += 1;
        // A point on the outer wall at mid height, on +X.
        let p = Point3::new(3.5, 0.0, 6.0);
        let (u, v) = surface
            .closest_point(&p, Tolerance::default())
            .expect("projects");
        let q = surface.point_at(u, v).expect("evaluates");
        let n = surface.normal_at(u, v).expect("normal") * face.orientation.sign();
        let away = Vector3::new(q.x, q.y, 0.0);
        assert!(
            n.dot(&away) > 0.0,
            "the re-imported outer wall faces the axis at {q:?} (normal {n:?})"
        );
    }
    assert_eq!(walls, 1, "the vase has one revolution wall");
}

/// A revolution whose profile is a B-spline on its OWN parameter domain --
/// here [3pi/2, 2pi], the domain OCCT gave every revolution profile of one
/// CADGenBench part -- is the surface that profile sweeps. The kernel's
/// surface of revolution reads its profile on [0, 1]; given this curve
/// as-is it evaluated at 0..1, clamped to the curve's start point, and the
/// whole wall collapsed onto one circle.
#[test]
fn a_revolution_of_a_spline_on_its_own_domain_is_the_swept_surface() {
    let body = WALL.replace(
        "#7=SURFACE_OF_REVOLUTION('',#4,#6);",
        "#8=CARTESIAN_POINT('',(10.,0.,5.));\n\
         #9=B_SPLINE_CURVE_WITH_KNOTS('',1,(#1,#8),.UNSPECIFIED.,.F.,.F.,(2,2),\
         (4.71238898038469,6.28318530717959),.UNSPECIFIED.);\n\
         #7=SURFACE_OF_REVOLUTION('',#9,#6);",
    );
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(&body), "rev")
        .expect("imports");
    let faces: Vec<_> = model.faces.iter().collect();
    assert_eq!(faces.len(), 1, "one face: {:#?}", report.warnings);
    let surface = model.surfaces.get(faces[0].1.surface_id).expect("surface");
    for z in [0.5, 2.5, 4.5] {
        let p = geometry_engine::math::Point3::new(
            10.0 * std::f64::consts::FRAC_1_SQRT_2,
            10.0 * std::f64::consts::FRAC_1_SQRT_2,
            z,
        );
        let (u, v) = surface
            .closest_point(&p, geometry_engine::math::Tolerance::default())
            .expect("projects");
        let q = surface.point_at(u, v).expect("evaluates");
        assert!(
            q.distance(&p) < 1e-6,
            "the wall point {p:?} is {:.3} off the imported surface (nearest {q:?})",
            q.distance(&p)
        );
    }
}

/// An imported revolution face is a PATCH of its surface (here a quarter of
/// the turn) and its mesh must face out where the patch actually is. The
/// structured wedge mesher read each cell's outward side from the surface
/// at a parameter scaled from the grid index -- right for a kernel wall that
/// spans the whole surface, but on a patch it sampled the surface elsewhere
/// (across the axis) and wound the triangles inward. Four hole walls of one
/// CADGenBench part meshed inside out against every neighbour.
#[test]
fn an_imported_revolution_patch_meshes_facing_out() {
    use geometry_engine::tessellation::{
        edge_cache::EdgeSampleCache, tessellate_face, TessellationParams, TriangleMesh,
    };
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(WALL), "rev")
        .expect("imports");
    let faces: Vec<_> = model.faces.iter().collect();
    assert_eq!(faces.len(), 1, "one face: {:#?}", report.warnings);
    let (_, face) = faces[0];
    let params = TessellationParams::default();
    let cache = EdgeSampleCache::new(&params);
    let mut mesh = TriangleMesh::new();
    tessellate_face(face, &model, &params, &cache, &mut mesh);
    assert!(!mesh.triangles.is_empty(), "the wall meshes");
    let mut inward = 0;
    for t in &mesh.triangles {
        let (a, b, c) = (
            mesh.vertices[t[0] as usize].position,
            mesh.vertices[t[1] as usize].position,
            mesh.vertices[t[2] as usize].position,
        );
        let n = (b - a).cross(&(c - a));
        let m = (a + b.to_vec() + c.to_vec()) * (1.0 / 3.0);
        // STEP says this wall faces away from the axis.
        if n.dot(&geometry_engine::math::Vector3::new(m.x, m.y, 0.0)) < 0.0 {
            inward += 1;
        }
    }
    assert_eq!(
        inward,
        0,
        "{inward} of {} triangles face the axis",
        mesh.triangles.len()
    );
}

/// A Backward revolution patch whose two rims have DIFFERENT radii -- a
/// conical band, as a countersink or chamfer wall is written -- so its rims
/// carry different sample counts and it meshes through the unequal-count
/// branch. That branch took the face's outward side from its boundary's
/// winding times the face sense, reading the loop as wound about the
/// SURFACE normal; the kernel winds it about the face's OUTWARD normal, so
/// every Backward band came out inside out (four countersink walls of one
/// CADGenBench part, against every neighbour).
#[test]
fn a_backward_conical_revolution_band_meshes_facing_the_axis() {
    use geometry_engine::tessellation::{
        edge_cache::EdgeSampleCache, tessellate_face, TessellationParams, TriangleMesh,
    };
    let body = "#1=CARTESIAN_POINT('',(10.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#3=DIRECTION('',(-0.847998304005088,0.,0.529998940003180));
#4=VECTOR('',#3,1.);
#9=LINE('',#1,#4);
#5=CARTESIAN_POINT('',(0.,0.,0.));
#6=AXIS1_PLACEMENT('',#5,#2);
#7=SURFACE_OF_REVOLUTION('',#9,#6);
#11=CARTESIAN_POINT('',(10.,0.,0.));
#12=CARTESIAN_POINT('',(0.,10.,0.));
#13=CARTESIAN_POINT('',(0.,2.,5.));
#14=CARTESIAN_POINT('',(2.,0.,5.));
#21=VERTEX_POINT('',#11);
#22=VERTEX_POINT('',#12);
#23=VERTEX_POINT('',#13);
#24=VERTEX_POINT('',#14);
#30=DIRECTION('',(1.,0.,0.));
#31=AXIS2_PLACEMENT_3D('',#5,#2,#30);
#32=CIRCLE('',#31,10.);
#33=CARTESIAN_POINT('',(0.,0.,5.));
#34=AXIS2_PLACEMENT_3D('',#33,#2,#30);
#35=CIRCLE('',#34,2.);
#36=CARTESIAN_POINT('',(0.,10.,0.));
#37=DIRECTION('',(0.,-0.847998304005088,0.529998940003180));
#40=VECTOR('',#37,1.);
#38=LINE('',#36,#40);
#51=EDGE_CURVE('',#21,#22,#32,.T.);
#52=EDGE_CURVE('',#22,#23,#38,.T.);
#53=EDGE_CURVE('',#24,#23,#35,.T.);
#54=EDGE_CURVE('',#21,#24,#9,.T.);
#61=ORIENTED_EDGE('',*,*,#51,.T.);
#62=ORIENTED_EDGE('',*,*,#52,.T.);
#63=ORIENTED_EDGE('',*,*,#53,.F.);
#64=ORIENTED_EDGE('',*,*,#54,.F.);
#70=EDGE_LOOP('',(#61,#62,#63,#64));
#71=FACE_BOUND('',#70,.F.);
#80=ADVANCED_FACE('',(#71),#7,.F.);
#90=OPEN_SHELL('',(#80));
#91=SHELL_BASED_SURFACE_MODEL('',(#90));";
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(body), "cone")
        .expect("imports");
    let faces: Vec<_> = model.faces.iter().collect();
    assert_eq!(faces.len(), 1, "one face: {:#?}", report.warnings);
    let (_, face) = faces[0];
    let params = TessellationParams::default();
    let cache = EdgeSampleCache::new(&params);
    let mut mesh = TriangleMesh::new();
    tessellate_face(face, &model, &params, &cache, &mut mesh);
    assert!(!mesh.triangles.is_empty(), "the band meshes");
    let mut away = 0;
    for t in &mesh.triangles {
        let (a, b, c) = (
            mesh.vertices[t[0] as usize].position,
            mesh.vertices[t[1] as usize].position,
            mesh.vertices[t[2] as usize].position,
        );
        let n = (b - a).cross(&(c - a));
        let m = (a + b.to_vec() + c.to_vec()) * (1.0 / 3.0);
        // Backward: this wall faces the axis.
        if n.dot(&geometry_engine::math::Vector3::new(m.x, m.y, 0.0)) > 0.0 {
            away += 1;
        }
    }
    assert_eq!(
        away,
        0,
        "{away} of {} triangles face away from the axis",
        mesh.triangles.len()
    );
}

/// A revolution of a CIRCLE (radius 5, centred `d` from the axis): a
/// torus-like, doubly curved wall. The patch runs from the profile's outer
/// equator (radius d + 5) up to its top (radius d), a quarter turn round
/// the axis. Its rims have different radii, so they can carry different
/// sample counts.
fn bulge_band(d: f64) -> String {
    let (ro, ri) = (d + 5.0, d);
    format!(
        "#5=CARTESIAN_POINT('',(0.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#30=DIRECTION('',(1.,0.,0.));
#6=AXIS1_PLACEMENT('',#5,#2);
#100=CARTESIAN_POINT('',({ri:.3},0.,0.));
#101=DIRECTION('',(0.,-1.,0.));
#102=AXIS2_PLACEMENT_3D('',#100,#101,#30);
#103=CIRCLE('',#102,5.);
#7=SURFACE_OF_REVOLUTION('',#103,#6);
#11=CARTESIAN_POINT('',({ro:.3},0.,0.));
#12=CARTESIAN_POINT('',(0.,{ro:.3},0.));
#13=CARTESIAN_POINT('',(0.,{ri:.3},5.));
#14=CARTESIAN_POINT('',({ri:.3},0.,5.));
#21=VERTEX_POINT('',#11);
#22=VERTEX_POINT('',#12);
#23=VERTEX_POINT('',#13);
#24=VERTEX_POINT('',#14);
#31=AXIS2_PLACEMENT_3D('',#5,#2,#30);
#32=CIRCLE('',#31,{ro:.3});
#33=CARTESIAN_POINT('',(0.,0.,5.));
#34=AXIS2_PLACEMENT_3D('',#33,#2,#30);
#35=CIRCLE('',#34,{ri:.3});
#110=CARTESIAN_POINT('',(0.,{ri:.3},0.));
#111=DIRECTION('',(0.,1.,0.));
#112=AXIS2_PLACEMENT_3D('',#110,#30,#111);
#38=CIRCLE('',#112,5.);
#51=EDGE_CURVE('',#21,#22,#32,.T.);
#52=EDGE_CURVE('',#22,#23,#38,.T.);
#53=EDGE_CURVE('',#24,#23,#35,.T.);
#54=EDGE_CURVE('',#21,#24,#103,.T.);
#61=ORIENTED_EDGE('',*,*,#51,.T.);
#62=ORIENTED_EDGE('',*,*,#52,.T.);
#63=ORIENTED_EDGE('',*,*,#53,.F.);
#64=ORIENTED_EDGE('',*,*,#54,.F.);
#70=EDGE_LOOP('',(#61,#62,#63,#64));
#71=FACE_OUTER_BOUND('',#70,.T.);
#80=ADVANCED_FACE('',(#71),#7,.T.);
#90=OPEN_SHELL('',(#80));
#91=SHELL_BASED_SURFACE_MODEL('',(#90));"
    )
}

/// That band, at several sizes. Where its two rims get different sample
/// counts it reaches the unequal-count branch -- which triangulated the
/// BOUNDARY ONLY, right for a cone or cylinder band (straight meridians) but
/// not a curved one: its triangles cut straight across the wall, standing up
/// to 89 degrees off the surface and through neighbouring faces (CADGenBench
/// part 104: two such faces crossed a torus).
#[test]
fn a_doubly_curved_revolution_band_meshes_on_its_surface() {
    use geometry_engine::tessellation::{
        edge_cache::EdgeSampleCache, tessellate_face, TessellationParams, TriangleMesh,
    };
    let params = TessellationParams::default();
    let mut unequal = 0;
    let mut bad = Vec::new();
    for d in [6.0_f64, 8.0, 12.0, 20.0, 40.0, 80.0] {
        let (model, report) = ExportEngine::new()
            .import_step_content(&envelope(&bulge_band(d)), "bulge")
            .expect("imports");
        let faces: Vec<_> = model.faces.iter().collect();
        assert_eq!(faces.len(), 1, "one face: {:#?}", report.warnings);
        let (_, face) = faces[0];
        let cache = EdgeSampleCache::new(&params);
        // The rims are the loop's 1st and 3rd edges.
        let lp = model.loops.get(face.outer_loop).expect("loop");
        let counts: Vec<usize> = lp
            .edges
            .iter()
            .map(|&e| cache.get_or_compute(e, &model).len())
            .collect();
        if counts[0] != counts[2] || counts[1] != counts[3] {
            unequal += 1;
        }
        let mut mesh = TriangleMesh::new();
        tessellate_face(face, &model, &params, &cache, &mut mesh);
        assert!(!mesh.triangles.is_empty(), "d {d}: the band meshes");
        // The wall's true normal: away from the profile circle's centre,
        // which turns with the point round the axis at radius d.
        let mut worst = 0.0_f64;
        for t in &mesh.triangles {
            let [a, b, c] = t.map(|i| mesh.vertices[i as usize].position);
            let m = (a + b.to_vec() + c.to_vec()) * (1.0 / 3.0);
            let rad = (m.x * m.x + m.y * m.y).sqrt();
            let core = geometry_engine::math::Point3::new(m.x * d / rad, m.y * d / rad, 0.0);
            let n = (m - core).normalize().expect("off the core");
            let Ok(w) = (b - a).cross(&(c - a)).normalize() else {
                continue;
            };
            worst = worst.max(w.dot(&n).abs().clamp(-1.0, 1.0).acos().to_degrees());
        }
        if worst > 40.0 {
            bad.push((d, counts, worst, mesh.triangles.len()));
        }
    }
    assert!(
        unequal > 0,
        "the fixture never reaches the unequal-count branch"
    );
    assert!(
        bad.is_empty(),
        "facets standing off the wall (d, rim counts, worst deg, tris): {bad:?}"
    );
}

/// A four-sided patch of the radius-10 sphere, built as a revolution of a
/// semicircle about +Z (an exact rational quadratic on the x > 0 side, so
/// every point of the sphere has one parameter pair), between the meridians at 0 and 90 degrees. Its other
/// two sides are NOT rings: each is the sphere's circle through
/// (8, 0, -+6), (0, 8, -+6) and a point at 45 degrees `bow` nearer the
/// equator, so it rises and falls round the axis. (CADGenBench part 146 has
/// such patches: no side keeps one height on the axis.)
fn tilted_rim_patch(bow_bottom: f64, bow_top: f64) -> String {
    use geometry_engine::math::{Point3, Vector3};
    let r = 10.0_f64;
    let h = 6.0_f64;
    let x0 = (r * r - h * h).sqrt();
    // The sphere's circle through a, b and the point at 45 degrees and
    // height z45: (centre, axis making a -> mid -> b counter-clockwise,
    // radius).
    let circle = |a: Point3, b: Point3, z45: f64| {
        let rho = (r * r - z45 * z45).sqrt();
        let m = Point3::new(rho / 2f64.sqrt(), rho / 2f64.sqrt(), z45);
        let n = (m - a).cross(&(b - m)).normalize().expect("a plane");
        // The plane's point nearest the sphere's centre (the origin).
        let c = n * n.dot(&(a - Point3::new(0.0, 0.0, 0.0)));
        let centre = Point3::new(c.x, c.y, c.z);
        let radius = (a - centre).magnitude();
        (centre, n, radius)
    };
    let a = Point3::new(x0, 0.0, -h);
    let b = Point3::new(0.0, x0, -h);
    let c = Point3::new(0.0, x0, h);
    let d = Point3::new(x0, 0.0, h);
    let (cb, nb, rb) = circle(a, b, -h + bow_bottom);
    let (ct, nt, rt) = circle(d, c, h - bow_top);
    let xb = (a - cb).normalize().expect("ref");
    let xt = (d - ct).normalize().expect("ref");
    let p = |v: Point3| format!("({:.17},{:.17},{:.17})", v.x, v.y, v.z);
    let q = |v: Vector3| format!("({:.17},{:.17},{:.17})", v.x, v.y, v.z);
    format!(
        "#1=CARTESIAN_POINT('',(0.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#3=DIRECTION('',(1.,0.,0.));
#4=CARTESIAN_POINT('',(0.,0.,-{r:.1}));
#5=CARTESIAN_POINT('',({r:.1},0.,-{r:.1}));
#9=CARTESIAN_POINT('',({r:.1},0.,0.));
#10=CARTESIAN_POINT('',({r:.1},0.,{r:.1}));
#15=CARTESIAN_POINT('',(0.,0.,{r:.1}));
#6=( BOUNDED_CURVE() B_SPLINE_CURVE(2,(#4,#5,#9,#10,#15),.UNSPECIFIED.,.F.,.F.)
 B_SPLINE_CURVE_WITH_KNOTS((3,2,3),(0.,0.5,1.),.UNSPECIFIED.) CURVE()
 GEOMETRIC_REPRESENTATION_ITEM()
 RATIONAL_B_SPLINE_CURVE((1.,{w:.17},1.,{w:.17},1.)) REPRESENTATION_ITEM('') );
#7=AXIS1_PLACEMENT('',#1,#2);
#8=SURFACE_OF_REVOLUTION('',#6,#7);
#11=CARTESIAN_POINT('',{pa});
#12=CARTESIAN_POINT('',{pb});
#13=CARTESIAN_POINT('',{pc});
#14=CARTESIAN_POINT('',{pd});
#21=VERTEX_POINT('',#11);
#22=VERTEX_POINT('',#12);
#23=VERTEX_POINT('',#13);
#24=VERTEX_POINT('',#14);
#30=CARTESIAN_POINT('',{pcb});
#31=DIRECTION('',{qnb});
#32=DIRECTION('',{qxb});
#33=AXIS2_PLACEMENT_3D('',#30,#31,#32);
#34=CIRCLE('',#33,{rb:.17});
#35=CARTESIAN_POINT('',{pct});
#36=DIRECTION('',{qnt});
#37=DIRECTION('',{qxt});
#38=AXIS2_PLACEMENT_3D('',#35,#36,#37);
#39=CIRCLE('',#38,{rt:.17});
#40=DIRECTION('',(1.,0.,0.));
#41=DIRECTION('',(0.,1.,0.));
#42=DIRECTION('',(0.,-1.,0.));
#43=DIRECTION('',(0.8,0.,-0.6));
#44=DIRECTION('',(0.,0.8,-0.6));
#45=AXIS2_PLACEMENT_3D('',#1,#40,#44);
#46=CIRCLE('',#45,{r:.1});
#47=AXIS2_PLACEMENT_3D('',#1,#42,#43);
#48=CIRCLE('',#47,{r:.1});
#51=EDGE_CURVE('',#21,#22,#34,.T.);
#52=EDGE_CURVE('',#22,#23,#46,.T.);
#53=EDGE_CURVE('',#24,#23,#39,.T.);
#54=EDGE_CURVE('',#21,#24,#48,.T.);
#61=ORIENTED_EDGE('',*,*,#51,.T.);
#62=ORIENTED_EDGE('',*,*,#52,.T.);
#63=ORIENTED_EDGE('',*,*,#53,.F.);
#64=ORIENTED_EDGE('',*,*,#54,.F.);
#70=EDGE_LOOP('',(#61,#62,#63,#64));
#71=FACE_OUTER_BOUND('',#70,.T.);
#80=ADVANCED_FACE('',(#71),#8,.T.);
#90=OPEN_SHELL('',(#80));
#91=SHELL_BASED_SURFACE_MODEL('',(#90));",
        pa = p(a),
        pb = p(b),
        pc = p(c),
        pd = p(d),
        pcb = p(cb),
        qnb = q(nb),
        qxb = q(xb),
        pct = p(ct),
        qnt = q(nt),
        qxt = q(xt),
        w = 0.5_f64.sqrt(),
    )
}

/// The revolution meshers fill a four-sided patch by turning one side about
/// the axis, which is exact only when the other two sides are rings. On a
/// patch whose sides rise and fall round the axis the turned side passes
/// outside the face, and its cells fold back over the rims: on CADGenBench
/// part 146 two such faces wound against themselves along 26 mesh edges and
/// against their neighbours along 11 more. Every facet must lie along the
/// sphere and the face must wind one way throughout.
#[test]
fn a_revolution_patch_whose_sides_are_not_rings_meshes_on_its_surface() {
    use geometry_engine::tessellation::{
        edge_cache::EdgeSampleCache, tessellate_face, TessellationParams, TriangleMesh,
    };
    use std::collections::HashMap;
    let params = TessellationParams::default();
    let mut bad = Vec::new();
    for (bow_bottom, bow_top) in [(1.5_f64, 1.5_f64), (1.5, 0.5)] {
        let (model, report) = ExportEngine::new()
            .import_step_content(&envelope(&tilted_rim_patch(bow_bottom, bow_top)), "patch")
            .expect("imports");
        let faces: Vec<_> = model.faces.iter().collect();
        assert_eq!(faces.len(), 1, "one face: {:#?}", report.warnings);
        let (_, face) = faces[0];
        let cache = EdgeSampleCache::new(&params);
        let mut mesh = TriangleMesh::new();
        tessellate_face(face, &model, &params, &cache, &mut mesh);
        assert!(!mesh.triangles.is_empty(), "the patch meshes");
        let key = |p: geometry_engine::math::Point3| {
            (
                (p.x * 1e6).round() as i64,
                (p.y * 1e6).round() as i64,
                (p.z * 1e6).round() as i64,
            )
        };
        let mut directed: HashMap<_, usize> = HashMap::new();
        let mut worst = 0.0_f64;
        let mut worst_facet = None;
        for t in &mesh.triangles {
            let [a, b, c] = t.map(|i| mesh.vertices[i as usize].position);
            for (u, v) in [(a, b), (b, c), (c, a)] {
                *directed.entry((key(u), key(v))).or_default() += 1;
            }
            let Ok(w) = (b - a).cross(&(c - a)).normalize() else {
                continue;
            };
            let m = (a + b.to_vec() + c.to_vec()) * (1.0 / 3.0);
            let n = (m - geometry_engine::math::Point3::new(0.0, 0.0, 0.0))
                .normalize()
                .expect("off the centre");
            let dev = w.dot(&n).abs().clamp(0.0, 1.0).acos().to_degrees();
            if dev > worst {
                worst = dev;
                worst_facet = Some([a, b, c]);
            }
        }
        let same_way = directed.values().filter(|&&n| n > 1).count();
        if same_way > 0 || worst > 40.0 {
            bad.push((bow_bottom, bow_top, same_way, worst, worst_facet));
        }
    }
    assert!(
        bad.is_empty(),
        "(bow bottom, bow top, mesh edges walked twice the same way, worst facet deg, its corners): {bad:?}"
    );
}

/// An octant of a radius-3 ball about the origin, written as a revolution:
/// the profile is the quarter arc from the equator (3,0,0) to the pole
/// (0,0,3), where it meets the axis -- as the cubic that stands within
/// 0.001 of that arc. The patch is three-sided: the equator ring from
/// (3,0,0) to (0,3,0) and two meridians up to the pole.
/// `outward` picks which side the face shows: away from the centre (a ball
/// corner) or toward it (a ball-ended pocket).
fn ball_octant(outward: bool) -> String {
    let (bound, sense) = if outward {
        ("FACE_OUTER_BOUND('',#70,.T.)", ".T.")
    } else {
        ("FACE_BOUND('',#70,.F.)", ".F.")
    };
    format!(
        "#5=CARTESIAN_POINT('',(0.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#30=DIRECTION('',(1.,0.,0.));
#41=CARTESIAN_POINT('',(3.,0.,1.6568542495));
#42=CARTESIAN_POINT('',(1.6568542495,0.,3.));
#4=B_SPLINE_CURVE_WITH_KNOTS('',3,(#11,#41,#42,#13),.UNSPECIFIED.,.F.,.F.,(4,4),(0.,1.),.UNSPECIFIED.);
#6=AXIS1_PLACEMENT('',#5,#2);
#7=SURFACE_OF_REVOLUTION('',#4,#6);
#11=CARTESIAN_POINT('',(3.,0.,0.));
#12=CARTESIAN_POINT('',(0.,3.,0.));
#13=CARTESIAN_POINT('',(0.,0.,3.));
#21=VERTEX_POINT('',#11);
#22=VERTEX_POINT('',#12);
#23=VERTEX_POINT('',#13);
#31=AXIS2_PLACEMENT_3D('',#5,#2,#30);
#32=CIRCLE('',#31,3.);
#43=CARTESIAN_POINT('',(0.,3.,1.6568542495));
#45=CARTESIAN_POINT('',(0.,1.6568542495,3.));
#44=B_SPLINE_CURVE_WITH_KNOTS('',3,(#12,#43,#45,#13),.UNSPECIFIED.,.F.,.F.,(4,4),(0.,1.),.UNSPECIFIED.);
#51=EDGE_CURVE('',#21,#22,#32,.T.);
#52=EDGE_CURVE('',#22,#23,#44,.T.);
#53=EDGE_CURVE('',#21,#23,#4,.T.);
#61=ORIENTED_EDGE('',*,*,#51,.T.);
#62=ORIENTED_EDGE('',*,*,#52,.T.);
#63=ORIENTED_EDGE('',*,*,#53,.F.);
#70=EDGE_LOOP('',(#61,#62,#63));
#71={bound};
#80=ADVANCED_FACE('',(#71),#7,{sense});
#90=OPEN_SHELL('',(#80));
#91=SHELL_BASED_SURFACE_MODEL('',(#90));"
    )
}

/// How a mesh of a patch of the radius-`r` ball about the origin sits on it:
/// (facets, facets showing their back to the side asked for, mesh edges
/// walked the same way by two facets, the farthest a facet's centre is from
/// the ball).
fn ball_patch_mesh_report(
    mesh: &geometry_engine::tessellation::TriangleMesh,
    r: f64,
    outward: bool,
) -> (usize, usize, usize, f64) {
    use std::collections::HashMap;
    let key = |p: geometry_engine::math::Point3| {
        (
            (p.x * 1e6).round() as i64,
            (p.y * 1e6).round() as i64,
            (p.z * 1e6).round() as i64,
        )
    };
    let mut directed: HashMap<_, usize> = HashMap::new();
    let (mut wrong_side, mut farthest) = (0usize, 0.0_f64);
    for t in &mesh.triangles {
        let [a, b, c] = t.map(|i| mesh.vertices[i as usize].position);
        for (p, q) in [(a, b), (b, c), (c, a)] {
            *directed.entry((key(p), key(q))).or_default() += 1;
        }
        let m = (a + b.to_vec() + c.to_vec()) * (1.0 / 3.0);
        let from_centre = m.to_vec();
        let facing = (b - a).cross(&(c - a)).dot(&from_centre);
        if (facing > 0.0) != outward {
            wrong_side += 1;
        }
        farthest = farthest.max((from_centre.magnitude() - r).abs());
    }
    let same_way = directed.values().filter(|&&n| n > 1).count();
    (mesh.triangles.len(), wrong_side, same_way, farthest)
}

/// A wide pole patch of a revolution -- an eighth of a ball -- meshes ON
/// the ball and faces the side the file says. The three-sided pole path
/// filled the patch from its boundary samples alone, which suits a thin
/// wedge of a kernel revolve but cuts a flat sheet across a quarter-turn
/// patch (its middle 1.27 from a radius-3 ball), and it read the outward
/// side as "boundary winding x sense", which turns every Backward patch
/// inside out. Seven ball-ended pocket corners of CADGenBench part 118
/// were wound against all their neighbours and stood up to 177 degrees
/// off their surface.
#[test]
fn a_ball_octant_written_as_a_revolution_meshes_on_the_ball_facing_out() {
    use geometry_engine::tessellation::{
        edge_cache::EdgeSampleCache, tessellate_face, TessellationParams, TriangleMesh,
    };
    let params = TessellationParams::default();
    let mut bad = Vec::new();
    for outward in [true, false] {
        let (model, report) = ExportEngine::new()
            .import_step_content(&envelope(&ball_octant(outward)), "octant")
            .expect("imports");
        let faces: Vec<_> = model.faces.iter().collect();
        assert_eq!(faces.len(), 1, "one face: {:#?}", report.warnings);
        let (_, face) = faces[0];
        let cache = EdgeSampleCache::new(&params);
        let mut mesh = TriangleMesh::new();
        tessellate_face(face, &model, &params, &cache, &mut mesh);
        let (facets, wrong_side, same_way, farthest) = ball_patch_mesh_report(&mesh, 3.0, outward);
        if facets == 0 || wrong_side > 0 || same_way > 0 || farthest > 0.05 {
            bad.push((outward, facets, wrong_side, same_way, farthest));
        }
    }
    assert!(
        bad.is_empty(),
        "(faces outward, facets, facets showing their back, mesh edges walked twice the same way, farthest facet centre from the ball): {bad:?}"
    );
}

/// A dome: the quarter arc from the equator (3,0,0) to the pole (0,0,3)
/// revolved a full turn, bounded by the one closed equator circle. Written
/// `outward` it is a ball's cap, otherwise the round bottom of a blind hole.
fn dome(outward: bool) -> String {
    let (bound, sense) = if outward {
        ("FACE_OUTER_BOUND('',#70,.T.)", ".T.")
    } else {
        ("FACE_BOUND('',#70,.F.)", ".F.")
    };
    format!(
        "#5=CARTESIAN_POINT('',(0.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#30=DIRECTION('',(1.,0.,0.));
#41=CARTESIAN_POINT('',(3.,0.,1.6568542495));
#42=CARTESIAN_POINT('',(1.6568542495,0.,3.));
#4=B_SPLINE_CURVE_WITH_KNOTS('',3,(#11,#41,#42,#13),.UNSPECIFIED.,.F.,.F.,(4,4),(0.,1.),.UNSPECIFIED.);
#6=AXIS1_PLACEMENT('',#5,#2);
#7=SURFACE_OF_REVOLUTION('',#4,#6);
#11=CARTESIAN_POINT('',(3.,0.,0.));
#13=CARTESIAN_POINT('',(0.,0.,3.));
#21=VERTEX_POINT('',#11);
#31=AXIS2_PLACEMENT_3D('',#5,#2,#30);
#32=CIRCLE('',#31,3.);
#51=EDGE_CURVE('',#21,#21,#32,.T.);
#61=ORIENTED_EDGE('',*,*,#51,.T.);
#70=EDGE_LOOP('',(#61));
#71={bound};
#80=ADVANCED_FACE('',(#71),#7,{sense});
#90=OPEN_SHELL('',(#80));
#91=SHELL_BASED_SURFACE_MODEL('',(#90));"
    )
}

/// A dome bounded by one closed rim meshes on its ball and faces the side
/// the file says, whichever that is. The full-pole fan wound every facet to
/// "rim winding x sense" -- one direction, along the axis, for a wall that
/// turns a quarter circle away from it, and the wrong side for a Backward
/// dome, whose rim the kernel winds about the face's OUTWARD normal.
#[test]
fn a_dome_written_as_a_revolution_meshes_facing_the_side_the_file_says() {
    use geometry_engine::tessellation::{
        edge_cache::EdgeSampleCache, tessellate_face, TessellationParams, TriangleMesh,
    };
    let params = TessellationParams::default();
    let mut bad = Vec::new();
    for outward in [true, false] {
        let (model, report) = ExportEngine::new()
            .import_step_content(&envelope(&dome(outward)), "dome")
            .expect("imports");
        let faces: Vec<_> = model.faces.iter().collect();
        assert_eq!(faces.len(), 1, "one face: {:#?}", report.warnings);
        let (_, face) = faces[0];
        let cache = EdgeSampleCache::new(&params);
        let mut mesh = TriangleMesh::new();
        tessellate_face(face, &model, &params, &cache, &mut mesh);
        let (facets, wrong_side, same_way, farthest) = ball_patch_mesh_report(&mesh, 3.0, outward);
        // A dome is the UPPER half: nothing may hang below the equator.
        let below = mesh
            .vertices
            .iter()
            .filter(|v| v.position.z < -1e-6)
            .count();
        if facets == 0 || wrong_side > 0 || same_way > 0 || farthest > 0.05 || below > 0 {
            bad.push((outward, facets, wrong_side, same_way, farthest, below));
        }
    }
    assert!(
        bad.is_empty(),
        "(faces outward, facets, facets showing their back, mesh edges walked twice the same way, farthest facet centre from the ball, vertices below the equator): {bad:?}"
    );
}

//// A three-sided patch of the radius-3 ball about the origin with NO corner
/// on the axis. The ball is the exact quarter arc from the equator (3,0,0)
/// to the pole (0,0,3), revolved about +Z. The patch runs a quarter of the
/// equator from (3,0,0) to (0,3,0), up that meridian to 45 degrees of
/// latitude, and back down a great circle. `outward` picks the side the
/// face shows: away from the centre or toward it.
fn ball_triangle_off_the_axis(outward: bool) -> String {
    let (bound, sense) = if outward {
        ("FACE_OUTER_BOUND('',#70,.T.)", ".T.")
    } else {
        ("FACE_BOUND('',#70,.F.)", ".F.")
    };
    format!(
        "#5=CARTESIAN_POINT('',(0.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#30=DIRECTION('',(1.,0.,0.));
#40=DIRECTION('',(0.,1.,0.));
#11=CARTESIAN_POINT('',(3.,0.,0.));
#12=CARTESIAN_POINT('',(0.,3.,0.));
#13=CARTESIAN_POINT('',(0.,2.12132034355964257,2.12132034355964257));
#14=CARTESIAN_POINT('',(3.,0.,3.));
#15=CARTESIAN_POINT('',(0.,0.,3.));
#4=( BOUNDED_CURVE() B_SPLINE_CURVE(2,(#11,#14,#15),.UNSPECIFIED.,.F.,.F.)
 B_SPLINE_CURVE_WITH_KNOTS((3,3),(0.,1.),.UNSPECIFIED.) CURVE()
 GEOMETRIC_REPRESENTATION_ITEM()
 RATIONAL_B_SPLINE_CURVE((1.,0.70710678118654752,1.)) REPRESENTATION_ITEM('') );
#6=AXIS1_PLACEMENT('',#5,#2);
#7=SURFACE_OF_REVOLUTION('',#4,#6);
#21=VERTEX_POINT('',#11);
#22=VERTEX_POINT('',#12);
#23=VERTEX_POINT('',#13);
#31=AXIS2_PLACEMENT_3D('',#5,#2,#30);
#32=CIRCLE('',#31,3.);
#43=AXIS2_PLACEMENT_3D('',#5,#30,#40);
#44=CIRCLE('',#43,3.);
#45=DIRECTION('',(0.,-0.70710678118654752,0.70710678118654752));
#46=AXIS2_PLACEMENT_3D('',#5,#45,#30);
#47=CIRCLE('',#46,3.);
#51=EDGE_CURVE('',#21,#22,#32,.T.);
#52=EDGE_CURVE('',#22,#23,#44,.T.);
#53=EDGE_CURVE('',#21,#23,#47,.T.);
#61=ORIENTED_EDGE('',*,*,#51,.T.);
#62=ORIENTED_EDGE('',*,*,#52,.T.);
#63=ORIENTED_EDGE('',*,*,#53,.F.);
#70=EDGE_LOOP('',(#61,#62,#63));
#71={bound};
#80=ADVANCED_FACE('',(#71),#7,{sense});
#90=OPEN_SHELL('',(#80));
#91=SHELL_BASED_SURFACE_MODEL('',(#90));"
    )
}

/// A three-sided revolution patch that is NOT a pole wedge meshes on its
/// surface. The three-sided path was written for the wedge a kernel revolve
/// leaves at a pole and fills from the boundary samples alone; it took
/// every three-sided revolution face, and a wide patch away from the axis
/// came out as flat sheets cutting under the surface (one face of
/// CADGenBench part 127 stood 64.5 degrees off its surface).
#[test]
fn a_three_sided_revolution_patch_off_the_axis_meshes_on_its_surface() {
    use geometry_engine::tessellation::{
        edge_cache::EdgeSampleCache, tessellate_face, TessellationParams, TriangleMesh,
    };
    let params = TessellationParams::default();
    let mut bad = Vec::new();
    for outward in [true, false] {
        let (model, report) = ExportEngine::new()
            .import_step_content(&envelope(&ball_triangle_off_the_axis(outward)), "patch")
            .expect("imports");
        let faces: Vec<_> = model.faces.iter().collect();
        assert_eq!(faces.len(), 1, "one face: {:#?}", report.warnings);
        let (_, face) = faces[0];
        let cache = EdgeSampleCache::new(&params);
        let mut mesh = TriangleMesh::new();
        tessellate_face(face, &model, &params, &cache, &mut mesh);
        let (facets, wrong_side, same_way, farthest) = ball_patch_mesh_report(&mesh, 3.0, outward);
        if facets == 0 || wrong_side > 0 || same_way > 0 || farthest > 0.05 {
            bad.push((outward, facets, wrong_side, same_way, farthest));
        }
    }
    assert!(
        bad.is_empty(),
        "(faces outward, facets, facets showing their back, mesh edges walked twice the same way, farthest facet centre from the ball): {bad:?}"
    );
}

/// A three-sided sliver of a radius-100 wall about +Z (a revolved LINE),
/// so slight a turn (0.06 rad) that it stays within 0.05 of a plane: a
/// stretch of the bottom ring from (100,0,0), the straight meridian 5 up
/// from the ring's end, and back down the slanted cut of a plane through
/// the axis' foot (an ellipse). `outward` picks the side the face shows:
/// away from the axis or toward it.
fn nearly_flat_wall_sliver(outward: bool) -> String {
    let (bound, sense) = if outward {
        ("FACE_OUTER_BOUND('',#70,.T.)", ".T.")
    } else {
        ("FACE_BOUND('',#70,.F.)", ".F.")
    };
    let turn = 0.06_f64;
    let (bx, by) = (100.0 * turn.cos(), 100.0 * turn.sin());
    // The cut plane z = m y through (100,0,0) and (bx,by,5): an ellipse
    // about the origin with semi-axes 100 along +X and 100 sqrt(1 + m^2)
    // along (0,1,m).
    let m = 5.0 / by;
    let len = (1.0 + m * m).sqrt();
    let (yy, yz) = (1.0 / len, m / len);
    format!(
        "#1=CARTESIAN_POINT('',(100.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#3=VECTOR('',#2,1.);
#4=LINE('',#1,#3);
#5=CARTESIAN_POINT('',(0.,0.,0.));
#6=AXIS1_PLACEMENT('',#5,#2);
#7=SURFACE_OF_REVOLUTION('',#4,#6);
#12=CARTESIAN_POINT('',({bx:.17},{by:.17},0.));
#13=CARTESIAN_POINT('',({bx:.17},{by:.17},5.));
#21=VERTEX_POINT('',#1);
#22=VERTEX_POINT('',#12);
#23=VERTEX_POINT('',#13);
#30=DIRECTION('',(1.,0.,0.));
#31=AXIS2_PLACEMENT_3D('',#5,#2,#30);
#32=CIRCLE('',#31,100.);
#37=VECTOR('',#2,1.);
#38=LINE('',#12,#37);
#41=DIRECTION('',(0.,{nz:.17},{ny:.17}));
#43=AXIS2_PLACEMENT_3D('',#5,#41,#30);
#44=ELLIPSE('',#43,100.,{major:.17});
#51=EDGE_CURVE('',#21,#22,#32,.T.);
#52=EDGE_CURVE('',#22,#23,#38,.T.);
#53=EDGE_CURVE('',#21,#23,#44,.T.);
#61=ORIENTED_EDGE('',*,*,#51,.T.);
#62=ORIENTED_EDGE('',*,*,#52,.T.);
#63=ORIENTED_EDGE('',*,*,#53,.F.);
#70=EDGE_LOOP('',(#61,#62,#63));
#71={bound};
#80=ADVANCED_FACE('',(#71),#7,{sense});
#90=OPEN_SHELL('',(#80));
#91=SHELL_BASED_SURFACE_MODEL('',(#90));",
        nz = -yz,
        ny = yy,
        major = 100.0 * len,
    )
}

/// A revolution patch flat enough to mesh as a plane (at the certificate's
/// 0.1 chord) still faces the side the file says. The planar mesher took
/// the outward side of every non-Plane face as "loop winding x sense",
/// reading the loop as wound about the SURFACE normal; an imported loop is
/// wound about the face's OUTWARD normal, so each Backward patch came out
/// inside out (five nearly flat revolution faces of CADGenBench part 127,
/// wound against all their neighbours).
#[test]
fn a_nearly_flat_revolution_patch_meshes_facing_the_side_the_file_says() {
    use geometry_engine::tessellation::{
        edge_cache::EdgeSampleCache, tessellate_face, TessellationParams, TriangleMesh,
    };
    let params = TessellationParams {
        chord_tolerance: 0.1,
        ..TessellationParams::default()
    };
    let mut bad = Vec::new();
    for outward in [true, false] {
        let (model, report) = ExportEngine::new()
            .import_step_content(&envelope(&nearly_flat_wall_sliver(outward)), "sliver")
            .expect("imports");
        let faces: Vec<_> = model.faces.iter().collect();
        assert_eq!(faces.len(), 1, "one face: {:#?}", report.warnings);
        let (_, face) = faces[0];
        let cache = EdgeSampleCache::new(&params);
        let mut mesh = TriangleMesh::new();
        tessellate_face(face, &model, &params, &cache, &mut mesh);
        let mut wrong_side = 0;
        for t in &mesh.triangles {
            let [a, b, c] = t.map(|i| mesh.vertices[i as usize].position);
            let m = (a + b.to_vec() + c.to_vec()) * (1.0 / 3.0);
            let from_axis = geometry_engine::math::Vector3::new(m.x, m.y, 0.0);
            if ((b - a).cross(&(c - a)).dot(&from_axis) > 0.0) != outward {
                wrong_side += 1;
            }
        }
        if mesh.triangles.is_empty() || wrong_side > 0 {
            bad.push((outward, mesh.triangles.len(), wrong_side));
        }
    }
    assert!(
        bad.is_empty(),
        "(faces outward, facets, facets showing their back): {bad:?}"
    );
}

/// A small three-sided patch of the radius-15 ball about the origin, by its
/// equator: 0.1137 rad of the equator from (15,0,0), up that meridian by
/// the same angle, and back down a great circle. It is 1.7 across and bows
/// 0.024 -- flat to within a 0.1 chord, with every edge bowing OUT of the
/// plane it is flat to. The ball is the exact quarter arc from the equator
/// to the pole, revolved about +Z.
fn slightly_bowed_ball_triangle(outward: bool) -> String {
    use geometry_engine::math::Vector3;
    let (bound, sense) = if outward {
        ("FACE_OUTER_BOUND('',#70,.T.)", ".T.")
    } else {
        ("FACE_BOUND('',#70,.F.)", ".F.")
    };
    let (r, turn) = (15.0_f64, 0.1137_f64);
    let b_hat = Vector3::new(turn.cos(), turn.sin(), 0.0);
    let b = b_hat * r;
    let c = (b_hat * turn.cos() + Vector3::Z * turn.sin()) * r;
    // The meridian through b: counter-clockwise about b_hat x Z from b_hat.
    let meridian_normal = b_hat.cross(&Vector3::Z);
    // The great circle from (r,0,0) to c: counter-clockwise about X x c.
    let great_normal = Vector3::X
        .cross(&c)
        .normalize()
        .expect("c is off the X axis");
    let p = |v: Vector3| format!("({:.17},{:.17},{:.17})", v.x, v.y, v.z);
    format!(
        "#5=CARTESIAN_POINT('',(0.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#30=DIRECTION('',(1.,0.,0.));
#11=CARTESIAN_POINT('',(15.,0.,0.));
#12=CARTESIAN_POINT('',{pb});
#13=CARTESIAN_POINT('',{pc});
#14=CARTESIAN_POINT('',(15.,0.,15.));
#15=CARTESIAN_POINT('',(0.,0.,15.));
#4=( BOUNDED_CURVE() B_SPLINE_CURVE(2,(#11,#14,#15),.UNSPECIFIED.,.F.,.F.)
 B_SPLINE_CURVE_WITH_KNOTS((3,3),(0.,1.),.UNSPECIFIED.) CURVE()
 GEOMETRIC_REPRESENTATION_ITEM()
 RATIONAL_B_SPLINE_CURVE((1.,0.70710678118654752,1.)) REPRESENTATION_ITEM('') );
#6=AXIS1_PLACEMENT('',#5,#2);
#7=SURFACE_OF_REVOLUTION('',#4,#6);
#21=VERTEX_POINT('',#11);
#22=VERTEX_POINT('',#12);
#23=VERTEX_POINT('',#13);
#31=AXIS2_PLACEMENT_3D('',#5,#2,#30);
#32=CIRCLE('',#31,15.);
#40=DIRECTION('',{pbh});
#41=DIRECTION('',{pmn});
#43=AXIS2_PLACEMENT_3D('',#5,#41,#40);
#44=CIRCLE('',#43,15.);
#45=DIRECTION('',{pgn});
#46=AXIS2_PLACEMENT_3D('',#5,#45,#30);
#47=CIRCLE('',#46,15.);
#51=EDGE_CURVE('',#21,#22,#32,.T.);
#52=EDGE_CURVE('',#22,#23,#44,.T.);
#53=EDGE_CURVE('',#21,#23,#47,.T.);
#61=ORIENTED_EDGE('',*,*,#51,.T.);
#62=ORIENTED_EDGE('',*,*,#52,.T.);
#63=ORIENTED_EDGE('',*,*,#53,.F.);
#70=EDGE_LOOP('',(#61,#62,#63));
#71={bound};
#80=ADVANCED_FACE('',(#71),#7,{sense});
#90=OPEN_SHELL('',(#80));
#91=SHELL_BASED_SURFACE_MODEL('',(#90));",
        pb = p(b),
        pc = p(c),
        pbh = p(b_hat),
        pmn = p(meridian_normal),
        pgn = p(great_normal),
    )
}

/// A curved patch whose LOOP is flat to within the chord tolerance has no
/// facet standing on edge to it. Such a patch is filled in its loop's
/// plane; an edge that bows out of that plane has its samples on one line
/// there, and the fill cut facets from three samples of the one edge --
/// standing in the edge's own plane, square to the face (two on a face of
/// CADGenBench part 127, 90 degrees off its surface).
#[test]
fn a_bowed_revolution_patch_with_a_flat_loop_has_no_facet_on_edge() {
    use geometry_engine::tessellation::{
        edge_cache::EdgeSampleCache, tessellate_face, TessellationParams, TriangleMesh,
    };
    let params = TessellationParams {
        chord_tolerance: 0.1,
        ..TessellationParams::default()
    };
    let mut bad = Vec::new();
    for outward in [true, false] {
        let (model, report) = ExportEngine::new()
            .import_step_content(&envelope(&slightly_bowed_ball_triangle(outward)), "patch")
            .expect("imports");
        let faces: Vec<_> = model.faces.iter().collect();
        assert_eq!(faces.len(), 1, "one face: {:#?}", report.warnings);
        let (_, face) = faces[0];
        let cache = EdgeSampleCache::new(&params);
        let mut mesh = TriangleMesh::new();
        tessellate_face(face, &model, &params, &cache, &mut mesh);
        let (facets, wrong_side, same_way, farthest) = ball_patch_mesh_report(&mesh, 15.0, outward);
        // The steepest a facet stands to the ball under it, in degrees.
        let mut steepest = 0.0_f64;
        for t in &mesh.triangles {
            let [a, b, c] = t.map(|i| mesh.vertices[i as usize].position);
            let g = (b - a).cross(&(c - a));
            if g.magnitude() < 1e-9 {
                continue;
            }
            let m = (a + b.to_vec() + c.to_vec()) * (1.0 / 3.0);
            let cos = g.dot(&m.to_vec()).abs() / (g.magnitude() * m.to_vec().magnitude());
            steepest = steepest.max(cos.clamp(0.0, 1.0).acos().to_degrees());
        }
        if facets == 0 || wrong_side > 0 || same_way > 0 || farthest > 0.1 || steepest > 45.0 {
            bad.push((outward, facets, wrong_side, same_way, farthest, steepest));
        }
    }
    assert!(
        bad.is_empty(),
        "(faces outward, facets, facets showing their back, mesh edges walked twice the same way, farthest facet centre from the ball, steepest facet deg): {bad:?}"
    );
}
