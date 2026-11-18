//! Scratch timing probe (not committed): where does registering an imported
//! assembly spend its time? Imports a STEP file, merges it into a fresh model
//! the way the import route does, then times one whole-model validation, one
//! orphan scan, and the per-solid certificate pieces on a sample of solids.

use std::time::Instant;

use export_engine::ExportEngine;
use geometry_engine::harness::watertight::manifold_and_shell_orientation_report;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::validation::{
    count_orphan_faces, validate_model_enhanced, ValidationLevel,
};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

fn main() {
    let path = std::env::args().nth(1).unwrap_or_default();
    let content = String::from_utf8_lossy(&std::fs::read(&path).unwrap_or_default()).into_owned();

    let t = Instant::now();
    let Ok((imported, _report)) = ExportEngine::new().import_step_content(&content, "probe") else {
        println!("import failed");
        return;
    };
    println!(
        "import_step_content (incl. its own per-solid certify): {:?}",
        t.elapsed()
    );

    let mut dst = BRepModel::new();
    let t = Instant::now();
    let ids = export_engine::formats::step::merge_solids_into(&mut dst, &imported);
    println!(
        "merge {} solids: {:?}  faces={}",
        ids.len(),
        t.elapsed(),
        dst.faces.len()
    );

    // PROBE_SOLIDS=569,570 : mesh only those parts at the volume
    // (fine) resolution, print size and time, and stop.
    if let Ok(list) = std::env::var("PROBE_SOLIDS") {
        for sid in list.split(',').filter_map(|s| s.trim().parse::<u32>().ok()) {
            let Some(solid) = dst.solids.get(sid) else {
                println!("solid {sid}: absent");
                continue;
            };
            let extent = dst.solid_world_bbox(sid).map(|b| b.size());
            let diag = extent
                .map(|s| (s.x * s.x + s.y * s.y + s.z * s.z).sqrt())
                .unwrap_or(1.0);
            let vol = |m: &geometry_engine::tessellation::TriangleMesh| -> f64 {
                m.triangles
                    .iter()
                    .map(|t| {
                        let a = m.vertices[t[0] as usize].position;
                        let b = m.vertices[t[1] as usize].position;
                        let c = m.vertices[t[2] as usize].position;
                        (a.x * (b.y * c.z - b.z * c.y) - a.y * (b.x * c.z - b.z * c.x)
                            + a.z * (b.x * c.y - b.y * c.x))
                            / 6.0
                    })
                    .sum()
            };
            let skip_fine = std::env::var("PROBE_SKIP_FINE").is_ok();
            let mut variants: Vec<(String, TessellationParams)> = Vec::new();
            if !skip_fine {
                variants.push(("fine".into(), TessellationParams::fine()));
            }
            variants.push((
                "mass_properties".into(),
                TessellationParams::mass_properties(solid, &dst),
            ));
            variants.push(("default".into(), TessellationParams::default()));
            for (label, params) in variants {
                let t = Instant::now();
                let mesh = tessellate_solid(solid, &dst, &params);
                println!(
                    "solid {sid}: {label} tess {:?} tris={} volume={:.6} (diag {diag:.2}, bbox {extent:?})",
                    t.elapsed(),
                    mesh.triangles.len(),
                    vol(&mesh)
                );
            }
        }
        return;
    }

    let t = Instant::now();
    let v = validate_model_enhanced(&dst, dst.tolerance, ValidationLevel::Standard);
    println!(
        "validate_model_enhanced x1: {:?} (errors {})",
        t.elapsed(),
        v.errors.len()
    );
    let t = Instant::now();
    let orphans = count_orphan_faces(&dst);
    println!("count_orphan_faces x1: {:?} ({orphans})", t.elapsed());

    let t = Instant::now();
    let mut slowest: Vec<(std::time::Duration, u32, usize)> = Vec::new();
    for &id in &ids {
        let ti = Instant::now();
        let _ = dst.calculate_solid_volume(id);
        let faces = dst
            .solids
            .get(id)
            .and_then(|s| dst.shells.get(s.outer_shell))
            .map(|sh| sh.faces.len())
            .unwrap_or(0);
        println!(
            "  volume {}/{}: solid {id} faces={faces} {:?} (total {:?})",
            slowest.len() + 1,
            ids.len(),
            ti.elapsed(),
            t.elapsed()
        );
        slowest.push((ti.elapsed(), id, faces));
    }
    println!("warm volumes all {}: {:?}", ids.len(), t.elapsed());
    slowest.sort_by(|a, b| b.0.cmp(&a.0));
    for (d, id, faces) in slowest.iter().take(8) {
        println!("  volume solid {id}: {d:?} faces={faces}");
    }
    let t = Instant::now();
    let _ = dst.certify_solid(ids[0]);
    println!("certify_solid one (cold after warm?): {:?}", t.elapsed());

    let t = Instant::now();
    let certs = dst.certify_solids(&ids);
    let sound = certs.iter().filter(|(_, c)| c.is_sound()).count();
    println!(
        "certify_solids all {}: {:?} (sound {sound})",
        certs.len(),
        t.elapsed()
    );
    return;

    let params = TessellationParams::default();
    let step = (ids.len() / 8).max(1);
    for &sid in ids.iter().step_by(step).take(8) {
        let t = Instant::now();
        let mesh = dst
            .solids
            .get(sid)
            .map(|s| tessellate_solid(s, &dst, &params));
        let t_tess = t.elapsed();
        let t = Instant::now();
        let _ = manifold_and_shell_orientation_report(&dst, sid, 0.1, 1e-6);
        let t_man = t.elapsed();
        let t = Instant::now();
        let cert = dst.certify_solid(sid);
        let t_cert = t.elapsed();
        println!(
            "solid {sid}: tris={} display_tess={t_tess:?} manifold={t_man:?} certify_total={t_cert:?} sound={}",
            mesh.map(|m| m.triangles.len()).unwrap_or(0),
            cert.is_sound()
        );
    }
}
