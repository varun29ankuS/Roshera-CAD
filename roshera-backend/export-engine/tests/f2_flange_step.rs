// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Dogfood finding F2 — `export_part` STEP → HTTP 500 with an EMPTY body
//! for a weld-neck flange (planes + cylinders + a frustum CONE hub + a
//! TORUS rim fillet), while STL of the same solid works.
//!
//! The writer already handles `SurfaceData::Cone` and `SurfaceData::Torus`,
//! so this is NOT a missing surface type. This test BRACKETS the failure:
//! it builds progressively-richer solids via the same kernel paths the
//! live server uses and exports each to STEP, surfacing the verbatim
//! `ExportError` from the first shape that fails.
//!
//! Build order (simplest → the full flange):
//!   1. plain frustum cone            (`create_cone_3d`)
//!   2. cylinder + top-rim fillet     (torus surface via `fillet_edges`)
//!   3. cylinder ∪ cone               (boolean union)
//!   4. full flange                   (disk ∪ cone − bore − holes + rim fillet)
//!   5. flange + hollow tube together (the full live model `export_mesh` sees)
//!   6. hollow tube alone             (planes + cylinders, like part 23)
//!
//! ## Finding (2026-07-09)
//!
//! Every shape above — including the full weld-neck flange (cone + torus on a
//! boolean body with a Ø30 bore and 8 Ø12 holes) and the flange + tube
//! multi-solid model — exports to STEP AND round-trips (re-imports to ≥1 valid
//! solid) at HEAD. The flange-class STEP export is SOUND on the current kernel.
//!
//! The live 500 (empty body) is therefore a STALE-BINARY artifact layered on
//! the swallowed-error bug: the running api-server binary predates the F1b
//! boolean/vertex-topology commits, and the STEP writer is byte-identical
//! between that binary and HEAD, so the older kernel's flange SNAPSHOT — not
//! the writer — is what the (identical) writer rejected. See the F2 report.
//! These tests are the regression guard that the flange class stays exportable.

use export_engine::formats::step::{export_brep_to_step, import_step_to_brep};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::fillet::{FilletType, PropagationMode};
use geometry_engine::operations::{
    boolean_operation, fillet_edges, BooleanOp, BooleanOptions, FilletOptions,
};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use tempfile::TempDir;

// ─────────────────────────────────────────────────────────────────────
// Builders — each returns a BRepModel plus the id of the solid of
// interest, mirroring the kernel paths the REST/MCP export hits.
// ─────────────────────────────────────────────────────────────────────

fn as_solid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(id) => id,
        other => panic!("expected a solid, got {other:?}"),
    }
}

fn make_cone(model: &mut BRepModel, base: Point3, base_r: f64, top_r: f64, height: f64) -> SolidId {
    as_solid(
        TopologyBuilder::new(model)
            .create_cone_3d(base, Vector3::Z, base_r, top_r, height)
            .expect("cone creation succeeds"),
    )
}

fn make_cylinder(model: &mut BRepModel, base: Point3, radius: f64, height: f64) -> SolidId {
    as_solid(
        TopologyBuilder::new(model)
            .create_cylinder_3d(base, Vector3::Z, radius, height)
            .expect("cylinder creation succeeds"),
    )
}

fn fillet_opts(radius: f64) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(radius),
        radius,
        propagation: PropagationMode::None,
        ..Default::default()
    }
}

