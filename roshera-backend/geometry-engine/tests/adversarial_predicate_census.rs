// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
// Polygon indexing is `% len`-bounded; a panic is the desired failure mode.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::indexing_slicing)]

//! EXACT PREDICATES Slice 1 — the census gate.
//!
//! Runs the CURRENT pipeline's 2D predicate algorithms and near-degenerate
//! boolean fixtures over the adversarial corpus (`tests/common/adversarial.rs`)
//! and RECORDS what they do today, against a `BigRational` oracle (every
//! finite f64 is an exact dyadic rational — lossless ground truth). This is
//! measurement, not repair: spec Part 4 Slice 1 ("pinning today's misbehavior
//! … if we can't produce a failing case for a site, we say so in the ledger").
//!
//! Two kinds of teeth:
//! * Tests that PIN behavior that is correct today (exact predicates match the
//!   oracle on the corpus; sound ε rows of the solid sweeps stay sound).
//! * Census tests that prove the corpus REACHES the lying regime of the raw
//!   f64 algorithms (mirrored here verbatim from their production call sites,
//!   each annotated with file:line). The per-call-site regression teeth are
//!   the unit REDs at the real private functions inside
//!   `operations/boolean.rs` / `operations/section.rs` (`#[ignore]`d until the
//!   Slice-2 migration turns them green) — the mirrors below exist only to
//!   measure lie RATES per algorithm variant, which an integration test
//!   cannot do against private fns.
//!
//! Slice 2 flips the production call sites to the exact core; the
//! `exact_pip_matches_oracle_on_full_corpus` test then covers the shared
//! implementation the call sites delegate to.

#[path = "common/adversarial.rs"]
mod adversarial;

use adversarial::{
    area_sign_corpus, pip_arc_chord_corpus, pip_edge_graze_corpus, pip_sliver_corpus,
    seg_cross_corpus, AreaSignCase, PipCase, SegCrossCase,
};
use num_rational::BigRational;
use num_traits::Signed;

fn rat(x: f64) -> BigRational {
    BigRational::from_float(x).expect("finite f64 has an exact rational")
}

/// Exact orientation sign of (a, b, p) via rationals: sign of
/// `(a-p) × (b-p)`. 1 = CCW, −1 = CW, 0 = collinear.
fn orient_sign_rat(a: (f64, f64), b: (f64, f64), p: (f64, f64)) -> i32 {
    let det = (rat(a.0) - rat(p.0)) * (rat(b.1) - rat(p.1))
        - (rat(a.1) - rat(p.1)) * (rat(b.0) - rat(p.0));
    if det.is_positive() {
        1
    } else if det.is_negative() {
        -1
    } else {
        0
    }
}

/// Ground-truth even-odd point-in-polygon: the standard half-open +x ray cast
/// with the crossing side decided in exact rational arithmetic.
/// `None` ⇒ the point lies EXACTLY on a straddling edge's carrier line
/// (on-boundary: parity truth is convention-dependent, excluded from census).
fn pip_oracle(p: (f64, f64), poly: &[(f64, f64)]) -> Option<bool> {
    let n = poly.len();
    if n < 3 {
        return Some(false);
    }
    let mut inside = false;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        // Straddle test on raw f64 comparisons is already exact.
        if (a.1 > p.1) != (b.1 > p.1) {
            let s = orient_sign_rat(a, b, p);
            if s == 0 {
                return None; // exactly on the edge carrier within the band
            }
            let upward = b.1 > a.1;
            // Upward edge: crossing to the right of p iff p is strictly left
            // of a→b (CCW). Downward edge: iff strictly right (CW). Same
            // derivation as `polygon_clip::point_in_polygon` (:289).
            if (s > 0) == upward {
                inside = !inside;
            }
        }
    }
    Some(inside)
}

/// Ground-truth shoelace orientation sign via rationals.
fn shoelace_sign_rat(poly: &[(f64, f64)]) -> i32 {
    let n = poly.len();
    let mut acc = BigRational::from_float(0.0).expect("zero");
    for i in 0..n {
        let (x0, y0) = poly[i];
        let (x1, y1) = poly[(i + 1) % n];
        acc += rat(x0) * rat(y1) - rat(x1) * rat(y0);
    }
    if acc.is_positive() {
        1
    } else if acc.is_negative() {
        -1
    } else {
        0
    }
}

// ───────────── float mirrors of the production algorithms (census only) ─────

