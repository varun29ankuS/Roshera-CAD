//! Live-dogfood regression (2026-07-10): filleting ALL edges of a corner-notched
//! box must NOT crash with a surgery-bookkeeping error.
//!
//! The notch (40³ box − 20³ corner-octant cube) has:
//!   * a supported concave degree-3 re-entrant corner (Task #82 Slice 1 —
//!     synthesizes an apex sphere cap), AND
//!   * three UNSUPPORTED `Mixed`-convexity corners (F5-δ / future) where the
//!     notch's concave edges meet the box's convex edges.
//!
//! `fillet_edges` with NO edge selection = ALL edges. The `fillet_edges`
//! contract (per its MCP description) is: "OMIT edge_ids to blend ALL edges;
//! seams / over-radius edges are SKIPPED — it rounds everything it can rather
//! than refusing the whole op." So on an unsupported (Mixed) corner the op must
//! either GRACEFULLY SKIP the Mixed-corner-incident edges (round the rest,
//! watertight) or REFUSE the WHOLE op with a TYPED error (transactionally,
//! model unchanged) — NEVER a surgery bookkeeping crash / InternalError /
//! `BlendEdgeSurgery original_v? N missing from model` InvalidGeometry.
use geometry_engine::math::Vector3;
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::fillet::{fillet_edges, FilletOptions, FilletType};
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::operations::OperationError;
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// A 40³ box centred at origin with a 20³ notch removed from the (+,+,+) octant
/// corner. Concave degree-3 re-entrant vertex at the origin; three Mixed
/// corners at (20,0,0)/(0,20,0)/(0,0,20).
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

fn fillet_opts(r: f64) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(r),
        radius: r,
        ..Default::default()
    }
}

/// GREEN CONTRACT: fillet-all on the notched box must not crash with a surgery
/// bookkeeping error. Either (A) it succeeds and the result is watertight, or
/// (B) it returns a typed refusal (NotImplemented / BlendFailed naming the
/// unsupported corner). An `InternalError`, or an `InvalidGeometry`/`InvalidBRep`
/// carrying "missing from model", is the CRASH we are fixing — that is RED.
#[test]
fn fillet_all_notched_box_no_surgery_crash() {
    let mut m = BRepModel::new();
    let s = notched_box(&mut m);

    // Precondition: the notch is sound before filleting.
    let pre = m.certify_solid(s);
    assert!(
        pre.brep_valid && pre.watertight && pre.manifold,
        "notched_box must be sound before filleting; brep_valid={} watertight={} manifold={} errors={:?}",
        pre.brep_valid, pre.watertight, pre.manifold, pre.errors,
    );

    let edges = all_edges(&m, s);
    assert!(
        edges.len() >= 12,
        "notched box should expose many edges; got {}",
        edges.len()
    );

    let result = fillet_edges(&mut m, s, edges, fillet_opts(3.0));

    match result {
        Ok(faces) => {
            // (A) graceful partial round — the supported edges (the box corners
            // untouched by the notch) must actually be rounded, and the result
            // must be watertight & sound.
            assert!(
                !faces.is_empty(),
                "graceful skip must still ROUND the supported edges, not drop everything",
            );
            let cert = m.certify_solid(s);
            assert!(
                cert.watertight && cert.manifold && cert.oriented && cert.brep_valid,
                "fillet-all partial round must leave a sound watertight solid; \
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
            // (B) typed refusal is acceptable; a surgery-bookkeeping crash is NOT.
            let is_surgery_crash = matches!(&e, OperationError::InternalError(_))
                || matches!(&e,
                    OperationError::InvalidGeometry(msg) | OperationError::InvalidBRep(msg)
                        if msg.contains("missing from model")
                );
            assert!(
                !is_surgery_crash,
                "fillet-all on the notched box crashed with a surgery-bookkeeping error \
                 instead of gracefully skipping or cleanly refusing: {:?}",
                e,
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
