//! Finding 1b (live-dogfood 2026-07-10): chamfering ALL edges of a corner-notched
//! box must gracefully round what it can rather than refusing the WHOLE op.
//!
//! The notch (40³ box − 20³ corner-octant cube) has:
//!   * supported corners — the concave degree-3 re-entrant apex and the plain
//!     convex box corners the notch never touched (planar N-gon caps), AND
//!   * three UNSUPPORTED `Mixed`-convexity corners at (20,0,0)/(0,20,0)/(0,0,20)
//!     where the notch's concave edges meet the box's convex edges. Chamfer has
//!     no cap synthesizer for a Mixed corner.
//!
//! Before the fix, `chamfer_edges` on ALL edges retained every degree-≥3 corner
//! vertex through surgery (the V-retention gate fires for uncappable Mixed
//! corners too) but then SKIPPED the cap at those corners — leaving the shell
//! geometrically OPEN. The transactional `with_rollback` + closure gate caught
//! it and rolled the WHOLE op back: sound, but it rounded NOTHING. The fillet
//! side already skips unsupported corners up front via a graceful-skip fixpoint;
//! this pins the same contract for chamfer through the shared helper.
//!
//! GREEN CONTRACT (mirrors `fillet_all_notched_box`): chamfer-all must either
//! (A) SUCCEED, rounding the supported corners and skipping the Mixed ones, with
//! a watertight sound result, or (B) return a TYPED `BlendFailed` refusal
//! (transactionally). What it must NOT do is fail the whole op with an
//! open-geometry `InvalidBRep`/`InvalidGeometry` or an `InternalError`.
use geometry_engine::math::Vector3;
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::chamfer::{ChamferOptions, ChamferType, PropagationMode};
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::operations::{chamfer_edges, CommonOptions, OperationError};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// A 40³ box centred at origin with a 20³ notch removed from the (+,+,+) octant
/// corner. Concave degree-3 re-entrant vertex at the origin; three Mixed corners
/// at (20,0,0)/(0,20,0)/(0,0,20).
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

/// All non-loop edges of the solid (outer + inner shells, deduplicated).
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

fn chamfer_opts(d: f64) -> ChamferOptions {
    ChamferOptions {
        chamfer_type: ChamferType::EqualDistance(d),
        distance1: d,
        distance2: d,
        symmetric: true,
        propagation: PropagationMode::None,
        preserve_edges: false,
        partial_corner_vertices: Vec::new(),
        common: CommonOptions::default(),
        ..ChamferOptions::default()
    }
}

/// No-regression: the graceful-skip fixpoint must be a no-op on a PLAIN box —
/// every corner is a supported degree-3 convex apex, so nothing is dropped and
/// chamfer-all bevels all twelve edges into a sound, watertight solid.
#[test]
fn chamfer_all_plain_box_bevels_all_twelve_edges() {
    let mut m = BRepModel::new();
    let s = match TopologyBuilder::new(&mut m)
        .create_box_3d(10.0, 10.0, 10.0)
        .expect("box")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid geometry for box, got {o:?}"),
    };

    let edges = all_edges(&m, s);
    assert_eq!(
        edges.len(),
        12,
        "a plain box has exactly 12 edges; got {}",
        edges.len()
    );

    let faces = chamfer_edges(&mut m, s, edges, chamfer_opts(1.0))
        .expect("plain-box chamfer-all must succeed (no unsupported corners to skip)");
    // 12 bevel faces + 8 corner caps — at minimum every one of the 12 edges
    // must have been bevelled (a spurious graceful-skip drop would fall short).
    assert!(
        faces.len() >= 12,
        "plain-box chamfer-all must bevel all 12 edges (+ corner caps); produced {} faces",
        faces.len()
    );
    let cert = m.certify_solid(s);
    assert!(
        cert.watertight && cert.manifold && cert.oriented && cert.brep_valid,
        "plain-box chamfer-all must leave a sound watertight solid; \
         watertight={} manifold={} oriented={} brep_valid={} errors={:?}",
        cert.watertight,
        cert.manifold,
        cert.oriented,
        cert.brep_valid,
        cert.errors,
    );
}

#[test]
fn chamfer_all_notched_box_rounds_gracefully() {
    let mut m = BRepModel::new();
    let s = notched_box(&mut m);

    // Precondition: the notch is sound before chamfering.
    let pre = m.certify_solid(s);
    assert!(
        pre.brep_valid && pre.watertight && pre.manifold,
        "notched_box must be sound before chamfering; brep_valid={} watertight={} manifold={} errors={:?}",
        pre.brep_valid, pre.watertight, pre.manifold, pre.errors,
    );

    let edges = all_edges(&m, s);
    assert!(
        edges.len() >= 12,
        "notched box should expose many edges; got {}",
        edges.len()
    );

    let result = chamfer_edges(&mut m, s, edges, chamfer_opts(3.0));

    match result {
        Ok(faces) => {
            // (A) graceful partial round — the supported corners' edges must
            // actually be bevelled, and the result must be watertight & sound.
            assert!(
                !faces.is_empty(),
                "graceful skip must still BEVEL the supported edges, not drop everything",
            );
            let cert = m.certify_solid(s);
            assert!(
                cert.watertight && cert.manifold && cert.oriented && cert.brep_valid,
                "chamfer-all partial round must leave a sound watertight solid; \
                 watertight={} manifold={} oriented={} brep_valid={} boundary_edges={} errors={:?}",
                cert.watertight,
                cert.manifold,
                cert.oriented,
                cert.brep_valid,
                cert.boundary_edges,
                cert.errors,
            );
        }
        Err(e) => {
            // (B) a TYPED refusal is acceptable; a whole-op OPEN-geometry failure
            // (InvalidBRep/InvalidGeometry) or an InternalError is NOT — that is
            // the "refused the whole op instead of rounding what it can" defect.
            let graceful = matches!(&e, OperationError::BlendFailed(_))
                || matches!(&e, OperationError::NotImplemented(_));
            assert!(
                graceful,
                "chamfer-all on the notched box must round gracefully or refuse with a \
                 TYPED BlendFailed — not fail the whole op with an open-geometry / internal \
                 error: {e:?}",
            );
            // On the refusal path the model must be transactionally restored.
            let post = m.certify_solid(s);
            assert!(
                post.watertight && post.brep_valid,
                "after a typed refusal the pre-op solid must be restored intact; \
                 watertight={} brep_valid={} errors={:?}",
                post.watertight,
                post.brep_valid,
                post.errors,
            );
        }
    }
}
