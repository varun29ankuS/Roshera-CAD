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
//! A section that is NOT CONVEX breaks a second rule, and for a different
//! reason. Each lateral quad used to pick its `FaceOrientation` from the radial
//! direction out of the midpoint of the two RING CENTROIDS, and a ring centroid
//! is the mean of the ring samples -- a point that need not lie inside the
//! material at all. On a thin L it lands in the notch, so the quads along the
//! two notch edges face away from it and get flagged opposite to every other
//! quad in their own band. The band is coherent only when every one of its faces
//! carries the same flag, so this either refuses on the way out or, when the
//! band also folds and the surface normal flips with it, mints a mesh that
//! closes without being consistently wound. Orientation is now taken from the
//! signed volume of the slab between the two rings -- an invariant, one decision
//! per band -- and `validate_lofted_solid` asks the welded mesh before the loft
//! returns.
//!
//! Nine fixtures pin it:
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
//! * `loft_l_section_to_a_congruent_l_section_builds_the_exact_prism` — a
//!   NON-CONVEX section against itself. Congruent sections make the answer
//!   exact again (`V = area · height`) and registration a no-op, so the case
//!   measures band orientation and nothing else.
//! * `loft_l_section_whose_cap_the_mesh_cannot_orient_is_refused_not_minted` —
//!   the same prism from counter-clockwise sections. Its band flags are right
//!   and its cap tessellation is torn, and the loft refuses instead of handing
//!   out a body whose certificate would read `oriented=false`.
//! * `loft_l_section_to_a_square_is_closed_oriented_and_sound` — a non-convex
//!   section against a dissimilar convex one: registration and band orientation
//!   have to hold at the same time.
//! * `closed_loft_of_rotated_squares_has_the_analytic_ring_volume` — the only
//!   `closed: true` loft in the repo. Every ruled patch of it is planar, so the
//!   body is a polyhedral annulus whose volume is `4·N·R·a²·sin(2π/N)`, and the
//!   seam band that closes ring `N-1` back onto ring `0` is inside that number.

use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Point3, Tolerance};
use geometry_engine::operations::loft::LoftType;
use geometry_engine::operations::{loft_profiles, CommonOptions, LoftOptions};
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

// ---------------------------------------------------------------------------
// Non-convex sections: the per-quad radial heuristic
// ---------------------------------------------------------------------------

/// Thickness of the L-section's two arms.
const L_THICK: f64 = 2.0;
/// Length of the L-section's arm along X.
const L_ARM_X: f64 = 24.0;
/// Length of the L-section's arm along Y. Different from `L_ARM_X` on purpose:
/// an L with equal arms is its own mirror image across `y = x`, and a ring that
/// maps onto itself under a reversal of traversal sense is a degenerate input to
/// `register_correspondence`'s min-rail search (see the report's findings). This
/// fixture is about band ORIENTATION and has no business also standing on that.
const L_ARM_Y: f64 = 14.0;

/// A thin L-shaped (NON-CONVEX) section at height `z`, counter-clockwise seen
/// from +Z.
///
/// The reflex corner at `(L_THICK, L_THICK)` is the whole point: the ring
/// centroid -- the mean of the ring samples, which is what
/// `create_ruled_surfaces_between_profiles` anchored its radial test to -- falls
/// in the NOTCH, outside the material. The two notch edges (`y = L_THICK` and
/// `x = L_THICK`) therefore face AWAY from it, and the radial test picks the
/// opposite flag for those quads than for the outer ones.
fn l_corners(z: f64) -> Vec<Point3> {
    vec![
        Point3::new(0.0, 0.0, z),
        Point3::new(L_ARM_X, 0.0, z),
        Point3::new(L_ARM_X, L_THICK, z),
        Point3::new(L_THICK, L_THICK, z),
        Point3::new(L_THICK, L_ARM_Y, z),
        Point3::new(0.0, L_ARM_Y, z),
    ]
}

/// The same L-section listed CLOCKWISE.
fn l_corners_cw(z: f64) -> Vec<Point3> {
    let mut c = l_corners(z);
    c.reverse();
    c
}

/// Area of the L-section: the bounding rectangle less the notch rectangle.
fn l_area() -> f64 {
    L_ARM_X * L_ARM_Y - (L_ARM_X - L_THICK) * (L_ARM_Y - L_THICK)
}

