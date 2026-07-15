// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Live-dogfood regression (confirmed live 2026-07-11, kernel-repro 2026-07-12):
//! `fillet_edges` in ALL-edges "round what it can" mode must NOT hard-500 the
//! WHOLE operation because it met ONE corner class whose same-kind patch
//! synthesis is not implemented. It must SKIP that corner's edges and round the
//! rest.
//!
//! ## The live bug
//! On a pocketed + bored part the live op refused with
//!   `500 … Not implemented: Edges A and B share corner vertex V
//!    (ConcaveCorner { degree: 1 }) … same-kind corner-patch synthesis … not yet
//!    implemented`
//! — i.e. `lifecycle::validate_corner_compatibility` (the entry-point pre-flight,
//! reached BEFORE the in-body graceful-skip fixpoint) hard-refusing the ENTIRE
//! op on the FIRST unsupported shared corner. A `Mixed` corner at a pocket
//! opening yields the sibling `Invalid geometry: No face perpendicular to blend …
//! requires a 3-valent corner` deep in corner surgery. Either way one unsupported
//! corner refused every otherwise-fillettable edge — contradicting the documented
//! "seams / over-radius edges are SKIPPED — it rounds everything it can" contract.
//!
//! ## The fix (scope: corners only)
//! In ALL-edges mode (`FilletOptions::graceful_corner_skip`) the entry point
//! PRE-DETECTS and DROPS the edges incident to unsupported corners
//! (`ConcaveCorner{degree 1|2}`, `Mixed`, `Cliff`, non-apex degrees) and the
//! single-edge terminations surgery cannot close, reports them, and rounds the
//! rest — instead of the whole-op refusal. The EXPLICIT-`edge_ids` contract is
//! untouched (it still honest-refuses through the pre-flight).
//!
//! NB the pocketed+bored kernel repro also exposes a *separate*, pre-existing
//! boolean-split-bore-rim fillet limitation (an un-synthesised rim is not a
//! corner-synthesis gap and is out of scope here); those tests therefore assert
//! only that the CORNER refusal is gone and the op stays transactional. The
//! notched-box test exercises a full sound graceful round.
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::fillet::{fillet_edges, FilletOptions, FilletType};
use geometry_engine::operations::lifecycle::{validate_can_apply, OpSpec};
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::operations::OperationError;
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// The live repro: a 60×40×30 box with a rectangular top pocket and a centred
/// bore that pierces the pocket floor. The wall/floor intersection is the
/// `ConcaveCorner` the live op refused on; the pocket opening corners are `Mixed`.
fn pocketed_bored_part(m: &mut BRepModel) -> SolidId {
    let base = match TopologyBuilder::new(m)
        .create_box_3d(60.0, 40.0, 30.0)
        .expect("base box")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid for base box, got {o:?}"),
    };
    // Pocket tool 40×20×20 raised +15 in z → cuts z ∈ [5,15]; pocket floor at z=5.
    let pocket = match TopologyBuilder::new(m)
        .create_box_3d(40.0, 20.0, 20.0)
        .expect("pocket tool")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid for pocket tool, got {o:?}"),
    };
    translate(
        m,
        vec![pocket],
        Vector3::Z,
        15.0,
        TransformOptions::default(),
    )
    .expect("raise pocket");
    let part = boolean_operation(
        m,
        base,
        pocket,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("cut pocket");
    // Centred bore r=4, axis +Z, z ∈ [-25, 20]: pierces the pocket floor (z=5).
    let bore = match TopologyBuilder::new(m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -25.0), Vector3::Z, 4.0, 45.0)
        .expect("bore")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid for bore, got {o:?}"),
    };
    boolean_operation(
        m,
        part,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("drill bore")
}

/// A 40³ box with a 20³ corner-octant notch removed. Concave degree-3 apex at the
/// origin (SUPPORTED) plus three `Mixed` corners at the notch mouths (unsupported)
/// and untouched convex degree-3 box corners far from the notch.
fn notched_box(m: &mut BRepModel) -> SolidId {
    let base = match TopologyBuilder::new(m)
        .create_box_3d(40.0, 40.0, 40.0)
        .expect("base")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid, got {o:?}"),
    };
    let tool = match TopologyBuilder::new(m)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("tool")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid, got {o:?}"),
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

fn fillet_opts(r: f64, graceful: bool) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(r),
        radius: r,
        graceful_corner_skip: graceful,
        ..Default::default()
    }
}

/// `true` iff the error is the unsupported-corner refusal this fix routes around
/// (the pre-flight `ConcaveCorner`/`Mixed` "corner-patch synthesis not yet
/// implemented" or the "requires a 3-valent corner" corner-surgery message).
fn is_corner_synthesis_refusal(e: &OperationError) -> bool {
    let msg = format!("{e:?}");
    msg.contains("corner-patch synthesis")
        || msg.contains("share corner vertex")
        || msg.contains("requires a 3-valent corner")
        || msg.contains("MIXED convexity")
}

/// Every unordered pair of `edges` that shares an endpoint vertex.
fn shared_vertex_pairs(model: &BRepModel, edges: &[EdgeId]) -> Vec<(EdgeId, EdgeId)> {
    let mut out = Vec::new();
    for i in 0..edges.len() {
        let Some(ei) = model.edges.get(edges[i]) else {
            continue;
        };
        for j in (i + 1)..edges.len() {
            let Some(ej) = model.edges.get(edges[j]) else {
                continue;
            };
            let shares = ei.start_vertex == ej.start_vertex
                || ei.start_vertex == ej.end_vertex
                || ei.end_vertex == ej.start_vertex
                || ei.end_vertex == ej.end_vertex;
            if shares {
                out.push((edges[i], edges[j]));
            }
        }
    }
    out
}

