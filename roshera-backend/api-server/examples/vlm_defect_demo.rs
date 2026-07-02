//! VLM-tier demo for the injected-defect benchmark (Move 2, Task 6).
//!
//! Renders the benchmark's 5 meshes (sound baseline + 4 silent lies) as shaded
//! PNGs, asks Claude vision whether each looks sound for manufacturing, and records
//! the honest outcome next to the cert's verdict. The expected story is that the
//! VLM says SOUND for the two flagship lies (flipped normal, self-intersection) —
//! they render identically to a sound part — while the analytic cert catches them.
//! Whatever Claude replies, the table records it truthfully. Verdicts are NEVER
//! scripted.
//!
//! Run manually:
//!
//! ```text
//! ANTHROPIC_API_KEY=sk-... cargo run -p api-server --example vlm_defect_demo
//! ```
//!
//! When `ANTHROPIC_API_KEY` is unset, the demo prints a one-line explanation and
//! exits 0 — CI-safe, network-free path.
//!
//! Artifacts are written to `target/injected_defect_benchmark/vlm/`:
//! * `<mesh>.png`   — 256×256 shaded render of each mesh
//! * `vlm_results.md` — the result table with cert + VLM verdicts

use ai_integration::providers::claude::{ClaudeConfig, ClaudeProvider};
use geometry_engine::harness::defect_injection::{
    delete_triangle, duplicate_triangle, flip_normal, inject_self_intersection,
};
use geometry_engine::harness::self_intersection::mesh_self_intersects_mesh;
use geometry_engine::harness::watertight::manifold_report_mesh;
use geometry_engine::math::Vector3;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::render::{render_mesh, CanonicalView, RenderMode, RenderOptions};
use geometry_engine::tessellation::mesh::TriangleMesh;
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
use std::error::Error;
use std::path::Path;

/// Weld epsilon for manifold analysis — matches the benchmark's value.
const WELD_EPS: f64 = 1e-6;

/// Render resolution (px). 256 is fast and large enough for Claude vision.
const RENDER_PX: usize = 256;

/// Fair, neutral VLM inspection prompt.
///
/// Does NOT hint at SOUND or DEFECTIVE — it is a genuine open question.
const VLM_PROMPT: &str = "You are inspecting a rendered 3D part for manufacturing. \
Based on this image, is this a sound, watertight, manufacturable solid? \
Reply with exactly SOUND or DEFECTIVE on the first line, then one line of reasoning.";

// ── Cert logic (mirrors the benchmark) ──────────────────────────────────────

struct MeshCert {
    watertight: bool,
    manifold: bool,
    oriented: bool,
    self_intersection_free: bool,
}

impl MeshCert {
    fn sound(&self) -> bool {
        self.watertight && self.manifold && self.oriented && self.self_intersection_free
    }

    fn verdict(&self) -> &'static str {
        if self.sound() {
            "SOUND"
        } else {
            "DEFECTIVE"
        }
    }
}

fn certify(mesh: &TriangleMesh) -> Result<MeshCert, Box<dyn Error>> {
    let r = manifold_report_mesh(mesh, WELD_EPS).ok_or("cannot certify: mesh has no triangles")?;
    Ok(MeshCert {
        watertight: r.boundary_edges == 0,
        manifold: r.nonmanifold_edges == 0,
        oriented: r.inconsistent_directed_edges == 0,
        self_intersection_free: !mesh_self_intersects_mesh(mesh),
    })
}

// ── Geometry helpers ─────────────────────────────────────────────────────────

fn build_base(model: &mut BRepModel) -> Result<SolidId, Box<dyn Error>> {
    let geom = TopologyBuilder::new(model)
        .create_sphere_3d(Vector3::ZERO, 3.0)
        .map_err(|e| format!("create_sphere_3d failed: {e:?}"))?;
    match geom {
        GeometryId::Solid(id) => Ok(id),
        other => Err(format!("expected Solid from create_sphere_3d, got {other:?}").into()),
    }
}

fn base_mesh(model: &BRepModel, solid: SolidId) -> Result<TriangleMesh, Box<dyn Error>> {
    let params = TessellationParams {
        chord_tolerance: 0.05,
        ..TessellationParams::default()
    };
    let solid_ref = model
        .solids
        .get(solid)
        .ok_or("solid not found in model after creation")?;
    Ok(tessellate_solid(solid_ref, model, &params))
}

// ── Render helper ─────────────────────────────────────────────────────────────

fn mesh_to_png(mesh: &TriangleMesh, label: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let opts = RenderOptions {
        width: RENDER_PX,
        height: RENDER_PX,
        view: CanonicalView::Isometric,
        mode: RenderMode::Shaded,
        tessellation: TessellationParams::default(),
    };
    let frame = render_mesh(mesh, &opts)
        .ok_or_else(|| format!("{label}: render_mesh returned None (empty mesh)"))?;
    let png = frame
        .to_png()
        .map_err(|e| format!("{label}: to_png failed: {e}"))?;
    Ok(png)
}

// ── VLM response parsing ──────────────────────────────────────────────────────

