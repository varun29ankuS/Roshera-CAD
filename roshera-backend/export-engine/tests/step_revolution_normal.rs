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
