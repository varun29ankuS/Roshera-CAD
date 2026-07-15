// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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
use geometry_engine::primitives::vertex::VertexId;

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

/// A plain axis-aligned cube of side `size` centred at the origin.
fn plain_cube(m: &mut BRepModel, size: f64) -> SolidId {
    match TopologyBuilder::new(m)
        .create_box_3d(size, size, size)
        .expect("cube")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid geometry for cube, got {o:?}"),
    }
}

/// FIX 1 (review ⚠, Finding 1) — the graceful-skip exemption must match the
/// surgery `corner_shared` triggers EXACTLY, never looser.
///
/// The corrupting path the review flagged: a corner left PENDING by a first
/// **fillet** opt-in call carries a prior *Fillet* (not Chamfer) blend and is
/// NOT opt-in on a later `fillet_edges` call. The old exemption exempted every
/// vertex in `pending_mixed_kind_corners` — so that corner was carved out of
/// the corner fixpoint yet, at the surgery, `corner_shared` was `false` (not a
/// degree-3 apex, not prior-chamfer, not opt-in on THIS call). An unsupported
/// multi-edge corner could then reach the destructive splice unshared and
/// crash with `BlendEdgeSurgery original_v? N missing from model` — the exact
/// bug commit 1dc1d12 fixed for the un-pended case.
///
/// GREEN CONTRACT: the second call must NOT crash with a surgery-bookkeeping
/// error. It either rounds gracefully or returns a typed refusal, and on the
/// refusal path the model is transactionally restored to its intermediate state.
///
/// Reachability note (documented in the D-2 report): on the default public path
/// this exact sequence is refused up front by the D-1
/// `validate_same_kind_scar_adjacency` pre-flight gate (lifecycle.rs) — the
/// second call's endpoints sit within setback of the first fillet's live scar
/// faces — so the loose-exemption crash never surfaces here. The exemption
/// tightening is therefore a defense-in-depth alignment: the corner fixpoint's
/// exemption set must equal the surgery `corner_shared` triggers even though an
/// upstream gate currently masks the divergence. This test pins the public
/// contract (no surgery-bookkeeping crash, transactional refusal) and guards
/// against the D-1 gate ever being relaxed without the fixpoint being sound.
#[test]
fn fillet_all_after_pending_fillet_corner_no_surgery_crash() {
    let mut m = BRepModel::new();
    let s = plain_cube(&mut m, 10.0);

    // The (+,+,+) corner and its three incident edges.
    let corner: VertexId = m
        .vertices
        .iter()
        .find(|(_, v)| v.position[0] > 4.0 && v.position[1] > 4.0 && v.position[2] > 4.0)
        .map(|(id, _)| id)
        .expect("(+,+,+) cube corner vertex");
    let mut corner_edges: Vec<EdgeId> = all_edges(&m, s)
        .into_iter()
        .filter(|&e| {
            m.edges
                .get(e)
                .map(|ed| ed.start_vertex == corner || ed.end_vertex == corner)
                .unwrap_or(false)
        })
        .collect();
    corner_edges.sort_unstable();
    assert_eq!(
        corner_edges.len(),
        3,
        "a plain cube corner must have exactly three incident edges; got {corner_edges:?}"
    );

    // First call: fillet TWO of the three corner edges, declaring the corner
    // partial-mixed (opt-in). This registers the corner in
    // `pending_mixed_kind_corners` carrying a prior FILLET blend and leaves it
    // deliberately open — the setup the second call must survive.
    let mut first = fillet_opts(1.0);
    first.partial_corner_vertices = vec![corner];
    fillet_edges(&mut m, s, vec![corner_edges[1], corner_edges[2]], first)
        .expect("first-call opt-in fillet on two of three corner edges succeeds");

    // Snapshot the intermediate (intentionally-open) state before the second
    // call so the refusal path can be checked for transactionality — the
    // pending corner from call 1 is deliberately open, so "restored" means
    // "identical to this intermediate state", NOT "brep_valid".
    let pre2 = m.certify_solid(s);

    // Second call: fillet ALL currently-live edges WITHOUT opt-in. The pending
    // corner now carries a prior Fillet (not Chamfer) and is not opt-in here.
    let edges = all_edges(&m, s);
    let result = fillet_edges(&mut m, s, edges, fillet_opts(1.0));

    if let Err(e) = result {
        let is_surgery_crash = matches!(&e, OperationError::InternalError(_))
            || matches!(&e,
                OperationError::InvalidGeometry(msg) | OperationError::InvalidBRep(msg)
                    if msg.contains("missing from model")
            );
        assert!(
            !is_surgery_crash,
            "fillet-all after a pending same-kind fillet corner crashed with a \
             surgery-bookkeeping error instead of gracefully skipping or cleanly \
             refusing: {e:?}",
        );
        // Typed refusal must be transactional — the model is restored to the
        // pre-call-2 intermediate state (same boundary-edge count & validity).
        let post = m.certify_solid(s);
        assert_eq!(
            (post.brep_valid, post.boundary_edges),
            (pre2.brep_valid, pre2.boundary_edges),
            "a typed refusal must roll back to the pre-call intermediate state; \
             post=({}, {}) pre2=({}, {}) errors={:?}",
            post.brep_valid,
            post.boundary_edges,
            pre2.brep_valid,
            pre2.boundary_edges,
            post.errors,
        );
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
