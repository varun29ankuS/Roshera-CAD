//! Adversarial near-degenerate fixture corpus — EXACT PREDICATES campaign
//! Slice 1 (spec `docs/superpowers/specs/2026-07-16-exact-predicates-design.md`,
//! Part 4 Slice 1: "an adversarial predicate-consumer fixture set …
//! near-collinear/near-coplanar boolean fixtures generated the way
//! `predicate_exactness_gate.rs` builds its sweeps").
//!
//! Two layers:
//!
//! 1. **2D predicate-consumer corpus** — polygons + query points engineered so
//!    the decision quantity (ray-crossing side, shoelace sign, segment-crossing
//!    orientation quad) is within a few ulps of zero: the regime where the raw
//!    f64 evaluations in the production call sites (census §2.3 rows #8/#10/#11)
//!    can return the wrong sign. Ground truth is NOT baked into the corpus —
//!    the consuming gate computes it with a `BigRational` oracle (every finite
//!    f64 is an exact dyadic rational).
//!
//! 2. **Solid-level near-degenerate builders** — boolean operand pairs whose
//!    separating quantities sit at a parameterized ε (coincident-within-ε
//!    planes, sliver walls, near-tangent cylinder/box) so the census gate can
//!    RECORD what the current pipeline does across the ε range that the
//!    tolerance census (spec §2.3/§2.4) shows is uncoordinated today.
//!
//! Consumed via `#[path = "common/adversarial.rs"]` by
//! `tests/adversarial_predicate_census.rs`; kept out of `common/mod.rs` so the
//! sketch-plate test binaries don't pay its compile cost.

// Test-support module: failing loudly at the fixture site is the desired
// failure mode; the workspace deny lints target production code.
#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(dead_code)]
// Corpus indexing is bounds-safe by construction (indices are `% len` or from
// `gen_range(0..len)`); a panic here is the desired test-failure mode anyway.
#![allow(clippy::indexing_slicing)]

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

// ─────────────────────────── 2D corpus ──────────────────────────────────────

/// One point-in-polygon probe: a simple polygon and a query point placed a few
/// ulps off one of its edges (the crossing-parity danger zone).
#[derive(Debug, Clone)]
pub struct PipCase {
    pub poly: Vec<(f64, f64)>,
    pub p: (f64, f64),
    /// Which corpus family produced it (for the census breakdown).
    pub family: &'static str,
}

/// One polygon-orientation probe (shoelace sign) with near-cancelling area.
#[derive(Debug, Clone)]
pub struct AreaSignCase {
    pub poly: Vec<(f64, f64)>,
    pub family: &'static str,
}

/// One proper-segment-crossing probe with a near-collinear orientation quad.
#[derive(Debug, Clone)]
pub struct SegCrossCase {
    pub a: (f64, f64),
    pub b: (f64, f64),
    pub c: (f64, f64),
    pub d: (f64, f64),
    pub family: &'static str,
}

/// Nudge `x` by `k` ulps (k may be negative). Exact: uses the f64 bit ladder.
pub fn ulps(x: f64, k: i64) -> f64 {
    let mut v = x;
    if k >= 0 {
        for _ in 0..k {
            v = next_up(v);
        }
    } else {
        for _ in 0..(-k) {
            v = next_down(v);
        }
    }
    v
}

fn next_up(x: f64) -> f64 {
    if x.is_nan() || x == f64::INFINITY {
        return x;
    }
    let bits = x.to_bits();
    let next = if x == 0.0 {
        1 // smallest positive subnormal
    } else if x > 0.0 {
        bits + 1
    } else {
        bits - 1
    };
    f64::from_bits(next)
}

fn next_down(x: f64) -> f64 {
    -next_up(-x)
}

/// A "dirty" (non-dyadic-nice) coordinate in `[-scale, scale]`.
fn dirty(rng: &mut StdRng, scale: f64) -> f64 {
    // Multiply two randoms so the mantissa is dense (a single `gen_range`
    // often lands on coarse dyadics near the range ends).
    rng.gen_range(-1.0..1.0) * rng.gen_range(0.5..1.0) * scale
}

