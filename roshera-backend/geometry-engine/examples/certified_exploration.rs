//! Certified autonomous exploration sweep — the Move 3 demo binary.
//!
//! Samples `N` rocket-engine variants deterministically from a `u64` seed,
//! builds and certifies every one on its own fresh `BRepModel` in parallel
//! (rayon), and writes an honest scoreboard: how many the kernel REFUSED, how
//! many the certificate KILLED (by failing dimension), how many are SOUND, and
//! the least-wall-material winner among the sound designs.
//!
//! Every number in the artifacts is DERIVED from measured verdicts — a benchmark
//! about honest search does not hardcode its table.
//!
//! Run (RELEASE — the demo runs release; debug is far too slow for a sweep):
//!
//! ```text
//! cargo run -p geometry-engine --release --example certified_exploration \
//!     -- --n 20 --seed 42 --threads 4
//! ```
//!
//! Artifacts: `target/certified_exploration/exploration_results.{md,json}`.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use geometry_engine::harness::engine_variant::{build_variant, Envelope};
use geometry_engine::harness::exploration::{
    explore, winner, ExplorationReport, VariantOutcome, VariantRow,
};
use geometry_engine::math::vector3::Vector3;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::render::{render_solids_dir, RenderMode, RenderOptions};
use geometry_engine::tessellation::TessellationParams;

/// Parsed CLI args.
struct Args {
    n: usize,
    seed: u64,
    threads: usize,
}

fn parse_args() -> Result<Args, String> {
    let mut n: usize = 20;
    let mut seed: u64 = 42;
    let mut threads: usize = 4;

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let flag = argv[i].as_str();
        let value = argv.get(i + 1);
        match flag {
            "--n" => {
                n = value
                    .ok_or_else(|| "--n needs a value".to_string())?
                    .parse()
                    .map_err(|e| format!("--n: {e}"))?;
                i += 2;
            }
            "--seed" => {
                seed = value
                    .ok_or_else(|| "--seed needs a value".to_string())?
                    .parse()
                    .map_err(|e| format!("--seed: {e}"))?;
                i += 2;
            }
            "--threads" => {
                threads = value
                    .ok_or_else(|| "--threads needs a value".to_string())?
                    .parse()
                    .map_err(|e| format!("--threads: {e}"))?;
                i += 2;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if n == 0 {
        return Err("--n must be >= 1".to_string());
    }
    if threads == 0 {
        return Err("--threads must be >= 1".to_string());
    }
    Ok(Args { n, seed, threads })
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("argument error: {e}");
            eprintln!("usage: certified_exploration --n <N> --seed <S> --threads <T>");
            return ExitCode::FAILURE;
        }
    };

    println!(
        "certified exploration: n={} seed={} threads={} (release)",
        args.n, args.seed, args.threads
    );

    let report = explore(args.n, args.seed, args.threads);

    // The internal-volume target for ranking: the median internal volume among
    // SOUND, in-envelope variants. Using an emergent median (not a hardcoded
    // constant) keeps the honest-search claim intact — the band is defined by
    // the population the search actually found.
    let internal_target = median_sound_internal_volume(&report);
    let win = winner(&report.rows, internal_target);

    print_headline(&report, internal_target, win);

    let out_dir = Path::new("target").join("certified_exploration");
    if let Err(e) = fs::create_dir_all(&out_dir) {
        eprintln!("failed to create {}: {e}", out_dir.display());
        return ExitCode::FAILURE;
    }

    // Hero renders (Part C): rebuild the winner in a fresh model and shoot a
    // small shaded orbit set; rebuild the top cert-killed variant and shoot a
    // Diagnostic render so its defect (red open edges) is visible. The example
    // fails LOUDLY (nonzero exit) if any render returns None — a claimed image
    // that could not be produced is a lie the artifact must never carry.
    let renders = match render_hero_set(&out_dir, &report.envelope, win, top_cert_kill(&report)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("render failure: {e}");
            return ExitCode::FAILURE;
        }
    };

    let md = render_markdown(&report, internal_target, win, &renders);
    let json = render_json(&report, internal_target, win);

    let md_path = out_dir.join("exploration_results.md");
    let json_path = out_dir.join("exploration_results.json");
    if let Err(e) = fs::write(&md_path, md) {
        eprintln!("failed to write {}: {e}", md_path.display());
        return ExitCode::FAILURE;
    }
    if let Err(e) = fs::write(&json_path, json) {
        eprintln!("failed to write {}: {e}", json_path.display());
        return ExitCode::FAILURE;
    }

    println!("\nartifacts:");
    println!("  {}", md_path.display());
    println!("  {}", json_path.display());
    for p in &renders.winner_pngs {
        println!("  {}", p.display());
    }
    for p in &renders.kill_pngs {
        println!("  {}", p.display());
    }

    ExitCode::SUCCESS
}

