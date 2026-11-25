//! THROWAWAY PROBE (not product code): import a STEP file and time the
//! tessellation of the listed faces alone.
//!
//! usage: tess_one_face <file.step> <face,face,...>
use std::time::Instant;

use export_engine::ExportEngine;
use geometry_engine::tessellation::edge_cache::EdgeSampleCache;
use geometry_engine::tessellation::mesh::TriangleMesh;
use geometry_engine::tessellation::{tessellate_face, TessellationParams};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_default();
    let faces: Vec<u32> = args
        .next()
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.parse().ok())
        .collect();
    let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap_or_default()).into_owned();
    let Ok((model, _)) = ExportEngine::new().import_step_content_uncertified(&text, "probe") else {
        println!("import failed");
        return;
    };
    let params = TessellationParams::default();
    for f in faces {
        let Some(face) = model.faces.get(f) else {
            println!("face {f}: absent");
            continue;
        };
        let cache = EdgeSampleCache::new(&params);
        let mut mesh = TriangleMesh::new();
        let t = Instant::now();
        tessellate_face(&face, &model, &params, &cache, &mut mesh);
        println!(
            "face {f} ({}, {:?}): {} tris in {:?}; inner loops {}; checksum {:016x}",
            model
                .surfaces
                .get(face.surface_id)
                .map(|s| s.type_name())
                .unwrap_or("?"),
            face.orientation,
            mesh.triangles.len(),
            t.elapsed(),
            face.inner_loops.len(),
            checksum(&mesh)
        );
        if std::env::var_os("PROBE_UV").is_some() {
            if let Some(surf) = model.surfaces.get(face.surface_id) {
                let b = surf.parameter_bounds();
                println!(
                    "  domain u {:?} v {:?}  periodic u {:?} v {:?}  closed u {} v {}",
                    b.0,
                    b.1,
                    surf.period_u(),
                    surf.period_v(),
                    surf.is_closed_u(),
                    surf.is_closed_v()
                );
                let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
                for v in &mesh.vertices {
                    let p = [v.position.x, v.position.y, v.position.z];
                    for k in 0..3 {
                        lo[k] = lo[k].min(p[k]);
                        hi[k] = hi[k].max(p[k]);
                    }
                }
                println!(
                    "  mesh bbox size {:.3} {:.3} {:.3}",
                    hi[0] - lo[0],
                    hi[1] - lo[1],
                    hi[2] - lo[2]
                );
                let tol = geometry_engine::math::Tolerance::from_distance(1e-6);
                let mut loops = vec![face.outer_loop];
                loops.extend(face.inner_loops.iter().copied());
                for l in loops {
                    let Some(lp) = model.loops.get(l) else {
                        continue;
                    };
                    for (i, &e) in lp.edges.iter().enumerate() {
                        let Some(edge) = model.edges.get(e) else {
                            continue;
                        };
                        let curve = model
                            .curves
                            .get(edge.curve_id)
                            .map(|c| c.type_name())
                            .unwrap_or("?");
                        if let Some(c) = model.curves.get(edge.curve_id) {
                            let r = edge.param_range;
                            for k in 0..=8 {
                                let t = r.start + (r.end - r.start) * k as f64 / 8.0;
                                let Ok(p) = c.point_at(t) else { continue };
                                match surf.closest_point(&p, tol) {
                                    Ok((u, v)) => {
                                        let q = surf.point_at(u, v).unwrap_or(p);
                                        println!(
                                            "    edge {i} t={t:.4} p=({:.3},{:.3},{:.3}) uv ({u:.6}, {v:.6}) gap {:.2e}",
                                            p.x, p.y, p.z, (q - p).magnitude()
                                        );
                                    }
                                    Err(err) => println!("    edge {i} t={t:.4}: ERR {err:?}"),
                                }
                            }
                        }
                        for vid in [edge.start_vertex, edge.end_vertex] {
                            let Some(vx) = model.vertices.get(vid) else {
                                continue;
                            };
                            let p = geometry_engine::math::Point3::new(
                                vx.position[0],
                                vx.position[1],
                                vx.position[2],
                            );
                            match surf.closest_point(&p, tol) {
                                Ok((u, v)) => {
                                    let q = surf.point_at(u, v).unwrap_or(p);
                                    println!(
                                        "  loop {l} edge {i} ({curve}) vertex {vid}: uv ({u:.6}, {v:.6}) gap {:.2e}",
                                        (q - p).magnitude()
                                    );
                                }
                                Err(err) => println!(
                                    "  loop {l} edge {i} vertex {vid}: closest_point ERR {err:?}"
                                ),
                            }
                        }
                    }
                }
            }
        }
        if std::env::var_os("PROBE_DUMP").is_some() {
            for v in &mesh.vertices {
                println!(
                    "  v ({:?},{:?},{:?})",
                    v.position.x, v.position.y, v.position.z
                );
            }
            for t in &mesh.triangles {
                println!("  t {:?}", t);
            }
        }
    }
}

fn checksum(mesh: &TriangleMesh) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut eat = |x: u64| {
        h ^= x;
        h = h.wrapping_mul(0x100000001b3);
    };
    for v in &mesh.vertices {
        eat(v.position.x.to_bits());
        eat(v.position.y.to_bits());
        eat(v.position.z.to_bits());
    }
    for t in &mesh.triangles {
        eat(u64::from(t[0]) << 40 | u64::from(t[1]) << 20 | u64::from(t[2]));
    }
    h
}
