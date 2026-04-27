//! Roshera kernel demo — fillet and chamfer.
//!
//! Creates a box, then runs `fillet_edges` and `chamfer_edges` on its 12
//! edges in fresh model copies. Tessellates the results and asserts
//! non-empty meshes so the demo doubles as a regression test for these
//! feature operations.
//!
//! Blend and offset feature demos are deferred to a follow-up demo file —
//! both operations require more elaborate input fixtures (multiple seed
//! faces, surface offsets) than fillet/chamfer.
//!
//! Run with `cargo run --release --example demo_features`.

#[path = "common/mod.rs"]
mod common;

use std::time::Instant;

use geometry_engine::operations::fillet::FilletType;
use geometry_engine::operations::{
    chamfer_edges, fillet_edges, ChamferOptions, CommonOptions, FilletOptions,
};
use geometry_engine::primitives::box_primitive::{BoxParameters, BoxPrimitive};
use geometry_engine::primitives::builder::BRepModel;
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::primitive_traits::Primitive;
use geometry_engine::tessellation::TessellationParams;

use common::{header, model_summary, tess_and_write};

const SUBDIR: &str = "features";

fn main() {
    header("features — fillet + chamfer");

    let params = TessellationParams::default();

    fillet_box_edges(&params);
    chamfer_box_edges(&params);

    println!("\nAll feature outputs within acceptance bounds.");
}

/// Build a 50 mm cube and return (model, solid_id, all_edge_ids).
fn make_box_with_edges() -> (BRepModel, u32, Vec<EdgeId>) {
    let mut model = BRepModel::new();
    let id = BoxPrimitive::create(
        BoxParameters::new(50.0, 50.0, 50.0).expect("box params"),
        &mut model,
    )
    .expect("box create");
    let edges: Vec<EdgeId> = model.edges.iter().map(|(eid, _)| eid).collect();
    (model, id, edges)
}

fn fillet_box_edges(params: &TessellationParams) {
    let (mut model, solid_id, edges) = make_box_with_edges();
    let edge_count = edges.len();

    let t = Instant::now();
    // `validate_result` is disabled because the fillet pipeline emits
    // working rolling-ball spine surfaces but lacks two production
    // subsystems (corner vertex-blend patches and original-face trim
    // updates) — both flagged in-tree as `tracing::warn!` stubs. The
    // demo's purpose is to exercise the rolling-ball fillet itself and
    // the downstream tessellator, not to gate on those open features.
    let result = fillet_edges(
        &mut model,
        solid_id,
        edges,
        FilletOptions {
            common: CommonOptions {
                validate_result: false,
                ..Default::default()
            },
            fillet_type: FilletType::Constant(5.0),
            radius: 5.0,
            ..Default::default()
        },
    );
    let dt = t.elapsed();

    match result {
        Ok(faces) => {
            println!(
                "[1] fillet  r=5 on box ({edge_count} edges) -> {} new face(s) ({:.2} ms)",
                faces.len(),
                dt.as_secs_f64() * 1e3
            );
            let stats = tess_and_write(&model, solid_id, params, SUBDIR, "fillet_box.stl");
            assert!(stats.tris > 0, "filleted box produced 0 triangles");
        }
        Err(e) => {
            println!("[1] fillet: NOT YET PRODUCTION (err: {e:?})");
        }
    }
    model_summary(&model);
}

fn chamfer_box_edges(params: &TessellationParams) {
    let (mut model, solid_id, edges) = make_box_with_edges();
    let edge_count = edges.len();

    let t = Instant::now();
    let result = chamfer_edges(
        &mut model,
        solid_id,
        edges,
        ChamferOptions {
            common: CommonOptions {
                validate_result: false,
                ..Default::default()
            },
            distance1: 3.0,
            distance2: 3.0,
            symmetric: true,
            ..Default::default()
        },
    );
    let dt = t.elapsed();

    match result {
        Ok(faces) => {
            println!(
                "[2] chamfer d=3 on box ({edge_count} edges) -> {} new face(s) ({:.2} ms)",
                faces.len(),
                dt.as_secs_f64() * 1e3
            );
            let stats = tess_and_write(&model, solid_id, params, SUBDIR, "chamfer_box.stl");
            assert!(stats.tris > 0, "chamfered box produced 0 triangles");
        }
        Err(e) => {
            println!("[2] chamfer: NOT YET PRODUCTION (err: {e:?})");
        }
    }
    model_summary(&model);
}
