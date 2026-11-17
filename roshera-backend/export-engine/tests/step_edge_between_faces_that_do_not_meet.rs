// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Two faces can share an edge without meeting there: they run alongside
//! each other a few micrometres apart and would only cross well outside
//! their boundaries (CADGenBench part 229: faces 3e-3 mm apart along an edge
//! whose curve the file wrote 1.5e-2 mm off one of them). Rebuilding the
//! edge on their intersection searched for the closest approach across the
//! edge, found none within reach -- the gap only shrinks toward the search
//! bound -- and gave up, leaving the edge beyond the repair cap and the part
//! refused. Such an edge is the foot on its nearer face, with the faces'
//! real separation as its tolerance.

use export_engine::ExportEngine;
use geometry_engine::math::{Point3, Tolerance};

fn envelope(body: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('alongside'),'2;1');\n\
         FILE_NAME('alongside.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n{body}\nENDSEC;\n\
         END-ISO-10303-21;\n"
    )
}

/// Face A is the plane z = 0 for -10 <= x <= 0; face B is the plane
/// z = 0.003 + 0.003 x for 0 <= x <= 10. They meet only at x = -1, outside
/// both. Their shared edge runs along y at x = 0, from (0, 0, 0.003) to
/// (0, 10, 0.003), written as a quadratic that bows up to z = 0.0165 at its
/// middle: 0.0165 mm off A, 0.0135 mm off B.
fn faces_alongside() -> String {
    let slope = 0.003f64;
    let len = (1.0 + slope * slope).sqrt();
    let (nx, nz) = (-slope / len, 1.0 / len);
    let (rx, rz) = (1.0 / len, slope / len);
    let zb = 0.003 + slope * 10.0;
    format!(
        "#1=CARTESIAN_POINT('',(0.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#3=DIRECTION('',(1.,0.,0.));
#4=AXIS2_PLACEMENT_3D('',#1,#2,#3);
#5=PLANE('',#4);
#6=CARTESIAN_POINT('',(0.,0.,0.003));
#7=DIRECTION('',({nx:.17},0.,{nz:.17}));
#8=DIRECTION('',({rx:.17},0.,{rz:.17}));
#9=AXIS2_PLACEMENT_3D('',#6,#7,#8);
#10=PLANE('',#9);
#11=CARTESIAN_POINT('',(-10.,0.,0.));
#12=CARTESIAN_POINT('',(0.,0.,0.003));
#13=CARTESIAN_POINT('',(0.,10.,0.003));
#14=CARTESIAN_POINT('',(-10.,10.,0.));
#15=CARTESIAN_POINT('',(10.,0.,{zb:.17}));
#16=CARTESIAN_POINT('',(10.,10.,{zb:.17}));
#21=VERTEX_POINT('',#11);
#22=VERTEX_POINT('',#12);
#23=VERTEX_POINT('',#13);
#24=VERTEX_POINT('',#14);
#25=VERTEX_POINT('',#15);
#26=VERTEX_POINT('',#16);
#30=CARTESIAN_POINT('',(0.,5.,0.03));
#31=B_SPLINE_CURVE_WITH_KNOTS('',2,(#12,#30,#13),.UNSPECIFIED.,.F.,.F.,(3,3),(0.,1.),.UNSPECIFIED.);
#32=DIRECTION('',(0.,1.,0.));
#33=VECTOR('',#32,1.);
#34=DIRECTION('',({rx:.17},0.,{rz:.17}));
#35=VECTOR('',#34,1.);
#36=DIRECTION('',(0.9999999550000029,0.,0.0002999999865000009));
#37=VECTOR('',#36,1.);
#40=LINE('',#11,#37);
#41=LINE('',#14,#33);
#42=LINE('',#11,#33);
#43=LINE('',#12,#35);
#44=LINE('',#13,#35);
#45=LINE('',#15,#33);
#46=LINE('',#14,#37);
#51=EDGE_CURVE('',#21,#22,#40,.T.);
#52=EDGE_CURVE('',#22,#23,#31,.T.);
#53=EDGE_CURVE('',#24,#23,#46,.T.);
#54=EDGE_CURVE('',#21,#24,#42,.T.);
#55=EDGE_CURVE('',#22,#25,#43,.T.);
#56=EDGE_CURVE('',#25,#26,#45,.T.);
#57=EDGE_CURVE('',#23,#26,#44,.T.);
#61=ORIENTED_EDGE('',*,*,#51,.T.);
#62=ORIENTED_EDGE('',*,*,#52,.T.);
#63=ORIENTED_EDGE('',*,*,#53,.F.);
#64=ORIENTED_EDGE('',*,*,#54,.F.);
#65=EDGE_LOOP('',(#61,#62,#63,#64));
#66=FACE_OUTER_BOUND('',#65,.T.);
#67=ADVANCED_FACE('',(#66),#5,.T.);
#71=ORIENTED_EDGE('',*,*,#55,.T.);
#72=ORIENTED_EDGE('',*,*,#56,.T.);
#73=ORIENTED_EDGE('',*,*,#57,.F.);
#74=ORIENTED_EDGE('',*,*,#52,.F.);
#75=EDGE_LOOP('',(#71,#72,#73,#74));
#76=FACE_OUTER_BOUND('',#75,.T.);
#77=ADVANCED_FACE('',(#76),#10,.T.);
#90=OPEN_SHELL('',(#67,#77));
#91=SHELL_BASED_SURFACE_MODEL('',(#90));"
    )
}

