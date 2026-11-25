//! THROWAWAY PROBE (not product code): import every `*/output.step` under a
//! folder, certify it once, and print one line per part plus a summary.
//!
//! usage: corpus_probe <folder>

use std::time::Instant;

use export_engine::ExportEngine;

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_default();
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .map(|r| r.filter_map(Result::ok).map(|e| e.path()).collect())
        .unwrap_or_default();
    entries.sort();
    let (mut parts, mut imported, mut sound, mut watertight, mut brep) = (0, 0, 0, 0, 0);
    let mut total = std::time::Duration::ZERO;
    for case in entries {
        let path = case.join("output.step");
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        parts += 1;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let name = case
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let t = Instant::now();
        let result = ExportEngine::new().import_step_content(&text, &name);
        let dt = t.elapsed();
        total += dt;
        match result {
            Ok((mut model, report)) => {
                if std::env::var("PROBE_DETAIL").is_ok() {
                    let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
                    for w in &report.warnings {
                        let head: String = w.message.chars().take(70).collect();
                        let key = format!("{:?} {} | {}", w.severity, w.entity, head);
                        *kinds.entry(key).or_default() += 1;
                    }
                    let mut v: Vec<_> = kinds.into_iter().collect();
                    v.sort_by(|a, b| b.1.cmp(&a.1));
                    for (k, n) in v.iter().take(12) {
                        println!("  warn x{n}: {k}");
                    }
                    let mut un: std::collections::BTreeMap<&str, usize> = Default::default();
                    for u in &report.unsupported {
                        *un.entry(u.entity.as_str()).or_default() += 1;
                    }
                    println!("  unsupported: {un:?}");
                    println!("  healings: {}", report.healings.len());
                }
                if let Ok(dir) = std::env::var("PROBE_DUMP") {
                    let mut lines: Vec<String> = report
                        .warnings
                        .iter()
                        .map(|w| {
                            format!(
                                "{:?} {} #{:?} {}",
                                w.severity, w.entity, w.instance, w.message
                            )
                        })
                        .collect();
                    lines.extend(report.healings.iter().map(|h| format!("HEAL {h:?}")));
                    let _ = std::fs::write(
                        std::path::Path::new(&dir).join(format!("{name}.txt")),
                        lines.join(
                            "
",
                        ),
                    );
                }
                // Faces that mesh to nothing, by surface type: an open
                // certificate mesh usually starts there.
                let mut empty: std::collections::BTreeMap<&str, usize> = Default::default();
                let params = geometry_engine::tessellation::TessellationParams::default();
                for (_, solid) in model.solids.iter() {
                    for sh in solid.all_shells() {
                        let Some(shell) = model.shells.get(sh) else {
                            continue;
                        };
                        for &fid in &shell.faces {
                            let Some(face) = model.faces.get(fid) else {
                                continue;
                            };
                            let cache =
                                geometry_engine::tessellation::edge_cache::EdgeSampleCache::new(
                                    &params,
                                );
                            let mut m = geometry_engine::tessellation::TriangleMesh::new();
                            geometry_engine::tessellation::tessellate_face(
                                face, &model, &params, &cache, &mut m,
                            );
                            if m.triangles.is_empty() {
                                let kind = model
                                    .surfaces
                                    .get(face.surface_id)
                                    .map(|s| s.type_name())
                                    .unwrap_or("?");
                                *empty.entry(kind).or_default() += 1;
                            }
                        }
                    }
                }
                if !empty.is_empty() {
                    println!("  {name} empty faces: {empty:?}");
                }
                if !model.solids.is_empty() {
                    imported += 1;
                }
                let v = &report.validation;
                let s = v.iter().filter(|x| x.sound).count();
                let w = v.iter().filter(|x| x.watertight).count();
                let b = v.iter().filter(|x| x.brep_valid).count();
                if !v.is_empty() && s == v.len() {
                    sound += 1;
                }
                if !v.is_empty() && w == v.len() {
                    watertight += 1;
                }
                if !v.is_empty() && b == v.len() {
                    brep += 1;
                }
                let why = v
                    .iter()
                    .flat_map(|x| x.errors.iter().take(1))
                    .next()
                    .map(|e| e.chars().take(90).collect::<String>())
                    .unwrap_or_default();
                // Every blocking check, with how many faces (or "-" for a
                // solid-wide one) it names -- the full inventory, not only
                // the first error.
                let ids: Vec<u32> = model.solids.iter().map(|(id, _)| id).collect();
                let mut blocks: std::collections::BTreeMap<String, usize> = Default::default();
                for sid in ids {
                    let cert = model.certify_solid(sid);
                    if std::env::var("PROBE_SELFX").is_ok()
                        && cert
                            .blocking
                            .iter()
                            .any(|b| b.conjunct == "self_intersection_free")
                    {
                        let tess_bad: Vec<u32> = cert
                            .blocking
                            .iter()
                            .filter(|b| {
                                b.conjunct == "tessellation" || b.conjunct == "mesh_quality"
                            })
                            .filter_map(|b| b.face)
                            .collect();
                        let pairs =
                            geometry_engine::harness::self_intersection::self_intersecting_faces(
                                &model, sid, 0.5,
                            );
                        let kind = |f: u32| {
                            model
                                .faces
                                .get(f)
                                .and_then(|f| model.surfaces.get(f.surface_id))
                                .map(|s| s.type_name())
                                .unwrap_or("?")
                        };
                        println!("    selfx {} pairs", pairs.len());
                        for (a, b) in pairs.iter().take(8) {
                            println!(
                                "    selfx {a} {} {} x {b} {} {}",
                                kind(*a),
                                if tess_bad.contains(a) {
                                    "[TESS-BAD]"
                                } else {
                                    ""
                                },
                                kind(*b),
                                if tess_bad.contains(b) {
                                    "[TESS-BAD]"
                                } else {
                                    ""
                                }
                            );
                        }
                    }
                    for b in &cert.blocking {
                        *blocks.entry(b.conjunct.clone()).or_default() += 1;
                        // PROBE_FACES: each blocking face's surface kind and
                        // what was measured on it.
                        if std::env::var("PROBE_FACES").is_ok() {
                            let kind = b
                                .face
                                .and_then(|f| model.faces.get(f))
                                .and_then(|f| model.surfaces.get(f.surface_id))
                                .map(|s| s.type_name())
                                .unwrap_or("-");
                            // The faces across each of its edges.
                            let mut next: Vec<u32> = Vec::new();
                            if let Some(f) = b.face.and_then(|f| model.faces.get(f)) {
                                let mut loops = vec![f.outer_loop];
                                loops.extend(f.inner_loops.iter().copied());
                                for l in loops {
                                    let Some(lp) = model.loops.get(l) else {
                                        continue;
                                    };
                                    for &e in &lp.edges {
                                        for (fid, other) in model.faces.iter() {
                                            if Some(fid) == b.face {
                                                continue;
                                            }
                                            let mut ol = vec![other.outer_loop];
                                            ol.extend(other.inner_loops.iter().copied());
                                            if ol.iter().any(|&x| {
                                                model
                                                    .loops
                                                    .get(x)
                                                    .is_some_and(|q| q.edges.contains(&e))
                                            }) && !next.contains(&fid)
                                            {
                                                next.push(fid);
                                            }
                                        }
                                    }
                                }
                            }
                            next.sort();
                            println!(
                                "    {} face {:?} {kind} {:?} next to {:?}",
                                b.conjunct, b.face, b.measured, next
                            );
                        }
                    }
                }
                if !blocks.is_empty() {
                    println!("  {name} blocking: {blocks:?}");
                }
                println!(
                    "{name}: {} solids, sound {s}/{}, watertight {w}, brep {b}, {} warnings, {dt:.2?} {why}",
                    model.solids.len(),
                    v.len(),
                    report.warnings.len()
                );
            }
            Err(e) => println!("{name}: IMPORT FAILED {e:?} ({dt:.2?})"),
        }
    }
    println!(
        "SUMMARY: {parts} parts, {imported} imported, {sound} sound, {watertight} watertight, \
         {brep} B-Rep valid, total {total:.1?}"
    );
}
