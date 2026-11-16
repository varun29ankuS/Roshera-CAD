// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! An edge is where its two faces meet. A STEP writer may still give it a
//! 3D curve that only approximates that intersection -- Open CASCADE writes
//! a rough companion curve and keeps the exact edge in pcurves and a per-edge
//! tolerance it does not export (CADGenBench part 231: plain cubic Beziers
//! standing for torus-plane cuts, 0.022 mm off the torus, exactly on the
//! plane). The importer rebuilds such an edge ON the intersection of its two
//! faces, so it lies on both, and reports the repair with the gap before and
//! after -- instead of inflating a tolerance around the file's curve.

use export_engine::ExportEngine;
use geometry_engine::math::{Point3, Tolerance};

fn envelope(body: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('cut'),'2;1');\n\
         FILE_NAME('cut.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n{body}\nENDSEC;\n\
         END-ISO-10303-21;\n"
    )
}

/// A quarter of the radius-20/2 torus about +Z between its outer equator and
/// the plane z = 1, and the annular sector of that plane out to radius 25.
/// They share the cut: the circle of radius rho = 20 + sqrt(3) at z = 1,
/// written as a cubic Bezier whose handles are `k` of the radius (the exact
/// quarter circle needs k = 0.5523; 0.5504 puts it ~0.02 mm off the torus).
fn torus_cut_by_a_plane(k: f64) -> String {
    torus_cut_at(1.0, k)
}

/// The same fixture with the plane at height `z` (0 < z <= 2): the cut is
/// the circle of radius 20 + sqrt(4 - z^2). At z = 2 the plane touches the
/// top of the tube -- a TANGENT cut, as where a fillet meets a flat face.
fn torus_cut_at(z: f64, k: f64) -> String {
    let rho = 20.0 + (4.0 - z * z).max(0.0).sqrt();
    format!(
        "#1=CARTESIAN_POINT('',(0.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#3=DIRECTION('',(1.,0.,0.));
#4=AXIS2_PLACEMENT_3D('',#1,#2,#3);
#5=TOROIDAL_SURFACE('',#4,20.,2.);
#6=CARTESIAN_POINT('',(0.,0.,{z:.15}));
#7=AXIS2_PLACEMENT_3D('',#6,#2,#3);
#8=PLANE('',#7);
#11=CARTESIAN_POINT('',(22.,0.,0.));
#12=CARTESIAN_POINT('',(0.,22.,0.));
#13=CARTESIAN_POINT('',({rho:.15},0.,{z:.15}));
#14=CARTESIAN_POINT('',(0.,{rho:.15},{z:.15}));
#15=CARTESIAN_POINT('',(25.,0.,{z:.15}));
#16=CARTESIAN_POINT('',(0.,25.,{z:.15}));
#21=VERTEX_POINT('',#11);
#22=VERTEX_POINT('',#12);
#23=VERTEX_POINT('',#13);
#24=VERTEX_POINT('',#14);
#25=VERTEX_POINT('',#15);
#26=VERTEX_POINT('',#16);
#31=CIRCLE('',#4,22.);
#32=CARTESIAN_POINT('',(20.,0.,0.));
#33=DIRECTION('',(0.,-1.,0.));
#34=AXIS2_PLACEMENT_3D('',#32,#33,#3);
#35=CIRCLE('',#34,2.);
#36=CARTESIAN_POINT('',(0.,20.,0.));
#37=DIRECTION('',(0.,1.,0.));
#38=AXIS2_PLACEMENT_3D('',#36,#3,#37);
#39=CIRCLE('',#38,2.);
#41=CARTESIAN_POINT('',({rho:.15},0.,{z:.15}));
#42=CARTESIAN_POINT('',({rho:.15},{kr:.15},{z:.15}));
#43=CARTESIAN_POINT('',({kr:.15},{rho:.15},{z:.15}));
#44=CARTESIAN_POINT('',(0.,{rho:.15},{z:.15}));
#45=B_SPLINE_CURVE_WITH_KNOTS('',3,(#41,#42,#43,#44),.UNSPECIFIED.,.F.,.F.,(4,4),(0.,1.),.UNSPECIFIED.);
#46=CIRCLE('',#7,25.);
#47=VECTOR('',#37,1.);
#48=LINE('',#14,#47);
#49=VECTOR('',#3,1.);
#50=LINE('',#13,#49);
#51=EDGE_CURVE('',#21,#22,#31,.T.);
#52=EDGE_CURVE('',#21,#23,#35,.T.);
#53=EDGE_CURVE('',#22,#24,#39,.T.);
#54=EDGE_CURVE('',#23,#24,#45,.T.);
#55=EDGE_CURVE('',#25,#26,#46,.T.);
#56=EDGE_CURVE('',#24,#26,#48,.T.);
#57=EDGE_CURVE('',#23,#25,#50,.T.);
#61=ORIENTED_EDGE('',*,*,#51,.T.);
#62=ORIENTED_EDGE('',*,*,#53,.T.);
#63=ORIENTED_EDGE('',*,*,#54,.F.);
#64=ORIENTED_EDGE('',*,*,#52,.F.);
#65=EDGE_LOOP('',(#61,#62,#63,#64));
#66=FACE_OUTER_BOUND('',#65,.T.);
#67=ADVANCED_FACE('',(#66),#5,.T.);
#71=ORIENTED_EDGE('',*,*,#54,.T.);
#72=ORIENTED_EDGE('',*,*,#56,.T.);
#73=ORIENTED_EDGE('',*,*,#55,.F.);
#74=ORIENTED_EDGE('',*,*,#57,.F.);
#75=EDGE_LOOP('',(#71,#72,#73,#74));
#76=FACE_OUTER_BOUND('',#75,.T.);
#77=ADVANCED_FACE('',(#76),#8,.T.);
#90=OPEN_SHELL('',(#67,#77));
#91=SHELL_BASED_SURFACE_MODEL('',(#90));",
        kr = k * rho
    )
}