#[test]
fn an_edge_between_faces_that_do_not_meet_is_placed_on_its_nearer_face() {
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(&faces_alongside()), "alongside")
        .expect("imports");
    let near = |v: u32, p: [f64; 3]| {
        model
            .vertices
            .get(v)
            .is_some_and(|x| (0..3).all(|k| (x.position[k] - p[k]).abs() < 1e-6))
    };
    let (_, edge) = model
        .edges
        .iter()
        .find(|(_, e)| {
            near(e.start_vertex, [0.0, 0.0, 0.003]) && near(e.end_vertex, [0.0, 10.0, 0.003])
        })
        .expect("the shared edge");
    let mut worst = 0.0_f64;
    for (_, face) in model.faces.iter() {
        let surface = model.surfaces.get(face.surface_id).expect("surface");
        for k in 0..=64 {
            let p = edge
                .evaluate(k as f64 / 64.0, &model.curves)
                .expect("evaluates");
            let (u, v) = surface
                .closest_point(&p, Tolerance::default())
                .expect("projects");
            let q: Point3 = surface.point_at(u, v).expect("evaluates");
            worst = worst.max((q - p).magnitude());
        }
    }
    assert!(
        worst <= 0.0035,
        "the edge lies within the faces' 3e-3 mm separation of both (worst {worst:.3e})"
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

/// The edge rebuild computes every candidate edge in parallel and applies
/// the results in edge-id order, so an import is the same however many
/// threads run it: same healings, same rebuilt curves.
#[test]
fn the_edge_rebuild_gives_the_same_result_on_one_thread_and_on_many() {
    let import = |threads: usize| {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("pool");
        pool.install(|| {
            let (model, report) = ExportEngine::new()
                .import_step_content(&envelope(&two_copies()), "alongside twice")
                .expect("imports");
            let edges: Vec<String> = model
                .edges
                .iter()
                .map(|(id, e)| {
                    let samples: Vec<String> = (0..=16)
                        .map(|k| {
                            e.evaluate(k as f64 / 16.0, &model.curves)
                                .map(|p| {
                                    format!("{:?}", (p.x.to_bits(), p.y.to_bits(), p.z.to_bits()))
                                })
                                .unwrap_or_default()
                        })
                        .collect();
                    format!(
                        "{id}: tol {:?} {}",
                        e.tolerance.to_bits(),
                        samples.join(",")
                    )
                })
                .collect();
            (format!("{:?}", report.healings), edges)
        })
    };
    let (one_healings, one_edges) = import(1);
    let (many_healings, many_edges) = import(4);
    assert!(
        one_healings
            .matches("EdgeRebuiltOnFaceIntersection")
            .count()
            >= 2,
        "the fixture rebuilds at least two edges, so their order is tested: {one_healings}"
    );
    assert_eq!(
        one_healings, many_healings,
        "healings differ by thread count"
    );
    assert_eq!(
        one_edges, many_edges,
        "rebuilt edges differ by thread count"
    );
}

/// The fixture twice in one file (the second copy's entities renumbered),
/// so the import rebuilds two edges and their order matters.
fn two_copies() -> String {
    let one = faces_alongside();
    let mut second = String::with_capacity(one.len());
    let mut chars = one.chars().peekable();
    while let Some(c) = chars.next() {
        second.push(c);
        if c == '#' {
            let mut digits = String::new();
            while let Some(d) = chars.peek().copied().filter(char::is_ascii_digit) {
                digits.push(d);
                chars.next();
            }
            let n: u64 = digits.parse().expect("an entity number");
            second.push_str(&(n + 100).to_string());
        }
    }
    format!(
        "{one}
{second}"
    )
}