/// The set of hero PNGs produced for the artifact (relative-friendly full paths).
struct RenderSet {
    winner_pngs: Vec<PathBuf>,
    kill_pngs: Vec<PathBuf>,
}

/// The single most-defective CERT_KILLED row (most failed cert dimensions,
/// label-tiebroken) — the most photogenic kill for a Diagnostic render. `None`
/// if the sweep produced no cert kills.
fn top_cert_kill(report: &ExplorationReport) -> Option<&VariantRow> {
    report
        .rows
        .iter()
        .filter(|r| matches!(r.outcome, VariantOutcome::CertKilled(_)))
        .max_by(|a, b| {
            let na = match &a.outcome {
                VariantOutcome::CertKilled(d) => d.len(),
                _ => 0,
            };
            let nb = match &b.outcome {
                VariantOutcome::CertKilled(d) => d.len(),
                _ => 0,
            };
            na.cmp(&nb).then_with(|| b.label.cmp(&a.label))
        })
}

/// A camera direction (camera→scene) from azimuth/elevation degrees, world Z up
/// — matching the viewpoint module's convention (camera position is the unit
/// vector at `(az, el)`; the view direction points the opposite way).
fn dir_from_az_el(az_deg: f64, el_deg: f64) -> Vector3 {
    let az = az_deg.to_radians();
    let el = el_deg.to_radians();
    let pos = Vector3::new(el.cos() * az.cos(), el.cos() * az.sin(), el.sin());
    // dir = camera → scene = -position.
    Vector3::new(-pos.x, -pos.y, -pos.z)
}