/// Every LATERAL face of a two-section loft: its loop vertex positions and the
/// face's ORIENTED outward normal -- the surface's intrinsic normal at the
/// parametric midpoint times the face's orientation sign, which is the exact
/// product `orient_face_for_outward` decides and the tessellator winds
/// triangles to. Cap faces (every vertex at one height) are skipped.
fn lateral_faces(model: &BRepModel, solid: SolidId) -> Vec<(Vec<Point3>, Point3)> {
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
        let Some(lp) = model.loops.get(face.outer_loop) else {
            continue;
        };
        let Ok(vids) = lp.vertices(&model.edges) else {
            continue;
        };
        let pts: Vec<Point3> = vids
            .iter()
            .filter_map(|&v| model.vertices.get(v))
            .map(|v| Point3::from(v.position))
            .collect();
        if pts.len() != vids.len() || pts.len() < 3 {
            continue;
        }
        let zmin = pts.iter().map(|p| p.z).fold(f64::INFINITY, f64::min);
        let zmax = pts.iter().map(|p| p.z).fold(f64::NEG_INFINITY, f64::max);
        if zmax - zmin < 1e-9 {
            continue;
        }
        let Some(surface) = model.surfaces.get(face.surface_id) else {
            continue;
        };
        let ((u0, u1), (v0, v1)) = surface.parameter_bounds();
        let Ok(n) = surface.normal_at(0.5 * (u0 + u1), 0.5 * (v0 + v1)) else {
            continue;
        };
        out.push((pts, n * face.orientation.sign()));
    }
    out
}

/// The mean of the solid's distinct vertices at height `z` -- the same quantity
/// `ring_centroid` computes for a loft ring, read back off the built solid.
fn ring_centroid_at(model: &BRepModel, solid: SolidId, z: f64) -> Point3 {
    let mut ids: Vec<u32> = shell_edges(model, solid)
        .iter()
        .filter_map(|&e| model.edges.get(e))
        .flat_map(|e| [e.start_vertex, e.end_vertex])
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let pts: Vec<Point3> = ids
        .iter()
        .filter_map(|&v| model.vertices.get(v))
        .map(|v| Point3::from(v.position))
        .filter(|p| (p.z - z).abs() < 1e-6)
        .collect();
    assert!(!pts.is_empty(), "no ring vertices at z={z}");
    let n = pts.len() as f64;
    pts.iter().fold(Point3::ORIGIN, |a, p| a + *p) * (1.0 / n)
}

/// How many of the loft's lateral faces carry a flag the per-quad RADIAL
/// heuristic would NOT have picked, out of how many lateral faces there are.
///
/// The heuristic is `dot(oriented_normal, quad_centroid - axis_mid) >= 0` with
/// `axis_mid` the midpoint of the two ring centroids -- exactly the reference
/// `create_ruled_surfaces_between_profiles` anchored to. A disagreement count
/// of zero means the fixture cannot discriminate: whatever the shipped code
/// decided, the heuristic would have decided the same, and the case proves
/// nothing about band orientation.
fn radial_heuristic_disagreements(
    model: &BRepModel,
    solid: SolidId,
    z0: f64,
    z1: f64,
) -> (usize, usize, Point3) {
    let axis_mid = (ring_centroid_at(model, solid, z0) + ring_centroid_at(model, solid, z1)) * 0.5;
    let faces = lateral_faces(model, solid);
    let total = faces.len();
    let disagreeing = faces
        .iter()
        .filter(|(pts, outward)| {
            let n = pts.len() as f64;
            let centroid = pts.iter().fold(Point3::ORIGIN, |a, p| a + *p) * (1.0 / n);
            (centroid - axis_mid).dot(outward) < 0.0
        })
        .count();
    (disagreeing, total, axis_mid)
}

