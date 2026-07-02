//! INJECTED-DEFECT BENCHMARK (Move 2 core) — the flagship proof that Roshera's
//! certified eye catches SILENT geometric lies that shallow checks wave through.
//!
//! We tessellate ONE sound solid, then mutate the delivered `TriangleMesh` four
//! ways — each a real class of geometric lie a downstream consumer (viewport,
//! exporter, printer) would swallow. For every lie we compare three verdicts:
//!
//!   * B1 `brep_valid`-only — validate the UNTOUCHED solid's B-Rep. The mesh
//!     mutation is invisible to it, so it PASSES all four lies (0/4 caught).
//!   * B2 "looks closed" — undirected mesh counts (`boundary_edges == 0 &&
//!     nonmanifold_edges == 0`). Blind to a winding flip and to crossing walls
//!     (they still close), so it passes those two; catches the torn/duplicated
//!     facet (2/4 caught).
//!   * The certified verdict — the conjunction of watertight ∧ manifold ∧
//!     oriented ∧ self-intersection-free from T1's mesh-core analyses. Catches
//!     all four (4/4), and each defect is caught by the RIGHT dimension.
//!
//! The two flagship lies (flipped normal, self-intersection) are the ones B1 AND
//! B2 both miss — rendered SHADED (double-sided), they even look solid, which is
//! the whole point the VLM tier will demonstrate. This CI-deterministic core
//! proves the analytic contrast with no network and no API key.
//!
//! Injectors live here (test-exempt from the workspace unwrap/panic lints) and
//! consume ONLY T1's public surface; the cert/analysis internals are untouched.

#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use geometry_engine::harness::defect_injection::{
    delete_triangle, duplicate_triangle, flip_normal, inject_self_intersection,
};
use geometry_engine::harness::self_intersection::mesh_self_intersects_mesh;
use geometry_engine::harness::watertight::manifold_report_mesh;
use geometry_engine::math::vector3::Vector3;
use geometry_engine::math::Tolerance;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};
use geometry_engine::render::{render_mesh, CanonicalView, RenderMode, RenderOptions};
use geometry_engine::tessellation::mesh::TriangleMesh;
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

/// Weld epsilon for the mesh-core connectivity analysis (well under the chord,
/// comfortably above f64 noise — the value the harness uses for 1–10 unit parts).
const WELD_EPS: f64 = 1e-6;

// ── The sound base part ─────────────────────────────────────────────────────

/// Build a clean radius-3 sphere; return its `SolidId`. A sphere is quick,
/// curved (a realistic triangle count), and guaranteed watertight/manifold/
/// oriented/self-intersection-free — the honest "sound" baseline every injector
/// perturbs.
fn build_base(model: &mut BRepModel) -> SolidId {
    match TopologyBuilder::new(model)
        .create_sphere_3d(Vector3::ZERO, 3.0)
        .expect("sphere")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected Solid from create_sphere_3d, got {other:?}"),
    }
}

/// Tessellate `solid` at a moderate chord — the mesh the benchmark analyses,
/// mutates and renders. Coarse enough to stay fast and deterministic, fine
/// enough to exercise the connectivity + self-intersection scans on hundreds of
/// facets.
fn base_mesh(model: &BRepModel, solid: SolidId) -> TriangleMesh {
    let params = TessellationParams {
        chord_tolerance: 0.05,
        ..TessellationParams::default()
    };
    let solid_ref = model.solids.get(solid).expect("solid ref");
    tessellate_solid(solid_ref, model, &params)
}

// ── Verdicts ────────────────────────────────────────────────────────────────

/// The certified verdict on a mesh: the four mesh-computable soundness
/// dimensions, from T1's mesh-core analyses.
#[derive(Debug, Clone, Copy)]
struct MeshCert {
    watertight: bool,
    manifold: bool,
    oriented: bool,
    self_intersection_free: bool,
}

impl MeshCert {
    /// Sound iff every dimension holds — the conjunction the benchmark reports.
    fn sound(&self) -> bool {
        self.watertight && self.manifold && self.oriented && self.self_intersection_free
    }
}

/// Run the certified mesh analysis (T1 surface only).
fn certify(mesh: &TriangleMesh) -> MeshCert {
    let r = manifold_report_mesh(mesh, WELD_EPS).expect("manifold report");
    MeshCert {
        watertight: r.boundary_edges == 0,
        manifold: r.nonmanifold_edges == 0,
        oriented: r.inconsistent_directed_edges == 0,
        self_intersection_free: !mesh_self_intersects_mesh(mesh),
    }
}

