// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A static study on kernel-built parts, end to end.

use std::path::PathBuf;

use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
use sim_engine::engines::{EngineInfo, Engines};
use sim_engine::loads::Spread;
use sim_engine::run::{run_static, StudyError, Verdict};
use sim_engine::study::{Goal, Load, Material, MeshControl, Study, Support, SupportKind};

/// A 100 x 10 x 10 bar; the face whose centroid is most extreme along
/// `axis` in direction `sign`.
fn bar() -> (BRepModel, u32) {
    let mut m = BRepModel::new();
    let s = match TopologyBuilder::new(&mut m).create_box_3d(100.0, 10.0, 10.0) {
        Ok(GeometryId::Solid(s)) => s,
        other => panic!("box: {other:?}"),
    };
    (m, s)
}

fn face_at(m: &BRepModel, solid: u32, axis: usize, sign: f64) -> u32 {
    let mesh = tessellate_solid(
        m.solids.get(solid).unwrap(),
        m,
        &TessellationParams::default(),
    );
    let mut best: Option<(f64, u32)> = None;
    for (t, &f) in mesh.triangles.iter().zip(&mesh.face_map) {
        let c: f64 = t
            .iter()
            .map(|&i| {
                let p = mesh.vertices[i as usize].position;
                [p.x, p.y, p.z][axis]
            })
            .sum::<f64>()
            / 3.0;
        let score = sign * c;
        if best.map_or(true, |(b, _)| score > b + 1e-9) {
            best = Some((score, f));
        }
    }
    best.unwrap().1
}

fn aluminium(yield_mpa: Option<f64>) -> Material {
    Material {
        name: "Al 6061-T6 (test values)".to_string(),
        youngs_modulus_mpa: 68_900.0,
        poisson_ratio: 0.33,
        yield_strength_mpa: yield_mpa,
        source: yield_mpa.map(|_| "test value, not a cited datasheet".to_string()),
        yield_basis: yield_mpa.map(|_| sim_engine::materials::YieldBasis::Typical),
    }
}

fn no_engines() -> Engines {
    let e = |name: &str| EngineInfo {
        name: name.to_string(),
        program: PathBuf::from("not-run"),
        version: "none".to_string(),
        interim: true,
    };
    Engines {
        mesher: e("none"),
        solver: e("none"),
    }
}

/// A bar resting on a roller (its bottom face held along its normal only)
/// can slide and turn: the study is refused before any mesh or solve, and
/// the refusal names what is free.
#[test]
fn a_part_on_a_roller_is_refused_with_its_free_motions() {
    let (m, s) = bar();
    let bottom = face_at(&m, s, 2, -1.0);
    let top = face_at(&m, s, 2, 1.0);
    let study = Study {
        supports: vec![Support {
            kind: SupportKind::Slider,
            faces: vec![bottom],
        }],
        loads: vec![Load {
            faces: vec![top],
            force_n: [0.0, 0.0, -100.0],
            spread: Spread::Even,
        }],
        material: aluminium(None),
        goal: Goal {
            min_safety_factor: 2.0,
        },
        mesh: MeshControl::default(),
        accuracy: Default::default(),
    };
    let dir = std::env::temp_dir().join("roshera-sim-roller");
    match run_static(&m, s, &study, &no_engines(), &dir) {
        Err(StudyError::CanMove(free)) => {
            assert_eq!(free.len(), 3, "two slides and a turn: {free:?}");
            let words = StudyError::CanMove(free).to_string();
            assert!(
                words.contains("can slide") && words.contains("can turn"),
                "{words}"
            );
        }
        other => panic!("expected a refusal naming the free motions: {other:?}"),
    }
}