/// A random simple star-shaped polygon (vertices sorted by angle around a
/// centre — always simple, arbitrary convexity) with dirty coordinates.
fn star_polygon(rng: &mut StdRng, n: usize, scale: f64) -> Vec<(f64, f64)> {
    let cx = dirty(rng, scale * 0.2);
    let cy = dirty(rng, scale * 0.2);
    let mut angles: Vec<f64> = (0..n)
        .map(|_| rng.gen_range(0.0..std::f64::consts::TAU))
        .collect();
    angles.sort_by(|a, b| a.total_cmp(b));
    // Reject angle collisions (degenerate duplicate vertices).
    angles.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    angles
        .iter()
        .map(|&t| {
            let r = rng.gen_range(0.3..1.0) * scale;
            (cx + r * t.cos(), cy + r * t.sin())
        })
        .collect()
}

/// Family A — query points a few ulps off a random edge of a random simple
/// polygon. The crossing decision for that edge is then decided by bits the
/// division-based ray cast rounds away.
pub fn pip_edge_graze_corpus(count: usize, seed: u64) -> Vec<PipCase> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut out = Vec::with_capacity(count);
    while out.len() < count {
        let n_verts = rng.gen_range(3..9);
        let poly = star_polygon(&mut rng, n_verts, 10.0);
        if poly.len() < 3 {
            continue;
        }
        let n = poly.len();
        let i = rng.gen_range(0..n);
        let (ax, ay) = poly[i];
        let (bx, by) = poly[(i + 1) % n];
        // Point ON the segment's f64-evaluated chord at parameter t, then
        // perturbed by ±1..4 ulps in each coordinate independently.
        let t: f64 = rng.gen_range(0.05..0.95);
        let px = ax + t * (bx - ax);
        let py = ay + t * (by - ay);
        let p = (
            ulps(px, rng.gen_range(-4..=4)),
            ulps(py, rng.gen_range(-4..=4)),
        );
        out.push(PipCase {
            poly,
            p,
            family: "edge_graze",
        });
    }
    out
}

/// Family B — sliver triangles (height ~1e-13·scale) probed near the long
/// edge: the census §2.3 row-#10 configuration where a sliver's own boundary
/// decides containment of points that are far from degenerate in x but
/// razor-close in the crossing direction.
pub fn pip_sliver_corpus(count: usize, seed: u64) -> Vec<PipCase> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let x0 = dirty(&mut rng, 5.0);
        let y0 = dirty(&mut rng, 5.0);
        let len = rng.gen_range(1.0..20.0);
        let h: f64 = rng.gen_range(1e-14..1e-12);
        // Long thin triangle: (x0,y0) → (x0+len, y0+tiny_slope) → apex barely
        // above the base.
        let slope = rng.gen_range(-1e-13..1e-13);
        let poly = vec![
            (x0, y0),
            (x0 + len, y0 + slope),
            (x0 + len * rng.gen_range(0.2..0.8), y0 + h),
        ];
        let t: f64 = rng.gen_range(0.1..0.9);
        let p = (x0 + t * len, ulps(y0 + t * slope, rng.gen_range(-3..=3)));
        out.push(PipCase {
            poly,
            p,
            family: "sliver",
        });
    }
    out
}

