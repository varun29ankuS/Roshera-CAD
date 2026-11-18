//! THROWAWAY PROBE (not product code): import STEP files without certifying
//! and print every importer warning about an edge's vertex lying off its
//! curve, with a per-file count.
//!
//! usage: trim_census <file.step> [<file.step> ...]

use export_engine::ExportEngine;

fn main() {
    for path in std::env::args().skip(1) {
        let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap_or_default()).into_owned();
        let Ok((model, report)) =
            ExportEngine::new().import_step_content_uncertified(&text, "probe")
        else {
            println!("{path}: import failed");
            continue;
        };
        let hits: Vec<_> = report
            .warnings
            .iter()
            .filter(|w| {
                std::env::var_os("CENSUS_ALL").is_some() || w.message.contains("off its curve")
            })
            .collect();
        println!(
            "{path}: {} off-curve warning(s), {} faces, {} edges, {} solids",
            hits.len(),
            model.faces.iter().count(),
            model.edges.iter().count(),
            model.solids.iter().count()
        );
        for w in hits {
            println!("  #{} {}: {}", w.instance.unwrap_or(0), w.entity, w.message);
        }
    }
}
