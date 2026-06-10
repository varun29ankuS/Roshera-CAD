//! Cone radial-family diagnostic — the post-sphere-campaign worst class.
//!
//! The 2026-06-10 full-matrix re-survey (box∘cone: 21 HARD / 30 checks) shows
//! every RADIAL cone cut (cone axis parallel to a box face, poking the box
//! sideways through a face / edge / corner) broken across ∩/∪/∖, while the
//! axial family (apex-through, frustum-through, contained) is mostly clean.
//! This serial repro prints, per failing cell × op: the concrete error variant
//! (the survey catalog only records "op errored"), the volume vs grid truth,
//! B-Rep open / nonmanifold edge counts, and the result's surface-type tally —
//! the same forensic signals that cracked the sphere corner/poke family.
//!
//! Run: `cargo test -p geometry-engine --test diag_cone_radial -- --ignored --nocapture`

use geometry_engine::harness::brep_integrity::brep_integrity;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

#[allow(clippy::expect_used, clippy::panic)] // diagnostic-only test fixture
fn the_box(model: &mut BRepModel) -> SolidId {
    match TopologyBuilder::new(model)
        .create_box_3d(2.0, 2.0, 2.0)
        .expect("box")
    {
        GeometryId::Solid(id) => id,
        o => panic!("box: {o:?}"),
    }
}

#[allow(clippy::expect_used, clippy::panic)] // diagnostic-only test fixture
fn cone(model: &mut BRepModel, bc: [f64; 3], rb: f64, rt: f64, h: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_cone_3d(Point3::new(bc[0], bc[1], bc[2]), Vector3::Z, rb, rt, h)
        .expect("cone")
    {
        GeometryId::Solid(id) => id,
        o => panic!("cone: {o:?}"),
    }
}

fn surface_tally(model: &BRepModel, solid: SolidId) -> std::collections::BTreeMap<String, usize> {
    let mut tally = std::collections::BTreeMap::new();
    let shell_ids = model
        .solids
        .get(solid)
        .map(|s| s.all_shells())
        .unwrap_or_default();
    for sid in shell_ids {
        let face_ids = model
            .shells
            .get(sid)
            .map(|sh| sh.faces.clone())
            .unwrap_or_default();
        for fid in face_ids {
            let Some(face) = model.faces.get(fid) else {
                continue;
            };
            let sty = model
                .surfaces
                .get(face.surface_id)
                .map(|s| format!("{:?}", s.surface_type()))
                .unwrap_or_else(|| "?".into());
            *tally.entry(sty).or_insert(0) += 1;
        }
    }
    tally
}

/// The six survey-HARD box∘cone cells (box [-1,1]³, z-axis cones), with the
/// grid-oracle truth volumes from the 2026-06-10 catalog for in-place reading.
/// (base_center, rb, rt, h, label, [truth ∩, truth ∪, truth ∖])
fn failing_cells() -> Vec<([f64; 3], f64, f64, f64, &'static str, [f64; 3])> {
    vec![
        (
            [1.0, 0.0, -0.5],
            0.5,
            0.3,
            1.0,
            "radial-face+x",
            [0.256, 8.000, 7.744],
        ),
        (
            [1.0, 1.0, -0.5],
            0.5,
            0.3,
            1.0,
            "radial-edge",
            [0.128, 8.000, 7.872],
        ),
        (
            [1.0, 1.0, 0.5],
            0.5,
            0.0,
            1.0,
            "corner",
            [0.057, 8.205, 7.943],
        ),
        (
            [1.4, 0.0, -0.5],
            0.6,
            0.4,
            1.0,
            "radial-poke-past",
            [0.048, 8.690, 7.952],
        ),
        (
            [0.0, 0.0, -1.5],
            1.5,
            0.5,
            3.0,
            "wider-than-box",
            [5.888, 12.323, 2.112],
        ),
        (
            [0.0, 0.0, -1.0],
            0.8,
            0.4,
            2.0,
            "frustum-through",
            [2.412, 8.000, 5.588],
        ),
    ]
}

#[test]
#[ignore = "diagnostic — box∘cone radial family, 21 survey-HARD checks (run with --ignored --nocapture)"]
fn diag_cone_radial_family() {
    let ops = [
        (BooleanOp::Intersection, "I", 0usize),
        (BooleanOp::Union, "U", 1),
        (BooleanOp::Difference, "D", 2),
    ];
    for (bc, rb, rt, h, label, truths) in failing_cells() {
        println!("\n##### {label}  bc={bc:?} rb={rb} rt={rt} h={h} #####");
        for (op, sym, ti) in ops {
            let mut model = BRepModel::new();
            let bx = the_box(&mut model);
            let cn = cone(&mut model, bc, rb, rt, h);
            match boolean_operation(&mut model, bx, cn, op, BooleanOptions::default()) {
                Ok(res) => {
                    let vol = model.calculate_solid_volume(res).unwrap_or(f64::NAN);
                    let rep = brep_integrity(&model, res, 1e-6);
                    println!(
                        "[{sym}] vol={vol:.4} (truth {t:.3})  open={o} nonman={n}  faces={tally:?}",
                        t = truths[ti],
                        o = rep.edges_used_once.len(),
                        n = rep.edges_used_3plus.len(),
                        tally = surface_tally(&model, res),
                    );
                }
                Err(e) => println!("[{sym}] ERR {e:?}  (truth {t:.3})", t = truths[ti]),
            }
        }
    }
}
