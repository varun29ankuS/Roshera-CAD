//! Scratch timing probe (not committed): how long does a drawing-driven edit
//! take to plan on a real history?
//!
//! usage: drive_probe <history.json> <dimension pid>
//! `history.json` is `GET /api/timeline/history/main` (a JSON array of events).

use std::time::Instant;

use timeline_engine::dimension_drive::resolve_dimension_driver_measured;
use timeline_engine::rebuild_certificate::certify_rebuild;
use timeline_engine::types::{Author, EventId, EventMetadata, Operation, TimelineEvent};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(path), Some(pid)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: drive_probe <history.json> <pid>");
        return;
    };
    let text = std::fs::read_to_string(path).unwrap_or_default();
    // The API's history DTO drops the struct-level inputs/outputs/metadata
    // (the recorded solid IO lives inside `parameters`), so rebuild each event
    // from its id, sequence and operation -- the fields replay reads.
    let raw: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap_or_default();
    let mut events: Vec<TimelineEvent> = raw
        .iter()
        .filter_map(|v| {
            Some(TimelineEvent {
                id: EventId(uuid::Uuid::parse_str(v["id"].as_str()?).ok()?),
                sequence_number: v["sequence_number"].as_u64()?,
                timestamp: chrono::Utc::now(),
                author: Author::System,
                operation: serde_json::from_value::<Operation>(v["operation"].clone()).ok()?,
                inputs: Default::default(),
                outputs: Default::default(),
                metadata: EventMetadata::default(),
            })
        })
        .collect();
    events.sort_by_key(|e| e.sequence_number);
    println!("events: {}", events.len());

    let t = Instant::now();
    let (_model, cert) = certify_rebuild(&events, None);
    println!(
        "one full certified rebuild: {:?} (sound {})",
        t.elapsed(),
        cert.is_sound()
    );

    let t = Instant::now();
    let (r, stats) = resolve_dimension_driver_measured(&events, pid);
    println!("resolve_dimension_driver: {:?} {stats:?}", t.elapsed());
    println!(
        "{}",
        serde_json::to_string(&r)
            .unwrap_or_default()
            .chars()
            .take(600)
            .collect::<String>()
    );
}
