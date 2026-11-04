//! STEP probe: import one third-party STEP file through the production
//! import path, then try the drawing pipeline on its solids.
//!
//! Usage: `cargo run -p export-engine --example step_probe -- <file.step> <out_dir> [max_drawn]`
//!
//! Prints one `FILE` summary line, one `SOLID` line per solid, and the top
//! unsupported / failed entity names. Writes an SVG per drawn solid into
//! `<out_dir>` so a human can look at the sheet.

use std::collections::BTreeMap;
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::Instant;

use export_engine::ExportEngine;
use geometry_engine::drawing::{render_drawing_svg, standard_drawing_auto, verify_drawing};
use geometry_engine::primitives::feature_recognition::recognize_features;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::readable::extract_dimensions;

fn face_count(model: &BRepModel, solid_id: u32) -> usize {
    let Some(solid) = model.solids.get(solid_id) else {
        return 0;
    };
    solid
        .all_shells()
        .iter()
        .filter_map(|sh| model.shells.get(*sh))
        .map(|sh| sh.faces.len())
        .sum()
}

fn top(map: &BTreeMap<String, u64>, n: usize) -> String {
    let mut v: Vec<_> = map.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1));
    v.iter()
        .take(n)
        .map(|(k, c)| format!("{k}={c}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: step_probe <file.step> <out_dir> [max_drawn]");
        std::process::exit(2);
    }
    let path = &args[1];
    let out_dir = std::path::Path::new(&args[2]);
    let max_drawn: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(8);
    let stem = std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().replace(' ', "_"))
        .unwrap_or_else(|| "file".to_string());

    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            println!("FILE {stem} READ_ERROR {e}");
            return;
        }
    };
    let content = String::from_utf8_lossy(&bytes).into_owned();

    let t0 = Instant::now();
    let engine = ExportEngine::new();
    let imported = catch_unwind(AssertUnwindSafe(|| {
        engine.import_step_content(&content, "probe")
    }));
    let import_ms = t0.elapsed().as_millis();
    let (model, report) = match imported {
        Ok(Ok(pair)) => pair,
        Ok(Err(e)) => {
            println!("FILE {stem} IMPORT_ERROR ms={import_ms} err={e}");
            return;
        }
        Err(_) => {
            println!("FILE {stem} IMPORT_PANIC ms={import_ms}");
            return;
        }
    };

    let mut solids: Vec<(u32, usize)> = model
        .solids
        .iter()
        .map(|(id, _)| (id, face_count(&model, id)))
        .collect();
    solids.sort_by(|a, b| b.1.cmp(&a.1));

    let sound = report.validation.iter().filter(|v| v.sound).count();
    let brep_valid = report.validation.iter().filter(|v| v.brep_valid).count();
    println!(
        "FILE {stem} ok={} schema={} unit={} import_ms={import_ms} solids={} brep_valid={brep_valid} sound={sound} roots={} warnings={} healings={} manifold_warn={}",
        report.ok,
        report.schema.as_deref().unwrap_or("?"),
        report.source_unit.as_deref().unwrap_or("?"),
        solids.len(),
        report.roots_resolved,
        report.warnings.len(),
        report.healings.len(),
        report.manifold_warnings.len(),
    );
    println!("  UNSUPPORTED {}", top(&report.counts.unsupported, 60));
    println!("  FAILED {}", top(&report.counts.failed, 12));
    println!("  SKIPPED {}", top(&report.counts.skipped, 8));
    let mut warn_kinds: BTreeMap<String, u64> = BTreeMap::new();
    for w in &report.warnings {
        let msg: String = w.message.chars().take(70).collect();
        *warn_kinds.entry(msg).or_default() += 1;
    }
    for (m, c) in warn_kinds.iter().take(6) {
        println!("  WARN x{c} {m}");
    }
    let _ = std::io::stdout().flush();

    let _ = std::fs::create_dir_all(out_dir);
    let mut feature_totals: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (rank, (sid, faces)) in solids.iter().enumerate() {
        let v = report.validation.iter().find(|v| v.solid_id == *sid);
        let feats =
            catch_unwind(AssertUnwindSafe(|| recognize_features(*sid, &model))).unwrap_or_default();
        for f in &feats {
            *feature_totals.entry(f.feature_type()).or_default() += 1;
        }
        let dims = catch_unwind(AssertUnwindSafe(|| extract_dimensions(&model, *sid)))
            .map(|d| d.len().to_string())
            .unwrap_or_else(|_| "PANIC".into());

        let mut drawing_col = "not_attempted".to_string();
        if rank < max_drawn {
            let t = Instant::now();
            let res = catch_unwind(AssertUnwindSafe(|| {
                standard_drawing_auto(&model, *sid, uuid::Uuid::new_v4())
            }));
            let ms = t.elapsed().as_millis();
            drawing_col = match res {
                Ok(Ok(d)) => {
                    let q = verify_drawing(&d);
                    let ndims: usize = d.views.iter().map(|v| v.dimensions.len()).sum();
                    let svg = render_drawing_svg(&d);
                    let file = out_dir.join(format!("{stem}__solid{sid}.svg"));
                    let _ = std::fs::write(&file, svg);
                    format!(
                        "OK ms={ms} views={} dims={ndims} holes={} passed={} errs={} warns={}",
                        d.views.len(),
                        d.hole_sites.len(),
                        q.passed,
                        q.error_count(),
                        q.warning_count()
                    )
                }
                Ok(Err(e)) => format!("ERR ms={ms} {e}"),
                Err(_) => format!("PANIC ms={ms}"),
            };
        }
        println!(
            "  SOLID {sid} faces={faces} sound={} brep_valid={} watertight={} manifold={} oriented={} errs={} features={} dim_records={dims} drawing: {drawing_col}",
            v.map(|v| v.sound.to_string()).unwrap_or_else(|| "?".into()),
            v.map(|v| v.brep_valid.to_string()).unwrap_or_else(|| "?".into()),
            v.map(|v| v.watertight.to_string()).unwrap_or_else(|| "?".into()),
            v.map(|v| v.manifold.to_string()).unwrap_or_else(|| "?".into()),
            v.map(|v| v.oriented.to_string()).unwrap_or_else(|| "?".into()),
            v.map(|v| v.error_count.to_string()).unwrap_or_else(|| "?".into()),
            feats.len(),
        );
        let _ = std::io::stdout().flush();
    }
    println!(
        "  FEATURES {}",
        feature_totals
            .iter()
            .map(|(k, c)| format!("{k}={c}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
}
