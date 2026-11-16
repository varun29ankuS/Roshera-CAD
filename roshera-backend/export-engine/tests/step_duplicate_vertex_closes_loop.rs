// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A STEP writer can emit two `VERTEX_POINT`s at the same point and hand one
//! to each of two edges that meet there. The loop then does not chain by
//! vertex: one edge ends at the first copy, the next starts at the second, and
//! the B-Rep is invalid (CADGenBench part 132: two vertices at exactly
//! (-40, 2.36e-13, 128); an odd Euler-Poincare count and two broken loops).
//! Two vertices a loop passes from one to the other at the same point are one
//! vertex; the importer merges them and says so.

use export_engine::ExportEngine;
use geometry_engine::lens::EventKind;

fn envelope(body: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('dup'),'2;1');\n\
         FILE_NAME('dup.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n{body}\nENDSEC;\n\
         END-ISO-10303-21;\n"
    )
}

/// A 10 x 10 square in z = 0 whose corner (10, 0, 0) is written twice: #22
/// ends the bottom edge, #25 (same point) starts the right edge.
const SQUARE_WITH_A_DUPLICATE_CORNER: &str = "#1=CARTESIAN_POINT('',(0.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#3=DIRECTION('',(1.,0.,0.));
#4=AXIS2_PLACEMENT_3D('',#1,#2,#3);
#5=PLANE('',#4);
#11=CARTESIAN_POINT('',(0.,0.,0.));
#12=CARTESIAN_POINT('',(10.,0.,0.));
#13=CARTESIAN_POINT('',(10.,10.,0.));
#14=CARTESIAN_POINT('',(0.,10.,0.));
#15=CARTESIAN_POINT('',(10.,0.,0.));
#21=VERTEX_POINT('',#11);
#22=VERTEX_POINT('',#12);
#23=VERTEX_POINT('',#13);
#24=VERTEX_POINT('',#14);
#25=VERTEX_POINT('',#15);
#31=DIRECTION('',(1.,0.,0.));
#32=DIRECTION('',(0.,1.,0.));
#33=DIRECTION('',(-1.,0.,0.));
#34=DIRECTION('',(0.,-1.,0.));
#35=VECTOR('',#31,1.);
#36=VECTOR('',#32,1.);
#37=VECTOR('',#33,1.);
#38=VECTOR('',#34,1.);
#41=LINE('',#11,#35);
#42=LINE('',#15,#36);
#43=LINE('',#13,#37);
#44=LINE('',#14,#38);
#51=EDGE_CURVE('',#21,#22,#41,.T.);
#52=EDGE_CURVE('',#25,#23,#42,.T.);
#53=EDGE_CURVE('',#23,#24,#43,.T.);
#54=EDGE_CURVE('',#24,#21,#44,.T.);
#61=ORIENTED_EDGE('',*,*,#51,.T.);
#62=ORIENTED_EDGE('',*,*,#52,.T.);
#63=ORIENTED_EDGE('',*,*,#53,.T.);
#64=ORIENTED_EDGE('',*,*,#54,.T.);
#70=EDGE_LOOP('',(#61,#62,#63,#64));
#71=FACE_OUTER_BOUND('',#70,.T.);
#80=ADVANCED_FACE('',(#71),#5,.T.);
#90=OPEN_SHELL('',(#80));
#91=SHELL_BASED_SURFACE_MODEL('',(#90));";

#[test]
fn two_vertices_a_loop_passes_between_at_one_point_are_merged() {
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(SQUARE_WITH_A_DUPLICATE_CORNER), "dup")
        .expect("imports");
    let (fid, face) = model.faces.iter().next().expect("one face");
    let lp = model.loops.get(face.outer_loop).expect("outer loop");
    let walked: Vec<(u32, u32)> = lp
        .edges
        .iter()
        .enumerate()
        .map(|(k, &e)| {
            let edge = model.edges.get(e).expect("edge");
            if lp.orientations.get(k).copied().unwrap_or(true) {
                (edge.start_vertex, edge.end_vertex)
            } else {
                (edge.end_vertex, edge.start_vertex)
            }
        })
        .collect();
    for k in 0..walked.len() {
        let next = walked[(k + 1) % walked.len()];
        assert_eq!(
            walked[k].1, next.0,
            "the loop breaks after edge {k}: it ends at vertex {} and the next edge \
             starts at vertex {} ({walked:?})",
            walked[k].1, next.0
        );
    }
    assert!(
        report
            .healings
            .iter()
            .any(|h| format!("{:?}", h.kind) == "VerticesMergedToCloseLoop"),
        "the merge is reported: {:?}",
        report.healings
    );
    let _ = fid;
}

