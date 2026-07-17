// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Roadmap #5 — mixed-CONVEXITY (Gregory) corner characterization.
//!
//! Where three degree-3 corners of a single convexity are handled today
//! (`apply_apex_sphere_corner`: convex apex sphere + concave re-entrant apex,
//! Task #82), and mixed-KIND (fillet+chamfer, same convexity) corners have a
//! single-patch cap with a typed `G1NotAchievable` refusal
//! (`mixed_kind_corner_cap`), the remaining open case is the mixed-CONVEXITY
//! corner: a vertex where a CONVEX blend edge and a CONCAVE blend edge meet.
//! No single sphere/plane is tangent to both a convex and a concave edge, so
//! neither apex synthesizer applies; this is the N-sided Gregory / GB-patch
//! case (Charrot-Gregory 1984; Vaitkus-Várady-Salvi GB-patches, CAGD 2018).
//!
//! CONTRACT (what "cannot lie" requires here): the kernel must refuse this
//! corner with a *typed* error — never a panic and never a silently-wrong
//! (non-watertight / self-intersecting) solid. This suite pins that contract
//! end-to-end through `fillet_edges` on real machined topology, and records
//! the exact refusal so a future Gregory implementation flips it to a build.

use geometry_engine::math::Vector3;
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::fillet::{fillet_edges, FilletOptions, FilletType};
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::operations::OperationError;
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::vertex::VertexId;

/// A 40³ box with a 20³ notch removed from the (+,+,+) octant. The notch's
/// three outer corners at (20,0,0)/(0,20,0)/(0,0,20) are MIXED-convexity
/// vertices: the concave notch-wall edge meets the convex outer-box edges.
fn notched_box(m: &mut BRepModel) -> SolidId {
    let base = match TopologyBuilder::new(m)
        .create_box_3d(40.0, 40.0, 40.0)
        .expect("base")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid geometry for base box, got {o:?}"),
    };
    let tool = match TopologyBuilder::new(m)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("tool")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid geometry for tool box, got {o:?}"),
    };
    translate(m, vec![tool], Vector3::X, 10.0, TransformOptions::default()).expect("tx");
    translate(m, vec![tool], Vector3::Y, 10.0, TransformOptions::default()).expect("ty");
    translate(m, vec![tool], Vector3::Z, 10.0, TransformOptions::default()).expect("tz");
    boolean_operation(
        m,
        base,
        tool,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("notch")
}

fn all_edges(model: &BRepModel, solid: SolidId) -> Vec<EdgeId> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let Some(s) = model.solids.get(solid) else {
        return out;
    };
    let mut shells = vec![s.outer_shell];
    shells.extend_from_slice(&s.inner_shells);
    for sh in shells {
        let Some(shell) = model.shells.get(sh) else {
            continue;
        };
        for &fid in &shell.faces {
            let Some(face) = model.faces.get(fid) else {
                continue;
            };
            for lid in face.all_loops() {
                if let Some(lp) = model.loops.get(lid) {
                    for &e in &lp.edges {
                        if seen.insert(e) {
                            out.push(e);
                        }
                    }
                }
            }
        }
    }
    out
}

fn vertex_at(model: &BRepModel, x: f64, y: f64, z: f64) -> VertexId {
    model
        .vertices
        .iter()
        .find(|(_, v)| {
            (v.position[0] - x).abs() < 1e-6
                && (v.position[1] - y).abs() < 1e-6
                && (v.position[2] - z).abs() < 1e-6
        })
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("vertex at ({x},{y},{z}) not found"))
}

fn edges_at_vertex(model: &BRepModel, solid: SolidId, vid: VertexId) -> Vec<EdgeId> {
    let mut out: Vec<EdgeId> = all_edges(model, solid)
        .into_iter()
        .filter(|&e| {
            model
                .edges
                .get(e)
                .map(|ed| ed.start_vertex == vid || ed.end_vertex == vid)
                .unwrap_or(false)
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn fillet_opts(r: f64) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(r),
        radius: r,
        ..Default::default()
    }
}

/// CHARACTERIZATION / CONTRACT: filleting every edge incident to a
/// mixed-convexity corner must refuse with a *typed* error and leave the model
/// unbuilt for that corner — not panic, not return a broken solid.
#[test]
fn mixed_convexity_corner_refuses_typed_not_a_lie() {
    let mut model = BRepModel::new();
    let solid = notched_box(&mut model);

    // The (20,0,0) notch corner: a concave notch-wall edge meets the convex
    // outer-box edges — a mixed-convexity (Gregory) vertex.
    let corner = vertex_at(&model, 20.0, 0.0, 0.0);
    let corner_edges = edges_at_vertex(&model, solid, corner);
    assert!(
        corner_edges.len() >= 3,
        "mixed corner (20,0,0) should have >=3 incident edges; got {}",
        corner_edges.len()
    );

    let result = fillet_edges(&mut model, solid, corner_edges.clone(), fillet_opts(3.0));

    match result {
        Ok(faces) => panic!(
            "mixed-convexity corner unexpectedly BUILT ({} faces) — if this is \
             genuinely sound it must be watertight/G1-verified; if not it is a \
             silent lie. Investigate before flipping this contract.",
            faces.len()
        ),
        Err(e) => {
            // The refusal must be a typed, honest one.
            let typed = matches!(
                e,
                OperationError::NotImplemented(_)
                    | OperationError::InvalidGeometry(_)
                    | OperationError::BlendFailed(_)
            );
            eprintln!("mixed_convexity_corner refusal: {e:?}");
            assert!(
                typed,
                "mixed-convexity corner must refuse with a typed error \
                 (NotImplemented / InvalidGeometry / BlendFailed); got {e:?}"
            );
            // The refusal must be ACCURATE, not merely typed: it must name the
            // mixed-convexity Gregory remainder and must NOT misdirect the
            // caller to the mixed-KIND `partial_corner_vertices` workaround,
            // which does not apply to a mixed-convexity corner.
            let msg = format!("{e:?}");
            assert!(
                msg.contains("MIXED convexity") && msg.contains("Gregory"),
                "mixed-convexity refusal must name the Gregory/GB-patch \
                 remainder; got: {msg}"
            );
            assert!(
                !msg.contains("pass `partial_corner_vertices"),
                "mixed-convexity refusal must NOT advise the mixed-KIND \
                 partial_corner_vertices route (inapplicable here); got: {msg}"
            );
        }
    }
}