/// Family C — the `is_point_in_face` arc-sampling configuration: a circle
/// polygonized at 24 samples/edge (the production constant) probed a few ulps
/// off one of its chords. Mirrors "points on/near arcs".
pub fn pip_arc_chord_corpus(count: usize, seed: u64) -> Vec<PipCase> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let cx = dirty(&mut rng, 3.0);
        let cy = dirty(&mut rng, 3.0);
        let r = rng.gen_range(0.5..15.0);
        let n = 24usize; // SAMPLES_PER_EDGE in `is_point_in_face`
        let phase = rng.gen_range(0.0..std::f64::consts::TAU);
        let poly: Vec<(f64, f64)> = (0..n)
            .map(|k| {
                let t = phase + std::f64::consts::TAU * (k as f64) / (n as f64);
                (cx + r * t.cos(), cy + r * t.sin())
            })
            .collect();
        let i = rng.gen_range(0..n);
        let (ax, ay) = poly[i];
        let (bx, by) = poly[(i + 1) % n];
        let t: f64 = rng.gen_range(0.1..0.9);
        let p = (
            ulps(ax + t * (bx - ax), rng.gen_range(-3..=3)),
            ulps(ay + t * (by - ay), rng.gen_range(-3..=3)),
        );
        out.push(PipCase {
            poly,
            p,
            family: "arc_chord",
        });
    }
    out
}

/// Shoelace-sign corpus: polygons whose signed area nearly cancels — a long
/// thin zigzag strip whose width is driven down to the last bits, plus
/// near-collinear triangles built exactly like `predicate_exactness_gate`'s
/// orient2d sweep (a point ON the line a→b nudged by a ±1e-13 perpendicular).
pub fn area_sign_corpus(count: usize, seed: u64) -> Vec<AreaSignCase> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut out = Vec::with_capacity(count);
    for k in 0..count {
        if k % 2 == 0 {
            // Near-collinear triangle (area within ulps of zero, sign delicate).
            let a = (dirty(&mut rng, 1.0), dirty(&mut rng, 1.0));
            let b = (dirty(&mut rng, 1.0), dirty(&mut rng, 1.0));
            let t: f64 = rng.gen_range(-1.0..2.0);
            let eps: f64 = rng.gen_range(-1.0..1.0) * 1e-14;
            let dx = b.0 - a.0;
            let dy = b.1 - a.1;
            let c = (a.0 + t * dx - eps * dy, a.1 + t * dy + eps * dx);
            out.push(AreaSignCase {
                poly: vec![a, b, c],
                family: "near_collinear_tri",
            });
        } else {
            // Thin quad: base segment and its ulp-shifted return path.
            let a = (dirty(&mut rng, 8.0), dirty(&mut rng, 8.0));
            let b = (dirty(&mut rng, 8.0), dirty(&mut rng, 8.0));
            let k1 = rng.gen_range(-3i64..=3);
            let k2 = rng.gen_range(-3i64..=3);
            let poly = vec![a, b, (b.0, ulps(b.1, k1)), (a.0, ulps(a.1, k2))];
            out.push(AreaSignCase {
                poly,
                family: "ulp_quad",
            });
        }
    }
    out
}

/// Proper-crossing corpus: segment pairs whose orientation quad has one or two
/// determinants within ulps of zero (endpoint of one segment placed nearly on
/// the other's carrier line).
pub fn seg_cross_corpus(count: usize, seed: u64) -> Vec<SegCrossCase> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let a = (dirty(&mut rng, 2.0), dirty(&mut rng, 2.0));
        let b = (dirty(&mut rng, 2.0), dirty(&mut rng, 2.0));
        let dx = b.0 - a.0;
        let dy = b.1 - a.1;
        // c strictly on one side; d placed nearly ON the line a→b so the
        // d1/d2 pair is the delicate one.
        let tc: f64 = rng.gen_range(0.2..0.8);
        let side: f64 = if rng.gen_bool(0.5) { 1.0 } else { -1.0 };
        let c = (
            a.0 + tc * dx - side * 0.5 * dy,
            a.1 + tc * dy + side * 0.5 * dx,
        );
        let td: f64 = rng.gen_range(0.2..0.8);
        let eps: f64 = rng.gen_range(-1.0..1.0) * 1e-15;
        let d = (a.0 + td * dx + eps * dy, a.1 + td * dy - eps * dx);
        out.push(SegCrossCase {
            a: c,
            b: d,
            c: a,
            d: b,
            family: "endpoint_on_carrier",
        });
    }
    out
}

// ───────────── Slice 3 corpus: circular order + area comparison ─────────────

