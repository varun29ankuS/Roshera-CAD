// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Importing the same STEP file twice gives every face the same numeric id.
//! An agent (or a person) that names "face 41" must get face 41 again after
//! a re-import; before, the importer walked the file's entities out of a
//! hash map whose order changes on every run, so the same face came back as
//! 4, 149, 141, 18... on successive imports of CADGenBench part 102.

use std::collections::BTreeMap;

use export_engine::ExportEngine;
use geometry_engine::math::Point3;
use geometry_engine::primitives::topology_builder::{BRepModel, TopologyBuilder};

/// A box with a through hole: a few dozen entities, enough for any hash
/// order to differ between runs.
async fn step_text() -> String {
    let mut model = BRepModel::new();
    TopologyBuilder::new(&mut model)
        .create_box_3d(20.0, 30.0, 10.0)
        .expect("box");
    TopologyBuilder::new(&mut model)
        .create_cylinder_3d(
            Point3::new(0.0, 0.0, 0.0),
            geometry_engine::math::Vector3::new(0.0, 0.0, 1.0),
            3.0,
            10.0,
        )
        .expect("cylinder");
    let mut path = std::env::temp_dir();
    path.push(format!("roshera_det_ids_{}.step", std::process::id()));
    export_engine::formats::step::export_brep_to_step(&model, &path)
        .await
        .expect("export");
    let text = std::fs::read_to_string(&path).expect("read back");
    let _ = std::fs::remove_file(&path);
    text
}

/// Each face's numeric id, by its persistent id.
fn ids_by_pid(text: &str) -> BTreeMap<u128, u32> {
    let (model, _) = ExportEngine::new()
        .import_step_content(text, "det")
        .expect("imports");
    model
        .faces
        .iter()
        .filter_map(|(id, _)| model.face_pid(id).map(|p| (p.0, id)))
        .collect()
}

#[tokio::test]
async fn the_same_file_imports_to_the_same_face_ids() {
    let text = step_text().await;
    let first = ids_by_pid(&text);
    assert!(
        first.len() >= 9,
        "box faces plus cylinder faces: {}",
        first.len()
    );
    for run in 1..6 {
        let again = ids_by_pid(&text);
        assert_eq!(again, first, "import {run} numbered the faces differently");
    }
}
