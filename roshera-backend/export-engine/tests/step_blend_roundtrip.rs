//! STEP round-trip GATE for BLENDED parts (fillet TORUS + chamfer CONE).
//!
//! Audit 2026-07-14 (HIGH interop): a plain prism / bored block round-trips
//! fine, but a part carrying a fillet **torus** or a chamfer **cone** loses
//! closure exactly at the blend surface on re-import — the re-imported solid
//! is watertight=false / oriented=false / manifold=false, with an open ring at
//! the torus boundary and an open arc at the cone.
//!
//! These gates build the minimal repro of that topology in the kernel, export
//! it through the export-engine STEP writer, re-import it, and assert the
//! FULL certificate (B-Rep valid + mesh watertight + closed + 2-manifold +
//! consistently oriented) survives the round-trip.

use std::f64::consts::TAU;

use export_engine::ExportEngine;
use geometry_engine::harness::watertight::{is_watertight, manifold_report};
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::chamfer::{chamfer_edges, ChamferOptions, ChamferType};
use geometry_engine::operations::fillet::{FilletOptions, FilletType, PropagationMode};
use geometry_engine::operations::fillet_edges;
use geometry_engine::operations::revolve::{revolve_profile, RevolveOptions};
use geometry_engine::primitives::curve::{Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::SurfaceType;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};
use tempfile::TempDir;

/// Revolve a closed (r, z) profile a full turn about +Z.
fn revolve_tube(m: &mut BRepModel, pts: &[(f64, f64)]) -> SolidId {
    let verts: Vec<_> = pts
        .iter()
        .map(|(r, z)| m.vertices.add(*r, 0.0, *z))
        .collect();
    let mut edges = Vec::new();
    for i in 0..pts.len() {
        let j = (i + 1) % pts.len();
        let line = Line::new(
            Point3::new(pts[i].0, 0.0, pts[i].1),
            Point3::new(pts[j].0, 0.0, pts[j].1),
        );
        let cid = m.curves.add(Box::new(line));
        edges.push(m.edges.add(Edge::new(
            0,
            verts[i],
            verts[j],
            cid,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )));
    }
    let opts = RevolveOptions {
        axis_origin: Point3::ZERO,
        axis_direction: Vector3::Z,
        angle: TAU,
        segments: 64,
        ..Default::default()
    };
    revolve_profile(m, edges, opts).expect("tube revolve")
}

/// Closed rim edge whose seam vertex sits at radius `r_want`, height `z_want`.
fn rim_at(m: &BRepModel, r_want: f64, z_want: f64) -> Option<EdgeId> {
    m.edges.iter().find_map(|(id, e)| {
        if !e.is_loop() {
            return None;
        }
        let p = m.vertices.get_position(e.start_vertex)?;
        let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
        if (r - r_want).abs() < 0.5 && (p[2] - z_want).abs() < 0.5 {
            Some(id)
        } else {
            None
        }
    })
}

fn count_surface(m: &BRepModel, s: SolidId, want: SurfaceType) -> usize {
    let solid = m.solids.get(s).expect("solid");
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    let mut n = 0;
    for sh in shells {
        if let Some(shell) = m.shells.get(sh) {
            for &fid in &shell.faces {
                if let Some(f) = m.faces.get(fid) {
                    if let Some(surf) = m.surfaces.get(f.surface_id) {
                        if surf.surface_type() == want {
                            n += 1;
                        }
                    }
                }
            }
        }
    }
    n
}

/// Export `model` to STEP, re-import it, and return the imported model + text.
async fn export_then_import(model: &BRepModel, name: &str) -> (String, BRepModel) {
    let temp = TempDir::new().expect("tmp");
    let engine = ExportEngine::with_output_directory(temp.path().to_string_lossy().to_string());
    let filename = engine.export_step(model, name).await.expect("export");
    let path = temp.path().join(&filename);
    let text = std::fs::read_to_string(&path).expect("read step");
    let (imported, _report) = export_engine::formats::step::import_step_to_brep_with_report(&path)
        .await
        .expect("import");
    (text, imported)
}