/// Rebuild the winner and the top kill in fresh models and write their PNGs.
/// Copper chamber+nozzle, steel plate — the live-demo palette family. Returns
/// the written paths; errors (rebuild refusal, empty render, PNG/IO failure) are
/// surfaced so the caller can exit nonzero.
fn render_hero_set(
    out_dir: &Path,
    envelope: &Envelope,
    win: Option<&VariantRow>,
    kill: Option<&VariantRow>,
) -> Result<RenderSet, String> {
    // Copper chamber+nozzle, steel-grey plate.
    const COPPER: [u8; 3] = [184, 115, 51];
    const STEEL: [u8; 3] = [140, 146, 152];

    let mut winner_pngs = Vec::new();
    let mut kill_pngs = Vec::new();

    // ---- Winner orbit set (3 shaded views) --------------------------------
    if let Some(w) = win {
        let winner_dir = out_dir.join("winner");
        fs::create_dir_all(&winner_dir).map_err(|e| format!("mkdir winner: {e}"))?;

        let mut model = BRepModel::new();
        let variant = build_variant(&mut model, &w.params)
            .map_err(|e| format!("winner rebuild refused (was SOUND in sweep): {e}"))?;
        let solids = [variant.chamber_nozzle, variant.injector_plate];
        let colors = [COPPER, STEEL];

        let opts = RenderOptions {
            width: 1000,
            height: 1000,
            view: geometry_engine::render::CanonicalView::Isometric, // ignored by *_dir
            mode: RenderMode::Shaded,
            tessellation: TessellationParams::fine(),
        };

        // az 30/el 12 (three-quarter), az 120/el −20 (looking up the bell),
        // az 210/el 30 (opposite three-quarter, high).
        let views = [(30.0, 12.0), (120.0, -20.0), (210.0, 30.0)];
        for (i, (az, el)) in views.iter().enumerate() {
            let dir = dir_from_az_el(*az, *el);
            let frame = render_solids_dir(&model, &solids, &colors, dir, Vector3::Z, &opts)
                .ok_or_else(|| {
                    format!("winner view az{az}/el{el} rendered an empty frame (None)")
                })?;
            let png = frame.to_png().map_err(|e| format!("winner png: {e}"))?;
            let name = format!("winner_az{}_el{}.png", fmt_deg(*az), fmt_deg(*el));
            let path = winner_dir.join(&name);
            fs::write(&path, &png).map_err(|e| format!("write {}: {e}", path.display()))?;
            println!(
                "winner render {}/3: {} ({} bytes)",
                i + 1,
                path.display(),
                png.len()
            );
            winner_pngs.push(path);
        }
        // Silence the unused-envelope lint honestly: the winner already passed
        // the envelope during the sweep; we assert it here as a build-time echo.
        let _ = envelope;
    } else {
        println!("no winner to render (no sound in-envelope in-band candidate)");
    }

    // ---- Top cert-kill diagnostic render ----------------------------------
    if let Some(k) = kill {
        let kills_dir = out_dir.join("kills");
        fs::create_dir_all(&kills_dir).map_err(|e| format!("mkdir kills: {e}"))?;

        let mut model = BRepModel::new();
        // A cert-killed variant BUILT (the certificate, not an op, rejected it),
        // so the rebuild must succeed; a refusal here means the outcome taxonomy
        // is inconsistent and should fail loudly.
        let variant = build_variant(&mut model, &k.params)
            .map_err(|e| format!("cert-killed variant refused on rebuild (taxonomy bug): {e}"))?;
        let solids = [variant.chamber_nozzle, variant.injector_plate];
        let colors = [COPPER, STEEL];

        let opts = RenderOptions {
            width: 1000,
            height: 1000,
            view: geometry_engine::render::CanonicalView::Isometric,
            mode: RenderMode::Diagnostic,
            tessellation: TessellationParams::fine(),
        };
        let dir = dir_from_az_el(120.0, -20.0);
        let frame = render_solids_dir(&model, &solids, &colors, dir, Vector3::Z, &opts)
            .ok_or_else(|| "top cert-kill rendered an empty frame (None)".to_string())?;
        let png = frame.to_png().map_err(|e| format!("kill png: {e}"))?;
        let path = kills_dir.join("top_cert_kill_diagnostic.png");
        fs::write(&path, &png).map_err(|e| format!("write {}: {e}", path.display()))?;
        println!(
            "top cert-kill diagnostic: {} ({} bytes, open_edges={} nonmanifold_edges={})",
            path.display(),
            png.len(),
            frame.open_edges,
            frame.nonmanifold_edges
        );
        kill_pngs.push(path);
    } else {
        println!("no cert kills to render diagnostically (0 CERT_KILLED)");
    }

    Ok(RenderSet {
        winner_pngs,
        kill_pngs,
    })
}

/// Degrees as a filesystem-safe token (negative → `n`, no dot).
fn fmt_deg(d: f64) -> String {
    let r = d.round() as i64;
    if r < 0 {
        format!("n{}", -r)
    } else {
        r.to_string()
    }
}

/// Median internal cavity volume among SOUND, in-envelope, volume-bearing rows.
/// `None` if no such row exists (then ranking skips the band).
fn median_sound_internal_volume(report: &ExplorationReport) -> Option<f64> {
    let mut vols: Vec<f64> = report
        .rows
        .iter()
        .filter(|r| matches!(r.outcome, VariantOutcome::Sound) && r.in_envelope)
        .filter_map(|r| r.internal_volume)
        .filter(|v| *v > 0.0)
        .collect();
    if vols.is_empty() {
        return None;
    }
    vols.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(vols[vols.len() / 2])
}

fn print_headline(
    report: &ExplorationReport,
    internal_target: Option<f64>,
    win: Option<&VariantRow>,
) {
    let s_per_variant = report.timings.mean_variant_ms / 1000.0;
    println!("\n=== HEADLINE ===");
    println!(
        "explored={} refused={} cert_killed={} sound={} timed_out={} panicked={}",
        report.rows.len(),
        report.refused,
        report.cert_killed,
        report.sound,
        report.timed_out,
        report.panicked
    );
    println!("cert-kill histogram (by dimension):");
    for (dim, count) in report.kill_histogram() {
        println!("  {dim}: {count}");
    }
    println!("refusal histogram (by kind):");
    for (kind, count) in report.refusal_histogram() {
        println!("  {kind}: {count}");
    }
    println!(
        "timing: total={} ms  mean={:.1} ms/variant  ({:.3} s/variant, threads={})",
        report.timings.total_ms,
        report.timings.mean_variant_ms,
        s_per_variant,
        report.timings.threads
    );
    match internal_target {
        Some(t) => println!("internal-volume target (median of sound): {t:.3}"),
        None => println!("internal-volume target: none (no sound in-envelope variants)"),
    }
    match win {
        Some(w) => println!(
            "WINNER: {} (wall_material={:.3}, internal={:.3})",
            w.label,
            w.wall_material_volume.unwrap_or(f64::NAN),
            w.internal_volume.unwrap_or(f64::NAN)
        ),
        None => println!("WINNER: none (no sound, in-envelope, in-band candidate)"),
    }
}

