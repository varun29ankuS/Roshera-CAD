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
