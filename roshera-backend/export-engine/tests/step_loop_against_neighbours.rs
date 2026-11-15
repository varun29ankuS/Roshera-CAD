// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Two faces sharing an edge walk it in OPPOSITE directions -- that is what
//! makes a shell consistently oriented. A face whose loop runs against every
//! neighbour while its sense agrees with them (its mesh faces out of the
//! solid) has its loop written backwards; OCCT's reader repairs exactly this
//! (ShapeFix_Shell::FixFaceOrientation) and so does this importer, openly:
//! the loop is reversed and the repair recorded on the import report. One
//! CADGenBench part's torus fillet walked 7 of its 9 edges the same way as
//! its neighbours and failed the Euler-Poincare check.

use std::collections::HashMap;

use export_engine::ExportEngine;

fn envelope(body: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('cube'),'2;1');\n\
         FILE_NAME('cube.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n{body}\nENDSEC;\n\
         END-ISO-10303-21;\n"
    )
}

/// A unit cube, every face's loop counter-clockwise seen from outside,
/// except face `reversed` (if any), whose loop is written backwards.
fn cube(reversed: Option<usize>) -> String {
    let v = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [1.0, 0.0, 1.0],
        [1.0, 1.0, 1.0],
        [0.0, 1.0, 1.0],
    ];
    // (vertex cycle seen from outside, outward normal, reference direction)
    let faces: [([usize; 4], [f64; 3], [f64; 3]); 6] = [
        ([0, 3, 2, 1], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0]),
        ([4, 5, 6, 7], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        ([0, 1, 5, 4], [0.0, -1.0, 0.0], [1.0, 0.0, 0.0]),
        ([3, 7, 6, 2], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
        ([0, 4, 7, 3], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([1, 2, 6, 5], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ];
    let mut s = String::new();
    let mut id = 1u64;
    let mut next = || {
        id += 1;
        id
    };
    let mut vid = Vec::new();
    for p in &v {
        let cp = next();
        s += &format!(
            "#{cp}=CARTESIAN_POINT('',({:.1},{:.1},{:.1}));",
            p[0], p[1], p[2]
        );
        let vp = next();
        s += &format!("#{vp}=VERTEX_POINT('',#{cp});");
        vid.push((cp, vp));
    }
    let mut edges: HashMap<(usize, usize), u64> = HashMap::new();
    let mut face_ids = Vec::new();
    for (k, (cycle, n, x)) in faces.iter().enumerate() {
        let mut cycle = cycle.to_vec();
        if reversed == Some(k) {
            cycle.reverse();
        }
        let mut oes = Vec::new();
        for i in 0..4 {
            let (a, b) = (cycle[i], cycle[(i + 1) % 4]);
            let (lo, hi) = (a.min(b), a.max(b));
            let ec = *edges.entry((lo, hi)).or_insert_with(|| {
                let d = [
                    v[hi][0] - v[lo][0],
                    v[hi][1] - v[lo][1],
                    v[hi][2] - v[lo][2],
                ];
                let dir = next();
                s += &format!(
                    "#{dir}=DIRECTION('',({:.1},{:.1},{:.1}));",
                    d[0], d[1], d[2]
                );
                let vec = next();
                s += &format!("#{vec}=VECTOR('',#{dir},1.);");
                let line = next();
                s += &format!("#{line}=LINE('',#{},#{vec});", vid[lo].0);
                let e = next();
                s += &format!(
                    "#{e}=EDGE_CURVE('',#{},#{},#{line},.T.);",
                    vid[lo].1, vid[hi].1
                );
                e
            });
            let oe = next();
            let fwd = if a < b { ".T." } else { ".F." };
            s += &format!("#{oe}=ORIENTED_EDGE('',*,*,#{ec},{fwd});");
            oes.push(format!("#{oe}"));
        }
        let lp = next();
        s += &format!("#{lp}=EDGE_LOOP('',({}));", oes.join(","));
        let fb = next();
        s += &format!("#{fb}=FACE_OUTER_BOUND('',#{lp},.T.);");
        let nd = next();
        s += &format!("#{nd}=DIRECTION('',({:.1},{:.1},{:.1}));", n[0], n[1], n[2]);
        let xd = next();
        s += &format!("#{xd}=DIRECTION('',({:.1},{:.1},{:.1}));", x[0], x[1], x[2]);
        let ax = next();
        s += &format!(
            "#{ax}=AXIS2_PLACEMENT_3D('',#{},#{nd},#{xd});",
            vid[cycle[0]].0
        );
        let pl = next();
        s += &format!("#{pl}=PLANE('',#{ax});");
        let f = next();
        s += &format!("#{f}=ADVANCED_FACE('',(#{fb}),#{pl},.T.);");
        face_ids.push(format!("#{f}"));
    }
    let sh = next();
    s += &format!("#{sh}=CLOSED_SHELL('',({}));", face_ids.join(","));
    let so = next();
    s += &format!("#{so}=MANIFOLD_SOLID_BREP('cube',#{sh});");
    s
}

/// Shared edges walked the SAME way by both faces using them.
fn same_way_edges(model: &geometry_engine::primitives::topology_builder::BRepModel) -> usize {
    let mut walks: HashMap<u32, Vec<bool>> = HashMap::new();
    for (_, face) in model.faces.iter() {
        for l in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
            let lp = model.loops.get(l).expect("loop");
            for (k, &e) in lp.edges.iter().enumerate() {
                walks.entry(e).or_default().push(lp.orientations[k]);
            }
        }
    }
    walks
        .values()
        .filter(|w| w.len() == 2 && w[0] == w[1])
        .count()
}

#[test]
fn a_consistent_cube_needs_no_repair() {
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(&cube(None)), "cube")
        .expect("imports");
    assert_eq!(same_way_edges(&model), 0, "the fixture is consistent");
    assert!(
        report
            .healings
            .iter()
            .all(|h| format!("{:?}", h.kind) != "LoopReversedToMatchNeighbours"),
        "nothing to repair: {:#?}",
        report.healings
    );
}

#[test]
fn a_face_walking_against_its_neighbours_has_its_loop_reversed_and_reported() {
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(&cube(Some(5))), "cube")
        .expect("imports");
    assert_eq!(
        same_way_edges(&model),
        0,
        "every shared edge is walked oppositely after the repair"
    );
    let repairs = report
        .healings
        .iter()
        .filter(|h| format!("{:?}", h.kind) == "LoopReversedToMatchNeighbours")
        .count();
    assert_eq!(
        repairs, 1,
        "the one repair is reported: {:#?}",
        report.healings
    );
}
