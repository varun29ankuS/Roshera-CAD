//! Integration property tests for dihedral edge classification
//! (CD-φ.2.1): convexity and the ±π/2 box dihedral are invariant under
//! the box's overall scale and aspect ratio.
//!
//! The in-crate unit tests pin a single unit cube. A box's exterior
//! dihedral is geometrically π/2 regardless of its dimensions, and every
//! edge is convex — properties that exercise the oriented-normal +
//! loop-aligned-tangent sign logic across shapes the unit cube can't
//! distinguish (e.g. a thin slab vs. a near-cube).

#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use geometry_engine::operations::edge_classification::{
    classify_all_unclassified_edges, classify_dihedral, DihedralClass,
};
use geometry_engine::primitives::topology_builder::{BRepModel, TopologyBuilder};

/// Build a box of the given dimensions and classify all its edges.
fn classified_box(w: f64, h: f64, d: f64) -> BRepModel {
    let mut model = BRepModel::new();
    {
        let mut builder = TopologyBuilder::new(&mut model);
        builder
            .create_box_3d(w, h, d)
            .expect("create_box_3d should succeed for positive dimensions");
    }
    classify_all_unclassified_edges(&mut model).expect("classification sweep");
    model
}

/// A box of any dimensions has exactly 12 convex edges, each with a
/// ±π/2 dihedral.
fn assert_all_edges_convex_right_angle(model: &BRepModel, label: &str) {
    let mut count = 0;
    for (eid, edge) in model.edges.iter() {
        let class = classify_dihedral(model, eid)
            .expect("classify_dihedral succeeds")
            .unwrap_or_else(|| panic!("{label}: edge {eid} has no dihedral"));
        assert_eq!(
            class,
            DihedralClass::Convex,
            "{label}: edge {eid} should be convex"
        );
        let dihedral = edge
            .attributes
            .dihedral_angle
            .unwrap_or_else(|| panic!("{label}: edge {eid} missing dihedral angle"));
        assert!(
            (dihedral.abs() - std::f64::consts::FRAC_PI_2).abs() < 1e-6,
            "{label}: edge {eid} dihedral should be ±π/2, got {dihedral}"
        );
        count += 1;
    }
    assert_eq!(count, 12, "{label}: a box has 12 edges");
}

#[test]
fn cube_edges_all_convex_right_angle() {
    let model = classified_box(2.0, 2.0, 2.0);
    assert_all_edges_convex_right_angle(&model, "cube 2×2×2");
}

#[test]
fn thin_slab_edges_all_convex_right_angle() {
    // Extreme aspect ratio: a thin plate. Dihedral is still π/2.
    let model = classified_box(10.0, 10.0, 0.1);
    assert_all_edges_convex_right_angle(&model, "slab 10×10×0.1");
}

#[test]
fn long_bar_edges_all_convex_right_angle() {
    let model = classified_box(20.0, 0.5, 0.5);
    assert_all_edges_convex_right_angle(&model, "bar 20×0.5×0.5");
}

#[test]
fn small_box_edges_all_convex_right_angle() {
    // Sub-millimetre box: convexity classification must not be fooled by
    // small absolute coordinates.
    let model = classified_box(0.01, 0.02, 0.03);
    assert_all_edges_convex_right_angle(&model, "tiny 0.01×0.02×0.03");
}

#[test]
fn no_box_edge_classifies_as_g1_or_concave() {
    // A convex polyhedron has no smooth or concave edges anywhere.
    let model = classified_box(3.0, 1.0, 2.0);
    for (eid, _) in model.edges.iter() {
        let class = classify_dihedral(&model, eid).expect("classify").expect("dihedral");
        assert!(
            !matches!(class, DihedralClass::G1Smooth | DihedralClass::Concave),
            "edge {eid} of a convex box must not be G1/concave, got {class:?}"
        );
    }
}
