//! Layer-1 loop diagnostic — the box∘cylinder HARD cells from the 2026-06-10
//! subprocess-isolated baseline (the honest one): `offset-through` (cylinder
//! through the box, axis offset from centre: ∩/∪ ERROR + ∖ −36.8% VOLUME that
//! was hidden under a HANG bin) and `axial-poke+z` (∪ ERROR, ∩/∖ open=32 —
//! the open count suggests an unwelded cut-circle subdivided ~32 ways).
//! Serial repro printing the concrete error variants, volumes vs grid truth,
//! B-Rep open/nonmanifold counts, and the result surface-type tally.
//!
//! Run: `cargo test -p geometry-engine --test diag_cyl_offset -- --ignored --nocapture`

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
fn cylinder(model: &mut BRepModel, base: [f64; 3], r: f64, h: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_cylinder_3d(Point3::new(base[0], base[1], base[2]), Vector3::Z, r, h)
        .expect("cylinder")
    {
        GeometryId::Solid(id) => id,
        o => panic!("cylinder: {o:?}"),
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

/// The two HARD box∘cylinder cells (box [-1,1]³, z-axis cylinders), with grid
/// truths from the isolated baseline for in-place reading.
/// (base, r, h, label, [truth ∩, truth ∪, truth ∖])
fn failing_cells() -> Vec<([f64; 3], f64, f64, &'static str, [f64; 3])> {
    vec![
        (
            [0.5, 0.3, -1.5],
            0.6,
            3.0,
            "offset-through",
            [2.172, 9.220, 5.828],
        ),
        (
            [0.0, 0.0, 0.0],
            0.5,
            1.0,
            "axial-poke+z",
            [0.785, 8.001, 7.215],
        ),
    ]
}

/// Ratchet gate (NON-ignored): the axial-poke+z ∩/∖ cells conquered by the
/// coplanar-cap arc-lift fix (chord-lifted cap rims could never weld to the
/// partner body's analytic arc rim — open=32 both ops, now 0). Cylinder
/// r=0.5 h=1 base origin sits fully inside the box, cap flush with z=1:
/// analytic ∩ = πr²h = π/4, ∖ = 8 − π/4. Serial run (correctness gates never
/// run under a wall-clock thread budget). ∪ is NOT pinned — still errors
/// (lone 1-planar-face component, coplanar cap disk unmerged under union).
#[test]
fn cyl_axial_poke_gate() {
    let truth_i = std::f64::consts::PI * 0.25;
    let cases = [
        (BooleanOp::Intersection, "I", truth_i),
        (BooleanOp::Difference, "D", 8.0 - truth_i),
    ];
    for (op, sym, truth) in cases {
        let mut model = BRepModel::new();
        let bx = the_box(&mut model);
        let cy = cylinder(&mut model, [0.0, 0.0, 0.0], 0.5, 1.0);
        let res = match boolean_operation(&mut model, bx, cy, op, BooleanOptions::default()) {
            Ok(res) => res,
            Err(e) => {
                assert!(false, "[{sym}] axial-poke errored: {e:?}");
                return;
            }
        };
        let vol = model.calculate_solid_volume(res).unwrap_or(f64::NAN);
        let rel = (vol - truth).abs() / truth;
        assert!(
            rel < 0.01,
            "[{sym}] axial-poke volume {vol:.4} vs analytic {truth:.4} (rel {rel:.4})"
        );
        let rep = brep_integrity(&model, res, 1e-6);
        assert!(
            rep.edges_used_once.is_empty(),
            "[{sym}] axial-poke open edges: {:?}",
            rep.edges_used_once
        );
        assert!(
            rep.edges_used_3plus.is_empty(),
            "[{sym}] axial-poke non-manifold edges: {:?}",
            rep.edges_used_3plus
        );
    }
}

#[test]
#[ignore = "diagnostic — box∘cylinder offset-through + axial-poke (run with --ignored --nocapture)"]
fn diag_cyl_offset_family() {
    let ops = [
        (BooleanOp::Intersection, "I", 0usize),
        (BooleanOp::Union, "U", 1),
        (BooleanOp::Difference, "D", 2),
    ];
    for (base, r, h, label, truths) in failing_cells() {
        println!("\n##### {label}  base={base:?} r={r} h={h} #####");
        for (op, sym, ti) in ops {
            let mut model = BRepModel::new();
            let bx = the_box(&mut model);
            let cy = cylinder(&mut model, base, r, h);
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
                    // Forensics for the weld question: every open (single-use)
                    // edge's curve type + endpoints. A subdivision mismatch
                    // shows as two coincident chains with different split
                    // points; a missing twin shows as an isolated full curve.
                    for &eid in &rep.edges_used_once {
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
                            "    open {eid:?} {cty} {} -> {}",
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
