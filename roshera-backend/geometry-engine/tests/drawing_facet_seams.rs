// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A faceted surface is drawn as its outline, not as every facet.
//!
//! Found 2026-11-19 rehearsing the aerospace video: the NACA 2412 wing rib is
//! a 161-point polygon extruded 6 mm, and its FRONT and SECTION views came out
//! striped -- every seam between two neighbouring facets (a few degrees
//! apart) was inked as a line across the rib's thickness. A drafter draws the
//! rib's outline and its real edges. The rule: a seam between two flat faces
//! a few degrees apart, inside a run of such facets, is not an edge of the
//! part -- unless it is where the surface turns away from the viewer (the
//! silhouette). A single shallow bend is a real edge and stays.

use geometry_engine::drawing::project_solid_edges_visibility;
use geometry_engine::drawing::types::ProjectionType;
use geometry_engine::math::{Point3, Tolerance};
use geometry_engine::operations::extrude::{extrude_polygon, ExtrudeOptions};
use geometry_engine::primitives::topology_builder::BRepModel;

const THICK: f64 = 6.0;

fn prism(profile: &[(f64, f64)]) -> (BRepModel, u32) {
    let mut m = BRepModel::new();
    let pts: Vec<Point3> = profile
        .iter()
        .map(|&(x, y)| Point3::new(x, y, 0.0))
        .collect();
    let opts = ExtrudeOptions {
        distance: THICK,
        ..ExtrudeOptions::default()
    };
    let s = extrude_polygon(&mut m, &pts, opts, Tolerance::default()).expect("prism");
    (m, s)
}

/// Where each line across the thickness (2D length ~ THICK) sits along the
/// view's horizontal axis -- one per vertical edge the view inks.
fn thickness_line_positions(m: &BRepModel, s: u32, p: ProjectionType) -> Vec<f64> {
    let e = project_solid_edges_visibility(m, s, p, 16).expect("view");
    e.visible
        .iter()
        .chain(e.hidden.iter())
        .filter_map(|pl| {
            let (a, b) = (pl.points[0], pl.points[pl.points.len() - 1]);
            let len = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
            ((len - THICK).abs() < 1e-6).then_some(a[0])
        })
        .collect()
}

fn thickness_lines(m: &BRepModel, s: u32) -> usize {
    [ProjectionType::Front, ProjectionType::Right]
        .into_iter()
        .map(|p| thickness_line_positions(m, s, p).len())
        .sum()
}

/// A lens: two arcs of 40 facets each, chord 100, 8 thick -- an aerofoil's
/// faceting without its camber.
fn lens() -> Vec<(f64, f64)> {
    let n = 40;
    let mut pts = Vec::new();
    for i in 0..=n {
        let t = std::f64::consts::PI * i as f64 / n as f64;
        pts.push((50.0 - 50.0 * t.cos(), 4.0 * t.sin()));
    }
    for i in 1..n {
        let t = std::f64::consts::PI * i as f64 / n as f64;
        pts.push((50.0 + 50.0 * t.cos(), -4.0 * t.sin()));
    }
    pts
}

#[test]
fn a_faceted_profile_is_drawn_as_its_outline() {
    let (m, s) = prism(&lens());
    // FRONT looks across the chord: the striped view. Only the lens's two
    // ends (its pointed tips, and the coarse first facet corners within a
    // fraction of a millimetre of them) may draw a line across the
    // thickness -- none along the chord, where every facet seam used to.
    let front = thickness_line_positions(&m, s, ProjectionType::Front);
    let striped: Vec<f64> = front
        .iter()
        .copied()
        .filter(|x| *x > 1.0 && *x < 99.0)
        .collect();
    assert!(
        striped.is_empty(),
        "FRONT inks facet seams across the chord at x = {striped:?}"
    );
    assert!(
        front.iter().any(|x| x.abs() < 1e-6) && front.iter().any(|x| (x - 100.0).abs() < 1e-6),
        "the outline's two ends stay: {front:?}"
    );
    // Seen end-on (RIGHT), the lens's top and bottom are silhouettes of the
    // faceted surface and must stay.
    let right = thickness_line_positions(&m, s, ProjectionType::Right);
    for y in [-4.0, 4.0] {
        assert!(
            right.iter().any(|v| (v - y).abs() < 1e-6),
            "the silhouette at {y} stays: {right:?}"
        );
    }
    // Before the rule: 160 lines across the thickness in FRONT + RIGHT.
    assert!(thickness_lines(&m, s) < 20);
}

#[test]
fn a_single_shallow_bend_is_a_real_edge() {
    // A 100 x 30 plate whose bottom side bends down 10 degrees halfway.
    let drop = 50.0 * 10f64.to_radians().tan();
    let flat = [(0.0, 0.0), (100.0, 0.0), (100.0, 30.0), (0.0, 30.0)];
    let bent = [
        (0.0, 0.0),
        (50.0, 0.0),
        (100.0, -drop),
        (100.0, 30.0),
        (0.0, 30.0),
    ];
    let (mf, sf) = prism(&flat);
    let (mb, sb) = prism(&bent);
    assert!(
        thickness_lines(&mb, sb) > thickness_lines(&mf, sf),
        "the bend at x = 50 is an edge of the part and stays on the sheet"
    );
}
