//! THROWAWAY SPIKE (not product code): does an incremental, content-keyed
//! kernel pay off on a real file? Meshes every face of a STEP import four ways:
//! cold serial, cold parallel, warm (cache keyed by a face fingerprint), and
//! after moving one part (only its faces should miss).
//!
//! usage: incremental_spike <file.step>

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use export_engine::ExportEngine;
use geometry_engine::math::{Matrix4, Vector3};
use geometry_engine::operations::transform::{transform_solid, TransformOptions};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::tessellation::edge_cache::EdgeSampleCache;
use geometry_engine::tessellation::{tessellate_face, TessellationParams, TriangleMesh};

/// Faces of a solid with the size-relative mesh parameters `tessellate_solid`
/// would use for it.
fn solid_faces(model: &BRepModel) -> Vec<(u32, u32, TessellationParams)> {
    let mut out = Vec::new();
    for (sid, solid) in model.solids.iter() {
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        let mut faces = Vec::new();
        for sh in solid.all_shells() {
            let Some(shell) = model.shells.get(sh) else {
                continue;
            };
            for &f in &shell.faces {
                faces.push(f);
                let Some(face) = model.faces.get(f) else {
                    continue;
                };
                let Some(lp) = model.loops.get(face.outer_loop) else {
                    continue;
                };
                for &e in &lp.edges {
                    let Some(edge) = model.edges.get(e) else {
                        continue;
                    };
                    for v in [edge.start_vertex, edge.end_vertex] {
                        if let Some(p) = model.vertices.get_position(v) {
                            for k in 0..3 {
                                lo[k] = lo[k].min(p[k]);
                                hi[k] = hi[k].max(p[k]);
                            }
                        }
                    }
                }
            }
        }
        let extent = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2))
            .sqrt()
            .max(1e-6);
        let base = TessellationParams::default();
        let params = TessellationParams {
            // What `tessellate_solid` does today: only the chord is floored.
            chord_tolerance: base.chord_tolerance.max(extent * 5e-4),
            ..base
        };
        for f in faces {
            out.push((sid, f, params.clone()));
        }
    }
    out
}

/// Geometric fingerprint of a face: its surface type, a 5×5 sample of the
/// surface, and its boundary vertices. Changes whenever the face moves or its
/// shape or trim changes.
fn fingerprint(model: &BRepModel, face_id: u32) -> u64 {
    let mut h = DefaultHasher::new();
    let q = |x: f64| (x * 1e6).round() as i64;
    let Some(face) = model.faces.get(face_id) else {
        return 0;
    };
    if let Some(s) = model.surfaces.get(face.surface_id) {
        s.type_name().hash(&mut h);
        let ((u0, u1), (v0, v1)) = s.parameter_bounds();
        let (u0, u1) = (u0.max(-1e3), u1.min(1e3));
        let (v0, v1) = (v0.max(-1e3), v1.min(1e3));
        for i in 0..5 {
            for j in 0..5 {
                let u = u0 + (u1 - u0) * i as f64 / 4.0;
                let v = v0 + (v1 - v0) * j as f64 / 4.0;
                if let Ok(p) = s.point_at(u, v) {
                    (q(p.x), q(p.y), q(p.z)).hash(&mut h);
                }
            }
        }
    }
    (face.orientation as u8).hash(&mut h);
    for lid in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
        let Some(lp) = model.loops.get(lid) else {
            continue;
        };
        for &e in &lp.edges {
            let Some(edge) = model.edges.get(e) else {
                continue;
            };
            for v in [edge.start_vertex, edge.end_vertex] {
                if let Some(p) = model.vertices.get_position(v) {
                    (q(p[0]), q(p[1]), q(p[2])).hash(&mut h);
                }
            }
        }
    }
    h.finish()
}

