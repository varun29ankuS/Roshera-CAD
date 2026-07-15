// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Live-dogfood gate — the descriptive edge selector must be able to filter by
//! CONVEXITY so an agent can say "select the concave edges" and then fillet only
//! the re-entrant corner.
//!
//! Fixture: a 40³ box with a 20³ corner-octant removed (boolean Difference),
//! copied from `edge_convexity_boolean_notch.rs`. That test PROVES the three
//! edges meeting at the re-entrant origin vertex classify `convexity == -1`
//! (concave) and every outer box edge stays `+1` (convex). Here we assert the
//! selector honours that classification:
//!   * `Convexity::Concave`  → resolves to EXACTLY those three notch edges;
//!   * `Convexity::Convex`   → never returns any of them.
//!
//! Before the filter exists this file does not compile (RED); once
//! `queries::select` grows a `convexity` criterion it goes GREEN.

use geometry_engine::math::Vector3;
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::edge_classification::classify_all_unclassified_edges;
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::queries::select::{
    resolve_edge, Convexity, CurveKind, EdgeQuery, SelectError,
};

/// A 40³ box centred at origin with the (+,+,+) 20³ corner octant removed. The
/// inner re-entrant vertex at (0,0,0) is a concave degree-3 corner.
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

/// The three concave edges incident on the re-entrant origin vertex.
fn reentrant_edges(m: &BRepModel) -> Vec<u32> {
    let mut origin_vid = None;
    let mut best = f64::MAX;
    for (vid, v) in m.vertices.iter() {
        let d = v.point().magnitude();
        if d < best {
            best = d;
            origin_vid = Some(vid);
        }
    }
    let origin_vid = origin_vid.expect("notched box must have vertices");
    assert!(
        best < 1e-6,
        "re-entrant vertex must sit at the origin (nearest {best})"
    );
    let mut out: Vec<u32> = Vec::new();
    for (eid, edge) in m.edges.iter() {
        if edge.start_vertex == origin_vid || edge.end_vertex == origin_vid {
            out.push(eid);
        }
    }
    out.sort_unstable();
    out
}

#[test]
fn concave_filter_resolves_exactly_the_notch_edges() {
    let mut m = BRepModel::new();
    let s = notched_box(&mut m);
    classify_all_unclassified_edges(&mut m).expect("classify");

    let mut expected = reentrant_edges(&m);
    expected.sort_unstable();
    assert_eq!(expected.len(), 3, "the origin corner is degree-3");

    // Concave, no extremal → three matches → Ambiguous carrying EXACTLY the
    // three notch edges (no more, no fewer). The selector refuses to guess
    // which of the three, but the candidate SET is the whole answer.
    let q = EdgeQuery::new(CurveKind::Any).convexity(Convexity::Concave);
    match resolve_edge(&mut m, s, &q) {
        Err(SelectError::Ambiguous(mut c)) => {
            c.sort_unstable();
            assert_eq!(
                c, expected,
                "concave filter must return exactly the three re-entrant notch edges"
            );
        }
        other => panic!("expected Ambiguous(3 concave edges), got {other:?}"),
    }
}

#[test]
fn convex_filter_excludes_the_notch_edges() {
    let mut m = BRepModel::new();
    let s = notched_box(&mut m);
    classify_all_unclassified_edges(&mut m).expect("classify");

    let reentrant = reentrant_edges(&m);

    // Convex filter → a box has many convex edges → Ambiguous, but NONE of the
    // returned candidates may be a concave notch edge.
    let q = EdgeQuery::new(CurveKind::Any).convexity(Convexity::Convex);
    match resolve_edge(&mut m, s, &q) {
        Err(SelectError::Ambiguous(c)) => {
            assert!(!c.is_empty(), "a notched box still has convex edges");
            for e in &reentrant {
                assert!(
                    !c.contains(e),
                    "concave notch edge {e} must NOT appear under the convex filter"
                );
            }
        }
        Ok(single) => {
            for e in &reentrant {
                assert_ne!(
                    &single, e,
                    "a convex-filtered resolve must never return a concave notch edge"
                );
            }
        }
        other => panic!("expected convex candidates, got {other:?}"),
    }
}

#[test]
fn default_any_is_unchanged() {
    // Back-compat: the default query (Convexity::Any) must behave exactly as
    // before — a notched box has many edges, so an unfiltered line query is
    // ambiguous, never filtered by convexity.
    let mut m = BRepModel::new();
    let s = notched_box(&mut m);
    classify_all_unclassified_edges(&mut m).expect("classify");

    let q = EdgeQuery::new(CurveKind::Line);
    assert_eq!(q.convexity, Convexity::Any, "default convexity is Any");
    match resolve_edge(&mut m, s, &q) {
        Err(SelectError::Ambiguous(c)) => {
            assert!(
                c.len() > 3,
                "unfiltered line edges of a notched box are many"
            );
        }
        other => panic!("expected Ambiguous (many line edges), got {other:?}"),
    }
}
