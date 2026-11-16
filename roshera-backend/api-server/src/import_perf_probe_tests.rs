//! Measurement probe for the import / open / snapshot paths — the costs
//! behind "the app feels very slow". Ignored by default; it reads real STEP
//! files from a directory and prints timings, it asserts nothing about them.
//!
//! ```text
//! ROSHERA_PERF_STEP_DIR=<dir of *.step / */output.step> ROSHERA_PERF_N=16 \
//!   cargo test -j 4 --bin api-server -- import_perf_probe --ignored --nocapture
//! ```
//!
//! Each file is imported through the live route; while the first import
//! runs, a reader times how long `model.read()` waits (is the app frozen?).
//! Then the scene snapshot is fetched twice (bytes, time), and the document
//! is re-opened (`documents::activate`, the tab-switch path) and timed.

#![cfg(test)]

use crate::durability_boot_tests::{build_state, open_db, post, temp_db_path};
use crate::{build_router, durability};

use axum::body::to_bytes;
use std::time::{Duration, Instant};
use tower::ServiceExt;

fn step_files(dir: &std::path::Path, n: usize) -> Vec<std::path::PathBuf> {
    let mut files: Vec<(u64, std::path::PathBuf)> = Vec::new();
    let mut push = |p: std::path::PathBuf| {
        let ext = p
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if ext == "step" || ext == "stp" {
            if let Ok(meta) = std::fs::metadata(&p) {
                files.push((meta.len(), p));
            }
        }
    };
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if let Ok(inner) = std::fs::read_dir(&p) {
                    for e in inner.flatten() {
                        push(e.path());
                    }
                }
            } else {
                push(p);
            }
        }
    }
    // The smallest first: a debug build imports large parts for minutes.
    files.sort();
    files.into_iter().take(n).map(|(_, p)| p).collect()
}

