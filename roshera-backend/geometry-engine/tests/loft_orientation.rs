// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// Reason: fixed-size corner tables and matched ring pairs -- every index is
// bounded by a length asserted or constructed a few lines above. Same rationale
// as `operations/loft.rs`'s module-level allow.
#![allow(clippy::indexing_slicing)]

//! A loft between sections whose vertex rings do not START at the same place
//! must still build a solid the mesh can orient.
//!
//! `establish_correspondence` reads each profile's ring off that profile's own
//! outer loop, and `densify_correspondence` resamples each ring from that
//! loop's first edge. Nothing registers ring `i` against ring `i-1`, so index
//! `k` of one ring pairs with index `k` of the next wherever their loops happen
//! to have started. Two sections that are geometrically ideal partners but whose
//! loops start at different angles are therefore lofted through a twisted band:
//! the rulings cross, the lateral quads fold, and the per-quad radial-outward
//! test that picks each face's `FaceOrientation` flips sign from quad to quad —
//! a welded mesh that CLOSES (`boundary_edges == 0`, `nonmanifold_edges == 0`)
//! but is not consistently wound, and a certificate that reads unsound.
//!
//! A cyclic shift cannot repair the harder half of the same gap: two rings of
//! OPPOSITE traversal sense have no shift that aligns them at all, and the
//! kernel ruled them together regardless.
//!
//! Five fixtures pin it:
//!
//! * `loft_index_offset_squares_builds_an_exact_box` — the same square at two
//!   heights, the upper one's edge list started one corner later. The correct
//!   answer is a box and nothing about it is approximate: `V = side² · height`
//!   exactly. This isolates ring registration from every other loft concern,
//!   because the two sections are congruent.
//! * `loft_opposite_winding_squares_builds_the_same_exact_box` — the same
//!   square again, the upper one listed clockwise. Same exact box: a section's
//!   authored direction is a statement about the edge list, not about the
//!   solid. This is the case no cyclic shift can reach.
//! * `loft_triangle_to_square_is_closed_oriented_and_sound` — the smallest
//!   dissimilar pair (3 edges to 4). Its volume is checked against the
//!   prismatoid (Simpson) closed form evaluated on the rings the solid ACTUALLY
//!   carries, read back out of its own topology, so the check judges the built
//!   body rather than re-running the pairing algorithm as its own oracle.
//! * `loft_with_a_supplied_correspondence_that_densify_rebuilds_is_still_registered`
//!   — the caller who supplies a `vertex_correspondence` that densification
//!   throws away. Registration must not stand down for them: their pairing was
//!   already discarded, so skipping registration leaves them on the un-registered
//!   index-k-to-index-k path this whole file exists to close.
//! * `minimal_twist_loft_is_bit_identical_to_linear` — the property that made it
//!   safe to delete `create_minimal_twist_loft`'s own shift search, now that
//!   `register_correspondence` minimises the same objective over a strictly
//!   larger candidate set ahead of the loft-type dispatch.

use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Point3, Tolerance};
use geometry_engine::operations::loft::LoftType;
use geometry_engine::operations::{loft_profiles, LoftOptions};
use geometry_engine::primitives::curve::Line;
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn line_edge(m: &mut BRepModel, a: u32, b: u32) -> EdgeId {
    let pa = m.vertices.get(a).expect("start vertex").position;
    let pb = m.vertices.get(b).expect("end vertex").position;
    let cid = m
        .curves
        .add(Box::new(Line::new(Point3::from(pa), Point3::from(pb))));
    m.edges
        .add(Edge::new_auto_range(0, a, b, cid, EdgeOrientation::Forward))
}

/// A closed polygon profile through `corners`, in the order given, returning
/// both its edges and the vertices they were built from (the latter is what a
/// caller-supplied `vertex_correspondence` is made of).
fn polygon_profile_verts(m: &mut BRepModel, corners: &[Point3]) -> (Vec<EdgeId>, Vec<u32>) {
    let ids: Vec<u32> = corners
        .iter()
        .map(|p| m.vertices.add(p.x, p.y, p.z))
        .collect();
    let edges = (0..ids.len())
        .map(|i| line_edge(m, ids[i], ids[(i + 1) % ids.len()]))
        .collect();
    (edges, ids)
}

