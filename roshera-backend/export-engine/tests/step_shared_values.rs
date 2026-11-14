// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A STEP file writes each geometric VALUE once.
//!
//! A B-Rep repeats the same values constantly: every edge end is a vertex's
//! point, every planar face of a box shares an axis direction, every
//! cylinder frame repeats the axis. STEP lets any number of entities
//! reference one `CARTESIAN_POINT` / `DIRECTION` / `VECTOR` / placement, so
//! writing a value twice only makes the file bigger and the reader slower.
//! Topology is untouched: coincident vertices stay distinct entities.

use std::collections::HashMap;

use export_engine::ExportEngine;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::topology_builder::{BRepModel, TopologyBuilder};
use tempfile::TempDir;

const SHARED: [&str; 5] = [
    "CARTESIAN_POINT",
    "DIRECTION",
    "VECTOR",
    "AXIS2_PLACEMENT_3D",
    "AXIS1_PLACEMENT",
];

fn bored_block() -> BRepModel {
    let mut m = BRepModel::new();
    {
        let mut b = TopologyBuilder::new(&mut m);
        b.create_box_3d(40.0, 30.0, 20.0).expect("box");
        b.create_cylinder_3d(Point3::new(5.0, 0.0, -15.0), Vector3::Z, 6.0, 30.0)
            .expect("cylinder");
    }
    let ids: Vec<_> = m.solids.iter().map(|(id, _)| id).collect();
    boolean_operation(
        &mut m,
        ids[0],
        ids[1],
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("bore");
    m
}

/// Each value entity's body (everything after `#n=`), with how many entities
/// carry it.
fn value_bodies(text: &str) -> HashMap<String, usize> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let Some((id, body)) = line.split_once('=') else {
            continue;
        };
        if !id.starts_with('#') {
            continue;
        }
        if SHARED.iter().any(|k| body.starts_with(&format!("{k}("))) {
            *out.entry(body.to_string()).or_insert(0) += 1;
        }
    }
    out
}

#[tokio::test]
async fn each_geometric_value_is_written_once_and_the_part_survives() {
    let mut model = bored_block();
    let original_volume = {
        let id = model.solids.iter().next().map(|(id, _)| id).expect("solid");
        model.calculate_solid_volume(id).expect("volume")
    };

    let temp = TempDir::new().expect("tmp");
    let engine = ExportEngine::with_output_directory(temp.path().to_string_lossy().to_string());
    let filename = engine
        .export_step(&model, "bored_block")
        .await
        .expect("export");
    let path = temp.path().join(&filename);
    let text = std::fs::read_to_string(&path).expect("read step");

    let bodies = value_bodies(&text);
    assert!(
        bodies.len() > 20,
        "the file must carry geometry ({} values)",
        bodies.len()
    );
    let repeated: Vec<_> = bodies.iter().filter(|(_, &n)| n > 1).collect();
    assert!(
        repeated.is_empty(),
        "{} value entities are written more than once, e.g. {:?}",
        repeated.len(),
        repeated.iter().take(3).collect::<Vec<_>>()
    );

    // The shared file still describes the same part.
    let (mut imported, report) =
        export_engine::formats::step::import_step_to_brep_with_report(&path)
            .await
            .expect("import");
    assert!(
        report.ok,
        "the re-imported part must be whole and sound: {report:?}"
    );
    assert_eq!(imported.solids.len(), 1);
    let id = imported
        .solids
        .iter()
        .next()
        .map(|(id, _)| id)
        .expect("solid");
    let volume = imported.calculate_solid_volume(id).expect("volume");
    assert!(
        (volume - original_volume).abs() <= 1e-6 * original_volume,
        "volume {volume} after the round trip, {original_volume} before"
    );
}
