// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Kernel Lens: the STEP importer records what it decided, on the face it
//! decided it for, keyed by that face's persistent id -- the journal the
//! person sees in the app and the agent reads over MCP. Each kind of
//! decision has a fixture that triggers it, and a clean part records none.

use std::collections::HashMap;

use export_engine::ExportEngine;
use geometry_engine::lens::{EntityRef, EventKind};

fn envelope(body: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('lens'),'2;1');\n\
         FILE_NAME('lens.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n{body}\nENDSEC;\n\
         END-ISO-10303-21;\n"
    )
}

/// How one face of the cube is written.
#[derive(Clone, Copy, PartialEq)]
enum Face5 {
    /// Counter-clockwise seen from outside, bound `.T.`.
    Plain,
    /// Its loop written backwards and its bound marked `.F.` -- the same
    /// face, as a writer may legally emit it.
    BoundF,
    /// Its loop written backwards with bound `.T.` -- against its neighbours.
    Backwards,
    /// Its plane lifted 0.005 mm off its edges (x = 1.005).
    Lifted,
}

/// A unit cube; face 5 (x = 1) written as `face5` says.
fn cube(face5: Face5) -> String {
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
        let special = if k == 5 { face5 } else { Face5::Plain };
        if matches!(special, Face5::BoundF | Face5::Backwards) {
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
        let flag = if special == Face5::BoundF {
            ".F."
        } else {
            ".T."
        };
        s += &format!("#{fb}=FACE_OUTER_BOUND('',#{lp},{flag});");
        let nd = next();
        s += &format!("#{nd}=DIRECTION('',({:.1},{:.1},{:.1}));", n[0], n[1], n[2]);
        let xd = next();
        s += &format!("#{xd}=DIRECTION('',({:.1},{:.1},{:.1}));", x[0], x[1], x[2]);
        let origin = if special == Face5::Lifted {
            let o = next();
            s += &format!("#{o}=CARTESIAN_POINT('',(1.005,0.,0.));");
            o
        } else {
            vid[cycle[0]].0
        };
        let ax = next();
        s += &format!("#{ax}=AXIS2_PLACEMENT_3D('',#{origin},#{nd},#{xd});");
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

/// Import `body`; the journal's events and every face's persistent id.
fn import(body: &str) -> (Vec<geometry_engine::lens::LensEvent>, Vec<u128>) {
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(body), "lens")
        .expect("imports");
    let pids = model
        .faces
        .iter()
        .filter_map(|(f, _)| model.face_pid(f).map(|p| p.0))
        .collect();
    (report.lens.events().to_vec(), pids)
}

/// Every event is on a face that exists in the imported model.
fn on_real_faces(events: &[geometry_engine::lens::LensEvent], pids: &[u128]) {
    for e in events {
        match e.entity {
            EntityRef::Face(p) => assert!(pids.contains(&p.0), "event on an unknown face: {e:?}"),
            EntityRef::Solid(_) => {}
        }
    }
}

#[test]
fn a_clean_cube_has_an_empty_journal() {
    let (events, _) = import(&cube(Face5::Plain));
    assert!(events.is_empty(), "{events:#?}");
}

#[test]
fn a_reversed_bound_is_in_the_journal() {
    let (events, pids) = import(&cube(Face5::BoundF));
    on_real_faces(&events, &pids);
    let n = events
        .iter()
        .filter(|e| e.kind == EventKind::LoopReversed)
        .count();
    assert_eq!(n, 1, "{events:#?}");
    assert!(
        events
            .iter()
            .all(|e| e.kind != EventKind::LoopReversedToMatchNeighbours),
        "a legally reversed bound needs no repair: {events:#?}"
    );
}

#[test]
fn a_face_reversed_against_its_neighbours_is_in_the_journal() {
    let (events, pids) = import(&cube(Face5::Backwards));
    on_real_faces(&events, &pids);
    let n = events
        .iter()
        .filter(|e| e.kind == EventKind::LoopReversedToMatchNeighbours)
        .count();
    assert_eq!(n, 1, "{events:#?}");
}

/// A cube face written 0.005 mm off its edges (x = 1.005): its four corners
/// meet three faces each, so the gap is CLOSED, not tolerated -- each corner
/// is tightened onto the plane where its three faces meet, and the journal
/// says so on every face at that corner, with the move measured. (Before
/// tightening the gap was kept and each edge's tolerance raised to it; a
/// single face written off its edges, with nothing to pin its corners, is
/// still repaired that way -- tests/step_edge_tolerance_repair.rs.)
#[test]
fn a_face_written_off_its_corners_has_them_tightened_in_the_journal() {
    let (events, pids) = import(&cube(Face5::Lifted));
    on_real_faces(&events, &pids);
    let tightened: Vec<_> = events
        .iter()
        .filter(|e| e.kind == EventKind::VertexTightened)
        .collect();
    let faces: std::collections::HashSet<_> = tightened
        .iter()
        .map(|e| format!("{:?}", e.entity))
        .collect();
    assert_eq!(
        faces.len(),
        5,
        "recorded on the lifted face and the four faces sharing its corners: {events:#?}"
    );
    assert_eq!(
        tightened.len(),
        12,
        "four corners, each on its three faces: {events:#?}"
    );
    for e in &tightened {
        let get = |k: &str| {
            e.measured
                .iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| *v)
                .expect("measured")
        };
        assert!(
            (get("moved_mm") - 0.005).abs() < 1e-6,
            "moved {}",
            get("moved_mm")
        );
        assert!((get("off_faces_before_mm") - 0.005).abs() < 1e-6);
        assert!(
            get("off_faces_after_mm") <= 1e-9,
            "after {}",
            get("off_faces_after_mm")
        );
        assert!(e.step_instance.is_some(), "the vertex's STEP # is named");
    }
    assert!(
        events
            .iter()
            .all(|e| e.kind != EventKind::EdgeToleranceRaised),
        "with the gap closed no tolerance needs raising: {events:#?}"
    );
}