/// Classify the first line of a VLM reply as SOUND, DEFECTIVE, or UNPARSEABLE.
///
/// Parsing is case-insensitive and prefix-matched (so "SOUND." or "DEFECTIVE,"
/// both count). UNPARSEABLE is recorded as-is when neither keyword appears —
/// the table NEVER substitutes a scripted verdict.
fn parse_verdict(reply: &str) -> &'static str {
    let first = reply.lines().next().unwrap_or("").trim().to_uppercase();
    if first.starts_with("SOUND") {
        "SOUND"
    } else if first.starts_with("DEFECTIVE") {
        "DEFECTIVE"
    } else {
        "UNPARSEABLE"
    }
}

fn parse_reasoning(reply: &str) -> String {
    reply
        .lines()
        .nth(1)
        .unwrap_or("")
        .trim()
        .chars()
        .take(90)
        .collect()
}

// ── Artifact output ───────────────────────────────────────────────────────────

struct DemoRow {
    label: String,
    png: Vec<u8>,
    cert: &'static str,
    vlm: &'static str,
    reasoning: String,
}

fn write_artifacts(dir: &Path, rows: &[DemoRow]) -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all(dir)?;
    for row in rows {
        std::fs::write(dir.join(format!("{}.png", row.label)), &row.png)?;
    }
    let mut md = String::from(
        "# VLM Defect Demo — Claude Vision vs Cert\n\n\
         The two flagship lies (flipped normal, self-intersection) render identically\n\
         to a sound part under double-sided shading. The analytic cert catches them;\n\
         the VLM verdict is recorded verbatim from the Claude API — never scripted.\n\n",
    );
    md.push_str("| Mesh | Cert verdict | VLM verdict | VLM reasoning |\n");
    md.push_str("|------|-------------|-------------|---------------|\n");
    for row in rows {
        md.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            row.label, row.cert, row.vlm, row.reasoning,
        ));
    }
    std::fs::write(dir.join("vlm_results.md"), md)?;
    Ok(())
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Gate: require ANTHROPIC_API_KEY; exit 0 gracefully if absent so CI stays clean.
    let api_key = match std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
    {
        Some(k) => k,
        None => {
            println!("ANTHROPIC_API_KEY not set — set the env var to run the Claude-vision tier.");
            return Ok(());
        }
    };

    let provider = ClaudeProvider::with_config(ClaudeConfig {
        api_key: Some(api_key),
        max_tokens: 256,
        ..ClaudeConfig::default()
    });

    // Build the sound base part (radius-3 sphere, identical to the benchmark).
    let mut model = BRepModel::new();
    let solid = build_base(&mut model)?;
    let base = base_mesh(&model, solid)?;

    // The 5 cases: sound baseline + 4 injected defects.  Processed in order so
    // the result table reads top-down from "ground truth" through each lie class.
    let cases: Vec<(&str, &str, TriangleMesh)> = vec![
        (
            "sound_base",
            "untouched Ø6 sphere tessellation",
            base.clone(),
        ),
        (
            "flipped_normal",
            "reverse one triangle winding [a,b,c]→[a,c,b]",
            flip_normal(&base),
        ),
        (
            "self_intersection",
            "translate +X vertex group a full span past −X",
            inject_self_intersection(&base),
        ),
        (
            "torn_facet",
            "delete one triangle → 3 boundary edges",
            delete_triangle(&base),
        ),
        (
            "duplicated_facet",
            "append a copy of one triangle → 3 non-manifold edges",
            duplicate_triangle(&base),
        ),
    ];

    let out_dir = std::path::PathBuf::from(
        std::env::var_os("CARGO_TARGET_DIR").unwrap_or_else(|| "target".into()),
    )
    .join("injected_defect_benchmark")
    .join("vlm");

    let mut rows: Vec<DemoRow> = Vec::new();
    let mut api_calls: usize = 0;

    for (label, injection, mesh) in &cases {
        let png = mesh_to_png(mesh, label)?;
        let cert = certify(mesh)?;
        let cert_verdict = cert.verdict();

        // Ask Claude vision — raw reply is parsed, never replaced.
        let reply = provider
            .generate_with_image(VLM_PROMPT, &png)
            .await
            .map_err(|e| format!("{label}: VLM call failed: {e}"))?;
        api_calls += 1;

        let vlm_verdict = parse_verdict(&reply);
        let reasoning = parse_reasoning(&reply);

        rows.push(DemoRow {
            label: label.to_string(),
            png,
            cert: cert_verdict,
            vlm: vlm_verdict,
            reasoning,
        });

        println!(
            "  [{api_calls}/5] {label:<25} injection: {injection:<50} cert={cert_verdict:<9} vlm={vlm_verdict}"
        );
    }

    write_artifacts(&out_dir, &rows)?;

    println!();
    println!("── VLM Defect Demo Results ──────────────────────────────────────────────────────────────────────────");
    println!(
        "{:<26} {:<12} {:<12} {}",
        "Mesh", "Cert", "VLM", "VLM Reasoning"
    );
    println!("{}", "─".repeat(110));
    for row in &rows {
        println!(
            "{:<26} {:<12} {:<12} {}",
            row.label, row.cert, row.vlm, row.reasoning,
        );
    }
    println!();
    println!(
        "API requests: {api_calls}  |  Artifacts: {}",
        out_dir.display()
    );

    Ok(())
}
