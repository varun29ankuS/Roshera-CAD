//! Drawing bench: one STEP part through the production import and sheet
//! pipeline, written out to be LOOKED at and measured.
//!
//! Usage: `cargo run -p export-engine --example drawing_bench -- <id> <file.step> <out_dir>`
//!
//! For the solid with the most faces in the file it writes, into
//! `<out_dir>/<id>/`:
//!
//! * `ours.svg`, `ours.pdf`, `ours.dxf` -- the automatic sheet, stamped with
//!   its certificate exactly as the export routes stamp it;
//! * `summary.json` -- what a benchmark needs without parsing ink: import
//!   verdict, stage timings, the sheet (size, views, scale), every dimension
//!   as inked, the hole table, the layout check by issue kind, and the sheet
//!   certificate (sound, complete, counts, each omission with its reason),
//!   and the space map's grade, below-bar count and findings by rule;
//! * `spaces.json` -- the whole space map (`drawing::spaces`): every space
//!   with its box, contents, findings and score;
//! * `spaces.svg` -- `ours.svg` with every space's box drawn over it
//!   (colour by kind, red where the space is below the bar), to check by eye
//!   that the boxes sit where the dimensions and tables are.
//!
//! One part per process on purpose: a part that panics or runs away costs the
//! caller (`roshera-eval/drawing-bench/bench.py`) one entry, not the whole
//! run. A failure at any stage is written to `summary.json` as that stage's
//! outcome, never dropped.

use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::time::Instant;

use export_engine::ExportEngine;
use geometry_engine::drawing::dimension_edit::dimension_ink;
use geometry_engine::drawing::layout::compute_layout;
use geometry_engine::drawing::spaces::{space_map, SpaceKind, SpaceMap};
use geometry_engine::drawing::title_block::compute_render_facts;
use geometry_engine::drawing::{
    analyse_natural_frame, certify_drawing, frame_for_drawing, render_drawing_dxf,
    render_drawing_pdf, render_drawing_svg, standard_drawing_auto, verify_drawing, Severity,
};
use geometry_engine::primitives::snapshot::ModelSnapshot;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::readable::extract_dimensions;
use serde_json::{json, Value};

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

fn ms(since: Instant) -> u64 {
    since.elapsed().as_millis() as u64
}

fn write_summary(dir: &Path, summary: &Value) {
    let text = serde_json::to_string_pretty(summary).unwrap_or_else(|_| "{}".to_string());
    if let Err(e) = std::fs::write(dir.join("summary.json"), text) {
        eprintln!("could not write summary.json: {e}");
    }
}

/// Record entering a stage ON DISK before the stage runs, so a process the
/// caller kills on its time limit still says where it was (import or sheet),
/// instead of leaving no summary at all.
fn enter_stage(dir: &Path, summary: &mut Value, stage: &str) {
    summary["stage"] = json!(stage);
    write_summary(dir, summary);
}

