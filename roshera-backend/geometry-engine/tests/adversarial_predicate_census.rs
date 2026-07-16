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
/// The danger zone at the modeling tolerance REPRODUCES on this stacked-
/// upstand shape (same band the memory's L-bracket f5 found: sound on both
/// sides, broken in the middle where plane-coincidence and vertex-weld
/// disagree). This test pins the SOUND rows; the broken band is the
/// `#[ignore]`d RED below (Slice 5's tolerance-authority deliverable).
#[test]
fn census_flush_upstand_union_epsilon_ladder_sound_rows() {
    assert_sound_rows(
        flush_upstand_union,
        "flush_upstand",
        &[0.0, 1e-15, 1e-12, 1e-9, 1e-3],
    );
}

/// SLICE-5 RED (danger zone): flush-upstand union must be sound at EVERY ε —
/// no danger zone (spec §3.4: "the L-bracket union must be sound at every ε").
/// Measured broken 2026-07-16: eps=1e-6 (sound=false) and eps=1e-4
/// (sound=false, bnd=4, euler=1). Un-ignore when the tolerance authority
/// (Slice 5) lands; the fix must be mutation-proven by re-splitting the
/// τ_coincide/τ_weld derivation.
#[test]
#[ignore = "Slice-5 RED: 1e-6/1e-4 danger zone (plane-coincidence vs vertex-weld disagreement)"]
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

/// SLICE-5 RED: sliver walls thinner than ~1e-4 corrupt the union (broken
/// weld-identity chain across the sliver's near-coincident lateral pair —
/// census rows #1/#13/#14's uncoordinated scales). Measured broken
/// 2026-07-16: t=1e-4 → sound=false bnd=7 nm=1 euler=3;
/// t=1e-5 → sound=false bnd=8 nm=0 euler=4.
#[test]
#[ignore = "Slice-5 RED: sub-1e-4 sliver walls break weld identity (bnd/nm > 0)"]
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
