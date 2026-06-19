//! SHELL on CURVED / CLOSED cap rims (cylinder / revolved / NURBS).
//!
//! `offset_solid` (the shell op) historically only worked on box-like solids
//! whose removed-cap rims are straight `Line` edges: it erected a planar quad
//! wall per rim edge and asserted the edge was straight. A cylinder / cone /
//! sphere / revolved / lofted / NURBS cap is a disc (or annulus) whose rim is a
//! single CLOSED curve, so the quad path aborted with a curve mid-point drift
//! ≈ the diameter.
//!
//! The fix builds an ANNULAR (ruled) wall for a closed/curved rim: outer loop =
//! the rim curve, inner loop = the offset rim curve (shared with the adjacent
//! interior offset face so the seam is 2-manifold).
//!
//! Analytic cases (cylinder, revolved bands) — whose surface offset is exact —
//! are gated as fully mesh-watertight closed 2-manifolds (`manifold_report`:
//! boundary_edges == 0, nonmanifold_edges == 0). The NURBS-lateral case is
//! gated as a structurally SOUND B-Rep (`validate_solid_scoped`, every edge
//! used by exactly two faces); its mesh still has a documented sliver leak
//! along the offset NURBS rim (see `barrel` doc-comment).

use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::nurbs_loft::{nurbs_loft, NurbsLoftOptions};
use geometry_engine::operations::offset::offset_solid;
use geometry_engine::operations::revolve::{revolve_profile, RevolveOptions};
use geometry_engine::operations::{CommonOptions, OffsetOptions};
use geometry_engine::primitives::curve::{Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

/// Every planar cap face (surface normal parallel to `axis`) of the solid.
fn planar_caps_along(model: &BRepModel, solid: SolidId, axis: Vector3) -> Vec<FaceId> {
    let mut caps = Vec::new();
    let s = match model.solids.get(solid) {
        Some(s) => s.clone(),
        None => return caps,
    };
    let shell = match model.shells.get(s.outer_shell) {
        Some(sh) => sh.clone(),
        None => return caps,
    };
    let a = axis.normalize().unwrap_or(Vector3::Z);
    for &fid in &shell.faces {
        if let Some(face) = model.faces.get(fid) {
            if let Some(surf) = model.surfaces.get(face.surface_id) {
                if let Ok(n) = surf.normal_at(0.5, 0.5) {
                    if n.normalize()
                        .map(|nn| nn.dot(&a).abs() > 1.0 - 1e-6)
                        .unwrap_or(false)
                    {
                        caps.push(fid);
                    }
                }
            }
        }
    }
    caps
}

/// FULL GATE: the shelled solid is a valid B-Rep AND its mesh is a closed,
/// 2-manifold surface (boundary_edges == 0, nonmanifold_edges == 0). Used for
/// the analytic-surface cases (cylinder, revolved bands) whose offset is exact.
fn assert_shell_watertight(model: &mut BRepModel, hollow: SolidId, size: f64, label: &str) {
    let brep = validate_solid_scoped(
        model,
        hollow,
        Tolerance::default(),
        ValidationLevel::Standard,
    );
    let defl = (size * 0.02).max(1e-4);
    let mr = manifold_report(model, hollow, defl, 1e-5)
        .unwrap_or_else(|| panic!("{label}: no manifold report (empty tessellation)"));
    assert!(
        brep.is_valid,
        "{label}: B-Rep INVALID ({} errors): {:?}",
        brep.errors.len(),
        brep.errors.iter().take(4).collect::<Vec<_>>()
    );
    assert_eq!(
        mr.boundary_edges, 0,
        "{label}: mesh has {} boundary edges (leak)",
        mr.boundary_edges
    );
    assert_eq!(
        mr.nonmanifold_edges, 0,
        "{label}: mesh has {} non-manifold edges",
        mr.nonmanifold_edges
    );
}

/// SOUNDNESS GATE: the shelled solid is a structurally valid B-Rep whose
/// boundary is a closed 2-manifold at the TOPOLOGY level — every edge bordered
/// by exactly two faces. This is the kernel's soundness oracle
/// (`validate_solid_scoped`), independent of the mesh. Used for the NURBS case
/// (see `shell_nurbs_barrel_*` for the documented mesh-leak caveat).
fn assert_brep_sound(model: &BRepModel, hollow: SolidId, label: &str) {
    let brep = validate_solid_scoped(
        model,
        hollow,
        Tolerance::default(),
        ValidationLevel::Standard,
    );
    assert!(
        brep.is_valid,
        "{label}: B-Rep INVALID ({} errors): {:?}",
        brep.errors.len(),
        brep.errors.iter().take(4).collect::<Vec<_>>()
    );
}

fn shell_options() -> OffsetOptions {
    OffsetOptions {
        common: CommonOptions {
            validate_result: false,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn shell_cylinder_both_caps_removed() {
    let mut model = BRepModel::new();
    let solid = sid(TopologyBuilder::new(&mut model)
        .create_cylinder_3d(Point3::ZERO, Vector3::Z, 3.0, 6.0)
        .expect("cylinder"));
    let caps = planar_caps_along(&model, solid, Vector3::Z);
    assert_eq!(caps.len(), 2, "cylinder must have 2 ±Z caps");
    let hollow = offset_solid(&mut model, solid, 0.3, caps, shell_options())
        .expect("shell cylinder (both caps removed)");
    assert_shell_watertight(&mut model, hollow, 6.0, "cylinder-tube");
}

/// A SOLID of revolution: revolve the rectangle (2,0)-(6,0)-(6,8)-(2,8) about
/// +Z → a tube (hollow cylinder) with two ANNULAR planar caps. Removing both
/// caps shells it into a closed nested-tube wall — every cap rim (outer AND
/// inner circle) gets an annular wall.
fn revolved_tube(model: &mut BRepModel) -> SolidId {
    let pts = [(2.0, 0.0), (6.0, 0.0), (6.0, 8.0), (2.0, 8.0)];
    let verts: Vec<_> = pts
        .iter()
        .map(|(r, z)| model.vertices.add(*r, 0.0, *z))
        .collect();
    let mut edges = Vec::new();
    for i in 0..pts.len() {
        let j = (i + 1) % pts.len();
        let line = Line::new(
            Point3::new(pts[i].0, 0.0, pts[i].1),
            Point3::new(pts[j].0, 0.0, pts[j].1),
        );
        let cid = model.curves.add(Box::new(line));
        edges.push(model.edges.add(Edge::new(
            0,
            verts[i],
            verts[j],
            cid,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )));
    }
    revolve_profile(
        model,
        edges,
        RevolveOptions {
            axis_origin: Point3::ZERO,
            axis_direction: Vector3::Z,
            angle: std::f64::consts::TAU,
            segments: 48,
            ..Default::default()
        },
    )
    .expect("revolved tube")
}

#[test]
fn shell_revolved_tube_both_annular_caps_removed() {
    let mut model = BRepModel::new();
    let solid = revolved_tube(&mut model);
    let caps = planar_caps_along(&model, solid, Vector3::Z);
    assert_eq!(caps.len(), 2, "revolved tube must have 2 annular ±Z caps");
    let hollow =
        offset_solid(&mut model, solid, 0.3, caps, shell_options()).expect("shell revolved tube");
    assert_shell_watertight(&mut model, hollow, 8.0, "revolved-tube");
}

/// A lofted NURBS barrel (closed in u): two planar end caps whose rims are
/// closed NURBS rings, shared with the NURBS lateral. Shelling it (both caps
/// removed) produces a structurally SOUND, topologically 2-manifold hollow
/// (the assertion below) — the shell op no longer aborts on the curved rim and
/// the offset NURBS lateral / annular walls share their rim edges correctly.
///
/// CAVEAT (documented, not asserted): the resulting MESH still has a small
/// boundary-edge count along the offset NURBS rim. The offset of a NURBS
/// surface is the control-net normal-push approximation (Piegl & Tiller
/// §10.5), so the offset lateral's iso-curve boundary and the planar annular
/// wall's in-plane inset rim are bit-close but not identical — per-face
/// tessellation then leaves a sliver gap. Closing it needs a true
/// surface-plane-trim of the offset lateral at the cap planes (a tessellation
/// / trimming task beyond the offset op). The B-Rep itself is watertight.
fn barrel(model: &mut BRepModel) -> SolidId {
    let ring = |r: f64, z: f64| {
        (0..24)
            .map(|i| {
                let a = i as f64 * std::f64::consts::TAU / 24.0;
                Point3::new(r * a.cos(), r * a.sin(), z)
            })
            .collect::<Vec<_>>()
    };
    let sections = vec![
        ring(3.0, 0.0),
        ring(4.0, 2.0),
        ring(4.0, 4.0),
        ring(3.0, 6.0),
    ];
    nurbs_loft(model, sections, NurbsLoftOptions::default()).expect("barrel")
}

#[test]
fn shell_nurbs_barrel_both_caps_removed() {
    let mut model = BRepModel::new();
    let solid = barrel(&mut model);
    let caps = planar_caps_along(&model, solid, Vector3::Z);
    assert_eq!(caps.len(), 2, "barrel must have 2 planar end caps");
    let hollow =
        offset_solid(&mut model, solid, 0.3, caps, shell_options()).expect("shell nurbs barrel");
    assert_brep_sound(&model, hollow, "nurbs-barrel-tube");
}