/// Full-certificate assertion on the FIRST re-imported solid.
fn assert_reimport_sound(imported: &mut BRepModel, what: &str) {
    assert_eq!(
        imported.solids.len(),
        1,
        "[{what}] blended part must re-import as exactly one solid"
    );
    let sid = imported
        .solids
        .iter()
        .next()
        .map(|(id, _)| id)
        .expect("imported solid id");

    let v = validate_solid_scoped(
        imported,
        sid,
        Tolerance::default(),
        ValidationLevel::Standard,
    );
    assert!(
        v.is_valid,
        "[{what}] re-imported solid must be B-Rep valid, got {} errors: {:?}",
        v.errors.len(),
        v.errors
            .iter()
            .take(5)
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
    );

    let report = manifold_report(imported, sid, 0.1, 1e-6)
        .unwrap_or_else(|| panic!("[{what}] re-imported solid did not tessellate"));
    assert_eq!(
        report.boundary_edges, 0,
        "[{what}] re-import not watertight: {} open edges",
        report.boundary_edges
    );
    assert_eq!(
        report.nonmanifold_edges, 0,
        "[{what}] re-import not 2-manifold: {} non-manifold edges",
        report.nonmanifold_edges
    );
    assert!(
        report.oriented,
        "[{what}] re-import not consistently oriented: {} inconsistent directed edges",
        report.inconsistent_directed_edges
    );
    assert!(
        is_watertight(imported, sid, 0.1, 1e-3),
        "[{what}] re-import fails volume-agreement watertightness"
    );
}

/// Tube: outer wall R10, bore R6, z 0..20.
const TUBE: &[(f64, f64)] = &[(10.0, 0.0), (10.0, 20.0), (6.0, 20.0), (6.0, 0.0)];

#[tokio::test]
async fn roundtrip_fillet_torus_bore_rim() {
    let mut m = BRepModel::new();
    let s = revolve_tube(&mut m, TUBE);
    let rim = rim_at(&m, 6.0, 20.0).expect("bore-top rim is a closed edge");
    fillet_edges(
        &mut m,
        s,
        vec![rim],
        FilletOptions {
            fillet_type: FilletType::Constant(1.0),
            radius: 1.0,
            propagation: PropagationMode::None,
            ..Default::default()
        },
    )
    .expect("bore rim fillet");
    assert_eq!(
        count_surface(&m, s, SurfaceType::Torus),
        1,
        "source must carry one torus blend face"
    );
    // Source must be sound before we test its round-trip.
    assert!(
        validate_solid_scoped(&m, s, Tolerance::default(), ValidationLevel::Standard).is_valid,
        "source filleted tube must be valid"
    );

    let (_text, mut imported) = export_then_import(&m, "rt_fillet_torus").await;
    assert_reimport_sound(&mut imported, "fillet-torus bore rim");
}

#[tokio::test]
async fn roundtrip_chamfer_cone_outer_rim() {
    let mut m = BRepModel::new();
    let s = revolve_tube(&mut m, TUBE);
    let rim = rim_at(&m, 10.0, 20.0).expect("outer-top rim is a closed edge");
    chamfer_edges(
        &mut m,
        s,
        vec![rim],
        ChamferOptions {
            chamfer_type: ChamferType::EqualDistance(1.0),
            distance1: 1.0,
            distance2: 1.0,
            ..Default::default()
        },
    )
    .expect("outer rim chamfer");
    assert_eq!(
        count_surface(&m, s, SurfaceType::Cone),
        1,
        "source must carry one cone chamfer face"
    );
    assert!(
        validate_solid_scoped(&m, s, Tolerance::default(), ValidationLevel::Standard).is_valid,
        "source chamfered tube must be valid"
    );

    let (_text, mut imported) = export_then_import(&m, "rt_chamfer_cone").await;
    assert_reimport_sound(&mut imported, "chamfer-cone outer rim");
}