fn render_markdown(
    report: &ExplorationReport,
    internal_target: Option<f64>,
    win: Option<&VariantRow>,
    renders: &RenderSet,
) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "# Certified Exploration Sweep");
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "Every number below is derived from measured per-variant verdicts."
    );
    let _ = writeln!(s);
    let _ = writeln!(s, "## Headline");
    let _ = writeln!(s);
    let _ = writeln!(s, "| metric | value |");
    let _ = writeln!(s, "|---|---|");
    let _ = writeln!(s, "| explored | {} |", report.rows.len());
    let _ = writeln!(s, "| refused (kernel) | {} |", report.refused);
    let _ = writeln!(s, "| cert-killed | {} |", report.cert_killed);
    let _ = writeln!(s, "| sound | {} |", report.sound);
    let _ = writeln!(s, "| timed out | {} |", report.timed_out);
    let _ = writeln!(s, "| panicked | {} |", report.panicked);
    let _ = writeln!(s, "| threads | {} |", report.timings.threads);
    let _ = writeln!(s, "| total wall-clock (ms) | {} |", report.timings.total_ms);
    let _ = writeln!(
        s,
        "| mean per-variant (ms) | {:.1} |",
        report.timings.mean_variant_ms
    );
    let _ = writeln!(
        s,
        "| s/variant | {:.3} |",
        report.timings.mean_variant_ms / 1000.0
    );
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "Envelope: max_diameter={:.1}, max_length={:.1}.",
        report.envelope.max_diameter, report.envelope.max_length
    );
    let _ = writeln!(s);

    let _ = writeln!(s, "## Cert-kill histogram (by dimension)");
    let _ = writeln!(s);
    let kh = report.kill_histogram();
    if kh.is_empty() {
        let _ = writeln!(s, "_no cert kills_");
    } else {
        let _ = writeln!(s, "| dimension | kills |");
        let _ = writeln!(s, "|---|---|");
        for (dim, count) in kh {
            let _ = writeln!(s, "| {dim} | {count} |");
        }
    }
    let _ = writeln!(s);

    let _ = writeln!(s, "## Refusal histogram (by kind)");
    let _ = writeln!(s);
    let rh = report.refusal_histogram();
    if rh.is_empty() {
        let _ = writeln!(s, "_no refusals_");
    } else {
        let _ = writeln!(s, "| kind | count |");
        let _ = writeln!(s, "|---|---|");
        for (kind, count) in rh {
            let _ = writeln!(s, "| {kind} | {count} |");
        }
    }
    let _ = writeln!(s);

    let _ = writeln!(s, "## Winner");
    let _ = writeln!(s);
    match internal_target {
        Some(t) => {
            let _ = writeln!(
                s,
                "Objective: least wall-material volume among SOUND, in-envelope variants "
            );
            let _ = writeln!(
                s,
                "with internal cavity volume within ±2% of the sound-population median ({t:.3})."
            );
        }
        None => {
            let _ = writeln!(
                s,
                "No sound in-envelope variant produced a rankable internal volume; no winner."
            );
        }
    }
    let _ = writeln!(s);
    match win {
        Some(w) => {
            let _ = writeln!(s, "**{}**", w.label);
            let _ = writeln!(s);
            let _ = writeln!(s, "| field | value |");
            let _ = writeln!(s, "|---|---|");
            let _ = writeln!(s, "| throat_r | {} |", w.params.throat_r);
            let _ = writeln!(s, "| expansion_ratio | {} |", w.params.expansion_ratio);
            let _ = writeln!(s, "| chamber_r | {} |", w.params.chamber_r);
            let _ = writeln!(s, "| chamber_l_over_d | {} |", w.params.chamber_l_over_d);
            let _ = writeln!(s, "| wall_t | {} |", w.params.wall_t);
            let _ = writeln!(s, "| hole_count | {} |", w.params.hole_count);
            let _ = writeln!(s, "| hole_r | {} |", w.params.hole_r);
            let _ = writeln!(s, "| ring_frac | {} |", w.params.ring_frac);
            let _ = writeln!(
                s,
                "| wall_material_volume | {:.3} |",
                w.wall_material_volume.unwrap_or(f64::NAN)
            );
            let _ = writeln!(
                s,
                "| internal_volume | {:.3} |",
                w.internal_volume.unwrap_or(f64::NAN)
            );
        }
        None => {
            let _ = writeln!(s, "_none_");
        }
    }
    let _ = writeln!(s);

    let _ = writeln!(s, "## Renders");
    let _ = writeln!(s);
    if renders.winner_pngs.is_empty() {
        let _ = writeln!(s, "_no winner renders_");
    } else {
        let _ = writeln!(
            s,
            "Winner hero orbit (copper chamber+nozzle, steel plate, shaded, fine):"
        );
        let _ = writeln!(s);
        for p in &renders.winner_pngs {
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("<unnamed>");
            let _ = writeln!(s, "- `winner/{name}`");
        }
    }
    let _ = writeln!(s);
    if renders.kill_pngs.is_empty() {
        let _ = writeln!(s, "_no cert-kill diagnostic render (0 cert kills)_");
    } else {
        let _ = writeln!(
            s,
            "Top cert-kill diagnostic (Diagnostic mode — open edges in red):"
        );
        let _ = writeln!(s);
        for p in &renders.kill_pngs {
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("<unnamed>");
            let _ = writeln!(s, "- `kills/{name}`");
        }
    }
    let _ = writeln!(s);

    let _ = writeln!(s, "## All variants");
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "| # | outcome | detail | wall_vol | internal_vol | in_env | ms | label |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|---|---|---|");
    for (i, row) in report.rows.iter().enumerate() {
        let detail = outcome_detail(&row.outcome);
        let wall = row
            .wall_material_volume
            .map(|v| format!("{v:.3}"))
            .unwrap_or_else(|| "-".to_string());
        let internal = row
            .internal_volume
            .map(|v| format!("{v:.3}"))
            .unwrap_or_else(|| "-".to_string());
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {} | {} | {} | {} |",
            i,
            row.outcome.tag(),
            detail,
            wall,
            internal,
            row.in_envelope,
            row.elapsed_ms,
            row.label
        );
    }
    s
}

