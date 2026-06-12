//! Tilted-cylinder UNION diagnostic — the box∘tcyl HARD residue after the
//! 2026-06-11 cylinder-family fixes (arc-lift, kept-island hole-attachment,
//! radial-dir). Survey state: 7 HARD / 18 checks, and EVERY failure is
//! UNION-only with small open/nonman counts (∩/∖ clean across the class):
//!   [∪] tilt-skew r=0.25 h=2          : op errored        (truth 8.048)
//!   [∪] tilt-edge-yz r=0.3 h=2        : open=4 nonman=2
//!   [∪] tilt-poke+z r=0.3 h=2         : open=2 nonman=1
//!   [∪] tilt-xy-horizontal r=0.3 h=2  : open=4 nonman=2
//! Tilted axis ⇒ oblique plane∘cylinder cuts (ellipse arm) — this smells
//! like the union rim-weld / kept-fragment family on ELLIPTICAL rims.
//! Serial repro printing per-op error variants, volume vs analytic/grid
//! truth, B-Rep open/nonmanifold counts, surface tally, and open-edge
//! forensics (curve type + endpoints).
//!
//! Run: `cargo test -p geometry-engine --test diag_tcyl -- --ignored --nocapture`

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
fn tcyl(model: &mut BRepModel, base: [f64; 3], axis: [f64; 3], r: f64, h: f64) -> SolidId {
    let mag = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    let u = Vector3::new(axis[0] / mag, axis[1] / mag, axis[2] / mag);
    match TopologyBuilder::new(model)
        .create_cylinder_3d(Point3::new(base[0], base[1], base[2]), u, r, h)
        .expect("tilted cylinder")
    {
        GeometryId::Solid(id) => id,
        o => panic!("tcyl: {o:?}"),
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

/// The four ∪-HARD box∘tcyl cells (box [-1,1]³), with survey grid truths.
/// (base, axis, r, h, label, [truth ∩, truth ∪, truth ∖])
fn failing_cells() -> Vec<([f64; 3], [f64; 3], f64, f64, &'static str, [f64; 3])> {
    vec![
        (
            [0.0, 0.0, -1.0],
            [0.3, 0.0, 1.0],
            0.3,
            2.0,
            "tilt-poke+z",
            [f64::NAN, f64::NAN, f64::NAN],
        ),
        (
            [0.0, -1.0, -0.5],
            [0.0, 1.0, 1.0],
            0.3,
            2.0,
            "tilt-edge-yz",
            [f64::NAN, f64::NAN, f64::NAN],
        ),
        (
            [-1.0, -0.6, 0.0],
            [1.0, 1.0, 0.0],
            0.3,
            2.0,
            "tilt-xy-horizontal",
            [f64::NAN, f64::NAN, f64::NAN],
        ),
        (
            [-0.5, -0.5, -1.2],
            [0.5, 0.5, 1.0],
            0.25,
            2.0,
            "tilt-skew",
            [f64::NAN, 8.048, f64::NAN],
        ),
    ]
}

#[test]
#[ignore = "diagnostic — box∘tilted-cylinder UNION residue (run with --ignored --nocapture)"]
fn diag_tcyl_union_family() {
    let ops = [
        (BooleanOp::Intersection, "I", 0usize),
        (BooleanOp::Union, "U", 1),
        (BooleanOp::Difference, "D", 2),
    ];
    for (base, axis, r, h, label, truths) in failing_cells() {
        println!("\n##### {label}  base={base:?} axis={axis:?} r={r} h={h} #####");
        for (op, sym, ti) in ops {
            let mut model = BRepModel::new();
            let bx = the_box(&mut model);
            let cy = tcyl(&mut model, base, axis, r, h);
            match boolean_operation(&mut model, bx, cy, op, BooleanOptions::default()) {
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
                    let nonman_ids: Vec<_> = rep.edges_used_3plus.iter().map(|t| t.0).collect();
                    for &eid in rep.edges_used_once.iter().chain(nonman_ids.iter()) {
                        let Some(edge) = model.edges.get(eid) else {
                            continue;
                        };
                        let cty = model
                            .curves
                            .get(edge.curve_id)
                            .map(|c| c.type_name())
                            .unwrap_or("?");
                        let pos = |vid| {
                            model
                                .vertices
                                .get(vid)
                                .map(|v| {
                                    format!(
                                        "({:.3},{:.3},{:.3})",
                                        v.position[0], v.position[1], v.position[2]
                                    )
                                })
                                .unwrap_or_else(|| "?".into())
                        };
                        println!(
                            "    edge {eid:?} {cty} {} -> {}",
                            pos(edge.start_vertex),
                            pos(edge.end_vertex)
                        );
                    }
                }
                Err(e) => println!("[{sym}] ERR {e:?}  (truth {t:.3})", t = truths[ti]),
            }
        }
    }
}