/// Mirror of `operations/boolean.rs::point_in_polygon_2d` (raw ray cast with
/// the `px < (xj-xi)*(py-yi)/(yj-yi) + xi` division) — also the algorithm of
/// `operations/section.rs::point_in_polygon` minus its 1e-18 denominator
/// guard, and of the two inline copies in `is_point_in_face`.
fn pip_float_v1(px: f64, py: f64, poly: &[(f64, f64)]) -> bool {
    if poly.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        let intersects = ((yi > py) != (yj > py)) && (px < (xj - xi) * (py - yi) / (yj - yi) + xi);
        if intersects {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Mirror of `operations/boolean.rs::uv_point_in_polygon` (t-parameter form of
/// the same division).
fn pip_float_v2(polygon: &[(f64, f64)], (u, v): (f64, f64)) -> bool {
    let n = polygon.len();
    if n < 3 {
        return false;
    }
    let mut crossings = 0u32;
    for i in 0..n {
        let (u1, v1) = polygon[i];
        let (u2, v2) = polygon[(i + 1) % n];
        if (v1 <= v && v2 > v) || (v2 <= v && v1 > v) {
            let t = (v - v1) / (v2 - v1);
            let u_cross = u1 + t * (u2 - u1);
            if u < u_cross {
                crossings += 1;
            }
        }
    }
    crossings % 2 == 1
}

/// Mirror of `operations/section.rs::point_in_polygon` (v1 + the #85b 1e-18
/// magnitude-with-sign denominator clamp).
fn pip_float_v3(p: (f64, f64), poly: &[(f64, f64)]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let (x, y) = p;
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        let dy = (yj - yi).abs().max(1e-18).copysign(yj - yi);
        let intersects = ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / dy + xi);
        if intersects {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Mirror of `operations/boolean.rs::segments_properly_intersect_2d` (raw f64
/// cross products).
fn seg_cross_float(p1: (f64, f64), p2: (f64, f64), p3: (f64, f64), p4: (f64, f64)) -> bool {
    let cross = |a: (f64, f64), b: (f64, f64), c: (f64, f64)| -> f64 {
        (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
    };
    let d1 = cross(p3, p4, p1);
    let d2 = cross(p3, p4, p2);
    let d3 = cross(p1, p2, p3);
    let d4 = cross(p1, p2, p4);
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

/// Ground-truth proper crossing via rational orientation signs (strictly
/// opposite on both quads; any collinearity ⇒ not a proper crossing, matching
/// the production function's documented contract).
fn seg_cross_oracle(c: &SegCrossCase) -> bool {
    let d1 = orient_sign_rat(c.c, c.d, c.a);
    let d2 = orient_sign_rat(c.c, c.d, c.b);
    let d3 = orient_sign_rat(c.a, c.b, c.c);
    let d4 = orient_sign_rat(c.a, c.b, c.d);
    (d1 * d2 == -1) && (d3 * d4 == -1)
}

fn full_pip_corpus() -> Vec<PipCase> {
    let mut corpus = pip_edge_graze_corpus(6000, 0xE1AC_7001);
    corpus.extend(pip_sliver_corpus(3000, 0xE1AC_7002));
    corpus.extend(pip_arc_chord_corpus(3000, 0xE1AC_7003));
    corpus
}

// ─────────────────────────── 2D census tests ────────────────────────────────

/// CENSUS: the raw ray-cast PIP algorithms shipped at the five §2.3-row-#10
/// call sites lie on the adversarial corpus. Records lie counts per variant
/// and per family; asserts the corpus actually reaches the lying regime (a
/// corpus that never trips the float paths would prove nothing).
#[test]
fn census_float_pip_lies_on_adversarial_corpus() {
    let corpus = full_pip_corpus();
    let mut decided = 0u64;
    let mut boundary = 0u64;
    let mut lies_v1 = 0u64;
    let mut lies_v2 = 0u64;
    let mut lies_v3 = 0u64;
    let mut first_v1: Option<(&'static str, PipCase, bool)> = None;
    let mut first_v2: Option<(&'static str, PipCase, bool)> = None;
    let mut first_v3: Option<(&'static str, PipCase, bool)> = None;

    for case in &corpus {
        let Some(truth) = pip_oracle(case.p, &case.poly) else {
            boundary += 1;
            continue;
        };
        decided += 1;
        let v1 = pip_float_v1(case.p.0, case.p.1, &case.poly);
        let v2 = pip_float_v2(&case.poly, case.p);
        let v3 = pip_float_v3(case.p, &case.poly);
        if v1 != truth {
            lies_v1 += 1;
            if first_v1.is_none() {
                first_v1 = Some((case.family, case.clone(), truth));
            }
        }
        if v2 != truth {
            lies_v2 += 1;
            if first_v2.is_none() {
                first_v2 = Some((case.family, case.clone(), truth));
            }
        }
        if v3 != truth {
            lies_v3 += 1;
            if first_v3.is_none() {
                first_v3 = Some((case.family, case.clone(), truth));
            }
        }
    }

    eprintln!(
        "[census] PIP corpus: {} cases ({} decided, {} exact-on-boundary excluded)",
        corpus.len(),
        decided,
        boundary
    );
    eprintln!(
        "[census] lies vs rational oracle: v1(boolean::point_in_polygon_2d)={} \
         v2(boolean::uv_point_in_polygon)={} v3(section::point_in_polygon)={}",
        lies_v1, lies_v2, lies_v3
    );
    for (name, first) in [("v1", &first_v1), ("v2", &first_v2), ("v3", &first_v3)] {
        if let Some((family, case, truth)) = first {
            eprintln!(
                "[census] first {name} lie (family={family}, truth={truth}): p={:?} poly={:?}",
                case.p, case.poly
            );
        }
    }

    assert!(
        decided > 8000,
        "corpus mostly excluded — generators degenerate"
    );
    assert!(
        lies_v1 + lies_v2 + lies_v3 > 0,
        "corpus never reached the lying regime of the float PIP algorithms — \
         tighten the ulp perturbations"
    );
}

/// CENSUS: the raw f64 shoelace sign lies on near-cancelling polygons — the
/// sign half of census rows #7/#8/#11.
#[test]
fn census_float_shoelace_sign_lies_on_adversarial_corpus() {
    let corpus = area_sign_corpus(8000, 0xA5EA_0001);
    let mut lies = 0u64;
    let mut first: Option<(&AreaSignCase, i32, i32)> = None;
    for case in &corpus {
        let truth = shoelace_sign_rat(&case.poly);
        // Mirror of the raw shoelace closures (boolean.rs `signed_area`,
        // section.rs `polygon_signed_area_2d`, tessellation/surface.rs copies).
        let mut a = 0.0f64;
        let n = case.poly.len();
        for i in 0..n {
            let (x0, y0) = case.poly[i];
            let (x1, y1) = case.poly[(i + 1) % n];
            a += x0 * y1 - x1 * y0;
        }
        let float_sign = if a > 0.0 {
            1
        } else if a < 0.0 {
            -1
        } else {
            0
        };
        if float_sign != truth {
            lies += 1;
            if first.is_none() {
                first = Some((case, float_sign, truth));
            }
        }
    }
    eprintln!(
        "[census] shoelace-sign corpus: {} cases, float-sign lies={}",
        corpus.len(),
        lies
    );
    if let Some((case, got, want)) = first {
        eprintln!(
            "[census] first shoelace lie (family={}, float={got}, exact={want}): {:?}",
            case.family, case.poly
        );
    }
    assert!(
        lies > 0,
        "corpus never reached the lying regime of the raw f64 shoelace sign"
    );
}

/// CENSUS: the raw f64 proper-crossing test lies on near-collinear quads —
/// census "segment-crossing existence" (spec §3.6 `segments_intersect_2d`).
#[test]
fn census_float_segment_crossing_lies_on_adversarial_corpus() {
    let corpus = seg_cross_corpus(8000, 0x5E6C_0001);
    let mut lies = 0u64;
    let mut first: Option<&SegCrossCase> = None;
    for case in &corpus {
        let truth = seg_cross_oracle(case);
        let got = seg_cross_float(case.a, case.b, case.c, case.d);
        if got != truth {
            lies += 1;
            if first.is_none() {
                first = Some(case);
            }
        }
    }
    eprintln!(
        "[census] segment-crossing corpus: {} cases, float lies={}",
        corpus.len(),
        lies
    );
    if let Some(case) = first {
        eprintln!(
            "[census] first crossing lie (truth={}, float={}): {case:?}",
            seg_cross_oracle(case),
            seg_cross_float(case.a, case.b, case.c, case.d)
        );
    }
    assert!(
        lies > 0,
        "corpus never reached the lying regime of the raw f64 crossing test"
    );
}

/// PIN (sanity floor): the in-tree exact predicates match the rational oracle
/// on every corpus case — the substrate Slice 2 migrates onto is itself sound
/// on exactly the inputs the pipeline will feed it.
#[test]
fn exact_orient2d_and_shoelace_match_oracle_on_corpus() {
    use geometry_engine::math::vector2::Vector2;
    use geometry_engine::math::{orient2d, signed_area_2d, Orientation};

    let sign_of = |o: Orientation| match o {
        Orientation::CounterClockwise => 1,
        Orientation::Clockwise => -1,
        Orientation::Collinear => 0,
    };

    for case in seg_cross_corpus(4000, 0x5E6C_0002) {
        for (a, b, p) in [
            (case.c, case.d, case.a),
            (case.c, case.d, case.b),
            (case.a, case.b, case.c),
            (case.a, case.b, case.d),
        ] {
            let want = orient_sign_rat(a, b, p);
            let got = sign_of(orient2d(
                &Vector2::new(a.0, a.1),
                &Vector2::new(b.0, b.1),
                &Vector2::new(p.0, p.1),
            ));
            assert_eq!(got, want, "orient2d vs oracle on ({a:?},{b:?},{p:?})");
        }
    }

    for case in area_sign_corpus(4000, 0xA5EA_0002) {
        let want = shoelace_sign_rat(&case.poly);
        let pts: Vec<Vector2> = case.poly.iter().map(|&(x, y)| Vector2::new(x, y)).collect();
        let got = sign_of(signed_area_2d(&pts));
        assert_eq!(
            got, want,
            "signed_area_2d vs oracle on {:?} (family {})",
            case.poly, case.family
        );
    }
}

/// SLICE-2 GREEN: the shared exact predicates the production call sites now
/// delegate to (`math::point_in_polygon_2d`, `segments_properly_intersect_2d`,
/// `polygon_orientation_2d`) match the rational oracle on the ENTIRE corpus —
/// zero mismatches, where the float algorithms lie hundreds of times (the
/// census above). This is the substrate proof for census rows #8/#10/#11.
#[test]
fn exact_pip_matches_oracle_on_full_corpus() {
    use geometry_engine::math::{
        point_in_polygon_2d, polygon_orientation_2d, segments_properly_intersect_2d, Orientation,
    };

    let mut checked = 0u64;
    for case in full_pip_corpus() {
        let Some(truth) = pip_oracle(case.p, &case.poly) else {
            continue; // exactly-on-boundary: parity is convention-owned
        };
        checked += 1;
        assert_eq!(
            point_in_polygon_2d(case.p.0, case.p.1, &case.poly),
            truth,
            "exact PIP vs oracle: p={:?} poly={:?} (family {})",
            case.p,
            case.poly,
            case.family
        );
    }
    assert!(
        checked > 8000,
        "corpus mostly excluded — generators degenerate"
    );

    for case in seg_cross_corpus(8000, 0x5E6C_0001) {
        assert_eq!(
            segments_properly_intersect_2d(case.a, case.b, case.c, case.d),
            seg_cross_oracle(&case),
            "exact proper-crossing vs oracle: {case:?}"
        );
    }

    let sign_of = |o: Orientation| match o {
        Orientation::CounterClockwise => 1,
        Orientation::Clockwise => -1,
        Orientation::Collinear => 0,
    };
    for case in area_sign_corpus(8000, 0xA5EA_0001) {
        assert_eq!(
            sign_of(polygon_orientation_2d(&case.poly)),
            shoelace_sign_rat(&case.poly),
            "exact polygon orientation vs oracle: {:?} (family {})",
            case.poly,
            case.family
        );
    }
}

// ───────────── Slice 3 census: circular order + |area| comparison ───────────

use adversarial::{area_pair_corpus, dir_pair_corpus, AreaPairCase, DirPairCase};
use std::cmp::Ordering as CmpOrd;

/// Ground-truth circular order (CCW from the +x axis) of two direction
/// vectors: the half-plane split is decided by exact f64 comparisons, the
/// within-half order by the RATIONAL cross sign.
fn circular_order_oracle(u: (f64, f64), v: (f64, f64)) -> CmpOrd {
    let half = |(x, y): (f64, f64)| -> u8 {
        if x == 0.0 && y == 0.0 {
            0
        } else if y > 0.0 || (y == 0.0 && x > 0.0) {
            1
        } else {
            2
        }
    };
    match half(u).cmp(&half(v)) {
        CmpOrd::Equal => {
            let cross = rat(u.0) * rat(v.1) - rat(u.1) * rat(v.0);
            if cross.is_positive() {
                CmpOrd::Less
            } else if cross.is_negative() {
                CmpOrd::Greater
            } else {
                CmpOrd::Equal
            }
        }
        o => o,
    }
}

/// CENSUS: the former DCEL angular-sort comparator — `atan2` keys compared
/// with `partial_cmp … NaN→Equal` (mirrored verbatim from the pre-Slice-3
/// `face_arrangement.rs` sort and the `boolean.rs` sphere-walker sort) —
/// COLLIDES on sub-ulp-separated direction pairs: it reports `Equal` where
/// the exact cross sign strictly orders the pair, silently handing the ring
/// order to an id-based tie-break. The exact `circular_order` must match the
/// rational oracle on every pair, both argument orders.
#[test]
fn census_atan2_angular_sort_collides_on_near_parallel_pairs() {
    use geometry_engine::math::circular_order;
    use geometry_engine::math::vector2::Vector2;

    let atan2_cmp = |u: (f64, f64), v: (f64, f64)| -> CmpOrd {
        let au = u.1.atan2(u.0);
        let av = v.1.atan2(v.0);
        au.partial_cmp(&av).unwrap_or(CmpOrd::Equal)
    };

    let corpus = dir_pair_corpus(20_000, 0xC1C0_1A12_0001);
    let mut collisions = 0u64;
    let mut strict = 0u64;
    let mut first: Option<DirPairCase> = None;
    for case in &corpus {
        let truth = circular_order_oracle(case.u, case.v);
        if truth != CmpOrd::Equal {
            strict += 1;
        }
        if truth != CmpOrd::Equal && atan2_cmp(case.u, case.v) == CmpOrd::Equal {
            collisions += 1;
            if first.is_none() {
                first = Some(*case);
            }
        }
        // The exact predicate must agree with the oracle in both orders.
        let vu = Vector2::new(case.u.0, case.u.1);
        let vv = Vector2::new(case.v.0, case.v.1);
        assert_eq!(
            circular_order(&vu, &vv),
            truth,
            "circular_order vs oracle on {case:?}"
        );
        assert_eq!(
            circular_order(&vv, &vu),
            truth.reverse(),
            "circular_order antisymmetry on {case:?}"
        );
    }
    eprintln!(
        "[census] dir-pair corpus: {} cases ({strict} strictly ordered), \
         atan2 collisions (float key Equal, truth strict) = {collisions}",
        corpus.len()
    );
    if let Some(case) = first {
        eprintln!("[census] first atan2 collision: {case:?}");
    }
    assert!(
        collisions > 0,
        "corpus never reached the atan2-collision regime — tighten dtheta"
    );
}

/// Ground-truth |shoelace| comparison via rationals.
fn abs_area_cmp_oracle(a: &[(f64, f64)], b: &[(f64, f64)]) -> CmpOrd {
    let abs_shoelace = |poly: &[(f64, f64)]| -> BigRational {
        let n = poly.len();
        if n < 3 {
            return BigRational::from_float(0.0).expect("zero");
        }
        let mut acc = BigRational::from_float(0.0).expect("zero");
        for i in 0..n {
            let (x0, y0) = poly[i];
            let (x1, y1) = poly[(i + 1) % n];
            acc += rat(x0) * rat(y1) - rat(x1) * rat(y0);
        }
        if acc.is_negative() {
            -acc
        } else {
            acc
        }
    };
    let (aa, ab) = (abs_shoelace(a), abs_shoelace(b));
    if aa > ab {
        CmpOrd::Greater
    } else if aa < ab {
        CmpOrd::Less
    } else {
        CmpOrd::Equal
    }
}

/// CENSUS: the former f64 |area| comparison (mirror of the pre-Slice-3
/// `boolean.rs` `signed_area` closure feeding the nesting ties, census row
/// #8) lies on near-equal polygon pairs; the exact `polygon_area_cmp_2d`
/// must match the rational oracle on every pair.
#[test]
fn census_float_abs_area_compare_lies_on_near_equal_pairs() {
    use geometry_engine::math::polygon_area_cmp_2d;

    let float_abs_area = |poly: &[(f64, f64)]| -> f64 {
        if poly.len() < 3 {
            return 0.0;
        }
        let mut a = 0.0;
        for i in 0..poly.len() {
            let (x0, y0) = poly[i];
            let (x1, y1) = poly[(i + 1) % poly.len()];
            a += x0 * y1 - x1 * y0;
        }
        (0.5 * a).abs()
    };

    let corpus = area_pair_corpus(6_000, 0xA2EA_9A12_0002);
    let mut lies = 0u64;
    let mut first: Option<&AreaPairCase> = None;
    for case in &corpus {
        let truth = abs_area_cmp_oracle(&case.a, &case.b);
        let float_verdict = float_abs_area(&case.a)
            .partial_cmp(&float_abs_area(&case.b))
            .unwrap_or(CmpOrd::Equal);
        if float_verdict != truth {
            lies += 1;
            if first.is_none() {
                first = Some(case);
            }
        }
        assert_eq!(
            polygon_area_cmp_2d(&case.a, &case.b),
            truth,
            "polygon_area_cmp_2d vs oracle (family {})",
            case.family
        );
    }
    eprintln!(
        "[census] area-pair corpus: {} cases, float |area| compare lies = {lies}",
        corpus.len()
    );
    if let Some(case) = first {
        eprintln!("[census] first area-compare lie (family={})", case.family);
    }
    assert!(
        lies > 0,
        "corpus never reached the lying regime of the f64 area comparison"
    );
}

// ───────────── Slice 4 census: 3D plane sidedness + sliver tetrahedra ───────

use adversarial::{plane_eval_corpus, sliver_tetra_corpus, PlaneEvalCase};

fn sign_to_ord(x: i32) -> CmpOrd {
    match x {
        1 => CmpOrd::Greater,
        -1 => CmpOrd::Less,
        _ => CmpOrd::Equal,
    }
}

/// Rational sign of `n·(p − o)`.
fn point_plane_oracle(c: &PlaneEvalCase) -> CmpOrd {
    let s = rat(c.n.0) * (rat(c.p.0) - rat(c.o.0))
        + rat(c.n.1) * (rat(c.p.1) - rat(c.o.1))
        + rat(c.n.2) * (rat(c.p.2) - rat(c.o.2));
    sign_to_ord(if s.is_positive() {
        1
    } else if s.is_negative() {
        -1
    } else {
        0
    })
}

/// Rational sign of `n·p − d` (the plane's (n, d) form; `d` itself is the
/// f64-rounded `n·o`, taken at face value as the represented plane).
fn plane_eval_oracle(c: &PlaneEvalCase) -> CmpOrd {
    let s = rat(c.n.0) * rat(c.p.0) + rat(c.n.1) * rat(c.p.1) + rat(c.n.2) * rat(c.p.2) - rat(c.d);
    sign_to_ord(if s.is_positive() {
        1
    } else if s.is_negative() {
        -1
    } else {
        0
    })
}

/// CENSUS: the raw f64 plane evaluations — `(p − o)·n` (mirror of the
/// pre-Slice-4 `on_kept_side` / `point_in_region` half-space tests and the
/// `surface_plane_intersection` grid dot) and `n·p − d` — lie on the
/// near-coplanar 3D corpus; the exact `point_plane_sidedness` and
/// `sign_of_plane_eval` must match the rational oracle on every case.
#[test]
fn census_float_plane_eval_sign_lies_on_near_coplanar_corpus() {
    use geometry_engine::math::{point_plane_sidedness, sign_of_plane_eval, Point3, Vector3};

    let corpus = plane_eval_corpus(12_000, 0x91A5_EE7A_0003);
    let mut lies_po = 0u64;
    let mut lies_nd = 0u64;
    let mut first: Option<&PlaneEvalCase> = None;
    for case in &corpus {
        let truth_po = point_plane_oracle(case);
        let truth_nd = plane_eval_oracle(case);

        // float mirrors
        let fo = case.n.0 * (case.p.0 - case.o.0)
            + case.n.1 * (case.p.1 - case.o.1)
            + case.n.2 * (case.p.2 - case.o.2);
        let fnd = case.n.0 * case.p.0 + case.n.1 * case.p.1 + case.n.2 * case.p.2 - case.d;
        let fo_ord = fo.partial_cmp(&0.0).unwrap_or(CmpOrd::Equal);
        let fnd_ord = fnd.partial_cmp(&0.0).unwrap_or(CmpOrd::Equal);
        if fo_ord != truth_po {
            lies_po += 1;
            if first.is_none() {
                first = Some(case);
            }
        }
        if fnd_ord != truth_nd {
            lies_nd += 1;
        }

        let n = Vector3::new(case.n.0, case.n.1, case.n.2);
        let o = Point3::new(case.o.0, case.o.1, case.o.2);
        let p = Point3::new(case.p.0, case.p.1, case.p.2);
        assert_eq!(
            point_plane_sidedness(&n, &o, &p),
            truth_po,
            "point_plane_sidedness vs oracle on {case:?}"
        );
        assert_eq!(
            sign_of_plane_eval(&n, case.d, &p),
            truth_nd,
            "sign_of_plane_eval vs oracle on {case:?}"
        );
    }
    eprintln!(
        "[census] plane-eval corpus: {} cases, float lies: (p−o)·n = {lies_po}, \
         n·p−d = {lies_nd}",
        corpus.len()
    );
    if let Some(case) = first {
        eprintln!("[census] first plane-eval lie: {case:?}");
    }
    assert!(
        lies_po + lies_nd > 0,
        "corpus never reached the lying regime of the raw f64 plane evals"
    );
}

/// CENSUS + PIN: sliver tetrahedra (near-coplanar quadruples). The naive f64
/// orient3d determinant lies; the exact `orient3d` matches the rational
/// oracle on the whole corpus (the same guarantee the predicate-exactness
/// gate proves on its own sweep — this row pins it on the census generator).
#[test]
fn census_orient3d_exact_on_sliver_tetrahedra() {
    use geometry_engine::math::{orient3d, Orientation, Point3};

    let corpus = sliver_tetra_corpus(12_000, 0x7E72_A512_0004);
    let mut naive_lies = 0u64;
    for case in &corpus {
        // Rational oracle, matching orient3d's sign convention (the negated
        // determinant, as in tests/predicate_exactness_gate.rs).
        let adx = rat(case.a.0) - rat(case.d.0);
        let ady = rat(case.a.1) - rat(case.d.1);
        let adz = rat(case.a.2) - rat(case.d.2);
        let bdx = rat(case.b.0) - rat(case.d.0);
        let bdy = rat(case.b.1) - rat(case.d.1);
        let bdz = rat(case.b.2) - rat(case.d.2);
        let cdx = rat(case.c.0) - rat(case.d.0);
        let cdy = rat(case.c.1) - rat(case.d.1);
        let cdz = rat(case.c.2) - rat(case.d.2);
        let s = &adx * (&bdy * &cdz - &bdz * &cdy)
            + &ady * (&bdz * &cdx - &bdx * &cdz)
            + &adz * (&bdx * &cdy - &bdy * &cdx);
        let truth = if s.is_positive() {
            -1
        } else if s.is_negative() {
            1
        } else {
            0
        };

        let (a, b, c, d) = (
            Point3::new(case.a.0, case.a.1, case.a.2),
            Point3::new(case.b.0, case.b.1, case.b.2),
            Point3::new(case.c.0, case.c.1, case.c.2),
            Point3::new(case.d.0, case.d.1, case.d.2),
        );
        let got = match orient3d(&a, &b, &c, &d) {
            Orientation::CounterClockwise => 1,
            Orientation::Clockwise => -1,
            Orientation::Collinear => 0,
        };
        assert_eq!(got, truth, "orient3d vs oracle on {case:?}");

        // Naive f64 mirror.
        let (fadx, fady, fadz) = (
            case.a.0 - case.d.0,
            case.a.1 - case.d.1,
            case.a.2 - case.d.2,
        );
        let (fbdx, fbdy, fbdz) = (
            case.b.0 - case.d.0,
            case.b.1 - case.d.1,
            case.b.2 - case.d.2,
        );
        let (fcdx, fcdy, fcdz) = (
            case.c.0 - case.d.0,
            case.c.1 - case.d.1,
            case.c.2 - case.d.2,
        );
        let det = -(fadx * (fbdy * fcdz - fbdz * fcdy)
            + fady * (fbdz * fcdx - fbdx * fcdz)
            + fadz * (fbdx * fcdy - fbdy * fcdx));
        let naive = if det > 0.0 {
            1
        } else if det < 0.0 {
            -1
        } else {
            0
        };
        if naive != truth {
            naive_lies += 1;
        }
    }
    eprintln!(
        "[census] sliver-tetra corpus: {} cases, naive f64 orient3d lies = {naive_lies}",
        corpus.len()
    );
    assert!(
        naive_lies > 0,
        "corpus never reached the lying regime of the naive orient3d"
    );
}

// ─────────────────────── solid-level census sweeps ──────────────────────────

use adversarial::{flush_upstand_union, near_tangent_cyl_union, sliver_wall_union};
use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;

/// (sound, boundary_edges, nonmanifold_edges, euler) for a result solid.
fn metrics(m: &mut BRepModel, s: SolidId, label: &str) -> (bool, usize, usize, i64) {
    let gt = m
        .ground_truth(s)
        .unwrap_or_else(|| panic!("{label}: no ground truth"));
    let mr = manifold_report(m, s, 0.05, 1.0e-5)
        .unwrap_or_else(|| panic!("{label}: no manifold report"));
    eprintln!(
        "[census] {label}: sound={} bnd={} nm={} euler={}",
        gt.certificate.is_sound(),
        mr.boundary_edges,
        mr.nonmanifold_edges,
        mr.euler_characteristic
    );
    (
        gt.certificate.is_sound(),
        mr.boundary_edges,
        mr.nonmanifold_edges,
        mr.euler_characteristic,
    )
}

fn assert_sound_rows<F: Fn(&mut BRepModel, f64) -> SolidId>(
    build: F,
    label: &str,
    eps_values: &[f64],
) {
    for &eps in eps_values {
        let mut m = BRepModel::new();
        let r = build(&mut m, eps);
        let (sound, bnd, nm, _euler) = metrics(&mut m, r, &format!("{label} eps={eps:e}"));
        assert!(
            sound && bnd == 0 && nm == 0,
            "{label} regressed at eps={eps:e}: sound={sound} bnd={bnd} nm={nm} \
             (was sound at the Slice-1 census, 2026-07-16)"
        );
    }
}

/// CENSUS + PIN: flush-upstand union across the ε ladder (the
/// `coincident-face-tolerance-gap` band, spec §3.4's permanent-sweep shape).
/// Slice-1 measurement (2026-07-16, this tree):
///
///   eps      sound  bnd  nm  euler
///   0        true    0    0   2
///   1e-15    true    0    0   2
///   1e-12    true    0    0   2
///   1e-9     true    0    0   2
///   1e-6     FALSE   0    0   2   ← certificate-unsound, topology clean
///   1e-4     FALSE   4    0   1   ← open shell
///   1e-3     true    0    0   2
///
/// The danger zone at the modeling tolerance REPRODUCED on this stacked-
/// upstand shape (same band the memory's L-bracket f5 found: sound on both
/// sides, broken in the middle where plane-coincidence and vertex-weld
/// disagree). This test pins the always-sound rows; the formerly broken
/// band (1e-6/1e-4) is the now-GREEN Slice-5 test below — together they
/// form the spec §3.4 permanent seven-rung ε sweep.
#[test]
fn census_flush_upstand_union_epsilon_ladder_sound_rows() {
    assert_sound_rows(
        flush_upstand_union,
        "flush_upstand",
        &[0.0, 1e-15, 1e-12, 1e-9, 1e-3],
    );
}

/// SLICE-5 GREEN (was the danger-zone RED): flush-upstand union is sound at
/// EVERY ε — no danger zone (spec §3.4: "the L-bracket union must be sound
/// at every ε"). Measured broken 2026-07-16 pre-Slice-5: eps=1e-6
/// (sound=false, cracked shell — the 2D coplanar clip absorbed the offset
/// wall while the 3D vertex weld kept it distinct) and eps=1e-4
/// (sound=false, bnd=4, euler=1 — the certification-mesh 1e-3 weld ate the
/// real 1e-4 ledge). Closed by the tolerance authority
/// (`math::tolerance::authority`): the τ_coincide plane-unification pre-pass
/// (`unify_near_coincident_planes`) rewrites ≤1e-5 plane pairs to exact
/// coincidence, and `MESH_WELD_CAP` (4·τ_weld) stops the certification mesh
/// from collapsing real sub-millimetre features. This sweep is the
/// PERMANENT ε fixture — a regression at any rung re-opens the danger zone.
#[test]
fn census_flush_upstand_union_epsilon_ladder_danger_zone() {
    assert_sound_rows(flush_upstand_union, "flush_upstand", &[1e-6, 1e-4]);
}

/// CENSUS + PIN: sliver-wall unions (coincident-within-thickness lateral
/// pairs). Slice-1 measurement (2026-07-16): t=1e-2 SOUND (pinned);
/// t=1e-4 BROKEN (sound=false bnd=7 nm=1 euler=3); t=1e-5 recorded by the
/// `#[ignore]`d RED below.
#[test]
fn census_sliver_wall_union_thickness_ladder_sound_rows() {
    assert_sound_rows(sliver_wall_union, "sliver_wall", &[1e-2]);
}

/// SLICE-5 GREEN (was the thin-band RED): sub-1e-4 sliver walls union
/// soundly. Measured broken 2026-07-16 pre-Slice-5: t=1e-4 → sound=false
/// bnd=7 nm=1 euler=3 (certification-mesh 1e-3 weld collapsed the wall's
/// lateral pair into a non-manifold membrane); t=1e-5 → sound=false bnd=8
/// euler=4 (the cap-merge ±ε probe couldn't resolve a wall thinner than the
/// classifier's OnBoundary band, read the blindness as "void", merged the
/// footprint seam away and stranded the wall as an open component). Closed
/// by `MESH_WELD_CAP` (4·τ_weld) + the tri-state probe honesty in
/// `coincident_coplanar_cap_merge` / `cull_internal_coincident_faces`
/// (undecidable probes fall back to symbolic partner-facing evidence, never
/// silent classification).
#[test]
fn census_sliver_wall_union_thickness_ladder_thin_band() {
    assert_sound_rows(sliver_wall_union, "sliver_wall", &[1e-5, 1e-4]);
}

/// CENSUS + PIN: near-tangent cylinder∪box (the #86 near-tangency class in
/// 3D; "points on/near arcs"). Slice-1 measurement (2026-07-16): the
/// SHORT-side placements (cylinder grazing short of the face plane,
/// eps<0) are sound — pinned here.
#[test]
fn census_near_tangent_cylinder_union_sound_rows() {
    assert_sound_rows(near_tangent_cyl_union, "near_tangent", &[-1e-1, -1e-6]);
}

/// #86-CLASS RED (recorded, NOT this campaign's claim to fix — spec Part 5
/// defers curved-intersection exactness to roadmap #2/#4): a cylinder poking
/// PAST the box face by a shallow margin corrupts the union. Measured broken
/// 2026-07-16: eps=+1e-1 → sound=false bnd=16 nm=2 euler=0;
/// eps=+1e-6 → sound=false bnd=36 nm=0 euler=-1 (shallow-lens imprint).
#[test]
#[ignore = "#86 near-tangency class: shallow cylinder/face lens breaks the union (roadmap #2/#4)"]
fn census_near_tangent_cylinder_union_shallow_lens_band() {
    assert_sound_rows(near_tangent_cyl_union, "near_tangent", &[1e-1, 1e-6]);
}
