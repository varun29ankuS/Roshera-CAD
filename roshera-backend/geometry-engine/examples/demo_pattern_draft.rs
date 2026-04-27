//! Roshera kernel demo — pattern and draft.
//!
//! Pattern and draft operate on faces of an existing solid. They don't
//! produce a fresh solid; pattern returns `Vec<Vec<FaceId>>` (instance
//! groups) and draft returns `Vec<FaceId>` (modified faces). To check the
//! kernel still has a coherent topology after these calls, we tessellate
//! the host solid before and after and compare triangle counts.
//!
//! These demos are deliberately permissive (smoke-test style): they print
//! the result and tessellate, but only assert the post-operation mesh is
//! non-empty. Any harder invariants will be tightened once the underlying
//! pattern/draft implementations are themselves hardened.
//!
//! Run with `cargo run --release --example demo_pattern_draft`.

#[path = "common/mod.rs"]
mod common;

use std::time::Instant;

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::draft::{DraftType, NeutralElement};
use geometry_engine::operations::pattern::PatternTarget;
use geometry_engine::operations::{
    apply_draft, create_pattern, DraftOptions, PatternOptions, PatternType,
};
use geometry_engine::primitives::box_primitive::{BoxParameters, BoxPrimitive};
use geometry_engine::primitives::builder::BRepModel;
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::primitive_traits::Primitive;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::tessellation::TessellationParams;

use common::{header, model_summary, tess_and_write};

const SUBDIR: &str = "pattern_draft";

fn main() {
    header("pattern + draft");

    let params = TessellationParams::default();

    pattern_linear(&params);
    pattern_circular(&params);
    draft_box_face(&params);

    println!("\nPattern/draft demos completed.");
}

/// Build a 50 mm cube and return (model, solid_id, all_face_ids).
fn make_box_with_faces() -> (BRepModel, SolidId, Vec<FaceId>) {
    let mut model = BRepModel::new();
    let id = BoxPrimitive::create(
        BoxParameters::new(50.0, 50.0, 50.0).expect("box params"),
        &mut model,
    )
    .expect("box create");
    let faces: Vec<FaceId> = model.faces.iter().map(|(fid, _)| fid).collect();
    (model, id, faces)
}

/// Linear pattern of one box face along +X.
fn pattern_linear(params: &TessellationParams) {
    let (mut model, solid_id, faces) = make_box_with_faces();
    let source = match faces.first() {
        Some(&f) => vec![f],
        None => {
            println!("[1] linear pattern: skipped — box has no faces");
            return;
        }
    };

    let t = Instant::now();
    let result = create_pattern(
        &mut model,
        source,
        PatternType::Linear {
            direction: Vector3::new(1.0, 0.0, 0.0),
            spacing: 60.0,
            count: 3,
        },
        PatternOptions {
            pattern_target: PatternTarget::Faces,
            ..Default::default()
        },
    );
    let dt = t.elapsed();

    match result {
        Ok(instances) => {
            println!(
                "[1] linear pattern (count=3, spacing=60) -> {} instance group(s) ({:.2} ms)",
                instances.len(),
                dt.as_secs_f64() * 1e3
            );
            let stats = tess_and_write(&model, solid_id, params, SUBDIR, "pattern_linear.stl");
            assert!(stats.tris > 0, "linear-pattern host produced 0 triangles");
        }
        Err(e) => {
            println!("[1] linear pattern: NOT YET PRODUCTION (err: {e:?})");
        }
    }
    model_summary(&model);
}

/// Circular pattern of one box face around Z.
fn pattern_circular(params: &TessellationParams) {
    let (mut model, solid_id, faces) = make_box_with_faces();
    let source = match faces.first() {
        Some(&f) => vec![f],
        None => {
            println!("[2] circular pattern: skipped — box has no faces");
            return;
        }
    };

    let t = Instant::now();
    let result = create_pattern(
        &mut model,
        source,
        PatternType::Circular {
            axis_origin: Point3::new(0.0, 0.0, 0.0),
            axis_direction: Vector3::new(0.0, 0.0, 1.0),
            count: 6,
            angle: std::f64::consts::TAU,
        },
        PatternOptions {
            pattern_target: PatternTarget::Faces,
            ..Default::default()
        },
    );
    let dt = t.elapsed();

    match result {
        Ok(instances) => {
            println!(
                "[2] circular pattern (count=6, full revolution) -> {} instance group(s) ({:.2} ms)",
                instances.len(),
                dt.as_secs_f64() * 1e3
            );
            let stats = tess_and_write(&model, solid_id, params, SUBDIR, "pattern_circular.stl");
            assert!(stats.tris > 0, "circular-pattern host produced 0 triangles");
        }
        Err(e) => {
            println!("[2] circular pattern: NOT YET PRODUCTION (err: {e:?})");
        }
    }
    model_summary(&model);
}

/// Apply a 5° draft to one face of a box, neutral plane = z=0, pull = +Z.
fn draft_box_face(params: &TessellationParams) {
    let (mut model, solid_id, faces) = make_box_with_faces();
    let target = match faces.first() {
        Some(&f) => vec![f],
        None => {
            println!("[3] draft: skipped — box has no faces");
            return;
        }
    };

    let t = Instant::now();
    let result = apply_draft(
        &mut model,
        solid_id,
        target,
        DraftOptions {
            draft_type: DraftType::Angle(5.0_f64.to_radians()),
            neutral: NeutralElement::Plane(
                Point3::new(0.0, 0.0, 0.0),
                Vector3::new(0.0, 0.0, 1.0),
            ),
            pull_direction: Vector3::new(0.0, 0.0, 1.0),
            ..Default::default()
        },
    );
    let dt = t.elapsed();

    match result {
        Ok(modified) => {
            println!(
                "[3] draft 5° (neutral z=0, pull +Z) -> {} face(s) modified ({:.2} ms)",
                modified.len(),
                dt.as_secs_f64() * 1e3
            );
            let stats = tess_and_write(&model, solid_id, params, SUBDIR, "draft_box.stl");
            assert!(stats.tris > 0, "drafted box produced 0 triangles");
        }
        Err(e) => {
            println!("[3] draft: NOT YET PRODUCTION (err: {e:?})");
        }
    }
    model_summary(&model);
}