/// The cantilever from the 11-23 workflow demo, through the real engines:
/// fixed at x = 0, 100 N spread evenly over the top face. Euler-Bernoulli
/// tip deflection w L^4 / (8 E I) = 0.2177 mm (shear adds about 1% at
/// L/h = 10). The supports must carry the load exactly.
///
/// Needs Gmsh and CalculiX (FreeCAD 1.0's, or ROSHERA_GMSH / ROSHERA_CCX):
/// run with `--ignored`. Ignored so a machine without them cannot pass it
/// silently.
#[test]
#[ignore = "needs Gmsh and CalculiX; run with --ignored"]
fn a_cantilever_deflects_as_the_beam_formula_says() {
    let engines = Engines::discover().expect("Gmsh and CalculiX installed");
    let (mut m, s) = bar();
    // A second part in the same document, as in the app, lying wholly inside
    // the beam's bounding box: only the studied solid may reach the mesher,
    // or this one is meshed with it (the mesher keeps every volume inside
    // that box). Built before the faces are named, so ids are final.
    match TopologyBuilder::new(&mut m).create_box_3d(4.0, 4.0, 4.0) {
        Ok(GeometryId::Solid(_)) => {}
        other => panic!("second part: {other:?}"),
    }
    let (lo_x, top) = (face_at(&m, s, 0, -1.0), face_at(&m, s, 2, 1.0));
    let study = Study {
        supports: vec![Support {
            kind: SupportKind::Fixed,
            faces: vec![lo_x],
        }],
        loads: vec![Load {
            faces: vec![top],
            force_n: [0.0, 0.0, -100.0],
            spread: Spread::Even,
        }],
        material: aluminium(Some(276.0)),
        goal: Goal {
            min_safety_factor: 2.0,
        },
        mesh: MeshControl {
            size_mm: Some(5.0),
            ..Default::default()
        },
        accuracy: Default::default(),
    };
    let dir = std::env::temp_dir().join("roshera-sim-cantilever");
    let _ = std::fs::remove_dir_all(&dir);
    let report = run_static(&m, s, &study, &engines, &dir).expect("the study runs");
    // What reached the mesher: the beam's six faces, nothing of the other part.
    for run in ["coarse", "fine"] {
        let step = std::fs::read_to_string(dir.join(run).join("part.step")).unwrap();
        assert_eq!(
            step.matches("ADVANCED_FACE").count(),
            6,
            "{run} mesher input"
        );
    }
    // The report without its field (thousands of vertices) for reading.
    let mut brief = report.clone();
    brief.field = Default::default();
    eprintln!(
        "{}",
        serde_json::to_string_pretty(&brief).unwrap_or_default()
    );
    let expected = 1.0 * 100f64.powi(4) / (8.0 * 68_900.0 * (10.0 * 1000.0 / 12.0));
    let got = report.fine.max_displacement_mm;
    assert!(
        (got - expected).abs() / expected < 0.03,
        "tip deflection {got:.5} mm vs beam formula {expected:.5} mm: {report:#?}"
    );
    let balance = report
        .checks
        .iter()
        .find(|c| c.name == "forces balance")
        .unwrap();
    assert!(balance.passed, "{balance:?}");
    // The clamped corners are singular: the peak may not converge, and then
    // the verdict must say so rather than give a safety factor.
    match &report.verdict {
        Verdict::Holds { .. } => {}
        Verdict::Undecided { reason } => assert!(reason.contains("converge"), "{reason}"),
        Verdict::Fails { .. } => {
            panic!("a 30 MPa beam in 276 MPa aluminium does not fail: {report:#?}")
        }
    }
    assert!(report.engines.mesher.interim);
    // A safety factor on a typical yield says so beside it.
    assert!(
        report.assumptions.iter().any(|a| a.contains("TYPICAL")),
        "{:?}",
        report.assumptions
    );

    // The surface field the viewport draws: a closed skin wound
    // consistently outward -- every edge used once in each direction --
    // on all six faces of the beam, peaking where the report says.
    let f = &report.field;
    assert!(!f.triangles.is_empty());
    assert_eq!(f.faces.len(), f.triangles.len());
    assert_eq!(f.positions.len(), f.von_mises.len());
    assert_eq!(f.positions.len(), f.displacement.len());
    let mut directed = std::collections::HashMap::<(u32, u32), u32>::new();
    for t in &f.triangles {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            *directed.entry((a, b)).or_default() += 1;
        }
    }
    for (&(a, b), &n) in &directed {
        assert_eq!(n, 1, "edge {a}->{b} used {n} times in one direction");
        assert!(
            directed.contains_key(&(b, a)),
            "edge {a}->{b} has no reverse: the skin is open or wound inconsistently"
        );
    }
    let mut faces = f.faces.clone();
    faces.sort_unstable();
    faces.dedup();
    assert_eq!(faces.len(), 6, "every face of the beam carries the field");
    let peak = report.fine.peak_von_mises_mpa as f32;
    assert!(
        (f.max_von_mises - peak).abs() <= 1e-3 * peak,
        "surface peak {} vs report peak {peak}",
        f.max_von_mises
    );
    // Outward, not merely consistent: the skin encloses positive volume.
    let volume: f64 = f
        .triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| f.positions[i as usize].map(f64::from));
            (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]))
                / 6.0
        })
        .sum();
    assert!(
        (volume - 100.0 * 10.0 * 10.0).abs() < 1.0,
        "the skin encloses {volume} mm^3, the beam is 10000"
    );
    // The mesh stage shows the mesh the run solves: same size rule, same
    // mesher call, so the same counts on both meshes.
    let (preview, wire) = sim_engine::preview::mesh_preview(
        &m,
        s,
        &study.mesh,
        &engines,
        &std::env::temp_dir().join("roshera-sim-cantilever-preview"),
    )
    .expect("the mesh stage runs");
    assert_eq!(preview.coarse.nodes, report.coarse.mesh.nodes);
    assert_eq!(preview.fine.nodes, report.fine.mesh.nodes);
    assert_eq!(preview.fine.elements, report.fine.mesh.elements);
    assert!(!wire.edges.is_empty());
    assert!(wire
        .edges
        .iter()
        .all(|e| (e[0] as usize) < wire.positions.len() && (e[1] as usize) < wire.positions.len()));
}