fn mesh_face(model: &BRepModel, face_id: u32, params: &TessellationParams) -> TriangleMesh {
    let mut mesh = TriangleMesh::new();
    if let Some(face) = model.faces.get(face_id) {
        let cache = EdgeSampleCache::new(params);
        tessellate_face(&face, model, params, &cache, &mut mesh);
    }
    mesh
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or_default();
    let content = String::from_utf8_lossy(&std::fs::read(&path).unwrap_or_default()).into_owned();
    // The route's shape: import without certifying, then certify once.
    let t = Instant::now();
    if let Ok((mut m, _)) = ExportEngine::new().import_step_content_uncertified(&content, "spike") {
        let read = t.elapsed();
        let ids: Vec<_> = m.solids.iter().map(|(sid, _)| sid).collect();
        let t2 = Instant::now();
        let certs = m.certify_solids(&ids);
        let sound = certs.iter().filter(|(_, c)| c.is_sound()).count();
        let watertight = certs.iter().filter(|(_, c)| c.watertight).count();
        let brep = certs.iter().filter(|(_, c)| c.brep_valid).count();
        println!(
            "verdict: {sound}/{} sound, {watertight} watertight, {brep} B-Rep valid",
            certs.len()
        );
        for (id, c) in &certs {
            if !c.is_sound() {
                println!(
                    "  part {id}: brep {} watertight {} manifold {} oriented {} errors {:?}",
                    c.brep_valid,
                    c.watertight,
                    c.manifold,
                    c.oriented,
                    c.errors.iter().take(2).collect::<Vec<_>>()
                );
            }
        }
        println!(
            "route shape: read {read:.2?} + certify once {:.2?} ({} solids)",
            t2.elapsed(),
            certs.len()
        );
    }
    let t = Instant::now();
    let Ok((mut model, _)) = ExportEngine::new().import_step_content(&content, "spike") else {
        println!("import failed");
        return;
    };
    println!("import (incl. certificate): {:.2?}", t.elapsed());
    if let Ok(dir) = std::env::var("SPIKE_EXPORT_DIR") {
        let engine = ExportEngine::with_output_directory(dir.clone());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        if let Ok(rt) = rt {
            match rt.block_on(engine.export_step(&model, "vice_reexport")) {
                Ok(name) => {
                    let path = std::path::Path::new(&dir).join(&name);
                    let text = std::fs::read_to_string(&path).unwrap_or_default();
                    let values = text
                        .lines()
                        .filter(|l| {
                            [
                                "=CARTESIAN_POINT(",
                                "=DIRECTION(",
                                "=VECTOR(",
                                "=AXIS2_PLACEMENT_3D(",
                                "=AXIS1_PLACEMENT(",
                            ]
                            .iter()
                            .any(|k| l.contains(k))
                        })
                        .count();
                    println!(
                        "re-export: {} bytes, {} entities, {} value entities",
                        text.len(),
                        text.lines().filter(|l| l.starts_with('#')).count(),
                        values
                    );
                }
                Err(e) => println!("re-export failed: {e:?}"),
            }
        }
    }
    let faces = solid_faces(&model);
    println!("faces: {}", faces.len());

    // 1. Cold, serial: today's cost.
    let t = Instant::now();
    let mut cache: HashMap<u64, usize> = HashMap::new();
    let mut tris = 0usize;
    let mut empty: Vec<(u32, &'static str)> = Vec::new();
    let mut slow: Vec<(std::time::Duration, u32, &'static str, usize)> = Vec::new();
    for (_, f, p) in &faces {
        let tf = Instant::now();
        let m = mesh_face(&model, *f, p);
        let dt = tf.elapsed();
        let kind = model
            .faces
            .get(*f)
            .and_then(|face| model.surfaces.get(face.surface_id))
            .map(|s| s.type_name())
            .unwrap_or("?");
        if m.triangles.is_empty() {
            empty.push((*f, kind));
        }
        slow.push((dt, *f, kind, m.triangles.len()));
        tris += m.triangles.len();
        cache.insert(fingerprint(&model, *f), m.triangles.len());
    }
    let cold = t.elapsed();
    println!("1 cold serial:   {cold:>10.2?}  ({tris} triangles)");
    if let Ok(csv) = std::env::var("SPIKE_CSV") {
        let mut rows = String::from(
            "face,kind,triangles,micros
",
        );
        let mut sorted = slow.clone();
        sorted.sort_by_key(|r| r.1);
        for (dt, f, k, n) in &sorted {
            rows.push_str(&format!(
                "{f},{k},{n},{}
",
                dt.as_micros()
            ));
        }
        let _ = std::fs::write(csv, rows);
    }
    let mut by_kind: HashMap<&str, usize> = HashMap::new();
    for (_, k) in &empty {
        *by_kind.entry(k).or_default() += 1;
    }
    println!("  EMPTY faces (0 triangles): {} {:?}", empty.len(), by_kind);
    slow.sort_by(|a, b| b.0.cmp(&a.0));
    for (dt, f, k, n) in slow.iter().take(5) {
        println!("  slow face {f} {k}: {dt:.2?} ({n} triangles)");
    }

    // 2. Cold, parallel over faces.
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let t = Instant::now();
    let chunk = faces.len().div_ceil(threads).max(1);
    let model_ref = &model;
    let par_tris: usize = std::thread::scope(|scope| {
        let handles: Vec<_> = faces
            .chunks(chunk)
            .map(|c| {
                scope.spawn(move || {
                    c.iter()
                        .map(|(_, f, p)| mesh_face(model_ref, *f, p).triangles.len())
                        .sum::<usize>()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or(0)).sum()
    });
    let par = t.elapsed();
    println!(
        "2 cold parallel: {par:>10.2?}  ({threads} threads, {par_tris} triangles, {:.1}x)",
        cold.as_secs_f64() / par.as_secs_f64().max(1e-9)
    );

    // 3. Warm: the same model again -- every face a cache hit.
    let t = Instant::now();
    let mut hits = 0;
    for (_, f, p) in &faces {
        if cache.contains_key(&fingerprint(&model, *f)) {
            hits += 1;
        } else {
            mesh_face(&model, *f, p);
        }
    }
    let warm = t.elapsed();
    println!(
        "3 warm (reopen): {warm:>10.2?}  ({hits}/{} hits, {:.0}x)",
        faces.len(),
        cold.as_secs_f64() / warm.as_secs_f64().max(1e-9)
    );

    // 4. Edit: move the largest part 5 mm; only its faces should miss.
    let mut per_solid: HashMap<u32, usize> = HashMap::new();
    for (s, _, _) in &faces {
        *per_solid.entry(*s).or_default() += 1;
    }
    let Some((&moved, &moved_faces)) = per_solid.iter().max_by_key(|(_, n)| **n) else {
        return;
    };
    let mut opts = TransformOptions::default();
    opts.common.validate_before = false;
    opts.common.validate_result = false;
    if let Err(e) = transform_solid(
        &mut model,
        moved,
        Matrix4::from_translation(&Vector3::new(5.0, 0.0, 0.0)),
        opts,
    ) {
        println!("4 edit: moving the part FAILED ({e:?}); no edit numbers");
        return;
    }
    let t = Instant::now();
    let (mut hit, mut miss) = (0, 0);
    for (_, f, p) in &faces {
        let key = fingerprint(&model, *f);
        if cache.contains_key(&key) {
            hit += 1;
        } else {
            miss += 1;
            let m = mesh_face(&model, *f, p);
            cache.insert(key, m.triangles.len());
        }
    }
    let edit = t.elapsed();
    if miss != moved_faces {
        println!("4 edit: WARNING remeshed {miss} faces but the moved part has {moved_faces}");
    }
    println!(
        "4 edit (moved the {moved_faces}-face part): {edit:>10.2?}  ({miss} remeshed, {hit} reused)"
    );
}