/// A thin L-section lofted to a CONGRUENT L-section directly above it.
///
/// The isolating fixture for band orientation, and the counterpart of
/// `loft_index_offset_squares_builds_an_exact_box` for a NON-CONVEX section:
/// congruent sections listed the same way make the correct answer exact -- the
/// body is the L-prism and its volume is `area * height` with no discretisation
/// term, because every ring sample lands on a straight section edge and a chord
/// reproduces a line exactly. Registration is a no-op on this input (the rings
/// are identical up to their height, so shift 0 unreversed is the strict
/// minimum), which is what leaves band ORIENTATION as the only thing the case
/// can fail on.
///
/// What a convex section cannot show and this does: the radial-outward test
/// that used to pick each lateral quad `FaceOrientation` was anchored at the
/// midpoint of the two RING CENTROIDS, and for an L that point sits in the notch
/// -- outside the material. The quads on the two notch edges face away from it,
/// so the radial test flagged them opposite to every other quad in the same
/// band, and a band whose faces disagree about which way is out cannot mint.
///
/// The section is listed CLOCKWISE, and that is load-bearing: the
/// counter-clockwise listing of the same L builds the same correctly oriented
/// B-Rep and then TEARS in the tessellator non-convex planar cap, which is a
/// different defect in a different subsystem. It has its own test immediately
/// below, so this one can measure band orientation without also standing on it.
#[test]
fn loft_l_section_to_a_congruent_l_section_builds_the_exact_prism() {
    const HEIGHT: f64 = 12.0;

    let mut m = BRepModel::new();
    let p0 = polygon_profile(&mut m, &l_corners_cw(0.0));
    let p1 = polygon_profile(&mut m, &l_corners_cw(HEIGHT));

    let solid = loft_profiles(&mut m, vec![p0, p1], loft_solid_opts())
        .expect("loft of two congruent L-sections must build");

    let (disagreeing, total, axis_mid) = radial_heuristic_disagreements(&m, solid, 0.0, HEIGHT);
    eprintln!(
        "L->congruent L: {disagreeing} of {total} lateral flags differ from the radial heuristic anchored at ({:.3}, {:.3}, {:.3})",
        axis_mid.x, axis_mid.y, axis_mid.z
    );

    let v = oracle(&mut m, solid);
    assert_closed_oriented_and_sound(&v, "L->congruent L");

    let expected = l_area() * HEIGHT;
    let rel = (v.volume - expected).abs() / expected;
    assert!(
        rel < 1e-3,
        "L->congruent L: volume {:.6} is {:.4}% off the exact prism {expected:.6}",
        v.volume,
        rel * 100.0
    );

    // The fixture must actually discriminate: if every lateral face agrees with
    // the radial heuristic, this case would pass just as well against the code
    // it exists to condemn.
    assert!(
        disagreeing > 0,
        "L->congruent L: all {total} lateral flags agree with the radial heuristic, so this fixture proves nothing"
    );
}