/// B1 — `brep_valid`-only shallow baseline. Validates the UNTOUCHED solid's
/// B-Rep; blind to whatever the delivered mesh actually is. `true` = "looks
/// sound to the B-Rep validator".
fn b1_brep_valid(model: &BRepModel, solid: SolidId) -> bool {
    validate_solid_scoped(
        model,
        solid,
        Tolerance::default(),
        ValidationLevel::Standard,
    )
    .is_valid
}

/// B2 — "looks closed" shallow baseline: undirected mesh counts only. `true` =
/// "no open or non-manifold edges → looks sound".
fn b2_looks_closed(mesh: &TriangleMesh) -> bool {
    match manifold_report_mesh(mesh, WELD_EPS) {
        Some(r) => r.boundary_edges == 0 && r.nonmanifold_edges == 0,
        None => false,
    }
}

// ── Artifact ────────────────────────────────────────────────────────────────

/// One scoreboard row.
struct Row {
    defect: String,
    injection: String,
    cert_dimension: String,
    b1: String,
    b2: String,
    cert: String,
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Render a MEASURED defect-row verdict as a table cell. The artifact must
/// report what was measured, never a hardcoded claim — a benchmark about lies
/// must not hardcode its own table.
fn verdict_cell(caught: bool) -> String {
    if caught { "CATCH" } else { "PASS (lie)" }.to_string()
}

/// Measured sanity-row cell: the untouched base should pass everything.
fn sanity_cell(pass: bool) -> String {
    if pass {
        "PASS (sound)".to_string()
    } else {
        "FAIL (unsound base)".to_string()
    }
}

/// Emit `benchmark_results.md` + `benchmark_results.json` under
/// `target/injected_defect_benchmark/` (a test may write under target/).
fn write_artifacts(rows: &[Row], cert_caught: usize, b1_caught: usize, b2_caught: usize) {
    let base = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("target")
        });
    let dir = base.join("injected_defect_benchmark");
    std::fs::create_dir_all(&dir).expect("create artifact dir");

    // Markdown.
    let mut md = String::new();
    md.push_str("# Injected-Defect Benchmark — certified eye vs shallow baselines\n\n");
    md.push_str(
        "One sound solid, tessellated once, then mutated four ways. Each row is a \
         real class of geometric lie. `PASS (lie)` = the baseline was fooled; \
         `CATCH` = the defect was flagged.\n\n",
    );
    md.push_str(
        "| Defect | Injection | Cert dimension | B1 brep-only | B2 mesh-count | Certified verdict |\n",
    );
    md.push_str(
        "|--------|-----------|----------------|--------------|---------------|-------------------|\n",
    );
    for r in rows {
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            r.defect, r.injection, r.cert_dimension, r.b1, r.b2, r.cert
        ));
    }
    md.push_str(&format!(
        "\n**Headline:** certified eye caught {cert_caught}/4. \
         brep-only (B1) caught {b1_caught}/4 (passed {} lies). \
         mesh-count (B2) caught {b2_caught}/4 (passed {} lies).\n",
        4 - b1_caught,
        4 - b2_caught,
    ));
    md.push_str(
        "\n> **Why B1 passes every lie:** B1 validates the *B-Rep solid*, which the \
         mesh mutations never touch — it is blind *architecturally*, because it never \
         looks at the delivered mesh. That is precisely the gap: a system that trusts \
         B-Rep validity alone will ship a lying mesh. The certified eye analyses the \
         delivered mesh itself, so the lie has nowhere to hide.\n",
    );
    std::fs::write(dir.join("benchmark_results.md"), md).expect("write md");

    // JSON.
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str(&format!(
        "  \"headline\": {{ \"cert_caught\": {cert_caught}, \"b1_caught\": {b1_caught}, \"b2_caught\": {b2_caught}, \"total_defects\": 4 }},\n"
    ));
    json.push_str("  \"rows\": [\n");
    for (i, r) in rows.iter().enumerate() {
        let comma = if i + 1 < rows.len() { "," } else { "" };
        json.push_str(&format!(
            "    {{ \"defect\": \"{}\", \"injection\": \"{}\", \"cert_dimension\": \"{}\", \"b1\": \"{}\", \"b2\": \"{}\", \"cert\": \"{}\" }}{}\n",
            json_escape(&r.defect),
            json_escape(&r.injection),
            json_escape(&r.cert_dimension),
            json_escape(&r.b1),
            json_escape(&r.b2),
            json_escape(&r.cert),
            comma
        ));
    }
    json.push_str("  ]\n}\n");
    std::fs::write(dir.join("benchmark_results.json"), json).expect("write json");
}