/// One near-parallel direction pair for the angular-sort census (census row
/// #6): two direction vectors separated by a sub-ulp rotation, the regime
/// where `atan2` COLLIDES (returns bit-identical f64 angles) while the exact
/// cross sign still orders them.
#[derive(Debug, Clone, Copy)]
pub struct DirPairCase {
    pub u: (f64, f64),
    pub v: (f64, f64),
    pub family: &'static str,
}

/// Near-parallel direction pairs around a dirty base angle, with DIFFERENT
/// magnitudes (so the pair samples distinct points of the f64 lattice) and a
/// log-uniform sub-ulp angular separation.
pub fn dir_pair_corpus(count: usize, seed: u64) -> Vec<DirPairCase> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let theta: f64 = rng.gen_range(0.0..std::f64::consts::TAU);
        let r1: f64 = rng.gen_range(0.5..20.0) * rng.gen_range(0.5..1.0);
        let r2: f64 = rng.gen_range(0.5..20.0) * rng.gen_range(0.5..1.0);
        // log-uniform separation across the atan2-collision window
        let exp: f64 = rng.gen_range(-19.0..-15.9);
        let dtheta = 10.0_f64.powf(exp);
        let a2 = theta + dtheta;
        out.push(DirPairCase {
            u: (r1 * theta.cos(), r1 * theta.sin()),
            v: (r2 * a2.cos(), r2 * a2.sin()),
            family: "subulp_dir_pair",
        });
    }
    out
}

/// One polygon pair for the exact-|area|-comparison census (census row #8's
/// nesting ties): two polygons whose absolute areas differ by less than the
/// f64 shoelace can resolve.
#[derive(Debug, Clone)]
pub struct AreaPairCase {
    pub a: Vec<(f64, f64)>,
    pub b: Vec<(f64, f64)>,
    pub family: &'static str,
}

/// Near-equal-|area| polygon pairs: a star polygon against (even) its own
/// dirty-translated copy — translation preserves area mathematically, but the
/// translated coordinates round, leaving a truth-nonzero sub-rounding area
/// difference — or (odd) a copy with one vertex nudged a few ulps.
pub fn area_pair_corpus(count: usize, seed: u64) -> Vec<AreaPairCase> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut out = Vec::with_capacity(count);
    while out.len() < count {
        let n_verts = rng.gen_range(4..9);
        let a = star_polygon(&mut rng, n_verts, 8.0);
        if a.len() < 3 {
            continue;
        }
        if out.len() % 2 == 0 {
            let (dx, dy) = (dirty(&mut rng, 3.0), dirty(&mut rng, 3.0));
            let b: Vec<(f64, f64)> = a.iter().map(|&(x, y)| (x + dx, y + dy)).collect();
            out.push(AreaPairCase {
                a,
                b,
                family: "translated_twin",
            });
        } else {
            let mut b = a.clone();
            let k = rng.gen_range(0..b.len());
            b[k].0 = ulps(b[k].0, rng.gen_range(-3..=3));
            b[k].1 = ulps(b[k].1, rng.gen_range(-3..=3));
            out.push(AreaPairCase {
                a,
                b,
                family: "ulp_vertex_twin",
            });
        }
    }
    out
}

// ───────────── Slice 4 corpus: 3D plane sidedness + sliver tetrahedra ───────

/// One point-vs-plane probe: a plane carrier `(n, o)` (unit-normalized dirty
/// normal, dirty anchor), its `(n, d = fl(n·o))` form, and a query point
/// placed IN the plane's span with a tiny normal offset — the near-coplanar
/// regime where the raw f64 evaluation `(p − o)·n` (and `n·p − d`) can return
/// the wrong sign.
#[derive(Debug, Clone, Copy)]
pub struct PlaneEvalCase {
    pub n: (f64, f64, f64),
    pub o: (f64, f64, f64),
    pub d: f64,
    pub p: (f64, f64, f64),
    pub family: &'static str,
}

