//! THROWAWAY PROBE (not product code): time each stage of a solid's
//! self-certificate separately -- the meshes it builds and the checks run
//! over them -- on an imported STEP part.
//!
//! usage: cert_stage_probe <file.step>

use std::time::Instant;

use export_engine::ExportEngine;
use geometry_engine::harness::watertight::{
    facet_true_normals, manifold_report_mesh, mesh_quality_with_normals,
    tessellation_quality_with_normals,
};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

fn main() {
    let path = std::env::args().nth(1).unwrap_or_default();
    let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap_or_default()).into_owned();
    let t = Instant::now();
    let Ok((model, _)) = ExportEngine::new().import_step_content_uncertified(&text, "probe") else {
        println!("import failed");
        return;
    };
    println!("import (uncertified): {:?}", t.elapsed());
    let ids: Vec<u32> = model.solids.iter().map(|(id, _)| id).collect();
    for sid in ids {
        let Some(solid) = model.solids.get(sid) else {
            continue;
        };
        let cert_params = TessellationParams {
            chord_tolerance: 0.1,
            ..TessellationParams::default()
        };
        let t = Instant::now();
        let cert_mesh = tessellate_solid(&solid, &model, &cert_params);
        println!(
            "solid {sid}: certificate mesh (chord 0.1): {} tris, {} verts, {:?}",
            cert_mesh.triangles.len(),
            cert_mesh.vertices.len(),
            t.elapsed()
        );
        let t = Instant::now();
        let report = manifold_report_mesh(&cert_mesh, 1e-6);
        println!(
            "solid {sid}: manifold analysis of it: {:?} (closed {:?})",
            t.elapsed(),
            report.map(|r| r.closed)
        );
        let t = Instant::now();
        let display = tessellate_solid(&solid, &model, &TessellationParams::default());
        println!(
            "solid {sid}: display mesh (chord 0.001): {} tris, {} verts, {:?}",
            display.triangles.len(),
            display.vertices.len(),
            t.elapsed()
        );
        let t = Instant::now();
        let normals = facet_true_normals(&model, &display);
        println!("solid {sid}: facet true normals: {:?}", t.elapsed());
        let t = Instant::now();
        let tq = tessellation_quality_with_normals(&model, &display, &normals);
        println!(
            "solid {sid}: tessellation quality: {:?} (clean {:?})",
            t.elapsed(),
            tq.map(|q| q.clean)
        );
        let t = Instant::now();
        let mq = mesh_quality_with_normals(&model, &display, &normals);
        println!(
            "solid {sid}: mesh quality: {:?}\n  RESULT {:?}",
            t.elapsed(),
            mq
        );
    }
}