/// A closed polygon profile through `corners`, in the order given.
fn polygon_profile(m: &mut BRepModel, corners: &[Point3]) -> Vec<EdgeId> {
    polygon_profile_verts(m, corners).0
}

/// The four corners of a `side`-square centred on the Z axis at height `z`,
/// counter-clockwise seen from +Z, starting at corner index `start`.
fn square_corners(side: f64, z: f64, start: usize) -> Vec<Point3> {
    let h = side / 2.0;
    let base = [
        Point3::new(-h, -h, z),
        Point3::new(h, -h, z),
        Point3::new(h, h, z),
        Point3::new(-h, h, z),
    ];
    (0..4).map(|i| base[(i + start) % 4]).collect()
}

/// A triangle centred on the Z axis at height `z`, circumradius `r`,
/// counter-clockwise seen from +Z.
fn triangle_corners(r: f64, z: f64) -> Vec<Point3> {
    (0..3)
        .map(|i| {
            let a = std::f64::consts::FRAC_PI_2 + (i as f64) * std::f64::consts::TAU / 3.0;
            Point3::new(r * a.cos(), r * a.sin(), z)
        })
        .collect()
}

fn loft_solid_opts() -> LoftOptions {
    LoftOptions {
        create_solid: true,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Readback: the rings the built solid actually carries
// ---------------------------------------------------------------------------

/// Every edge reachable from the solid's outer shell.
fn shell_edges(model: &BRepModel, solid: SolidId) -> Vec<EdgeId> {
    let mut out = Vec::new();
    let Some(s) = model.solids.get(solid) else {
        return out;
    };
    let Some(shell) = model.shells.get(s.outer_shell) else {
        return out;
    };
    for &fid in &shell.faces {
        let Some(face) = model.faces.get(fid) else {
            continue;
        };
        for lid in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied()) {
            if let Some(lp) = model.loops.get(lid) {
                out.extend_from_slice(&lp.edges);
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// The lofted body's two vertex rings, PAIRED as the solid built them.
///
/// The bottom ring is recovered from the solid's own vertices at `z0` and
/// ordered by angle about the Z axis — for the convex, axis-containing sections
/// used here that angular order *is* the boundary order. Each bottom vertex's
/// partner is then the far end of its one rail edge (the only incident edge
/// whose other end sits at `z1`). The pairing therefore comes from the built
/// topology, not from re-running the loft's own correspondence rule.
///
/// Returns `(bottom_ring, top_ring)` in matched index order.
fn paired_rings(model: &BRepModel, solid: SolidId, z0: f64, z1: f64) -> (Vec<Point3>, Vec<Point3>) {
    const Z_EPS: f64 = 1e-6;
    let edges = shell_edges(model, solid);

    let pos =
        |v: u32| -> Point3 { Point3::from(model.vertices.get(v).expect("solid vertex").position) };

    // Rails: an edge with one end on each ring.
    let mut partner: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    for &eid in &edges {
        let Some(e) = model.edges.get(eid) else {
            continue;
        };
        let (a, b) = (e.start_vertex, e.end_vertex);
        let (pa, pb) = (pos(a), pos(b));
        if (pa.z - z0).abs() < Z_EPS && (pb.z - z1).abs() < Z_EPS {
            partner.insert(a, b);
        } else if (pb.z - z0).abs() < Z_EPS && (pa.z - z1).abs() < Z_EPS {
            partner.insert(b, a);
        }
    }

    // A ring vertex has exactly one rail. If any had two, the `insert` above
    // would have silently kept the last and the readback would lie in exactly
    // the failure case it exists to catch.
    let ring_vertices: std::collections::HashSet<u32> = edges
        .iter()
        .filter_map(|&eid| model.edges.get(eid))
        .flat_map(|e| [e.start_vertex, e.end_vertex])
        .filter(|&v| (pos(v).z - z0).abs() < Z_EPS)
        .collect();
    assert_eq!(
        partner.len(),
        ring_vertices.len(),
        "readback: {} of {} bottom-ring vertices carry exactly one rail",
        partner.len(),
        ring_vertices.len()
    );

    let mut bottom: Vec<u32> = partner.keys().copied().collect();
    bottom.sort_by(|&x, &y| {
        let (px, py) = (pos(x), pos(y));
        px.y.atan2(px.x)
            .partial_cmp(&py.y.atan2(py.x))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let top: Vec<Point3> = bottom
        .iter()
        .map(|v| pos(*partner.get(v).expect("rail partner")))
        .collect();
    let bottom: Vec<Point3> = bottom.iter().map(|&v| pos(v)).collect();
    (bottom, top)
}

/// Shoelace signed area of a planar polygon projected on XY.
fn signed_area_xy(pts: &[Point3]) -> f64 {
    let n = pts.len();
    let mut a = 0.0;
    for i in 0..n {
        let p = pts[i];
        let q = pts[(i + 1) % n];
        a += p.x * q.y - q.x * p.y;
    }
    a * 0.5
}

/// Prismatoid volume of the ruled body between two matched rings in parallel
/// planes `h` apart.
///
/// The lateral surface is ruled, so the cross-section at parameter `t` is the
/// polygon of `lerp(bottom_i, top_i, t)` and its area is a QUADRATIC in `t`.
/// Simpson's rule is therefore exact, not an approximation:
/// `V = h/6 · (A0 + 4·A_mid + A1)`.
fn prismatoid_volume(bottom: &[Point3], top: &[Point3], h: f64) -> f64 {
    let mid: Vec<Point3> = bottom
        .iter()
        .zip(top.iter())
        .map(|(b, t)| Point3::new((b.x + t.x) * 0.5, (b.y + t.y) * 0.5, (b.z + t.z) * 0.5))
        .collect();
    let a0 = signed_area_xy(bottom);
    let a1 = signed_area_xy(top);
    let am = signed_area_xy(&mid);
    (h / 6.0 * (a0 + 4.0 * am + a1)).abs()
}

// ---------------------------------------------------------------------------
// The oracle every case runs
// ---------------------------------------------------------------------------

struct Verdict {
    boundary_edges: usize,
    nonmanifold_edges: usize,
    oriented: bool,
    brep_valid: bool,
    sound: bool,
    failed_dimensions: Vec<&'static str>,
    volume: f64,
}

impl Verdict {
    fn line(&self, case: &str) -> String {
        format!(
            "{case}: be={} nme={} oriented={} brep_valid={} cert.is_sound={} fails=[{}] volume={:.6}",
            self.boundary_edges,
            self.nonmanifold_edges,
            self.oriented,
            self.brep_valid,
            self.sound,
            self.failed_dimensions.join(","),
            self.volume,
        )
    }
}

/// Run the kernel's own ground truth on `solid`: scoped B-Rep validation, the
/// welded-mesh manifold report, the intrinsic certificate, and the volume.
fn oracle(model: &mut BRepModel, solid: SolidId) -> Verdict {
    let val = validate_solid_scoped(
        model,
        solid,
        Tolerance::default(),
        ValidationLevel::Standard,
    );
    // chord 0.5, weld 1e-6 -- the harness convention for 1..30-unit solids,
    // matching op_stress_round3 so the two agree on what "closed" means.
    let mr = manifold_report(model, solid, 0.5, 1e-6);
    let cert = model.certify_solid(solid);
    let volume = model.calculate_solid_volume(solid).unwrap_or(f64::NAN);

    let mut failed_dimensions = Vec::new();
    if !cert.brep_valid {
        failed_dimensions.push("brep_valid");
    }
    if !cert.watertight {
        failed_dimensions.push("watertight");
    }
    if !cert.manifold {
        failed_dimensions.push("manifold");
    }
    if !cert.oriented {
        failed_dimensions.push("oriented");
    }
    if !cert.self_intersection_free {
        failed_dimensions.push("self_intersection_free");
    }
    if !cert.construction_consistent.is_sound() {
        failed_dimensions.push("construction_consistent");
    }
    if !cert.tessellation.clean {
        failed_dimensions.push("tessellation");
    }
    if !cert.mesh_quality.clean {
        failed_dimensions.push("mesh_quality");
    }

    Verdict {
        boundary_edges: mr.as_ref().map(|r| r.boundary_edges).unwrap_or(usize::MAX),
        nonmanifold_edges: mr
            .as_ref()
            .map(|r| r.nonmanifold_edges)
            .unwrap_or(usize::MAX),
        oriented: mr.as_ref().map(|r| r.oriented).unwrap_or(false),
        brep_valid: val.is_valid,
        sound: cert.is_sound(),
        failed_dimensions,
        volume,
    }
}

fn assert_closed_oriented_and_sound(v: &Verdict, case: &str) {
    let line = v.line(case);
    assert!(
        !line.contains("  "),
        "verdict line has a double space: {line}"
    );
    assert_eq!(v.boundary_edges, 0, "{line}");
    assert_eq!(v.nonmanifold_edges, 0, "{line}");
    assert!(v.oriented, "{line}");
    assert!(v.brep_valid, "{line}");
    assert!(v.sound, "{line}");
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

/// Two congruent squares, the upper one's edge list started one corner later.
///
/// Congruent sections make the correct answer exact: the body is a `side ×
/// side × height` box and its volume is `side² · height` with no
/// discretisation term at all — every ring sample lands on the section
/// boundary, and a straight edge is reproduced exactly by its chord. Only the
/// index offset stands between the input and that box, so this case measures
/// ring registration and nothing else.
#[test]
fn loft_index_offset_squares_builds_an_exact_box() {
    const SIDE: f64 = 10.0;
    const HEIGHT: f64 = 20.0;

    let mut m = BRepModel::new();
    let p0 = polygon_profile(&mut m, &square_corners(SIDE, 0.0, 0));
    // Same square, same winding -- but its loop starts one corner round.
    let p1 = polygon_profile(&mut m, &square_corners(SIDE, HEIGHT, 1));

    let solid = loft_profiles(&mut m, vec![p0, p1], loft_solid_opts())
        .expect("loft of two congruent squares must build");

    let v = oracle(&mut m, solid);
    assert_closed_oriented_and_sound(&v, "square->index-offset square");

    let expected = SIDE * SIDE * HEIGHT;
    let rel = (v.volume - expected).abs() / expected;
    assert!(
        rel < 1e-3,
        "square->index-offset square: volume {:.6} is {:.4}% off the exact box {expected:.6}",
        v.volume,
        rel * 100.0
    );
}

/// Two congruent squares, the upper one listed CLOCKWISE.
///
/// No cyclic shift can ever align two rings of opposite traversal sense, so
/// this is the case that shift-only registration cannot reach: the kernel used
/// to rule the rings together anyway and build a band twisted through a full
/// turn. The correct answer is the same exact box as the index-offset case,
/// because a section's authored direction is a statement about the edge list,
/// not about the solid. This exercises the reversal end to end -- through the
/// lateral band AND through `build_loft_cap`, which a unit test on the
/// correspondence alone cannot do.
#[test]
fn loft_opposite_winding_squares_builds_the_same_exact_box() {
    const SIDE: f64 = 10.0;
    const HEIGHT: f64 = 20.0;

    let mut m = BRepModel::new();
    let p0 = polygon_profile(&mut m, &square_corners(SIDE, 0.0, 0));
    let mut upper = square_corners(SIDE, HEIGHT, 0);
    upper.reverse();
    let p1 = polygon_profile(&mut m, &upper);

    let solid = loft_profiles(&mut m, vec![p0, p1], loft_solid_opts())
        .expect("loft of two oppositely wound squares must build");

    let v = oracle(&mut m, solid);
    assert_closed_oriented_and_sound(&v, "square->clockwise square");

    let expected = SIDE * SIDE * HEIGHT;
    let rel = (v.volume - expected).abs() / expected;
    assert!(
        rel < 1e-3,
        "square->clockwise square: volume {:.6} is {:.4}% off the exact box {expected:.6}",
        v.volume,
        rel * 100.0
    );
}

/// The smallest dissimilar pair: a 3-edge section lofted to a 4-edge one.
///
/// Both densify to the same ring size, so the build cannot fail on vertex
/// counts; what it can (and did) fail on is WHICH bottom sample pairs with
/// which top sample. The volume is judged against the prismatoid closed form
/// evaluated on the rings read back out of the solid's own topology, which is
/// exact for a ruled lateral surface and does not consult the loft's pairing
/// rule.
#[test]
fn loft_triangle_to_square_is_closed_oriented_and_sound() {
    const R: f64 = 8.0;
    const SIDE: f64 = 12.0;
    const HEIGHT: f64 = 15.0;

    let mut m = BRepModel::new();
    let p0 = polygon_profile(&mut m, &triangle_corners(R, 0.0));
    let p1 = polygon_profile(&mut m, &square_corners(SIDE, HEIGHT, 0));

    let solid = loft_profiles(&mut m, vec![p0, p1], loft_solid_opts())
        .expect("loft of a triangle to a square must build");

    let v = oracle(&mut m, solid);
    assert_closed_oriented_and_sound(&v, "triangle->square");

    let (bottom, top) = paired_rings(&m, solid, 0.0, HEIGHT);
    assert!(
        bottom.len() >= 8 && bottom.len() == top.len(),
        "triangle->square: expected matched rings of at least 8, got {} and {}",
        bottom.len(),
        top.len()
    );

    // The rings must be the sections themselves: every ring sample lies on a
    // straight section edge, so the ring polygons reproduce the profile areas
    // exactly. If registration ever starts inventing samples this trips first.
    let tri_area = 3.0 * 3.0_f64.sqrt() / 4.0 * R * R;
    assert!(
        (signed_area_xy(&bottom).abs() - tri_area).abs() / tri_area < 1e-9,
        "triangle->square: bottom ring area {:.9} is not the triangle's {tri_area:.9}",
        signed_area_xy(&bottom).abs()
    );
    assert!(
        (signed_area_xy(&top).abs() - SIDE * SIDE).abs() / (SIDE * SIDE) < 1e-9,
        "triangle->square: top ring area {:.9} is not the square's {:.9}",
        signed_area_xy(&top).abs(),
        SIDE * SIDE
    );

    let expected = prismatoid_volume(&bottom, &top, HEIGHT);
    let rel = (v.volume - expected).abs() / expected;
    assert!(
        rel < 1e-3,
        "triangle->square: volume {:.6} is {:.4}% off the prismatoid closed form {expected:.6}",
        v.volume,
        rel * 100.0
    );
}

/// A caller-supplied `vertex_correspondence` that densification REBUILDS must
/// still be registered.
///
/// `densify_correspondence` resamples every profile from its own outer loop
/// whenever any ring's length differs from the chord-sag target, which throws a
/// supplied correspondence away and rebuilds exactly the index-k-to-index-k
/// pairing registration exists to repair. So the caller who supplies a
/// correspondence of the "wrong" size -- here the four raw square corners
/// against a target of eight -- is the caller most exposed to the defect, and a
/// registration gate that merely asks "did the caller supply one?" stands down
/// for precisely them.
///
/// Same congruent index-offset squares as
/// `loft_index_offset_squares_builds_an_exact_box`, so the correct answer is the
/// same exact box.
#[test]
fn loft_with_a_supplied_correspondence_that_densify_rebuilds_is_still_registered() {
    const SIDE: f64 = 10.0;
    const HEIGHT: f64 = 20.0;

    let mut m = BRepModel::new();
    let (p0, v0) = polygon_profile_verts(&mut m, &square_corners(SIDE, 0.0, 0));
    let (p1, v1) = polygon_profile_verts(&mut m, &square_corners(SIDE, HEIGHT, 1));

    // Four vertices per ring; the chord-sag target for a square is eight, so
    // densification rebuilds both rings and this correspondence does not
    // survive to the loft.
    let opts = LoftOptions {
        create_solid: true,
        vertex_correspondence: Some(vec![v0, v1]),
        ..Default::default()
    };

    let solid = loft_profiles(&mut m, vec![p0, p1], opts)
        .expect("loft with a supplied correspondence must build");

    let v = oracle(&mut m, solid);
    assert_closed_oriented_and_sound(&v, "supplied-correspondence square->offset square");

    let expected = SIDE * SIDE * HEIGHT;
    let rel = (v.volume - expected).abs() / expected;
    assert!(
        rel < 1e-3,
        "supplied-correspondence square->offset square: volume {:.6} is {:.4}% off the exact box {expected:.6}",
        v.volume,
        rel * 100.0
    );
}

/// An identity fingerprint for a built solid: outer-shell face count, the
/// bit-exact positions of every vertex it reaches (sorted, so store insertion
/// order cannot make two identical bodies look different), and the bit pattern
/// of its volume.
fn solid_fingerprint(model: &mut BRepModel, solid: SolidId) -> (usize, Vec<[u64; 3]>, u64) {
    let faces = model
        .solids
        .get(solid)
        .and_then(|s| model.shells.get(s.outer_shell))
        .map(|sh| sh.faces.len())
        .expect("solid outer shell");

    let mut verts: Vec<[u64; 3]> = shell_edges(model, solid)
        .iter()
        .filter_map(|&eid| model.edges.get(eid))
        .flat_map(|e| [e.start_vertex, e.end_vertex])
        .filter_map(|v| model.vertices.get(v))
        .map(|v| {
            [
                v.position[0].to_bits(),
                v.position[1].to_bits(),
                v.position[2].to_bits(),
            ]
        })
        .collect();
    verts.sort_unstable();
    verts.dedup();

    let volume = model
        .calculate_solid_volume(solid)
        .expect("solid volume")
        .to_bits();
    (faces, verts, volume)
}

/// `LoftType::MinimalTwist` is now redundant by construction.
///
/// Its whole content was a per-pair search for the cyclic shift minimising
/// total squared rail length. `register_correspondence` runs that same
/// objective -- over a strictly larger candidate set, with the same
/// previous-ring chaining and the same strict `<` tie-break -- on the way in,
/// for every loft type. So by the time `create_minimal_twist_loft` is reached
/// its search can only re-find shift 0, and `MinimalTwist` must produce a body
/// bit-identical to `Linear` on the same input.
///
/// That is the property this test pins, and it is what made removing the
/// vestigial search safe: the identity held with the search in place and holds
/// with it gone. A dissimilar, index-offset pair is used because it is the
/// input on which an un-registered loft and a registered one differ most.
#[test]
fn minimal_twist_loft_is_bit_identical_to_linear() {
    const R: f64 = 8.0;
    const SIDE: f64 = 12.0;
    const HEIGHT: f64 = 15.0;

    let build = |loft_type: LoftType| -> (BRepModel, SolidId) {
        let mut m = BRepModel::new();
        let p0 = polygon_profile(&mut m, &triangle_corners(R, 0.0));
        let p1 = polygon_profile(&mut m, &square_corners(SIDE, HEIGHT, 1));
        let opts = LoftOptions {
            create_solid: true,
            loft_type,
            ..Default::default()
        };
        let s = loft_profiles(&mut m, vec![p0, p1], opts).expect("loft must build");
        (m, s)
    };

    let (mut ml, sl) = build(LoftType::Linear);
    let (mut mt, st) = build(LoftType::MinimalTwist);

    let linear = solid_fingerprint(&mut ml, sl);
    let twist = solid_fingerprint(&mut mt, st);

    eprintln!(
        "linear: faces={} verts={} volume={}",
        linear.0,
        linear.1.len(),
        f64::from_bits(linear.2)
    );
    eprintln!(
        "minimal-twist: faces={} verts={} volume={}",
        twist.0,
        twist.1.len(),
        f64::from_bits(twist.2)
    );

    assert_eq!(
        linear.0, twist.0,
        "face counts differ: Linear {} vs MinimalTwist {}",
        linear.0, twist.0
    );
    assert_eq!(
        linear.1, twist.1,
        "vertex position sets are not bit-identical between Linear and MinimalTwist"
    );
    assert_eq!(
        linear.2,
        twist.2,
        "volumes are not bit-identical: Linear {} vs MinimalTwist {}",
        f64::from_bits(linear.2),
        f64::from_bits(twist.2)
    );
}
