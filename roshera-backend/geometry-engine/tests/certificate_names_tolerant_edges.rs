// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A solid whose edge carries a tolerance above the ordinary bound certifies
//! at that tolerance -- the B-Rep check judges the edge by it. The
//! certificate has to say so: it names every such edge with the tolerance it
//! carries, so "valid" is never read as "exact" for a part that is valid
//! only to within a tolerant edge.

use geometry_engine::primitives::provenance::TOLERANT_EDGE_THRESHOLD;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn box_model() -> (BRepModel, u32) {
    let mut model = BRepModel::new();
    let solid = match TopologyBuilder::new(&mut model)
        .create_box_3d(20.0, 14.0, 10.0)
        .expect("box")
    {
        GeometryId::Solid(s) => s,
        other => panic!("expected a solid, got {other:?}"),
    };
    (model, solid)
}

#[test]
fn an_exact_box_names_no_tolerant_edge() {
    let (mut model, solid) = box_model();
    let cert = model.certify_solid(solid);
    assert!(cert.is_sound());
    assert!(cert.tolerant_edges.is_empty(), "{:?}", cert.tolerant_edges);
}

#[test]
fn a_tolerant_edge_is_named_with_its_tolerance() {
    let (mut model, solid) = box_model();
    let mut ids: Vec<u32> = model.edges.iter().map(|(id, _)| id).collect();
    ids.sort_unstable();
    let (loose, at_the_bound) = (ids[3], ids[7]);
    model
        .edges
        .get_mut(loose)
        .expect("edge")
        .set_tolerance(0.03);
    // Exactly the bound is ordinary accuracy, not a tolerant edge.
    model
        .edges
        .get_mut(at_the_bound)
        .expect("edge")
        .set_tolerance(TOLERANT_EDGE_THRESHOLD);
    let cert = model.certify_solid(solid);
    assert_eq!(
        cert.tolerant_edges.len(),
        1,
        "one edge above the bound: {:?}",
        cert.tolerant_edges
    );
    assert_eq!(cert.tolerant_edges[0].edge, loose);
    assert!((cert.tolerant_edges[0].tolerance - 0.03).abs() < 1e-15);
    assert!(
        cert.is_sound(),
        "the disclosure qualifies soundness, it does not withdraw it: {:?}",
        cert.errors
    );
    assert_eq!(
        model.tolerant_edges_of(solid),
        cert.tolerant_edges,
        "the kernel query and the certificate agree"
    );
}