/// Sliders hold only along their normal: a cube on rollers on three faces
/// (x = min, y = min, z = min) under an even push on top is in uniaxial
/// compression, exactly, for any mesh -- stress p everywhere, the top drops
/// p L / E, and the free sides bulge out nu p L / E. A fixed base would
/// stop the bulge at the bottom; the rollers must let it slide.
///
/// Needs Gmsh and CalculiX: run with `--ignored`.
#[test]
#[ignore = "needs Gmsh and CalculiX; run with --ignored"]
fn a_cube_on_rollers_compresses_exactly() {
    let engines = Engines::discover().expect("Gmsh and CalculiX installed");
    let mut m = BRepModel::new();
    let s = match TopologyBuilder::new(&mut m).create_box_3d(20.0, 20.0, 20.0) {
        Ok(GeometryId::Solid(s)) => s,
        other => panic!("box: {other:?}"),
    };
    let roller = |axis| Support {
        kind: SupportKind::Slider,
        faces: vec![face_at(&m, s, axis, -1.0)],
    };
    let (e, nu, force, side) = (68_900.0, 0.33, 100.0, 20.0);
    let study = Study {
        supports: vec![roller(0), roller(1), roller(2)],
        loads: vec![Load {
            faces: vec![face_at(&m, s, 2, 1.0)],
            force_n: [0.0, 0.0, -force],
            spread: Spread::Even,
        }],
        material: aluminium(None),
        goal: Goal {
            min_safety_factor: 2.0,
        },
        mesh: MeshControl {
            size_mm: Some(5.0),
            ..Default::default()
        },
        accuracy: Default::default(),
    };
    let dir = std::env::temp_dir().join("roshera-sim-rollers");
    let _ = std::fs::remove_dir_all(&dir);
    let report = run_static(&m, s, &study, &engines, &dir).expect("a study on sliders runs");
    let p = force / (side * side);
    let drop = p * side / e;
    let bulge = nu * drop;
    let corner = (drop * drop + 2.0 * bulge * bulge).sqrt();
    let got = report.fine.max_displacement_mm;
    assert!(
        (got - corner).abs() / corner < 0.01,
        "far corner moves {got:.4e} mm, exact {corner:.4e}"
    );
    let peak = report.fine.peak_von_mises_mpa;
    assert!(
        (peak - p).abs() / p < 0.01,
        "stress {peak:.5} MPa everywhere, exact {p}"
    );
    let f = &report.field;
    let max_ux = f
        .displacement
        .iter()
        .map(|d| d[0] as f64)
        .fold(f64::MIN, f64::max);
    assert!(
        (max_ux - bulge).abs() / bulge < 0.02,
        "sides bulge {max_ux:.4e} mm, exact {bulge:.4e}"
    );
    let balance = report
        .checks
        .iter()
        .find(|c| c.name == "forces balance")
        .unwrap();
    assert!(balance.passed, "{balance:?}");
}