pub fn plane_eval_corpus(count: usize, seed: u64) -> Vec<PlaneEvalCase> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut out = Vec::with_capacity(count);
    while out.len() < count {
        let nr = [
            dirty(&mut rng, 1.0),
            dirty(&mut rng, 1.0),
            dirty(&mut rng, 1.0),
        ];
        let len = (nr[0] * nr[0] + nr[1] * nr[1] + nr[2] * nr[2]).sqrt();
        if len < 0.05 {
            continue;
        }
        let n = (nr[0] / len, nr[1] / len, nr[2] / len);
        let o = (
            dirty(&mut rng, 5.0),
            dirty(&mut rng, 5.0),
            dirty(&mut rng, 5.0),
        );
        // In-plane basis (float Gram-Schmidt — construction only).
        let helper = if n.0.abs() < 0.7 {
            (1.0, 0.0, 0.0)
        } else {
            (0.0, 1.0, 0.0)
        };
        let hd = helper.0 * n.0 + helper.1 * n.1 + helper.2 * n.2;
        let e1r = (
            helper.0 - hd * n.0,
            helper.1 - hd * n.1,
            helper.2 - hd * n.2,
        );
        let e1l = (e1r.0 * e1r.0 + e1r.1 * e1r.1 + e1r.2 * e1r.2).sqrt();
        let e1 = (e1r.0 / e1l, e1r.1 / e1l, e1r.2 / e1l);
        let e2 = (
            n.1 * e1.2 - n.2 * e1.1,
            n.2 * e1.0 - n.0 * e1.2,
            n.0 * e1.1 - n.1 * e1.0,
        );
        let s: f64 = rng.gen_range(-4.0..4.0);
        let t: f64 = rng.gen_range(-4.0..4.0);
        let w: f64 = rng.gen_range(-1.0..1.0) * 1e-14;
        let p = (
            ulps(o.0 + s * e1.0 + t * e2.0 + w * n.0, rng.gen_range(-2..=2)),
            ulps(o.1 + s * e1.1 + t * e2.1 + w * n.1, rng.gen_range(-2..=2)),
            ulps(o.2 + s * e1.2 + t * e2.2 + w * n.2, rng.gen_range(-2..=2)),
        );
        let d = n.0 * o.0 + n.1 * o.1 + n.2 * o.2;
        out.push(PlaneEvalCase {
            n,
            o,
            d,
            p,
            family: "near_coplanar_3d",
        });
    }
    out
}

/// One sliver tetrahedron: `d` an affine combination of (a, b, c) plus a tiny
/// per-coordinate nudge — the near-coplanar orient3d regime.
#[derive(Debug, Clone, Copy)]
pub struct TetraCase {
    pub a: (f64, f64, f64),
    pub b: (f64, f64, f64),
    pub c: (f64, f64, f64),
    pub d: (f64, f64, f64),
    pub family: &'static str,
}

pub fn sliver_tetra_corpus(count: usize, seed: u64) -> Vec<TetraCase> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let pt = |rng: &mut StdRng| (dirty(rng, 2.0), dirty(rng, 2.0), dirty(rng, 2.0));
        let a = pt(&mut rng);
        let b = pt(&mut rng);
        let c = pt(&mut rng);
        let s: f64 = rng.gen_range(-1.0..2.0);
        let t: f64 = rng.gen_range(-1.0..2.0);
        let nudge = 1e-14;
        let d = (
            a.0 + s * (b.0 - a.0) + t * (c.0 - a.0) + rng.gen_range(-1.0..1.0) * nudge,
            a.1 + s * (b.1 - a.1) + t * (c.1 - a.1) + rng.gen_range(-1.0..1.0) * nudge,
            a.2 + s * (b.2 - a.2) + t * (c.2 - a.2) + rng.gen_range(-1.0..1.0) * nudge,
        );
        out.push(TetraCase {
            a,
            b,
            c,
            d,
            family: "sliver_tetra",
        });
    }
    out
}