/// DETERMINISTIC pre-flight contract (the crux of the fix): a fillet selection
/// that lands on an unsupported shared corner is HONEST-REFUSED by
/// `validate_can_apply` in EXPLICIT mode (`graceful_corner_skip == false`) — the
/// live-500 mechanism — but is ALLOWED in ALL-edges mode
/// (`graceful_corner_skip == true`), where the entry point instead skips the
/// corner and rounds the rest. Same model, same edges: only the flag differs.
#[test]
fn validate_can_apply_graceful_flag_skips_corner_refusal() {
    let mut m = BRepModel::new();
    let s = notched_box(&mut m);
    let edges = all_edges(&m, s);

    // Find a two-edge selection whose shared corner the pre-flight refuses in
    // explicit mode (the notch's Mixed / concave-non-apex corners).
    let mut refusing_pair: Option<(EdgeId, EdgeId)> = None;
    for (a, b) in shared_vertex_pairs(&m, &edges) {
        let explicit = validate_can_apply(
            &m,
            OpSpec::FilletEdges {
                solid_id: s,
                edges: &[a, b],
                partial_corner_vertices: &[],
                setback: 1.5,
                graceful_corner_skip: false,
            },
        );
        if explicit.is_err() {
            refusing_pair = Some((a, b));
            break;
        }
    }
    let (a, b) = refusing_pair.expect(
        "the notched box must expose at least one unsupported shared corner the \
         explicit pre-flight refuses",
    );

    // EXPLICIT mode refuses this exact selection.
    let explicit = validate_can_apply(
        &m,
        OpSpec::FilletEdges {
            solid_id: s,
            edges: &[a, b],
            partial_corner_vertices: &[],
            setback: 1.5,
            graceful_corner_skip: false,
        },
    );
    let err = explicit.expect_err("explicit pre-flight must refuse the unsupported corner");
    assert!(
        is_corner_synthesis_refusal(&err),
        "explicit refusal should name the unsupported corner, got {err:?}",
    );

    // ALL-edges mode ALLOWS it (the entry point will skip the corner, not refuse).
    let graceful = validate_can_apply(
        &m,
        OpSpec::FilletEdges {
            solid_id: s,
            edges: &[a, b],
            partial_corner_vertices: &[],
            setback: 1.5,
            graceful_corner_skip: true,
        },
    );
    assert!(
        graceful.is_ok(),
        "graceful mode must NOT hard-refuse at the corner pre-flight; got {graceful:?}",
    );
}

/// THE FIX: with the graceful opt-in the same fillet-all no longer refuses the
/// whole op on the unsupported corners — the corner-synthesis refusal is gone.
/// (This kernel-built part's bore rims trip a *separate*, out-of-scope
/// split-rim limitation, so we assert the corner refusal is eliminated and the
/// op is transactional rather than requiring full success on the bore rims.)
#[test]
fn fillet_all_graceful_mode_eliminates_corner_refusal() {
    let mut m = BRepModel::new();
    let s = pocketed_bored_part(&mut m);

    let edges = all_edges(&m, s);
    match fillet_edges(&mut m, s, edges, fillet_opts(0.8, true)) {
        Ok(_) => { /* fully rounded — the strongest outcome */ }
        Err(e) => {
            assert!(
                !is_corner_synthesis_refusal(&e),
                "graceful mode must SKIP unsupported corners, not refuse the whole op on them; \
                 still got the corner refusal: {e:?}",
            );
            // Any residual failure (the separate bore-rim limitation) must be
            // transactional — the pre-op solid is restored intact.
            let post = m.certify_solid(s);
            assert!(
                post.brep_valid && post.watertight,
                "graceful failure must roll back to the intact part: {:?}",
                post.errors,
            );
        }
    }
}

/// Round-what-it-can, SOUND result: on the notched box the graceful all-edges
/// fillet skips the three unsupported `Mixed` corners, rounds the supported
/// edges, and leaves a watertight, manifold, oriented, valid B-Rep.
#[test]
fn fillet_all_graceful_mode_rounds_supported_and_is_sound() {
    let mut m = BRepModel::new();
    let s = notched_box(&mut m);

    let pre = m.certify_solid(s);
    assert!(
        pre.brep_valid && pre.watertight && pre.manifold,
        "notched box unsound pre-fillet: {:?}",
        pre.errors,
    );

    let edges = all_edges(&m, s);
    let faces = fillet_edges(&mut m, s, edges, fillet_opts(3.0, true))
        .expect("graceful all-edges fillet on the notched box must SUCCEED");
    assert!(
        !faces.is_empty(),
        "graceful skip must still ROUND the supported edges"
    );

    let cert = m.certify_solid(s);
    assert!(
        cert.watertight && cert.manifold && cert.oriented && cert.brep_valid,
        "graceful partial round must leave a sound watertight solid; \
         watertight={} manifold={} oriented={} brep_valid={} boundary_edges={} errors={:?}",
        cert.watertight,
        cert.manifold,
        cert.oriented,
        cert.brep_valid,
        cert.boundary_edges,
        cert.errors,
    );
}