/// A box written with one corner as TWO vertices: one edge into the corner
/// names the copy. Imported, the box is one sound solid again, and the Lens
/// journal names the faces whose loops the merge closed.
#[tokio::test]
async fn a_box_with_a_corner_written_twice_imports_sound() {
    use geometry_engine::primitives::topology_builder::{BRepModel, TopologyBuilder};
    let mut model = BRepModel::new();
    TopologyBuilder::new(&mut model)
        .create_box_3d(20.0, 30.0, 10.0)
        .expect("box");
    let mut path = std::env::temp_dir();
    path.push(format!("roshera_dup_vertex_{}.step", std::process::id()));
    export_engine::formats::step::export_brep_to_step(&model, &path)
        .await
        .expect("export");
    let text = std::fs::read_to_string(&path).expect("read back");
    let _ = std::fs::remove_file(&path);

    // Duplicate the first VERTEX_POINT and hand the copy to the first
    // EDGE_CURVE that ENDS at it.
    let lines: Vec<&str> = text.lines().collect();
    let entity = |l: &str| l.split('=').next().unwrap_or("").trim().to_string();
    let vp_line = lines
        .iter()
        .find(|l| l.contains("=VERTEX_POINT("))
        .expect("a vertex");
    let original = entity(vp_line);
    let copy = "#999999";
    let mut out: Vec<String> = Vec::new();
    let mut handed = false;
    for l in &lines {
        if !handed && l.contains("=EDGE_CURVE(") {
            // EDGE_CURVE('',start,end,curve,sense);
            let args: Vec<&str> = l.split(',').collect();
            if args.len() >= 5 && args[2].trim() == original {
                let mut a: Vec<String> = args.iter().map(|x| x.to_string()).collect();
                a[2] = copy.to_string();
                out.push(a.join(","));
                handed = true;
                continue;
            }
        }
        out.push(l.to_string());
        if l == vp_line {
            out.push(l.replacen(&original, copy, 1));
        }
    }
    assert!(handed, "an edge ends at the duplicated corner");
    let text = out.join(
        "
",
    );

    let (mut model, report) = ExportEngine::new()
        .import_step_content(&text, "dup-box")
        .expect("imports");
    assert!(
        report
            .healings
            .iter()
            .any(|h| format!("{:?}", h.kind) == "VerticesMergedToCloseLoop"),
        "the merge is reported: {:?}",
        report.healings
    );
    let solids: Vec<u32> = model.solids.iter().map(|(id, _)| id).collect();
    assert_eq!(solids.len(), 1, "one solid");
    let cert = model.certify_solid(solids[0]);
    assert!(
        cert.is_sound(),
        "the box certifies sound: {:?}",
        cert.errors
    );
    let named: Vec<_> = model
        .faces
        .iter()
        .filter_map(|(f, _)| model.face_pid(f))
        .filter(|&pid| {
            report
                .lens
                .for_face(pid)
                .any(|e| e.kind == EventKind::VerticesMerged)
        })
        .collect();
    assert!(
        !named.is_empty(),
        "the Lens journal names a face whose loop was closed"
    );
}

/// Two vertices at one point that no loop passes between -- the corners of
/// two separate squares that touch -- are NOT merged: welding them would join
/// two bodies at a point.
#[test]
fn vertices_that_only_touch_are_left_apart() {
    let (_, report) = ExportEngine::new()
        .import_step_content(&envelope(&touching_squares()), "touch")
        .expect("imports");
    assert!(
        report
            .healings
            .iter()
            .all(|h| format!("{:?}", h.kind) != "VerticesMergedToCloseLoop"),
        "nothing merged: {:?}",
        report.healings
    );
}

/// Two closed unit squares in z = 0, the second's corner on the first's
/// corner (1, 1, 0), each with its own vertex there.
fn touching_squares() -> String {
    let mut s = String::from(
        "#1=CARTESIAN_POINT('',(0.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#3=DIRECTION('',(1.,0.,0.));
#4=AXIS2_PLACEMENT_3D('',#1,#2,#3);
#5=PLANE('',#4);
#31=DIRECTION('',(1.,0.,0.));
#32=DIRECTION('',(0.,1.,0.));
#33=DIRECTION('',(-1.,0.,0.));
#34=DIRECTION('',(0.,-1.,0.));
#35=VECTOR('',#31,1.);
#36=VECTOR('',#32,1.);
#37=VECTOR('',#33,1.);
#38=VECTOR('',#34,1.);
",
    );
    let mut faces = Vec::new();
    for (k, (x, y)) in [(0.0, 0.0), (1.0, 1.0)].into_iter().enumerate() {
        let b = 100 * (k as u32 + 1);
        let corners = [(x, y), (x + 1.0, y), (x + 1.0, y + 1.0), (x, y + 1.0)];
        for (i, (cx, cy)) in corners.iter().enumerate() {
            s += &format!(
                "#{p}=CARTESIAN_POINT('',({cx:.1},{cy:.1},0.));\n#{v}=VERTEX_POINT('',#{p});\n",
                p = b + i as u32,
                v = b + 10 + i as u32
            );
        }
        for i in 0..4u32 {
            let (from, to) = (b + 10 + i, b + 10 + (i + 1) % 4);
            s += &format!(
                "#{l}=LINE('',#{p},#{vec});\n#{e}=EDGE_CURVE('',#{from},#{to},#{l},.T.);\n\
                 #{o}=ORIENTED_EDGE('',*,*,#{e},.T.);\n",
                l = b + 20 + i,
                p = b + i,
                vec = 35 + i,
                e = b + 30 + i,
                o = b + 40 + i
            );
        }
        s += &format!(
            "#{lp}=EDGE_LOOP('',(#{o0},#{o1},#{o2},#{o3}));\n#{fb}=FACE_OUTER_BOUND('',#{lp},.T.);\n\
             #{f}=ADVANCED_FACE('',(#{fb}),#5,.T.);\n",
            lp = b + 50,
            o0 = b + 40,
            o1 = b + 41,
            o2 = b + 42,
            o3 = b + 43,
            fb = b + 51,
            f = b + 52
        );
        faces.push(format!("#{}", b + 52));
    }
    s += &format!(
        "#900=OPEN_SHELL('',({}));\n#901=SHELL_BASED_SURFACE_MODEL('',(#900));",
        faces.join(",")
    );
    s
}