// ─────────────────────── solid-level builders ───────────────────────────────

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

pub fn box_at(m: &mut BRepModel, w: f64, h: f64, d: f64, tx: f64, ty: f64, tz: f64) -> SolidId {
    let s = match TopologyBuilder::new(m).create_box_3d(w, h, d).unwrap() {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    };
    if tx != 0.0 {
        translate(m, vec![s], Vector3::X, tx, TransformOptions::default()).expect("tx");
    }
    if ty != 0.0 {
        translate(m, vec![s], Vector3::Y, ty, TransformOptions::default()).expect("ty");
    }
    if tz != 0.0 {
        translate(m, vec![s], Vector3::Z, tz, TransformOptions::default()).expect("tz");
    }
    s
}

pub fn cylinder(
    m: &mut BRepModel,
    base: Point3,
    axis: Vector3,
    radius: f64,
    height: f64,
) -> SolidId {
    match TopologyBuilder::new(m)
        .create_cylinder_3d(base, axis, radius, height)
        .unwrap()
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

pub fn union(m: &mut BRepModel, a: SolidId, b: SolidId) -> SolidId {
    boolean_operation(m, a, b, BooleanOp::Union, BooleanOptions::default())
        .expect("union must complete")
}

pub fn difference(m: &mut BRepModel, a: SolidId, b: SolidId) -> SolidId {
    boolean_operation(m, a, b, BooleanOp::Difference, BooleanOptions::default())
        .expect("difference must complete")
}

/// The near-coincident-plane union family (census row #14 / the
/// `coincident-face-tolerance-gap` shape class): a 20×20×10 base plate with a
/// 10×20×10 upstand stacked on its top face (z=10 planes exactly coincident),
/// the upstand's +x lateral face offset by `eps` from being coplanar with the
/// base's +x face. At `eps = 0` the walls are exactly flush; small `eps`
/// probes the plane-coincidence vs vertex-weld disagreement band.
pub fn flush_upstand_union(m: &mut BRepModel, eps: f64) -> SolidId {
    // Base: x∈[-10,10], y∈[-10,10], z∈[0,10].
    let base = box_at(m, 20.0, 20.0, 10.0, 0.0, 0.0, 5.0);
    // Upstand: x∈[eps-10, eps], y∈[-10,10], z∈[10,20] — its +x face sits at
    // x = eps, i.e. 10-eps short of the base's +x face; its −x face sits at
    // eps-10, coplanar-within-eps with the base's −x face at −10.
    let upstand = box_at(m, 10.0, 20.0, 10.0, -5.0 + eps, 0.0, 15.0);
    union(m, base, upstand)
}

/// Sliver-wall union: a paper-thin (`thickness`) wall standing on the base
/// plate's top — every wall lateral pair is coincident-within-`thickness`.
pub fn sliver_wall_union(m: &mut BRepModel, thickness: f64) -> SolidId {
    let base = box_at(m, 20.0, 20.0, 10.0, 0.0, 0.0, 5.0);
    let wall = box_at(m, thickness, 12.0, 8.0, 0.0, 0.0, 14.0);
    union(m, base, wall)
}

/// Near-tangent cylinder∪box: the cylinder axis is parallel to the box's +x
/// face at distance `r − eps` from it, so the lateral surface grazes the face
/// plane within `eps` (the #86 near-tangency class, census "points on/near
/// arcs" in 3D).
pub fn near_tangent_cyl_union(m: &mut BRepModel, eps: f64) -> SolidId {
    let base = box_at(m, 20.0, 20.0, 10.0, 0.0, 0.0, 5.0);
    let r = 4.0;
    // Box +x face at x=10; axis at x = 10 - r + eps ⇒ cylinder pokes past the
    // face plane by eps (eps>0) or grazes short of it (eps<0).
    let cyl = cylinder(
        m,
        Point3::new(10.0 - r + eps, 0.0, 10.0),
        Vector3::Z,
        r,
        8.0,
    );
    union(m, base, cyl)
}