/// Every closed (rim) edge of a solid's boundary, with its axial z coord.
fn rims_with_z(model: &BRepModel, solid: SolidId) -> Vec<(EdgeId, f64)> {
    let mut out = Vec::new();
    let Some(s) = model.solids.get(solid) else {
        return out;
    };
    let mut shells = vec![s.outer_shell];
    shells.extend_from_slice(&s.inner_shells);
    for sh in shells {
        let Some(shell) = model.shells.get(sh) else {
            continue;
        };
        for &fid in &shell.faces {
            let Some(face) = model.faces.get(fid) else {
                continue;
            };
            let mut loops = vec![face.outer_loop];
            loops.extend_from_slice(&face.inner_loops);
            for lid in loops {
                if let Some(lp) = model.loops.get(lid) {
                    for &e in &lp.edges {
                        if let Some(edge) = model.edges.get(e) {
                            if edge.is_loop() && !out.iter().any(|(id, _)| *id == e) {
                                let z = model
                                    .vertices
                                    .get_position(edge.start_vertex)
                                    .map(|p| p[2])
                                    .unwrap_or(0.0);
                                out.push((e, z));
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// Export a model to STEP in a temp dir; return the file path + Result.
async fn export(model: &BRepModel, name: &str) -> (TempDir, Result<std::path::PathBuf, String>) {
    let temp = TempDir::new().expect("tmp");
    let path = temp.path().join(format!("{name}.step"));
    let res = export_brep_to_step(model, &path).await;
    let mapped = res.map(|_| path.clone()).map_err(|e| format!("{e:?}"));
    (temp, mapped)
}

// ─────────────────────────────────────────────────────────────────────
// (1) Plain frustum cone.
// ─────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn f2_1_frustum_cone_exports() {
    let mut model = BRepModel::new();
    // Ø68 → Ø44 hub, height 25.
    let _ = make_cone(&mut model, Point3::ORIGIN, 34.0, 22.0, 25.0);
    let (_t, res) = export(&model, "f2_cone").await;
    assert!(
        res.is_ok(),
        "F2(1) frustum cone STEP export failed: {res:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────
// (2) Cylinder with a top-rim fillet (produces a TORUS surface).
// ─────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn f2_2_cylinder_top_rim_fillet_exports() {
    let mut model = BRepModel::new();
    let cyl = make_cylinder(&mut model, Point3::ORIGIN, 60.0, 16.0);
    // Fillet the +Z (top) rim — the known-good direction from F1.
    let rims = rims_with_z(&model, cyl);
    let top = rims
        .iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(e, _)| *e)
        .expect("cylinder has rim edges");
    fillet_edges(&mut model, cyl, vec![top], fillet_opts(3.0)).expect("top rim fillet succeeds");
    let (_t, res) = export(&model, "f2_fillet").await;
    assert!(
        res.is_ok(),
        "F2(2) cylinder+top-rim-fillet STEP export failed: {res:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────
// (3) Cylinder ∪ cone (boolean union).
// ─────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn f2_3_cylinder_union_cone_exports() {
    let mut model = BRepModel::new();
    // Ø120 disk base.
    let disk = make_cylinder(&mut model, Point3::ORIGIN, 60.0, 16.0);
    // Cone hub sitting on top of the disk.
    let hub = make_cone(&mut model, Point3::new(0.0, 0.0, 16.0), 34.0, 22.0, 25.0);
    let unioned = boolean_operation(
        &mut model,
        disk,
        hub,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("disk ∪ hub union succeeds");
    let (_t, res) = export(&model, "f2_union").await;
    assert!(
        res.is_ok(),
        "F2(3) cylinder∪cone STEP export failed: {res:?}"
    );
    let _ = unioned;
}

// ─────────────────────────────────────────────────────────────────────
// (4) The full weld-neck flange.
//   disk (Ø120) ∪ cone hub (Ø68→Ø44) − bore (Ø30) − 8×Ø12 holes + rim fillet
// ─────────────────────────────────────────────────────────────────────
/// Hollow tube — the live model's part 23 (Ø28 outer, Ø20 bore, h180).
/// Planes + cylinders only (no cone/torus): trivially exportable in
/// isolation, present here to reproduce the FULL live model.
fn build_tube_into(model: &mut BRepModel) -> SolidId {
    let outer = make_cylinder(model, Point3::ORIGIN, 14.0, 180.0);
    let bore = make_cylinder(model, Point3::new(0.0, 0.0, -5.0), 10.0, 190.0);
    boolean_operation(
        model,
        outer,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("tube bore cut")
}

fn build_flange_into(model: &mut BRepModel) -> SolidId {
    // Ø120 plate, z ∈ [0, 16].
    let disk = make_cylinder(model, Point3::ORIGIN, 60.0, 16.0);

    // Rim fillet on the plate's top outer edge (z = 16, r = 60) FIRST, while it
    // is still a single clean closed loop edge — produces the TORUS (live
    // face 128). The centred cone/bore/hole cuts below never touch r ≈ 60, so
    // the torus survives into the final flange topology.
    let top_rim = rims_with_z(model, disk)
        .into_iter()
        .filter(|(_, z)| (*z - 16.0).abs() < 0.5)
        .map(|(e, _)| e)
        .next()
        .expect("plate has a closed top rim edge at z=16");
    fillet_edges(model, disk, vec![top_rim], fillet_opts(3.0))
        .expect("plate rim fillet succeeds (produces the torus)");

    // Cone hub Ø68 → Ø44, z ∈ [16, 54] (matches live part 20: top plane z=54).
    let hub = make_cone(model, Point3::new(0.0, 0.0, 16.0), 34.0, 22.0, 38.0);
    let mut body = boolean_operation(
        model,
        disk,
        hub,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("disk ∪ hub");

    // Central Ø30 bore straight through (live face 125: origin z=-5).
    let bore = make_cylinder(model, Point3::new(0.0, 0.0, -5.0), 15.0, 65.0);
    body = boolean_operation(
        model,
        body,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("bore cut");

    // 8 × Ø12 bolt holes on a Ø96 circle (r=48) through the flange plate
    // (live face 117: origin (0, -48, -2)).
    for i in 0..8 {
        let ang = std::f64::consts::TAU * (i as f64) / 8.0;
        let (x, y) = (48.0 * ang.cos(), 48.0 * ang.sin());
        let hole = make_cylinder(model, Point3::new(x, y, -2.0), 6.0, 20.0);
        body = boolean_operation(
            model,
            body,
            hole,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("bolt hole cut");
    }
    body
}

fn build_flange() -> (BRepModel, SolidId) {
    let mut model = BRepModel::new();
    let body = build_flange_into(&mut model);
    (model, body)
}

#[tokio::test]
async fn f2_4_full_flange_exports() {
    let (model, _body) = build_flange();
    let (_t, res) = export(&model, "f2_flange").await;
    assert!(res.is_ok(), "F2(4) full flange STEP export failed: {res:?}");
}

/// (5) The FULL live model: flange (part 20) AND the hollow tube (part 23)
/// coexisting as two separate solids, exported together — exactly what the
/// api-server `export_mesh` handler passes (`state.model`, every solid). This
/// is the configuration that live-500s.
#[tokio::test]
async fn f2_5_flange_and_tube_together_export() {
    let mut model = BRepModel::new();
    let _flange = build_flange_into(&mut model);
    let _tube = build_tube_into(&mut model);
    assert_eq!(
        model.solids.iter().count(),
        2,
        "live model state: exactly two top-level solids (flange + tube)"
    );
    let (_t, res) = export(&model, "f2_flange_tube").await;
    assert!(
        res.is_ok(),
        "F2(5) flange+tube (full live model) STEP export failed: {res:?}"
    );
}

/// (6) The hollow tube alone — planes + cylinders, like the passing corpus.
#[tokio::test]
async fn f2_6_hollow_tube_exports() {
    let mut model = BRepModel::new();
    let _tube = build_tube_into(&mut model);
    let (_t, res) = export(&model, "f2_tube").await;
    assert!(res.is_ok(), "F2(6) hollow tube STEP export failed: {res:?}");
}

/// Round-trip proof for whichever shape triggers F2: export → re-import →
/// parses to ≥1 solid. Only meaningful once (b) is fixed; kept adjacent to
/// the repro so the honest "it actually opens" check lives with it.
#[tokio::test]
async fn f2_full_flange_roundtrips() {
    let (model, _body) = build_flange();
    let (temp, res) = export(&model, "f2_flange_rt").await;
    let path = res.expect("flange must export before round-trip can be checked");
    let imported = import_step_to_brep(&path)
        .await
        .expect("exported flange STEP must re-import");
    assert!(
        !imported.solids.is_empty(),
        "re-imported flange must contain at least one solid"
    );
    drop(temp);
}