/// The same L-prism, its sections listed COUNTER-CLOCKWISE: every lateral flag
/// is still right, the mesh still cannot be oriented, and the loft REFUSES.
///
/// A section authored direction is a statement about the edge list, not about
/// the solid (`loft_opposite_winding_squares_builds_the_same_exact_box` pins
/// that for a square), so this input asks for the same 864-unit prism as the
/// case above. It does not get it -- and the reason is not the band. This test
/// measures both halves of that sentence:
///
/// * built with `validate_result` off, EVERY lateral face oriented outward
///   normal agrees with its own boundary-traversal normal, which is the property
///   the band-orientation fix is for. The notch faces included.
/// * built normally, the operation returns a typed error naming the mesh
///   orientation. The tessellator tears this L-shaped cap -- the last measured
///   symptom was 6 boundary edges and 2 inconsistently-directed edges, every one
///   of them on the cap face, while its 8 vertices and its B-Rep loop are intact
///   -- and a solid whose mesh cannot be oriented is exactly what the post-check
///   exists to keep from minting. Refusing is the honest answer; the alternative
///   is a certificate that reads `oriented=false` on a body the kernel handed
///   out anyway.
///
/// When the cap tessellation is fixed, this test goes red by BUILDING, and its
/// replacement is the same assertions as the case above.
#[test]
fn loft_l_section_whose_cap_the_mesh_cannot_orient_is_refused_not_minted() {
    const HEIGHT: f64 = 12.0;

    // Half one: the band flags are right.
    let mut m = BRepModel::new();
    let p0 = polygon_profile(&mut m, &l_corners(0.0));
    let p1 = polygon_profile(&mut m, &l_corners(HEIGHT));
    let unvalidated = LoftOptions {
        create_solid: true,
        common: CommonOptions {
            validate_result: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let solid = loft_profiles(&mut m, vec![p0, p1], unvalidated)
        .expect("the unvalidated L-section loft must still build");

    let faces = lateral_faces(&m, solid);
    assert!(
        faces.len() >= 8,
        "expected at least 8 lateral faces, got {}",
        faces.len()
    );
    for (pts, outward) in &faces {
        let mut traversal = Point3::ORIGIN;
        for k in 0..pts.len() {
            traversal += pts[k].cross(&pts[(k + 1) % pts.len()]);
        }
        assert!(
            traversal.dot(outward) > 0.0,
            "a lateral outward normal ({:.3},{:.3},{:.3}) opposes its own traversal normal ({:.3},{:.3},{:.3})",
            outward.x,
            outward.y,
            outward.z,
            traversal.x,
            traversal.y,
            traversal.z
        );
    }
    let (disagreeing, total, axis_mid) = radial_heuristic_disagreements(&m, solid, 0.0, HEIGHT);
    eprintln!(
        "L->L counter-clockwise: {disagreeing} of {total} lateral flags differ from the radial heuristic anchored at ({:.3}, {:.3}, {:.3})",
        axis_mid.x, axis_mid.y, axis_mid.z
    );
    assert!(
        disagreeing > 0,
        "L->L counter-clockwise: all {total} lateral flags agree with the radial heuristic, so this fixture proves nothing"
    );

    // Half two: the validated loft refuses rather than minting it.
    let mut m2 = BRepModel::new();
    let q0 = polygon_profile(&mut m2, &l_corners(0.0));
    let q1 = polygon_profile(&mut m2, &l_corners(HEIGHT));
    let err = loft_profiles(&mut m2, vec![q0, q1], loft_solid_opts())
        .expect_err("a loft whose mesh cannot be oriented must not mint");
    let msg = format!("{err}");
    assert!(
        msg.contains("not a consistently oriented mesh"),
        "the refusal must name the mesh orientation, got: {msg}"
    );
    assert!(!msg.contains("  "), "refusal has a double space: {msg}");
}

/// A thin L-section lofted to the rectangle that bounds it.
///
/// The brief's case: dissimilar sections, one of them non-convex. Both the
/// registration Task 42 added and the band orientation this task adds have to
/// hold at once -- the rings differ in size (6 corners against 4) AND the
/// bottom ring's centroid falls outside its own material.
#[test]
fn loft_l_section_to_a_square_is_closed_oriented_and_sound() {
    const HEIGHT: f64 = 12.0;

    let mut m = BRepModel::new();
    let p0 = polygon_profile(&mut m, &l_corners(0.0));
    let p1 = polygon_profile(
        &mut m,
        &[
            Point3::new(0.0, 0.0, HEIGHT),
            Point3::new(L_ARM_X, 0.0, HEIGHT),
            Point3::new(L_ARM_X, L_ARM_Y, HEIGHT),
            Point3::new(0.0, L_ARM_Y, HEIGHT),
        ],
    );

    let solid = loft_profiles(&mut m, vec![p0, p1], loft_solid_opts())
        .expect("loft of an L-section to a square must build");

    let (disagreeing, total, axis_mid) = radial_heuristic_disagreements(&m, solid, 0.0, HEIGHT);
    eprintln!(
        "L->square: {disagreeing} of {total} lateral flags differ from the radial heuristic anchored at ({:.3}, {:.3}, {:.3})",
        axis_mid.x, axis_mid.y, axis_mid.z
    );

    let v = oracle(&mut m, solid);
    assert_closed_oriented_and_sound(&v, "L->square");

    assert!(
        disagreeing > 0,
        "L->square: all {total} lateral flags agree with the radial heuristic, so this fixture proves nothing"
    );
}

/// A CLOSED loft: `N` congruent squares standing in radial planes around the
/// Z axis, each listed from a different corner.
///
/// `closed: true` adds the SEAM band, ring `N-1` against ring `0`, and ring 0
/// is the anchor registration never re-indexes. Nothing else in the repo builds
/// a closed loft, and this one's volume is exactly computable: every ruled patch
/// of the body is PLANAR (the top and bottom patches lie in `z = +a` and
/// `z = -a`; the inner and outer wall patches each span two parallel vertical
/// lines), so the solid is the polyhedral annulus between two regular `N`-gons
/// of circumradius `R + a` and `R - a`, extruded through `2a`:
///
/// `V = 2a * (N/2) * sin(2*pi/N) * ((R+a)^2 - (R-a)^2) = 4*N*R*a^2*sin(2*pi/N)`.
#[test]
fn closed_loft_of_rotated_squares_has_the_analytic_ring_volume() {
    const N: usize = 8;
    const R: f64 = 10.0;
    const A: f64 = 2.0;

    let mut m = BRepModel::new();
    let mut profiles = Vec::with_capacity(N);
    for k in 0..N {
        let theta = std::f64::consts::TAU * (k as f64) / (N as f64);
        let (c, s) = (theta.cos(), theta.sin());
        // Local (radial, z) corners of the square section, counter-clockwise in
        // the section plane; started at corner `k % 4` so the sections do not
        // share an index origin.
        let local = [(R - A, -A), (R + A, -A), (R + A, A), (R - A, A)];
        let corners: Vec<Point3> = (0..4)
            .map(|i| {
                let (rho, z) = local[(i + k) % 4];
                Point3::new(rho * c, rho * s, z)
            })
            .collect();
        profiles.push(polygon_profile(&mut m, &corners));
    }

    let opts = LoftOptions {
        create_solid: true,
        closed: true,
        ..Default::default()
    };
    let solid = loft_profiles(&mut m, profiles, opts).expect("closed loft of squares must build");

    let v = oracle(&mut m, solid);
    assert_closed_oriented_and_sound(&v, "closed loft of rotated squares");

    let expected = 4.0 * (N as f64) * R * A * A * (std::f64::consts::TAU / (N as f64)).sin();
    let rel = (v.volume - expected).abs() / expected;
    assert!(
        rel < 1e-3,
        "closed loft of rotated squares: volume {:.6} is {:.4}% off the analytic ring {expected:.6}",
        v.volume,
        rel * 100.0
    );
}

// ---------------------------------------------------------------------------
// The seam band of a closed loft
// ---------------------------------------------------------------------------

/// Vertices of `solid` lying in the radial half-plane at azimuth `theta`,
/// as `(vertex id, position)`.
fn radial_plane_vertices(model: &BRepModel, solid: SolidId, theta: f64) -> Vec<(u32, Point3)> {
    let mut ids: Vec<u32> = shell_edges(model, solid)
        .iter()
        .filter_map(|&e| model.edges.get(e))
        .flat_map(|e| [e.start_vertex, e.end_vertex])
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids.iter()
        .filter_map(|&v| model.vertices.get(v).map(|p| (v, Point3::from(p.position))))
        .filter(|(_, p)| {
            let mut d = p.y.atan2(p.x) - theta;
            while d > std::f64::consts::PI {
                d -= std::f64::consts::TAU;
            }
            while d < -std::f64::consts::PI {
                d += std::f64::consts::TAU;
            }
            d.abs() < 1e-9
        })
        .collect()
}

/// A CLOSED loft whose correspondence has HOLONOMY: the turn the chain
/// accumulates around the ring lands entirely on the seam band.
///
/// `N` congruent squares stand in radial planes about Z at `2*pi*k/N`, and
/// section `k` is additionally rotated `(90/N) degrees * k` IN ITS OWN PLANE.
/// Every consecutive pair therefore sits `90/N = 11.25` degrees apart, which the
/// registration chain resolves at shift 0 -- it has nothing to undo. By ring
/// `N-1` the accumulated in-plane turn is `7 * 11.25 = 78.75` degrees, and the
/// SEAM band pairs that ring against ring 0 at 0 degrees. A square is symmetric
/// under a quarter turn, so the honest seam pairing is the one that treats
/// `78.75` as `90 - 11.25`: it shifts by one quarter turn of the ring's samples
/// and leaves each seam rail 11.25 degrees short. Ring 0's OWN indexing -- what
/// the seam band used before this task -- pairs at `78.75` degrees and builds a
/// band twisted by seven eighths of a quarter turn.
///
/// This is the production witness for the seam relabeling: nothing else in the
/// suite produces a non-identity seam permutation (the closed ring of
/// `closed_loft_of_rotated_squares_has_the_analytic_ring_volume` has no
/// holonomy, so its seam permutation is the identity and is its own inverse --
/// which is why applying the relabeling backwards cannot be caught there).
///
/// The seam assertion is independent of the loft's pairing rule: it measures
/// the in-plane angle each built seam rail spans between ring `N-1` and ring 0
/// and bounds it below the next available shift, from the fixture's own
/// geometry. A seam twisted by 78.75 degrees violates that by a wide margin,
/// and the check never consults `best_relabeling`.
#[test]
fn closed_loft_of_a_twisted_ring_registers_its_seam_band() {
    const N: usize = 8;
    const R: f64 = 10.0;
    const A: f64 = 2.0;

    let rho = A * std::f64::consts::SQRT_2;
    let twist = std::f64::consts::FRAC_PI_2 / (N as f64);
    let section = |k: usize| -> Vec<Point3> {
        let theta = std::f64::consts::TAU * (k as f64) / (N as f64);
        (0..4)
            .map(|j| {
                let phi = std::f64::consts::FRAC_PI_4
                    + (j as f64) * std::f64::consts::FRAC_PI_2
                    + (k as f64) * twist;
                let (u, v) = (rho * phi.cos(), rho * phi.sin());
                Point3::new((R + u) * theta.cos(), (R + u) * theta.sin(), v)
            })
            .collect()
    };

    let mut m = BRepModel::new();
    let profiles: Vec<Vec<EdgeId>> = (0..N)
        .map(|k| polygon_profile(&mut m, &section(k)))
        .collect();

    let opts = LoftOptions {
        create_solid: true,
        closed: true,
        ..Default::default()
    };
    let solid = loft_profiles(&mut m, profiles, opts).expect("closed twisted-ring loft must build");

    let v = oracle(&mut m, solid);
    assert_closed_oriented_and_sound(&v, "closed twisted ring");

    // The seam band: ring N-1 against ring 0.
    let last_theta = std::f64::consts::TAU * ((N - 1) as f64) / (N as f64);
    let ring_last = radial_plane_vertices(&m, solid, last_theta);
    let ring_first = radial_plane_vertices(&m, solid, 0.0);
    assert!(
        ring_last.len() >= 4 && ring_last.len() == ring_first.len(),
        "seam rings read back as {} and {} samples",
        ring_last.len(),
        ring_first.len()
    );

    // Every seam rail must join two samples whose IN-PLANE offset angles agree
    // to within the residual turn. Each ring's 8 densified samples sit at
    // in-plane angles 45 degrees apart (4 corners at 45 + 90j, 4 edge midpoints
    // at 90j), so a seam pairing shifted by `s` samples leaves a residual of
    // `78.75 - 45*s` degrees: 78.75 at shift 0 (ring 0's own indexing, what the
    // seam band used before this task), 33.75 at shift 1, and -11.25 at shift 2,
    // which is the honest one. The bound below sits between 11.25 and 33.75, so
    // it admits only the honest pairing, and it is computed from the fixture's
    // own geometry rather than from anything the loft did.
    const RESIDUAL_BOUND_DEG: f64 = 20.0;
    let in_plane_angle = |p: Point3| -> f64 {
        let radius = (p.x * p.x + p.y * p.y).sqrt();
        p.z.atan2(radius - R)
    };
    let wrap_deg = |a: f64| -> f64 {
        let mut d = a.to_degrees() % 360.0;
        while d > 180.0 {
            d -= 360.0;
        }
        while d < -180.0 {
            d += 360.0;
        }
        d
    };

    let last_ids: std::collections::HashMap<u32, Point3> = ring_last.iter().copied().collect();
    let first_ids: std::collections::HashMap<u32, Point3> = ring_first.iter().copied().collect();
    let mut residuals: Vec<f64> = Vec::new();
    let mut worst_rail = 0.0f64;
    for &eid in &shell_edges(&m, solid) {
        let Some(e) = m.edges.get(eid) else { continue };
        let (a, b) = (e.start_vertex, e.end_vertex);
        let pair = match (last_ids.get(&a), first_ids.get(&b)) {
            (Some(&pa), Some(&pb)) => Some((pa, pb)),
            _ => match (last_ids.get(&b), first_ids.get(&a)) {
                (Some(&pa), Some(&pb)) => Some((pa, pb)),
                _ => None,
            },
        };
        let Some((pa, pb)) = pair else { continue };
        residuals.push(wrap_deg(in_plane_angle(pa) - in_plane_angle(pb)));
        worst_rail = worst_rail.max((pa - pb).magnitude());
    }
    let worst_residual = residuals.iter().fold(0.0f64, |w, r| w.max(r.abs()));
    // Printed BEFORE the assertions, so a failing build reports its numbers too.
    eprintln!(
        "closed twisted ring: {} seam rails, worst in-plane residual {worst_residual:.3} deg, longest rail {worst_rail:.3}, volume {:.6}",
        residuals.len(),
        v.volume
    );
    for residual in &residuals {
        assert!(
            residual.abs() <= RESIDUAL_BOUND_DEG,
            "seam rail joins in-plane angles {residual:.3} degrees apart, past the {RESIDUAL_BOUND_DEG} degree bound: the seam band carries the ring's accumulated turn"
        );
    }
    let rails = residuals.len();
    assert_eq!(
        rails,
        ring_last.len(),
        "every ring-{} sample must carry exactly one seam rail",
        N - 1
    );
}