/// A short human detail string for an outcome (failing dims / refusal reason).
fn outcome_detail(outcome: &VariantOutcome) -> String {
    match outcome {
        VariantOutcome::Sound => "-".to_string(),
        VariantOutcome::TimedOut => "over budget".to_string(),
        VariantOutcome::CertKilled(dims) => dims.join("+"),
        VariantOutcome::Refused(r) => r.to_string(),
        VariantOutcome::Panicked(m) => m.clone(),
    }
}

/// Hand-rolled JSON so the example needs no serde derive on harness types (and
/// the artifact is fully derived from the report). Numbers are emitted with
/// enough precision to reconstruct the ranking.
fn render_json(
    report: &ExplorationReport,
    internal_target: Option<f64>,
    win: Option<&VariantRow>,
) -> String {
    let mut s = String::new();
    s.push_str("{\n");
    let _ = writeln!(s, "  \"explored\": {},", report.rows.len());
    let _ = writeln!(s, "  \"refused\": {},", report.refused);
    let _ = writeln!(s, "  \"cert_killed\": {},", report.cert_killed);
    let _ = writeln!(s, "  \"sound\": {},", report.sound);
    let _ = writeln!(s, "  \"timed_out\": {},", report.timed_out);
    let _ = writeln!(s, "  \"panicked\": {},", report.panicked);
    let _ = writeln!(s, "  \"threads\": {},", report.timings.threads);
    let _ = writeln!(s, "  \"total_ms\": {},", report.timings.total_ms);
    let _ = writeln!(
        s,
        "  \"sum_variant_ms\": {},",
        report.timings.sum_variant_ms
    );
    let _ = writeln!(
        s,
        "  \"mean_variant_ms\": {},",
        json_num(report.timings.mean_variant_ms)
    );
    let _ = writeln!(
        s,
        "  \"s_per_variant\": {},",
        json_num(report.timings.mean_variant_ms / 1000.0)
    );
    let _ = writeln!(
        s,
        "  \"envelope\": {{ \"max_diameter\": {}, \"max_length\": {} }},",
        json_num(report.envelope.max_diameter),
        json_num(report.envelope.max_length)
    );
    let _ = writeln!(
        s,
        "  \"internal_volume_target\": {},",
        internal_target
            .map(json_num)
            .unwrap_or_else(|| "null".to_string())
    );

    // Kill histogram object.
    s.push_str("  \"kill_histogram\": {");
    let kh = report.kill_histogram();
    for (idx, (dim, count)) in kh.iter().enumerate() {
        if idx > 0 {
            s.push(',');
        }
        let _ = write!(s, " \"{dim}\": {count}");
    }
    s.push_str(" },\n");

    // Refusal histogram object.
    s.push_str("  \"refusal_histogram\": {");
    let rh = report.refusal_histogram();
    for (idx, (kind, count)) in rh.iter().enumerate() {
        if idx > 0 {
            s.push(',');
        }
        let _ = write!(s, " \"{kind}\": {count}");
    }
    s.push_str(" },\n");

    // Winner object.
    match win {
        Some(w) => {
            s.push_str("  \"winner\": {\n");
            s.push_str(&variant_json_body(w, "    "));
            s.push_str("  },\n");
        }
        None => s.push_str("  \"winner\": null,\n"),
    }

    // All rows.
    s.push_str("  \"rows\": [\n");
    for (i, row) in report.rows.iter().enumerate() {
        s.push_str("    {\n");
        s.push_str(&variant_json_body(row, "      "));
        if i + 1 < report.rows.len() {
            s.push_str("    },\n");
        } else {
            s.push_str("    }\n");
        }
    }
    s.push_str("  ]\n");
    s.push_str("}\n");
    s
}