/// `svg` with every space of `map` outlined on top: one colour per kind,
/// red and thicker where the space is below the bar, its id written small at
/// its top-left corner.
fn spaces_overlay(svg: &str, map: &SpaceMap) -> String {
    let mut g = String::from("<g id=\"space-map-overlay\" fill=\"none\">\n");
    for sp in &map.spaces {
        let colour = match sp.kind {
            SpaceKind::Frame => "#888888",
            SpaceKind::TitleBlock | SpaceKind::NotesStrip => "#7b3fbf",
            SpaceKind::TableColumn | SpaceKind::Table => "#c77d00",
            SpaceKind::ViewCell => "#1f6fd1",
            SpaceKind::ViewGeometry => "#00a0a0",
            SpaceKind::Band => "#2aa02a",
            SpaceKind::Lane => "#d12fa6",
            SpaceKind::Free => "#9fc5e8",
        };
        let below = sp.score < map.bar;
        let (stroke, width) = if below {
            ("#e00000", 0.45)
        } else {
            (colour, 0.25)
        };
        let r = sp.rect;
        let dash = if sp.kind == SpaceKind::Free {
            " stroke-dasharray=\"2 1\""
        } else {
            ""
        };
        g.push_str(&format!(
            "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" stroke=\"{stroke}\" stroke-width=\"{width}\"{dash}/>\n",
            r.x0,
            r.y0,
            r.width().max(0.2),
            r.height().max(0.2)
        ));
        if sp.kind != SpaceKind::Frame {
            let id = sp.id.replace('&', "&amp;").replace('<', "&lt;");
            g.push_str(&format!(
                "<text x=\"{:.2}\" y=\"{:.2}\" font-size=\"1.6\" fill=\"{stroke}\" stroke=\"none\">{id} {:.2}</text>\n",
                r.x0 + 0.4,
                r.y0 + 1.8,
                sp.score
            ));
        }
    }
    g.push_str("</g>\n");
    match svg.rfind("</svg>") {
        Some(at) => format!("{}{g}{}", &svg[..at], &svg[at..]),
        None => format!("{svg}{g}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: drawing_bench <id> <file.step> <out_dir>");
        std::process::exit(2);
    }
    let (id, path) = (&args[1], &args[2]);
    let dir = Path::new(&args[3]).join(id);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("could not create {}: {e}", dir.display());
        std::process::exit(2);
    }
    let mut summary = json!({ "id": id, "step": path, "stage": "read" });

    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            summary["error"] = json!(format!("the STEP file could not be read: {e}"));
            write_summary(&dir, &summary);
            return;
        }
    };
    let content = String::from_utf8_lossy(&bytes).into_owned();

    // ── Import ────────────────────────────────────────────────────────────
    enter_stage(&dir, &mut summary, "import");
    let t = Instant::now();
    let engine = ExportEngine::new();
    let imported = catch_unwind(AssertUnwindSafe(|| {
        engine.import_step_content(&content, "bench")
    }));
    let import_ms = ms(t);
    let (mut model, report) = match imported {
        Ok(Ok(pair)) => pair,
        Ok(Err(e)) => {
            summary["import"] = json!({ "ok": false, "ms": import_ms, "error": e.to_string() });
            write_summary(&dir, &summary);
            return;
        }
        Err(_) => {
            summary["import"] = json!({ "ok": false, "ms": import_ms, "error": "panic" });
            write_summary(&dir, &summary);
            return;
        }
    };
    let mut solids: Vec<(u32, usize)> = model
        .solids
        .iter()
        .map(|(sid, _)| (sid, face_count(&model, sid)))
        .collect();
    solids.sort_by(|a, b| b.1.cmp(&a.1));
    summary["import"] = json!({
        "ok": report.ok,
        "ms": import_ms,
        "schema": report.schema,
        "unit": report.source_unit,
        "solids": solids.len(),
        "sound_solids": report.validation.iter().filter(|v| v.sound).count(),
        "warnings": report.warnings.len(),
    });
    let Some(&(solid, faces)) = solids.first() else {
        summary["error"] = json!("the file imported no solid");
        write_summary(&dir, &summary);
        return;
    };
    let validation = report.validation.iter().find(|v| v.solid_id == solid);
    summary["solid"] = json!({
        "id": solid,
        "faces": faces,
        "sound": validation.map(|v| v.sound),
        "brep_valid": validation.map(|v| v.brep_valid),
        "watertight": validation.map(|v| v.watertight),
    });

    // ── Sheet ─────────────────────────────────────────────────────────────
    enter_stage(&dir, &mut summary, "sheet");
    // A8 before: how far the part sits from square as the file holds it.
    summary["orientation_as_imported"] = match analyse_natural_frame(&mut model, solid) {
        Ok(a) => json!({ "tilt_deg": a.tilt_deg }),
        Err(e) => json!({ "error": e.to_string() }),
    };
    // The sheet is built on a framed copy and certified against the model as
    // imported, exactly as the drawing routes do.
    let mut live = {
        let mut copy = BRepModel::new();
        ModelSnapshot::take(&model).restore(&mut copy);
        copy
    };
    let (frame, drawn_frame) = frame_for_drawing(&mut model, solid);
    summary["frame"] = json!(format!("{frame:?}"));
    // A8: how far the part, as it is DRAWN, sits from square. Measured on
    // the model the sheet is built from, after any framing.
    summary["orientation"] = match analyse_natural_frame(&mut model, solid) {
        Ok(a) => json!({
            "tilt_deg": a.tilt_deg,
            "primary": [a.primary.direction.x, a.primary.direction.y, a.primary.direction.z],
            "has_secondary": a.secondary.is_some(),
            "directions": a.votes.len(),
        }),
        Err(e) => json!({ "error": e.to_string() }),
    };
    let t = Instant::now();
    let built = catch_unwind(AssertUnwindSafe(|| {
        standard_drawing_auto(&model, solid, uuid::Uuid::new_v4())
    }));
    let sheet_ms = ms(t);
    let mut drawing = match built {
        Ok(Ok(d)) => d,
        Ok(Err(e)) => {
            summary["sheet"] = json!({ "ok": false, "ms": sheet_ms, "error": e.to_string() });
            write_summary(&dir, &summary);
            return;
        }
        Err(_) => {
            summary["sheet"] = json!({ "ok": false, "ms": sheet_ms, "error": "panic" });
            write_summary(&dir, &summary);
            return;
        }
    };
    drawing.name = format!("Bench {id}");
    drawing.drawn_frame = drawn_frame;
    // The app stores (and its quality route and export gate re-verify) the
    // sheet after giving its views stable ids; check the same drawing here.
    drawing.stabilize_view_ids();

    // ── Certificate + layout check ────────────────────────────────────────
    enter_stage(&dir, &mut summary, "certify");
    let t = Instant::now();
    let certified = catch_unwind(AssertUnwindSafe(|| certify_drawing(&live, &drawing)));
    let certify_ms = ms(t);
    let quality = verify_drawing(&drawing);
    let mut by_kind: BTreeMap<String, [usize; 2]> = BTreeMap::new();
    for issue in &quality.issues {
        let slot = by_kind.entry(format!("{:?}", issue.kind)).or_default();
        if issue.severity == Severity::Error {
            slot[0] += 1;
        } else {
            slot[1] += 1;
        }
    }
    let layout = compute_layout(&drawing);
    // The space map, from the same layout the quality check and renderer use.
    let spaces = space_map(&drawing, &layout);
    if let Ok(text) = serde_json::to_string(&spaces) {
        if let Err(e) = std::fs::write(dir.join("spaces.json"), text) {
            eprintln!("could not write spaces.json: {e}");
        }
    }
    // The views' dimensions (view mm) and every placed callout, to read why
    // a dimension was or was not placed.
    let views_dims: Vec<Value> = drawing
        .views
        .iter()
        .map(|v| {
            json!({
                "name": v.name,
                "scale": v.scale,
                "position": v.position_mm,
                "dimensions": v.dimensions.iter().map(|d| json!({
                    "id": d.id, "kind": d.kind, "label": d.label, "a": d.a, "b": d.b,
                    "faces": d.entities, "members": d.members.len(),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let placed_dump: Vec<Value> = layout
        .dimensions
        .iter()
        .map(|pd| {
            json!({"view": pd.owner_view, "id": pd.dim_id, "label": pd.label,
            "line": pd.line, "ext": pd.ext, "text": pd.text_anchor, "rot": pd.text_rot_deg})
        })
        .collect();
    if let Ok(text) = serde_json::to_string(&json!({"views": views_dims, "placed": placed_dump})) {
        if let Err(e) = std::fs::write(dir.join("dims.json"), text) {
            eprintln!("could not write dims.json: {e}");
        }
    }
    let mut inked: Vec<Value> = Vec::new();
    let mut carried = 0usize;
    for (vi, view) in drawing.views.iter().enumerate() {
        carried += view.dimensions.len();
        for dim in &view.dimensions {
            let placed = layout
                .dimensions
                .iter()
                .any(|p| p.owner_view == vi && p.dim_id == dim.id);
            if !placed {
                continue;
            }
            let edit = dim
                .key
                .as_ref()
                .and_then(|k| drawing.dimension_edits.get(k));
            inked.push(json!({
                "view": view.name,
                "kind": dim.kind,
                "value": dim.value,
                "unit": dim.unit,
                "label": dimension_ink(dim, edit, drawing.document_unit).label,
                "faces": dim.entities,
            }));
        }
    }
    let records = catch_unwind(AssertUnwindSafe(|| extract_dimensions(&model, solid)))
        .map(|r| r.len())
        .ok();
    summary["sheet"] = json!({
        "ok": true,
        "ms": sheet_ms,
        "size": drawing.sheet_size.label(),
        "projection": drawing.projection,
        "views": drawing.views.iter().map(|v| json!({
            "name": v.name,
            "scale": v.scale,
            "dimensions": v.dimensions.len(),
            "visible_polylines": v.polylines.len(),
            "hidden_polylines": v.hidden_polylines.len(),
        })).collect::<Vec<_>>(),
        "has_section": drawing.section.is_some(),
        "dimension_records_on_model": records,
        "dimensions_carried": carried,
        "dimensions_inked": inked.len(),
        "inked": inked,
        "hole_table": drawing.hole_sites.iter().map(|h| json!({
            "tag": h.tag,
            "diameter_mm": h.diameter_mm,
            "size": h.dia_label,
            "depth": h.depth_label,
            "x": h.x_label,
            "y": h.y_label,
        })).collect::<Vec<_>>(),
        "round_part": drawing.round_part.is_some(),
        "hole_patterns": drawing.hole_features.as_ref().map(|h| h.patterns.len()),
        "datum_symbols": drawing.datum_symbols.len(),
        "fcf_blocks": drawing.fcf_blocks.len(),
        // every text item of the layout the quality check reads, with the
        // sheet-mm box it is checked by (to compare with what is drawn)
        "layout_text": layout.items.iter().filter(|it| it.text.is_some()).map(|it| json!({
            "kind": format!("{:?}", it.kind),
            "text": it.text,
            "view": it.owner_view,
            "bbox": [it.bbox.x0, it.bbox.y0, it.bbox.x1, it.bbox.y1],
        })).collect::<Vec<_>>(),
    });
    summary["quality"] = json!({
        "passed": quality.passed,
        "errors": quality.error_count(),
        "warnings": quality.warning_count(),
        "sheet_utilization": quality.sheet_utilization,
        // issue kind -> [errors, warnings]
        "by_kind": by_kind,
        // every finding as the quality report states it
        "issues": quality
            .issues
            .iter()
            .map(|i| json!({
                "severity": format!("{:?}", i.severity),
                "kind": format!("{:?}", i.kind),
                "view": i.view,
                "message": i.message,
            }))
            .collect::<Vec<_>>(),
    });
    // The ten lowest-scoring spaces and the rules that put them there.
    let mut low: Vec<&geometry_engine::drawing::spaces::Space> =
        spaces.spaces.iter().filter(|s| s.score < 1.0).collect();
    low.sort_by(|a, b| a.score.total_cmp(&b.score).then(a.id.cmp(&b.id)));
    let worst_spaces: Vec<Value> = low
        .iter()
        .take(10)
        .map(|s| {
            json!({
                "id": s.id,
                "score": s.score,
                "rules": s.findings.iter().map(|f| f.rule.id()).collect::<Vec<_>>(),
            })
        })
        .collect();
    summary["spaces"] = json!({
        "grade": spaces.grade,
        "bar": spaces.bar,
        "below_bar": spaces.below_bar,
        "spaces": spaces.spaces.len(),
        "worst": spaces.worst,
        // rule id -> [errors, warnings]
        "by_rule": spaces.by_rule,
        "worst_spaces": worst_spaces,
    });
    summary["certificate"] = match &certified {
        Ok(cert) => {
            let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
            for o in cert.omissions.iter().filter(|o| o.waiver.is_none()) {
                *reasons.entry(format!("{:?}", o.reason)).or_default() += 1;
            }
            json!({
                "ms": certify_ms,
                "sound": cert.sound,
                "complete": cert.complete,
                "counts": cert.counts,
                "omission_summary": cert.omission_summary,
                "omissions_by_reason": reasons,
                "omissions": cert.omissions.iter().map(|o| json!({
                    "view": o.view,
                    "kind": o.kind,
                    "label": o.label,
                    "value": o.value,
                    "faces": o.entities,
                    "reason": o.reason,
                })).collect::<Vec<_>>(),
            })
        }
        Err(_) => json!({ "ms": certify_ms, "error": "panic" }),
    };

    // ── Renders ───────────────────────────────────────────────────────────
    enter_stage(&dir, &mut summary, "render");
    // Stamp the title block as the export routes do.
    // Against the model as imported, as the export routes stamp it: the
    // sheet carries its frame, and the certificate applies it.
    drawing.render_facts = compute_render_facts(&mut live, &drawing);
    let mut renders = serde_json::Map::new();
    let t = Instant::now();
    match catch_unwind(AssertUnwindSafe(|| render_drawing_svg(&drawing))) {
        Ok(svg) => {
            let wrote = std::fs::write(dir.join("ours.svg"), &svg).is_ok();
            if let Err(e) = std::fs::write(dir.join("spaces.svg"), spaces_overlay(&svg, &spaces)) {
                eprintln!("could not write spaces.svg: {e}");
            }
            renders.insert(
                "svg".into(),
                json!({ "ok": wrote, "ms": ms(t), "bytes": svg.len() }),
            );
        }
        Err(_) => {
            renders.insert("svg".into(), json!({ "ok": false, "error": "panic" }));
        }
    }
    let t = Instant::now();
    match catch_unwind(AssertUnwindSafe(|| render_drawing_pdf(&drawing))) {
        Ok(Ok(pdf)) => {
            let wrote = std::fs::write(dir.join("ours.pdf"), &pdf).is_ok();
            renders.insert(
                "pdf".into(),
                json!({ "ok": wrote, "ms": ms(t), "bytes": pdf.len() }),
            );
        }
        Ok(Err(e)) => {
            renders.insert("pdf".into(), json!({ "ok": false, "error": e.to_string() }));
        }
        Err(_) => {
            renders.insert("pdf".into(), json!({ "ok": false, "error": "panic" }));
        }
    }
    let t = Instant::now();
    match catch_unwind(AssertUnwindSafe(|| render_drawing_dxf(&drawing))) {
        Ok(Ok(dxf)) => {
            let wrote = std::fs::write(dir.join("ours.dxf"), &dxf).is_ok();
            renders.insert(
                "dxf".into(),
                json!({ "ok": wrote, "ms": ms(t), "bytes": dxf.len() }),
            );
        }
        Ok(Err(e)) => {
            renders.insert("dxf".into(), json!({ "ok": false, "error": e.to_string() }));
        }
        Err(_) => {
            renders.insert("dxf".into(), json!({ "ok": false, "error": "panic" }));
        }
    }
    summary["renders"] = Value::Object(renders);
    summary["stage"] = json!("done");
    write_summary(&dir, &summary);
    println!(
        "{id} done: sheet {sheet_ms} ms, {} inked, quality errors {}, certificate {}",
        summary["sheet"]["dimensions_inked"],
        summary["quality"]["errors"],
        summary["certificate"]["complete"],
    );
}