/// The audit's SECOND failing case class, which the tube fixture above does NOT
/// reproduce: a SMALL chamfer cone on a DRILLED-hole rim. A block with an r2.5
/// through-bore (boolean Difference) whose two rims are chamfered d=0.4 makes
/// two small cone frusta whose re-import previously tessellated with a 4-open-
/// edge quad hole at each cone's u=0 seam (8 boundary edges total; the audit
/// block with 4 bores showed 4×8 = 32). The large-cone tube case passed because
/// its fall-through mesher happened to close there; this pins the small-frustum
/// class.
#[tokio::test]
async fn roundtrip_chamfer_small_cones_on_drilled_rims() {
    use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use geometry_engine::operations::transform::{translate, TransformOptions};
    use geometry_engine::primitives::topology_builder::{GeometryId, TopologyBuilder};

    let mut m = BRepModel::new();
    let block = match TopologyBuilder::new(&mut m)
        .create_box_3d(20.0, 20.0, 10.0)
        .expect("box")
    {
        GeometryId::Solid(s) => s,
        o => panic!("unexpected geometry {o:?}"),
    };
    // create_box_3d is centred on the origin (z ∈ [-5, 5]); lift to z ∈ [0, 10].
    translate(
        &mut m,
        vec![block],
        Vector3::Z,
        5.0,
        TransformOptions::default(),
    )
    .expect("lift block");
    let hole = match TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -1.0), Vector3::Z, 2.5, 12.0)
        .expect("cyl")
    {
        GeometryId::Solid(s) => s,
        o => panic!("unexpected geometry {o:?}"),
    };
    let s = boolean_operation(
        &mut m,
        block,
        hole,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("through-bore difference");

    // Chamfer BOTH bore rims (the boolean over-splits each rim circle into
    // arcs; chamfer_edges coalesces them back to the canonical closed rims).
    let rim_arcs = bore_rim_arc_edges(&m, s, 2.5);
    assert!(
        !rim_arcs.is_empty(),
        "expected the bore-rim arcs of both rims"
    );
    chamfer_edges(
        &mut m,
        s,
        rim_arcs,
        ChamferOptions {
            chamfer_type: ChamferType::EqualDistance(0.4),
            distance1: 0.4,
            distance2: 0.4,
            ..Default::default()
        },
    )
    .expect("both drilled-rim chamfers");
    assert_eq!(
        count_surface(&m, s, SurfaceType::Cone),
        2,
        "source must carry one cone frustum per chamfered rim"
    );
    assert!(
        validate_solid_scoped(&m, s, Tolerance::default(), ValidationLevel::Standard).is_valid,
        "source chamfered bored block must be valid"
    );

    let (_text, mut imported) = export_then_import(&m, "rt_small_cones").await;
    assert_reimport_sound(&mut imported, "small chamfer cones on drilled rims");
}

/// The over-split rim ARCS of both bore rims (radius ≈ `r_want`, both endpoints
/// at the same height — i.e. horizontal), mirroring the live drilled-hole path.
fn bore_rim_arc_edges(m: &BRepModel, s: SolidId, r_want: f64) -> Vec<EdgeId> {
    let solid = m.solids.get(s).expect("solid");
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    let pos = |vid| -> Option<[f64; 3]> { m.vertices.get_position(vid) };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for sh in shells {
        let Some(shell) = m.shells.get(sh) else {
            continue;
        };
        for &fid in &shell.faces {
            let Some(face) = m.faces.get(fid) else {
                continue;
            };
            let mut loops = vec![face.outer_loop];
            loops.extend_from_slice(&face.inner_loops);
            for lid in loops {
                let Some(lp) = m.loops.get(lid) else { continue };
                for &eid in &lp.edges {
                    let Some(e) = m.edges.get(eid) else { continue };
                    let (Some(a), Some(b)) = (pos(e.start_vertex), pos(e.end_vertex)) else {
                        continue;
                    };
                    let ra = (a[0] * a[0] + a[1] * a[1]).sqrt();
                    let rb = (b[0] * b[0] + b[1] * b[1]).sqrt();
                    let horizontal = (a[2] - b[2]).abs() < 1e-6;
                    if (ra - r_want).abs() < 0.25
                        && (rb - r_want).abs() < 0.25
                        && horizontal
                        && seen.insert(eid)
                    {
                        out.push(eid);
                    }
                }
            }
        }
    }
    out
}