/// Render a lying mesh SHADED (double-sided) → PNG, and assert the PNG is real.
/// This is the artifact the VLM tier will consume; the point of the flagship
/// lies is that they still LOOK solid here while the cert flags them.
fn assert_renders_to_png(mesh: &TriangleMesh, label: &str) {
    let opts = RenderOptions {
        width: 256,
        height: 256,
        view: CanonicalView::Isometric,
        mode: RenderMode::Shaded,
        tessellation: TessellationParams::default(),
    };
    let frame = render_mesh(mesh, &opts).unwrap_or_else(|| panic!("{label}: render_mesh"));
    assert_eq!(frame.width, 256, "{label}: width");
    assert_eq!(frame.height, 256, "{label}: height");
    let png = frame.to_png().unwrap_or_else(|_| panic!("{label}: to_png"));
    assert!(!png.is_empty(), "{label}: PNG must not be empty");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "{label}: PNG signature");
}

// ── The benchmark ───────────────────────────────────────────────────────────

#[test]
fn injected_defect_benchmark() {
    let mut model = BRepModel::new();
    let solid = build_base(&mut model);
    let base = base_mesh(&model, solid);
    assert!(
        base.triangles.len() > 50,
        "base mesh should be a real part (got {} triangles)",
        base.triangles.len()
    );

    let mut rows: Vec<Row> = Vec::new();

    // ── Sanity row: the untouched part passes EVERYTHING ────────────────────
    let sane = certify(&base);
    assert!(sane.watertight, "sanity: base must be watertight");
    assert!(sane.manifold, "sanity: base must be manifold");
    assert!(sane.oriented, "sanity: base must be oriented");
    assert!(
        sane.self_intersection_free,
        "sanity: base must be self-intersection-free"
    );
    assert!(sane.sound(), "sanity: base cert must be sound");
    let b1_base = b1_brep_valid(&model, solid);
    let b2_base = b2_looks_closed(&base);
    assert!(b1_base, "sanity: B1 must pass base");
    assert!(b2_base, "sanity: B2 must pass base");
    rows.push(Row {
        defect: "(none) sound base".into(),
        injection: "untouched Ø6 sphere tessellation".into(),
        cert_dimension: "—".into(),
        b1: sanity_cell(b1_base),
        b2: sanity_cell(b2_base),
        cert: sanity_cell(sane.sound()),
    });

    // B1 is computed on the untouched solid → identical for every defect.
    let b1 = b1_brep_valid(&model, solid);

    let mut cert_caught = 0usize;
    let mut b1_caught = 0usize;
    let mut b2_caught = 0usize;

    // Helper closure would borrow `rows`/counters mutably in a loop; keep the four
    // cases explicit so each asserts the RIGHT dimension flags.

    // ── #1 FLIPPED NORMAL — flagship: oriented catches, B1 & B2 both fooled ──
    {
        let m = flip_normal(&base);
        let c = certify(&m);
        assert!(!c.oriented, "flip: oriented MUST flag the winding reversal");
        assert!(
            c.watertight,
            "flip: watertight must stay clean (no edge added)"
        );
        assert!(c.manifold, "flip: manifold must stay clean");
        assert!(
            c.self_intersection_free,
            "flip: geometry unchanged → no self-intersection"
        );
        assert!(!c.sound(), "flip: cert verdict must be unsound");
        assert!(b1, "flip: B1 (brep-only) is fooled — passes the lie");
        assert!(
            b2_looks_closed(&m),
            "flip: B2 (mesh-count) is fooled — a directed flip is invisible to undirected counts"
        );
        assert_renders_to_png(&m, "flip_normal");
        // Derived verdicts — counters and cells report what was MEASURED.
        let (cert_sound, b2_pass) = (c.sound(), b2_looks_closed(&m));
        if !cert_sound {
            cert_caught += 1;
        }
        if !b1 {
            b1_caught += 1;
        }
        if !b2_pass {
            b2_caught += 1;
        }
        rows.push(Row {
            defect: "flipped face normal".into(),
            injection: "reverse one triangle's winding [a,b,c]→[a,c,b]".into(),
            cert_dimension: "oriented".into(),
            b1: verdict_cell(!b1),
            b2: verdict_cell(!b2_pass),
            cert: verdict_cell(!cert_sound),
        });
    }

    // ── #2 SELF-INTERSECTION — flagship: self_int catches, B1 & B2 fooled ────
    {
        let m = inject_self_intersection(&base);
        let c = certify(&m);
        assert!(
            !c.self_intersection_free,
            "self-int: self_intersection_free MUST flag the crossing walls"
        );
        assert!(
            c.watertight,
            "self-int: moving a welded group preserves closure (B2 stays blind)"
        );
        assert!(c.manifold, "self-int: no non-manifold edge introduced");
        assert!(
            c.oriented,
            "self-int: windings unchanged → orientation intact"
        );
        assert!(!c.sound(), "self-int: cert verdict must be unsound");
        assert!(b1, "self-int: B1 (brep-only) is fooled");
        assert!(
            b2_looks_closed(&m),
            "self-int: B2 (mesh-count) is fooled — crossing walls still count closed"
        );
        assert_renders_to_png(&m, "inject_self_intersection");
        // Derived verdicts — counters and cells report what was MEASURED.
        let (cert_sound, b2_pass) = (c.sound(), b2_looks_closed(&m));
        if !cert_sound {
            cert_caught += 1;
        }
        if !b1 {
            b1_caught += 1;
        }
        if !b2_pass {
            b2_caught += 1;
        }
        rows.push(Row {
            defect: "self-intersection (crossing walls)".into(),
            injection: "translate the +X welded vertex group a full span past −X".into(),
            cert_dimension: "self_intersection_free".into(),
            b1: verdict_cell(!b1),
            b2: verdict_cell(!b2_pass),
            cert: verdict_cell(!cert_sound),
        });
    }

    // ── #3 TORN FACET — control: watertight catches; B2 ALSO catches ────────
    {
        let m = delete_triangle(&base);
        let c = certify(&m);
        assert!(
            !c.watertight,
            "delete: watertight MUST flag the boundary edges"
        );
        assert!(c.manifold, "delete: no non-manifold edge from a deletion");
        assert!(
            c.oriented,
            "delete: no duplicated directed edge from a deletion"
        );
        assert!(
            c.self_intersection_free,
            "delete: geometry of remaining facets unchanged"
        );
        assert!(!c.sound(), "delete: cert verdict must be unsound");
        assert!(b1, "delete: B1 (brep-only) is fooled — solid is untouched");
        assert!(
            !b2_looks_closed(&m),
            "delete: B2 (mesh-count) CATCHES the boundary edges (honest control)"
        );
        // Derived verdicts — counters and cells report what was MEASURED.
        let (cert_sound, b2_pass) = (c.sound(), b2_looks_closed(&m));
        if !cert_sound {
            cert_caught += 1;
        }
        if !b1 {
            b1_caught += 1;
        }
        if !b2_pass {
            b2_caught += 1;
        }
        rows.push(Row {
            defect: "torn facet (gap / unwelded seam)".into(),
            injection: "delete one triangle → 3 boundary edges".into(),
            cert_dimension: "watertight".into(),
            b1: verdict_cell(!b1),
            b2: verdict_cell(!b2_pass),
            cert: verdict_cell(!cert_sound),
        });
    }

    // ── #4 DUPLICATED FACET — control: manifold catches; B2 ALSO catches ────
    {
        let m = duplicate_triangle(&base);
        let c = certify(&m);
        assert!(
            !c.manifold,
            "duplicate: manifold MUST flag the non-manifold edges"
        );
        assert!(c.watertight, "duplicate: no boundary edge from a duplicate");
        assert!(
            c.self_intersection_free,
            "duplicate: coincident facet is not an interior crossing"
        );
        // oriented ALSO fires here — a same-winding duplicate repeats directed
        // edges. Assert it so the second dimension is a verified fact, not an
        // unchecked comment; `manifold` remains the NAMED/authoritative dimension
        // for this defect class in the artifact.
        assert!(
            !c.oriented,
            "duplicate: oriented also flags the repeated directed edges"
        );
        assert!(!c.sound(), "duplicate: cert verdict must be unsound");
        assert!(
            b1,
            "duplicate: B1 (brep-only) is fooled — solid is untouched"
        );
        assert!(
            !b2_looks_closed(&m),
            "duplicate: B2 (mesh-count) CATCHES the non-manifold edges (honest control)"
        );
        // Derived verdicts — counters and cells report what was MEASURED.
        let (cert_sound, b2_pass) = (c.sound(), b2_looks_closed(&m));
        if !cert_sound {
            cert_caught += 1;
        }
        if !b1 {
            b1_caught += 1;
        }
        if !b2_pass {
            b2_caught += 1;
        }
        rows.push(Row {
            defect: "duplicated facet (non-manifold)".into(),
            injection: "append a copy of one triangle → 3 non-manifold edges".into(),
            cert_dimension: "manifold (oriented also fires)".into(),
            b1: verdict_cell(!b1),
            b2: verdict_cell(!b2_pass),
            cert: verdict_cell(!cert_sound),
        });
    }

    // ── The honest headline ─────────────────────────────────────────────────
    assert_eq!(cert_caught, 4, "certified eye must catch 4/4");
    assert_eq!(
        b1_caught, 0,
        "brep-only baseline must catch 0/4 (passes every lie)"
    );
    assert_eq!(b2_caught, 2, "mesh-count baseline must catch 2/4");

    write_artifacts(&rows, cert_caught, b1_caught, b2_caught);
}