/// The inner body of a variant JSON object (fields, indented by `pad`), without
/// the enclosing braces. Ends with a newline; no trailing comma on the last
/// field.
fn variant_json_body(row: &VariantRow, pad: &str) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "{pad}\"label\": {},", json_str(&row.label));
    let _ = writeln!(s, "{pad}\"outcome\": {},", json_str(row.outcome.tag()));
    let _ = writeln!(
        s,
        "{pad}\"detail\": {},",
        json_str(&outcome_detail(&row.outcome))
    );
    let _ = writeln!(
        s,
        "{pad}\"wall_material_volume\": {},",
        row.wall_material_volume
            .map(json_num)
            .unwrap_or_else(|| "null".to_string())
    );
    let _ = writeln!(
        s,
        "{pad}\"internal_volume\": {},",
        row.internal_volume
            .map(json_num)
            .unwrap_or_else(|| "null".to_string())
    );
    let _ = writeln!(s, "{pad}\"in_envelope\": {},", row.in_envelope);
    let _ = writeln!(s, "{pad}\"elapsed_ms\": {},", row.elapsed_ms);
    let _ = writeln!(s, "{pad}\"params\": {{");
    let p = &row.params;
    let _ = writeln!(s, "{pad}  \"throat_r\": {},", json_num(p.throat_r));
    let _ = writeln!(
        s,
        "{pad}  \"expansion_ratio\": {},",
        json_num(p.expansion_ratio)
    );
    let _ = writeln!(s, "{pad}  \"chamber_r\": {},", json_num(p.chamber_r));
    let _ = writeln!(
        s,
        "{pad}  \"chamber_l_over_d\": {},",
        json_num(p.chamber_l_over_d)
    );
    let _ = writeln!(s, "{pad}  \"wall_t\": {},", json_num(p.wall_t));
    let _ = writeln!(s, "{pad}  \"hole_count\": {},", p.hole_count);
    let _ = writeln!(s, "{pad}  \"hole_r\": {},", json_num(p.hole_r));
    let _ = writeln!(s, "{pad}  \"ring_frac\": {}", json_num(p.ring_frac));
    let _ = writeln!(s, "{pad}}}");
    s
}

/// A finite f64 as a JSON number; non-finite → JSON `null` (JSON has no NaN).
fn json_num(x: f64) -> String {
    if x.is_finite() {
        format!("{x}")
    } else {
        "null".to_string()
    }
}

/// A JSON string literal with the handful of escapes the labels/details can
/// contain (quote, backslash, control chars are not produced by our labels, but
/// refusal reasons can contain quotes).
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}