/// Determinism probe: the same file imported repeatedly — does each part
/// (keyed by its content-derived PID) come back with the same solid id and
/// the same face-id / edge-id sequence? Prints what varies.
#[test]
#[ignore = "determinism probe: prints what varies between imports of one file"]
fn import_determinism_probe() {
    let dir = std::env::var("ROSHERA_PERF_STEP_DIR").ok();
    let mut texts: Vec<(String, String)> = Vec::new();
    if let Some(dir) = dir {
        for f in step_files(std::path::Path::new(&dir), 2) {
            let bytes = std::fs::read(&f).expect("readable");
            texts.push((
                f.display().to_string(),
                export_engine::formats::step::parser::decode_step_bytes(bytes),
            ));
        }
    }
    let mut two = geometry_engine::primitives::topology_builder::BRepModel::new();
    {
        let mut b = geometry_engine::primitives::topology_builder::TopologyBuilder::new(&mut two);
        b.create_box_3d(5.9, 5.3, 1.7).expect("box");
        b.create_box_3d(2.3, 9.7, 4.3).expect("box");
    }
    let mut p = std::env::temp_dir();
    p.push(format!("roshera_det_{}.step", uuid::Uuid::new_v4()));
    tokio::runtime::Runtime::new()
        .expect("rt")
        .block_on(export_engine::formats::step::export_brep_to_step(&two, &p))
        .expect("export");
    texts.push((
        "two boxes".into(),
        std::fs::read_to_string(&p).expect("read"),
    ));
    let _ = std::fs::remove_file(&p);

    for (label, text) in &texts {
        let mut runs = Vec::new();
        for _ in 0..4 {
            let (imported, _) =
                export_engine::formats::step::import_step_text_uncertified(text, "probe")
                    .expect("import");
            let mut model = geometry_engine::primitives::topology_builder::BRepModel::new();
            let ids = export_engine::formats::step::merge_solids_into(&mut model, &imported);
            let mut parts: Vec<(String, u32, Vec<u32>, usize)> = ids
                .iter()
                .map(|&id| {
                    let s = model.solids.get(id).expect("solid");
                    let faces: Vec<u32> = model
                        .shells
                        .get(s.outer_shell)
                        .map(|sh| sh.faces.clone())
                        .unwrap_or_default();
                    (
                        format!("{:?}", model.solid_pid(id)),
                        id,
                        faces,
                        model.edges.len(),
                    )
                })
                .collect();
            parts.sort();
            runs.push(parts);
        }
        let same = runs.windows(2).all(|w| w[0] == w[1]);
        eprintln!(
            "DET {label}: {} parts; identical (pid -> solid id, face ids) across 4 imports: {same}",
            runs[0].len()
        );
        if !same {
            for r in &runs {
                eprintln!(
                    "DET   {:?}",
                    r.iter()
                        .map(|p| (p.0.clone(), p.1, p.2.first().copied()))
                        .collect::<Vec<_>>()
                );
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "measurement probe: needs ROSHERA_PERF_STEP_DIR, prints timings"]
async fn import_perf_probe() {
    let Ok(dir) = std::env::var("ROSHERA_PERF_STEP_DIR") else {
        eprintln!("ROSHERA_PERF_STEP_DIR not set; nothing to measure");
        return;
    };
    let n: usize = std::env::var("ROSHERA_PERF_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(16);
    let files = step_files(std::path::Path::new(&dir), n);

    // What an import registration spends its time on, per phase, for the
    // first few files (fresh model each, exactly the route's calls).
    for file in files.iter().take(4) {
        let bytes = std::fs::read(file).expect("STEP file must be readable");
        let text = export_engine::formats::step::parser::decode_step_bytes(bytes);
        let t = Instant::now();
        let (imported, _report) =
            export_engine::formats::step::import_step_text_uncertified(&text, "probe")
                .expect("import must succeed");
        let parse = t.elapsed();
        let mut model = geometry_engine::primitives::topology_builder::BRepModel::new();
        let ids = export_engine::formats::step::merge_solids_into(&mut model, &imported);
        let t = Instant::now();
        model.calculate_solid_volumes(&ids);
        let volumes = t.elapsed();
        let t = Instant::now();
        let _ = model.certify_solids(&ids);
        let certify = t.elapsed();
        let t = Instant::now();
        let _ = geometry_engine::tessellation::tessellate_solids(
            &model,
            &ids,
            &geometry_engine::tessellation::TessellationParams::default(),
        );
        let tessellate = t.elapsed();
        eprintln!(
            "PERF phases {}: parse {parse:?}, volumes {volumes:?}, certify {certify:?}, tessellate {tessellate:?}",
            file.display()
        );
    }

    let state = build_state(open_db(&temp_db_path()).await, true).await;

    let mut import_total = Duration::ZERO;
    let mut worst_read_wait = Duration::ZERO;
    for (i, file) in files.iter().enumerate() {
        let request = post(
            "/api/geometry/import_step",
            serde_json::json!({ "path": file.to_string_lossy(), "name": format!("part {i}") }),
        );
        let router = build_router(state.clone());
        let t = Instant::now();
        let import = tokio::spawn(async move { router.oneshot(request).await });
        // A reader arriving mid-import: how long is the model unreadable?
        let mut waits = Vec::new();
        while !import.is_finished() {
            tokio::time::sleep(Duration::from_millis(25)).await;
            let r = Instant::now();
            let guard = state.model.read().await;
            drop(guard);
            waits.push(r.elapsed());
        }
        let response = import.await.expect("import task").expect("import response");
        let status = response.status();
        let took = t.elapsed();
        import_total += took;
        let worst = waits.iter().max().copied().unwrap_or_default();
        worst_read_wait = worst_read_wait.max(worst);
        eprintln!(
            "PERF import {i} {} -> {status} in {took:?}; worst model.read() wait {worst:?}",
            file.display()
        );
    }
    state
        .timeline_recorder
        .flush()
        .await
        .expect("recorder flush must succeed");
    let solids = state.model.read().await.solids.len();
    eprintln!(
        "PERF imports: {} files, {solids} solids, total {import_total:?}, worst read wait {worst_read_wait:?}",
        files.len()
    );

    for pass in 0..2 {
        let t = Instant::now();
        let response = build_router(state.clone())
            .oneshot(crate::durability_boot_tests::get("/api/scene/snapshot"))
            .await
            .expect("snapshot response");
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("snapshot body");
        eprintln!(
            "PERF snapshot pass {pass}: {} bytes in {:?}",
            bytes.len(),
            t.elapsed()
        );
    }

    let rss = || {
        use sysinfo::{Pid, System};
        let mut sys = System::new();
        let pid = Pid::from_u32(std::process::id());
        sys.refresh_process(pid);
        sys.process(pid)
            .map(|p| p.memory() / (1024 * 1024))
            .unwrap_or(0)
    };
    let (rss_before, frames_before) = (rss(), state.frames.len());
    let t = Instant::now();
    let status = crate::documents::activate(&state, durability::DURABILITY_SESSION_ID).await;
    eprintln!(
        "PERF activate (re-open): {:?}; solids after = {}; status = {status:?}",
        t.elapsed(),
        state.model.read().await.solids.len()
    );
    eprintln!(
        "PERF activate memory: rss {rss_before} MiB -> {} MiB; frames {frames_before} -> {}",
        rss(),
        state.frames.len()
    );

    let t = Instant::now();
    let response = build_router(state.clone())
        .oneshot(crate::durability_boot_tests::get("/api/scene/snapshot"))
        .await
        .expect("snapshot response");
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("snapshot body");
    eprintln!(
        "PERF snapshot after re-open: {} bytes in {:?}",
        bytes.len(),
        t.elapsed()
    );

    // The re-open's phases, one at a time, on the same history.
    let rows = state
        .database
        .load_all_timeline_events(durability::DURABILITY_SESSION_ID)
        .await
        .expect("event log must read");
    let events: Vec<timeline_engine::TimelineEvent> = rows
        .iter()
        .filter_map(|r| serde_json::from_value(r.data.clone()).ok())
        .collect();
    let t = Instant::now();
    let _ = timeline_engine::certify_rebuild(&events, None);
    eprintln!(
        "PERF phase certify_rebuild ({} events): {:?}",
        events.len(),
        t.elapsed()
    );
    let t = Instant::now();
    let mut m = geometry_engine::primitives::topology_builder::BRepModel::new();
    let _ = timeline_engine::rebuild_model_from_events(&mut m, &events);
    eprintln!("PERF phase rebuild_model_from_events: {:?}", t.elapsed());
    let frames = timeline_engine::frames::FrameCache::new(64);
    let t = Instant::now();
    let _ =
        timeline_engine::rebuild_certificate::certify_rebuild_with_frames(&events, None, &frames);
    eprintln!(
        "PERF phase certify_rebuild_with_frames (cold): {:?}",
        t.elapsed()
    );
    let t = Instant::now();
    let _ = frames.replay(&events);
    eprintln!("PERF phase frames.replay (warm): {:?}", t.elapsed());
    let t = Instant::now();
    let _ = timeline_engine::drawing_history::rebuild_drawings(&events, &frames);
    eprintln!(
        "PERF phase rebuild_drawings (warm frames): {:?}",
        t.elapsed()
    );
}
