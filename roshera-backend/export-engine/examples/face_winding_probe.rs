//! Diagnostic: per face, does its mesh wind with the face's outward normal?
//! usage: face_winding_probe <file.step>
use export_engine::ExportEngine;
use geometry_engine::math::{Point3, Tolerance};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
use std::collections::BTreeMap;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_default();
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!("cannot read {path}");
        return;
    };
    let Ok((model, _)) = ExportEngine::new().import_step_content(&text, "probe") else {
        eprintln!("import failed");
        return;
    };
    for (sid, solid) in model.solids.iter() {
        let mesh = tessellate_solid(solid, &model, &TessellationParams::coarse());
        let mut per: BTreeMap<u32, (usize, usize)> = BTreeMap::new();
        for (t, tri) in mesh.triangles.iter().enumerate() {
            let fid = mesh.face_map.get(t).copied().unwrap_or(u32::MAX);
            let [a, b, c] = tri.map(|i| mesh.vertices[i as usize].position);
            let g = (b - a).cross(&(c - a));
            let ctr = Point3::new(
                (a.x + b.x + c.x) / 3.0,
                (a.y + b.y + c.y) / 3.0,
                (a.z + b.z + c.z) / 3.0,
            );
            let Some(face) = model.faces.get(fid) else {
                continue;
            };
            let Some(surf) = model.surfaces.get(face.surface_id) else {
                continue;
            };
            let n = surf
                .closest_point(&ctr, Tolerance::default())
                .and_then(|(u, v)| surf.normal_at(u, v));
            let e = per.entry(fid).or_default();
            if let Ok(n) = n {
                if g.dot(&(n * face.orientation.sign())) >= 0.0 {
                    e.0 += 1
                } else {
                    e.1 += 1
                }
            }
        }
        // Directed-edge clash: two triangles traversing a shared edge the SAME way.
        let key = |i: u32| {
            let p = mesh.vertices[i as usize].position;
            (
                (p.x * 1e6).round() as i64,
                (p.y * 1e6).round() as i64,
                (p.z * 1e6).round() as i64,
            )
        };
        let mut dir: std::collections::HashMap<_, Vec<u32>> = std::collections::HashMap::new();
        for (t, tri) in mesh.triangles.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (key(tri[k]), key(tri[(k + 1) % 3]));
                dir.entry((a, b))
                    .or_default()
                    .push(mesh.face_map.get(t).copied().unwrap_or(u32::MAX));
            }
        }
        let mut clash: BTreeMap<(u32, u32), usize> = BTreeMap::new();
        for faces in dir.values() {
            if faces.len() >= 2 {
                let (x, y) = (faces[0].min(faces[1]), faces[0].max(faces[1]));
                *clash.entry((x, y)).or_default() += 1;
            }
        }
        for ((x, y), n) in &clash {
            println!("clash faces {x} / {y}: {n} same-direction edge(s)");
        }
        for (fid, (ok, bad)) in per {
            let face = model.faces.get(fid);
            let ty = face
                .and_then(|f| model.surfaces.get(f.surface_id))
                .map(|s| s.type_name())
                .unwrap_or("?");
            let inner = face.map(|f| f.inner_loops.len()).unwrap_or(0);
            let tris: Vec<usize> = (0..mesh.triangles.len())
                .filter(|&t| mesh.face_map.get(t) == Some(&fid))
                .collect();
            let mut c = [0.0; 3];
            let mut nsum = [0.0; 3];
            for &t in &tris {
                let [a, b, cc] = mesh.triangles[t].map(|i| mesh.vertices[i as usize].position);
                let g = (b - a).cross(&(cc - a));
                for (k, v) in [(a.x + b.x + cc.x), (a.y + b.y + cc.y), (a.z + b.z + cc.z)]
                    .iter()
                    .enumerate()
                {
                    c[k] += v / 3.0 / tris.len() as f64;
                }
                nsum[0] += g.x;
                nsum[1] += g.y;
                nsum[2] += g.z;
            }
            let orient = face
                .map(|f| format!("{:?}", f.orientation))
                .unwrap_or_default();
            println!(
                "solid {sid} face {fid} {ty:9} inner={inner} with={ok} against={bad} {orient} centre=({:.2},{:.2},{:.2}) meshN=({:.2},{:.2},{:.2})",
                c[0], c[1], c[2], nsum[0], nsum[1], nsum[2]
            );
        }
    }
}
