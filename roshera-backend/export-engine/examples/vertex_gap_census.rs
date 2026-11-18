//! THROWAWAY PROBE (not product code): for every edge end whose vertex lies
//! more than 1e-3 off the edge's curve after import, say which of the two is
//! ON the faces that meet there -- the vertex, or the curve's end point --
//! by measuring each one's distance to every face surface at that vertex.
//!
//! usage: vertex_gap_census <file.step> [<file.step> ...]

use std::collections::{BTreeMap, BTreeSet};

use export_engine::ExportEngine;
use geometry_engine::math::{Point3, Tolerance};

fn main() {
    let (mut vertex_wins, mut curve_wins, mut neither, mut total) = (0, 0, 0, 0);
    for path in std::env::args().skip(1) {
        let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap_or_default()).into_owned();
        let Ok((model, _report)) =
            ExportEngine::new().import_step_content_uncertified(&text, "probe")
        else {
            println!("{path}: import failed");
            continue;
        };
        // Faces touching each vertex (through any edge of any loop).
        let mut vertex_faces: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
        for (fid, face) in model.faces.iter() {
            for l in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
                let Some(lp) = model.loops.get(l) else {
                    continue;
                };
                for &eid in &lp.edges {
                    if let Some(e) = model.edges.get(eid) {
                        vertex_faces.entry(e.start_vertex).or_default().insert(fid);
                        vertex_faces.entry(e.end_vertex).or_default().insert(fid);
                    }
                }
            }
        }
        let off_faces = |p: &Point3, faces: &BTreeSet<u32>| -> Vec<(String, f64)> {
            faces
                .iter()
                .filter_map(|&f| {
                    let face = model.faces.get(f)?;
                    let s = model.surfaces.get(face.surface_id)?;
                    let (u, v) = s.closest_point(p, Tolerance::default()).ok()?;
                    let q = s.point_at(u, v).ok()?;
                    Some((format!("{}#{f}", s.type_name()), p.distance(&q)))
                })
                .collect()
        };
        let mut lines = 0;
        for (eid, e) in model.edges.iter() {
            let Some(c) = model.curves.get(e.curve_id) else {
                continue;
            };
            let r = e.param_range;
            let (Ok(a), Ok(b)) = (c.evaluate(r.start), c.evaluate(r.end)) else {
                continue;
            };
            for vid in [e.start_vertex, e.end_vertex] {
                let Some(v) = model.vertices.get(vid) else {
                    continue;
                };
                let p = v.point();
                let end = if p.distance(&a.position) <= p.distance(&b.position) {
                    a.position
                } else {
                    b.position
                };
                let gap = p.distance(&end);
                if gap <= 1e-3 {
                    continue;
                }
                total += 1;
                let faces = vertex_faces.get(&vid).cloned().unwrap_or_default();
                let dv = off_faces(&p, &faces);
                let dc = off_faces(&end, &faces);
                let worst = |d: &[(String, f64)]| d.iter().map(|x| x.1).fold(0.0_f64, f64::max);
                let (wv, wc) = (worst(&dv), worst(&dc));
                if wv <= 1e-5 && wc > 1e-5 {
                    vertex_wins += 1;
                } else if wc <= 1e-5 && wv > 1e-5 {
                    curve_wins += 1;
                } else {
                    neither += 1;
                }
                if lines < 12 {
                    lines += 1;
                    println!(
                        "  edge {eid} ({}) vertex {vid}: gap {gap:.3e}; {} faces; vertex off faces {wv:.2e}, curve end off faces {wc:.2e}  {:?}",
                        c.type_name(),
                        faces.len(),
                        dv.iter()
                            .zip(dc.iter())
                            .map(|(x, y)| format!("{} v{:.1e}/c{:.1e}", x.0, x.1, y.1))
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
        println!("{path}: done");
    }
    println!(
        "TOTAL {total} off-curve edge ends: vertex on faces {vertex_wins}, curve end on faces {curve_wins}, neither/both {neither}"
    );
}