/// The largest distance from the shared cut's samples to each face's surface.
fn cut_gaps(model: &geometry_engine::primitives::topology_builder::BRepModel) -> (f64, f64) {
    cut_gaps_at(model, 1.0)
}

fn cut_gaps_at(
    model: &geometry_engine::primitives::topology_builder::BRepModel,
    z: f64,
) -> (f64, f64) {
    let rho = 20.0 + (4.0 - z * z).max(0.0).sqrt();
    let near = |v: u32, p: [f64; 3]| {
        model.vertices.get(v).is_some_and(|x| {
            (x.position[0] - p[0]).abs() < 1e-6
                && (x.position[1] - p[1]).abs() < 1e-6
                && (x.position[2] - p[2]).abs() < 1e-6
        })
    };
    let (eid, edge) = model
        .edges
        .iter()
        .find(|(_, e)| near(e.start_vertex, [rho, 0.0, z]) && near(e.end_vertex, [0.0, rho, z]))
        .expect("the shared cut");
    let _ = eid;
    let mut gaps = Vec::new();
    for (_, face) in model.faces.iter() {
        let surface = model.surfaces.get(face.surface_id).expect("surface");
        let mut worst = 0.0_f64;
        for k in 0..=64 {
            let p = edge
                .evaluate(k as f64 / 64.0, &model.curves)
                .expect("edge evaluates");
            let (u, v) = surface
                .closest_point(&p, Tolerance::default())
                .expect("projects");
            let q: Point3 = surface.point_at(u, v).expect("evaluates");
            worst = worst.max((q - p).magnitude());
        }
        gaps.push((surface.type_name(), worst));
    }
    let torus = gaps.iter().find(|g| g.0 == "Torus").expect("torus face").1;
    let plane = gaps.iter().find(|g| g.0 == "Plane").expect("plane face").1;
    (torus, plane)
}

#[test]
fn an_approximate_edge_is_rebuilt_on_the_intersection_of_its_two_faces() {
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(&torus_cut_by_a_plane(0.5504)), "cut")
        .expect("imports");
    let (torus, plane) = cut_gaps(&model);
    assert!(
        torus <= 1e-6 && plane <= 1e-6,
        "the cut must lie on both faces: torus gap {torus:.3e}, plane gap {plane:.3e}"
    );
    let rebuilt: Vec<_> = report
        .healings
        .iter()
        .filter(|h| format!("{:?}", h.kind) == "EdgeRebuiltOnFaceIntersection")
        .collect();
    assert_eq!(
        rebuilt.len(),
        1,
        "the rebuild is reported: {:?}",
        report.healings
    );
    assert!(
        rebuilt[0].deviation > 0.01 && rebuilt[0].deviation < 0.05,
        "with the file's gap: {:?}",
        rebuilt[0]
    );
}

/// An edge already on both faces -- the cut written as the exact circle --
/// is left exactly as written.
#[test]
fn an_exact_edge_is_not_rebuilt() {
    let rho = 20.0 + 3f64.sqrt();
    let exact = torus_cut_by_a_plane(0.5504).replace(
        "#45=B_SPLINE_CURVE_WITH_KNOTS(",
        &format!(
            "#45=CIRCLE('',#7,{rho:.15});
#145=B_SPLINE_CURVE_WITH_KNOTS("
        ),
    );
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(&exact), "cut")
        .expect("imports");
    let (torus, plane) = cut_gaps(&model);
    assert!(
        torus <= 1e-6 && plane <= 1e-6,
        "the circle is on both: {torus:.3e} {plane:.3e}"
    );
    assert!(
        report
            .healings
            .iter()
            .all(|h| format!("{:?}", h.kind) != "EdgeRebuiltOnFaceIntersection"),
        "nothing to rebuild: {:?}",
        report.healings
    );
}

/// Where a fillet meets a flat face the two surfaces are TANGENT along the
/// edge, and alternating projection between them closes in slowly: on
/// CADGenBench part 205 (a torus blending into a plane, the file's edge
/// 0.0114 mm off the torus) it reached 1e-5 mm, short of the 1e-9 the rebuild
/// demanded, so the edge was left 0.0114 off and the part invalid. A rebuild
/// that fits both faces a thousand times better than the file's curve is
/// kept, with the edge's tolerance at what it reached.
#[test]
fn an_edge_where_a_fillet_meets_a_face_tangentially_is_rebuilt() {
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(&torus_cut_at(2.0, 0.5504)), "tangent")
        .expect("imports");
    let (torus, plane) = cut_gaps_at(&model, 2.0);
    assert!(
        torus <= 1e-4 && plane <= 1e-4,
        "the tangent cut must lie on both faces: torus gap {torus:.3e}, plane gap {plane:.3e}"
    );
    assert!(
        report
            .healings
            .iter()
            .any(|h| format!("{:?}", h.kind) == "EdgeRebuiltOnFaceIntersection"),
        "the rebuild is reported: {:?}",
        report.healings
    );
}
