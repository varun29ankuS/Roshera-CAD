// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! An edge shorter than its own tolerance has no extent -- it is a point --
//! but as an edge it keeps two vertices a hair apart, and the mesh weld
//! joins or separates them by rounding, so the faces meeting there do not
//! close (CADGenBench part 107: a 1.2 micrometre edge at a four-face corner
//! left four open mesh edges). The importer collapses such an edge and says
//! so.

use export_engine::ExportEngine;
use geometry_engine::primitives::topology_builder::{BRepModel, TopologyBuilder};

/// A box written with one corner split by a 1.2 micrometre edge: the first
/// EDGE_CURVE is cut just short of its end vertex, and the stub from there
/// to the vertex is a second edge on the same line, in both loops that use
/// the original.
async fn box_with_a_micro_edge() -> String {
    let mut model = BRepModel::new();
    TopologyBuilder::new(&mut model)
        .create_box_3d(20.0, 30.0, 10.0)
        .expect("box");
    let mut path = std::env::temp_dir();
    path.push(format!("roshera_micro_edge_{}.step", std::process::id()));
    export_engine::formats::step::export_brep_to_step(&model, &path)
        .await
        .expect("export");
    let text = std::fs::read_to_string(&path).expect("read back");
    let _ = std::fs::remove_file(&path);

    let entity = |l: &str| l.split('=').next().unwrap_or("").trim().to_string();
    let find = |id: &str| -> String {
        text.lines()
            .find(|l| entity(l) == id)
            .unwrap_or_else(|| panic!("entity {id}"))
            .to_string()
    };
    // The first straight EDGE_CURVE('',#start,#end,#line,.T.);
    let ec = text
        .lines()
        .find(|l| {
            l.contains("=EDGE_CURVE(") && {
                let curve = l.split(',').nth(3).unwrap_or("").trim().to_string();
                find(&curve).contains("=LINE(")
            }
        })
        .expect("a straight edge")
        .to_string();
    let parts: Vec<&str> = ec.split(',').collect();
    let (start, end, line) = (parts[1].trim(), parts[2].trim(), parts[3].trim());
    let coords = |vp: &str| -> Vec<f64> {
        let pt = find(vp)
            .split(',')
            .nth(1)
            .unwrap_or("")
            .split(')')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        let l = find(&pt);
        let open = l.rfind('(').expect("(");
        let close = open + l[open..].find(')').expect(")");
        l[open + 1..close]
            .split(',')
            .map(|c| c.trim().parse().expect("number"))
            .collect()
    };
    let (a, b) = (coords(start), coords(end));
    let len: f64 = a
        .iter()
        .zip(&b)
        .map(|(x, y)| (y - x) * (y - x))
        .sum::<f64>()
        .sqrt();
    let t = 1.0 - 1.2e-6 / len;
    let cut: Vec<f64> = a.iter().zip(&b).map(|(x, y)| x + (y - x) * t).collect();

    // New point/vertex at the cut, the original edge ends there, the stub
    // runs from it to the old end on the same line.
    let added = format!(
        "#900001=CARTESIAN_POINT('',({:.15},{:.15},{:.15}));\n#900002=VERTEX_POINT('',#900001);\n\
         #900003=EDGE_CURVE('',#900002,{end},{line},.T.);",
        cut[0], cut[1], cut[2]
    );
    let shortened = ec.replacen(&format!(",{end},"), ",#900002,", 1);
    let ec_id = entity(&ec);
    let mut out = String::new();
    let mut next_oe = 900100;
    for l in text.lines() {
        if l == ec {
            out.push_str(&shortened);
            out.push('\n');
            out.push_str(&added);
            out.push('\n');
            continue;
        }
        // Every ORIENTED_EDGE of the original gets a twin for the stub,
        // placed so the loop still chains: after it when walked forward,
        // before it when walked backward.
        if l.contains("=ORIENTED_EDGE(") && l.contains(&format!(",{ec_id},")) {
            let forward = l.contains(".T.");
            let oe = entity(l);
            let twin = format!("#{next_oe}");
            next_oe += 1;
            out.push_str(l);
            out.push('\n');
            out.push_str(&format!(
                "{twin}=ORIENTED_EDGE('',*,*,#900003,{});\n",
                if forward { ".T." } else { ".F." }
            ));
            // Remember the substitution for the loop lists below.
            out.push_str(&format!(
                "/*SPLIT {oe} {twin} {}*/\n",
                if forward { 1 } else { 0 }
            ));
            continue;
        }
        out.push_str(l);
        out.push('\n');
    }
    // Splice the twins into the EDGE_LOOP lists.
    let mut text = out.clone();
    for l in out.lines().filter(|l| l.starts_with("/*SPLIT ")) {
        let f: Vec<&str> = l
            .trim_start_matches("/*SPLIT ")
            .trim_end_matches("*/")
            .split(' ')
            .collect();
        let (oe, twin, fwd) = (f[0], f[1], f[2] == "1");
        let pattern_mid = format!("{oe},");
        let loop_line = text
            .lines()
            .find(|x| {
                x.contains("=EDGE_LOOP(")
                    && (x.contains(&format!("({oe},"))
                        || x.contains(&format!(",{oe},"))
                        || x.contains(&format!(",{oe}))"))
                        || x.contains(&format!("({oe}))")))
            })
            .expect("its loop")
            .to_string();
        let replaced = if fwd {
            loop_line.replacen(oe, &format!("{oe},{twin}"), 1)
        } else {
            loop_line.replacen(oe, &format!("{twin},{oe}"), 1)
        };
        let _ = pattern_mid;
        text = text.replace(&loop_line, &replaced);
    }
    text.lines()
        .filter(|l| !l.starts_with("/*SPLIT "))
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn an_edge_shorter_than_its_tolerance_is_collapsed_and_the_box_is_sound() {
    let text = box_with_a_micro_edge().await;
    let (mut model, report) = ExportEngine::new()
        .import_step_content(&text, "micro")
        .expect("imports");
    let solids: Vec<u32> = model.solids.iter().map(|(id, _)| id).collect();
    assert_eq!(
        solids.len(),
        1,
        "one solid: {:?}",
        report.warnings.iter().take(5).collect::<Vec<_>>()
    );
    let cert = model.certify_solid(solids[0]);
    assert!(
        cert.is_sound(),
        "the box certifies sound: {:?}",
        cert.errors
    );
    assert!(
        report
            .healings
            .iter()
            .any(|h| format!("{:?}", h.kind) == "SmallEdgeCollapsed"),
        "the collapse is reported: {:?}",
        report.healings
    );
}
