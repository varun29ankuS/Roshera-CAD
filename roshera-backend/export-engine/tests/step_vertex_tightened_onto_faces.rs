// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A STEP writer keeps a tolerance on each vertex, and imported vertices sit
//! up to ~5e-2 off the point where their faces actually meet (CADGenBench
//! parts 211, 212, 240, 246: of 137 measured corners the written vertex lay
//! on all its faces in none). Accepting the gap as a tolerance leaves the
//! model inexact. The importer TIGHTENS such a vertex: it moves it to the
//! point that lies on its faces (within the move cap), re-fits the edges that
//! end there, and reports the move with its before / after distances.

use std::collections::{BTreeMap, BTreeSet};

use export_engine::ExportEngine;
use geometry_engine::math::{Point3, Tolerance};
use geometry_engine::primitives::topology_builder::BRepModel;

/// Every vertex's largest distance to the surfaces of the faces that meet
/// there.
fn vertex_offsets(model: &BRepModel) -> BTreeMap<u32, f64> {
    let mut faces_at: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for (fid, face) in model.faces.iter() {
        for l in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
            let lp = model.loops.get(l).expect("loop");
            for &eid in &lp.edges {
                let e = model.edges.get(eid).expect("edge");
                faces_at.entry(e.start_vertex).or_default().insert(fid);
                faces_at.entry(e.end_vertex).or_default().insert(fid);
            }
        }
    }
    faces_at
        .into_iter()
        .map(|(vid, faces)| {
            let p = model.vertices.get(vid).expect("vertex").point();
            let worst = faces
                .iter()
                .map(|&f| {
                    let s = model
                        .surfaces
                        .get(model.faces.get(f).expect("face").surface_id)
                        .expect("surface");
                    let (u, v) = s.closest_point(&p, Tolerance::default()).expect("project");
                    p.distance(&s.point_at(u, v).expect("evaluate"))
                })
                .fold(0.0_f64, f64::max);
            (vid, worst)
        })
        .collect()
}

/// A 20 x 30 x 10 box exported to STEP with one corner's VERTEX_POINT moved
/// to a new point (2, 1.5, -1) x 1e-3 off the corner where its three planes
/// meet. Returns the text and the true corner.
async fn box_with_a_corner_written_off(offset: [f64; 3]) -> (String, Point3) {
    use geometry_engine::primitives::topology_builder::TopologyBuilder;
    let mut model = BRepModel::new();
    TopologyBuilder::new(&mut model)
        .create_box_3d(20.0, 30.0, 10.0)
        .expect("box");
    let mut path = std::env::temp_dir();
    path.push(format!(
        "roshera_tighten_vertex_{}.step",
        std::process::id()
    ));
    export_engine::formats::step::export_brep_to_step(&model, &path)
        .await
        .expect("export");
    let text = std::fs::read_to_string(&path).expect("read back");
    let _ = std::fs::remove_file(&path);

    let lines: Vec<&str> = text.lines().collect();
    let vp = lines
        .iter()
        .find(|l| l.contains("=VERTEX_POINT("))
        .expect("a vertex");
    // #n=VERTEX_POINT('',#p);
    let point_ref = vp
        .rsplit('#')
        .next()
        .expect("point ref")
        .trim_end_matches(|c: char| c == ';' || c == ')' || c.is_whitespace());
    let point_line = lines
        .iter()
        .find(|l| {
            l.trim_start()
                .starts_with(&format!("#{point_ref}=CARTESIAN_POINT("))
        })
        .expect("the vertex's point");
    let coords: Vec<f64> = point_line
        .split("((")
        .nth(1)
        .or_else(|| point_line.split(",(").nth(1))
        .expect("coordinates")
        .trim_end_matches(|c: char| c == ';' || c == ')' || c.is_whitespace())
        .split(',')
        .map(|x| x.trim().parse::<f64>().expect("a number"))
        .collect();
    let corner = Point3::new(coords[0], coords[1], coords[2]);
    let moved = format!(
        "#999998=CARTESIAN_POINT('',({:.9},{:.9},{:.9}));",
        coords[0] + offset[0],
        coords[1] + offset[1],
        coords[2] + offset[2]
    );
    let mut out: Vec<String> = Vec::new();
    for l in &lines {
        if *l == *vp {
            out.push(moved.clone());
            out.push(l.replace(&format!("#{point_ref})"), "#999998)"));
        } else {
            out.push(l.to_string());
        }
    }
    (out.join("\n"), corner)
}

#[tokio::test]
async fn a_vertex_written_off_its_corner_is_moved_onto_its_faces() {
    let (text, corner) = box_with_a_corner_written_off([2.0e-3, 1.5e-3, -1.0e-3]).await;
    let (mut model, report) = ExportEngine::new()
        .import_step_content(&text, "tighten")
        .expect("imports");

    let offsets = vertex_offsets(&model);
    let worst = offsets.values().copied().fold(0.0_f64, f64::max);
    assert!(
        worst <= 1e-9,
        "every vertex lies on the faces that meet there; the worst is {worst:.3e} off ({offsets:?})"
    );
    let nearest = model
        .vertices
        .iter()
        .map(|(_, v)| v.point().distance(&corner))
        .fold(f64::INFINITY, f64::min);
    assert!(
        nearest <= 1e-9,
        "the written-off vertex now sits at the true corner ({nearest:.3e} away)"
    );
    let moves: Vec<_> = report
        .healings
        .iter()
        .filter(|h| format!("{:?}", h.kind) == "VertexTightenedOntoFaces")
        .collect();
    assert_eq!(moves.len(), 1, "one vertex moved: {:?}", report.healings);
    assert!(
        (moves[0].deviation - 2.0e-3).abs() < 1e-4 && moves[0].tolerance <= 1e-9,
        "the move reports the vertex's distance off its faces before ({:.3e}) and after ({:.3e})",
        moves[0].deviation,
        moves[0].tolerance
    );
    let solids: Vec<u32> = model.solids.iter().map(|(id, _)| id).collect();
    assert_eq!(solids.len(), 1, "one solid");
    let cert = model.certify_solid(solids[0]);
    assert!(
        cert.is_sound(),
        "the box certifies sound: {:?}",
        cert.errors
    );
}

/// A vertex written farther off than the move cap is NOT moved: no point
/// that far from where the file put it is the file's vertex.
#[tokio::test]
async fn a_vertex_beyond_the_move_cap_is_left_where_it_was_written() {
    let (text, corner) = box_with_a_corner_written_off([0.08, 0.0, 0.0]).await;
    let (model, report) = ExportEngine::new()
        .import_step_content_uncertified(&text, "too-far")
        .expect("imports");
    assert!(
        report
            .healings
            .iter()
            .all(|h| format!("{:?}", h.kind) != "VertexTightenedOntoFaces"),
        "nothing moved: {:?}",
        report.healings
    );
    let written = Point3::new(corner.x + 0.08, corner.y, corner.z);
    assert!(
        model
            .vertices
            .iter()
            .any(|(_, v)| v.point().distance(&written) <= 1e-9),
        "the vertex stays where the file wrote it"
    );
}