/// A curved wall is meshed ON the true surface: every boundary node of a
/// quadratic mesh of a cylinder -- corner and mid-side alike -- lies at the
/// cylinder's radius (a mid-side node left on the chord would sit inside it,
/// making a curved part stiffer and its stresses wrong).
///
/// Needs Gmsh: run with `--ignored`.
#[test]
#[ignore = "needs Gmsh; run with --ignored"]
fn a_curved_wall_is_meshed_on_the_true_surface() {
    use geometry_engine::math::{Point3, Vector3};
    let engines = Engines::discover().expect("Gmsh installed");
    let mut m = BRepModel::new();
    let (r, h) = (10.0, 20.0);
    let s = match TopologyBuilder::new(&mut m).create_cylinder_3d(
        Point3::new(0.0, 0.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
        r,
        h,
    ) {
        Ok(GeometryId::Solid(s)) => s,
        other => panic!("cylinder: {other:?}"),
    };
    let dir = std::env::temp_dir().join("roshera-sim-cylinder");
    let _ = std::fs::remove_dir_all(&dir);
    let (mesh, _) = sim_engine::mesh::mesh_solid(
        &m,
        s,
        3.0,
        &engines.mesher,
        &dir,
        std::time::Duration::from_secs(120),
    )
    .expect("the cylinder meshes");
    let mut on_wall = 0usize;
    let mut worst = 0.0_f64;
    for b in &mesh.boundary {
        for n in b.nodes {
            let p = mesh.nodes[&n];
            // Wall nodes: strictly between the caps.
            if p[2] > 1e-6 && p[2] < h - 1e-6 {
                on_wall += 1;
                worst = worst.max(((p[0] * p[0] + p[1] * p[1]).sqrt() - r).abs());
            }
        }
    }
    assert!(on_wall > 50, "the wall carries nodes: {on_wall}");
    assert!(
        worst < 1e-6,
        "a wall node lies {worst:.3e} mm off the radius"
    );
}

/// The filleted L-bracket of 2026-11-24: a 60 x 40 x 6 base and a 6 x 40 x 50
/// upright joined, the inside corner rounded R5. Returns the model, the
/// solid, the base's bottom, the upright's top and the fillet face.
fn filleted_l() -> (BRepModel, u32, u32, u32, u32) {
    use geometry_engine::math::{Matrix4, Vector3};
    use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use geometry_engine::operations::fillet::{fillet_edges, FilletOptions, FilletType};
    use geometry_engine::operations::transform::{transform_solid, TransformOptions};
    let mut m = BRepModel::new();
    let mut place = |m: &mut BRepModel, w: f64, d: f64, h: f64, at: [f64; 3]| -> u32 {
        let s = match TopologyBuilder::new(m).create_box_3d(w, d, h) {
            Ok(GeometryId::Solid(s)) => s,
            other => panic!("box: {other:?}"),
        };
        let lo = [
            face_at(m, s, 0, -1.0),
            face_at(m, s, 1, -1.0),
            face_at(m, s, 2, -1.0),
        ];
        let _ = lo;
        let mesh = tessellate_solid(m.solids.get(s).unwrap(), m, &TessellationParams::default());
        let mut c = [0.0; 3];
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for v in &mesh.vertices {
            let p = [v.position.x, v.position.y, v.position.z];
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
        for i in 0..3 {
            c[i] = (lo[i] + hi[i]) / 2.0;
        }
        transform_solid(
            m,
            s,
            Matrix4::from_translation(&Vector3::new(at[0] - c[0], at[1] - c[1], at[2] - c[2])),
            TransformOptions::default(),
        )
        .unwrap();
        s
    };
    let base = place(&mut m, 60.0, 40.0, 6.0, [0.0, 0.0, 3.0]);
    let upright = place(&mut m, 6.0, 40.0, 50.0, [-27.0, 0.0, 25.0]);
    let l = boolean_operation(
        &mut m,
        base,
        upright,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .unwrap();
    // The inside corner: the edge shared by the base's top and the upright's
    // inner face (each the face whose triangles sit at that plane).
    let top = face_at_plane(&m, l, 2, 6.0);
    let inner = face_at_plane(&m, l, 0, -24.0);
    let edges = |f: u32| -> Vec<u32> {
        let face = m.faces.get(f).unwrap();
        m.loops.get(face.outer_loop).unwrap().edges.clone()
    };
    let corner: Vec<u32> = edges(top)
        .into_iter()
        .filter(|e| edges(inner).contains(e))
        .collect();
    assert_eq!(corner.len(), 1, "one inside corner");
    let blend = fillet_edges(
        &mut m,
        l,
        corner,
        FilletOptions {
            fillet_type: FilletType::Constant(5.0),
            radius: 5.0,
            ..Default::default()
        },
    )
    .unwrap();
    let bottom = face_at(&m, l, 2, -1.0);
    let upright_top = face_at(&m, l, 2, 1.0);
    let _ = m.certify_solid(l);
    (m, l, bottom, upright_top, blend[0])
}

/// The planar face of `solid` whose triangles all lie at `value` on `axis`.
fn face_at_plane(m: &BRepModel, solid: u32, axis: usize, value: f64) -> u32 {
    let mesh = tessellate_solid(
        m.solids.get(solid).unwrap(),
        m,
        &TessellationParams::default(),
    );
    let mut on: std::collections::BTreeMap<u32, bool> = std::collections::BTreeMap::new();
    for (t, &f) in mesh.triangles.iter().zip(&mesh.face_map) {
        let all = t.iter().all(|&i| {
            let p = mesh.vertices[i as usize].position;
            ([p.x, p.y, p.z][axis] - value).abs() < 1e-6
        });
        let e = on.entry(f).or_insert(true);
        *e = *e && all;
    }
    *on.iter().find(|(_, &v)| v).expect("a face on that plane").0
}

/// Refinement at the peak: on the filleted bracket the default meshes leave
/// the fillet's peak unsettled (37 -> 40 MPa, 2026-11-24); refining around
/// the peak must settle it -- a real stress concentration converges -- and
/// with refinement off the run must say the peak did not settle.
///
/// Needs Gmsh and CalculiX: run with `--ignored`.
#[test]
#[ignore = "needs Gmsh and CalculiX; run with --ignored"]
fn a_fillet_peak_settles_when_refined_at_the_peak() {
    let engines = Engines::discover().expect("Gmsh and CalculiX installed");
    let (m, s, bottom, top, _) = filleted_l();
    let study = |refine: bool| Study {
        supports: vec![Support {
            kind: SupportKind::Fixed,
            faces: vec![bottom],
        }],
        loads: vec![Load {
            faces: vec![top],
            force_n: [200.0, 0.0, 0.0],
            spread: Spread::Even,
        }],
        material: aluminium(Some(276.0)),
        goal: Goal {
            min_safety_factor: 2.0,
        },
        mesh: MeshControl {
            refine_at_peak: Some(refine),
            ..Default::default()
        },
        accuracy: Default::default(),
    };
    let dir = std::env::temp_dir().join("roshera-sim-fillet");
    let _ = std::fs::remove_dir_all(&dir);
    let off = run_static(&m, s, &study(false), &engines, &dir.join("off")).expect("runs");
    let peak_check = |r: &sim_engine::run::StaticReport| {
        r.checks
            .iter()
            .find(|c| c.name == "peak stress converged")
            .cloned()
            .unwrap()
    };
    assert!(off.refined.is_empty());
    assert!(
        !peak_check(&off).passed,
        "unrefined, the fillet peak does not settle: {:?}",
        peak_check(&off)
    );
    let on = run_static(&m, s, &study(true), &engines, &dir.join("on")).expect("runs");
    assert!(!on.refined.is_empty(), "the run refined at the peak");
    let check = peak_check(&on);
    eprintln!(
        "refined: {:?}",
        on.refined
            .iter()
            .map(|r| (r.mesh.nodes, r.peak_von_mises_mpa))
            .collect::<Vec<_>>()
    );
    eprintln!("{check:?}");
    assert!(
        check.passed,
        "refined at the peak, the fillet's stress settles: {check:?}"
    );
    assert!(
        matches!(on.verdict, Verdict::Holds { .. }),
        "{:?}",
        on.verdict
    );
}

/// Finer here: a local size on the fillet face makes a finer mesh there --
/// more nodes than the default -- and the element-shape figures are real.
///
/// Needs Gmsh: run with `--ignored`.
#[test]
#[ignore = "needs Gmsh; run with --ignored"]
fn a_local_size_makes_the_mesh_finer_there_and_shape_is_reported() {
    let engines = Engines::discover().expect("Gmsh installed");
    let (m, s, _, _, fillet) = filleted_l();
    let dir = std::env::temp_dir().join("roshera-sim-local");
    let _ = std::fs::remove_dir_all(&dir);
    let (plain, _) = sim_engine::preview::mesh_preview(
        &m,
        s,
        &MeshControl::default(),
        &engines,
        &dir.join("plain"),
    )
    .unwrap();
    let finer = MeshControl {
        local: vec![sim_engine::study::LocalSize {
            faces: vec![fillet],
            size_mm: 0.6,
        }],
        ..Default::default()
    };
    let (local, _) =
        sim_engine::preview::mesh_preview(&m, s, &finer, &engines, &dir.join("local")).unwrap();
    assert!(
        local.fine.nodes > plain.fine.nodes * 11 / 10,
        "finer near the fillet: {} vs {} nodes",
        local.fine.nodes,
        plain.fine.nodes
    );
    for st in [&plain.fine, &local.fine] {
        assert!(st.quality_min > 0.0 && st.quality_min <= 1.0, "{st:?}");
        assert!(st.quality_mean > 0.5 && st.quality_mean <= 1.0, "{st:?}");
    }
}
