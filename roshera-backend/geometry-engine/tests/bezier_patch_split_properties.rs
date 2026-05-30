//! Property tests for `BezierPatch` de Casteljau split + bounds
//! (CD-φ.1.3 / 1.4), exercising the public API across many split
//! parameters and a double (u∘v) split.
//!
//! The core invariant needs no closed-form oracle: a Bézier patch split
//! at `t` must, on each half, re-evaluate to exactly the original patch
//! over the corresponding sub-rectangle. This pins the rational de
//! Casteljau implementation against silent regressions at split
//! parameters and patch shapes the in-crate unit tests don't cover
//! (they split a single biquadratic at one `t`).

#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]
#![allow(clippy::needless_range_loop)]

use geometry_engine::math::bezier_patch::BezierPatch;
use geometry_engine::math::Point3;

const TOL: f64 = 1e-9;

/// A degree-(3,2) rational patch: asymmetric degrees + a couple of
/// non-unit weights, so the test stresses the rational path and the
/// row/column asymmetry that a square biquadratic can't.
fn asymmetric_rational_patch() -> BezierPatch {
    let nu = 4; // degree_u = 3
    let nv = 3; // degree_v = 2
    let mut cp = Vec::with_capacity(nu);
    let mut w = Vec::with_capacity(nu);
    for i in 0..nu {
        let mut crow = Vec::with_capacity(nv);
        let mut wrow = Vec::with_capacity(nv);
        for j in 0..nv {
            // A mildly twisted, non-planar net.
            let x = i as f64;
            let y = j as f64;
            let z = (i as f64 - 1.5) * (j as f64 - 1.0) * 0.4;
            crow.push(Point3::new(x, y, z));
            wrow.push(if (i + j) % 2 == 0 { 1.0 } else { 1.5 });
        }
        cp.push(crow);
        w.push(wrow);
    }
    BezierPatch {
        degree_u: 3,
        degree_v: 2,
        control_points: cp,
        weights: w,
        domain_u: (0.0, 1.0),
        domain_v: (0.0, 1.0),
    }
}

/// Dense grid of local sample parameters.
fn grid() -> Vec<f64> {
    vec![0.0, 0.15, 0.37, 0.5, 0.62, 0.83, 1.0]
}

#[test]
fn split_u_reproduces_original_across_many_parameters() {
    let patch = asymmetric_rational_patch();
    for &t in &[0.1_f64, 0.25, 0.5, 0.5001, 0.75, 0.9] {
        let (left, right) = patch.split_u(t);
        for &s in &grid() {
            for &v in &grid() {
                // Left half local s ↔ original u = s·t.
                let exp_l = patch.evaluate(s * t, v);
                let got_l = left.evaluate(s, v);
                assert!(
                    (got_l - exp_l).magnitude() < TOL,
                    "split_u t={t}: left mismatch at s={s}, v={v}: {got_l:?} vs {exp_l:?}"
                );
                // Right half local s ↔ original u = t + s·(1−t).
                let exp_r = patch.evaluate(t + s * (1.0 - t), v);
                let got_r = right.evaluate(s, v);
                assert!(
                    (got_r - exp_r).magnitude() < TOL,
                    "split_u t={t}: right mismatch at s={s}, v={v}"
                );
            }
        }
    }
}

#[test]
fn split_v_reproduces_original_across_many_parameters() {
    let patch = asymmetric_rational_patch();
    for &t in &[0.1_f64, 0.33, 0.5, 0.66, 0.95] {
        let (lower, upper) = patch.split_v(t);
        for &u in &grid() {
            for &s in &grid() {
                let exp_lo = patch.evaluate(u, s * t);
                assert!(
                    (lower.evaluate(u, s) - exp_lo).magnitude() < TOL,
                    "split_v t={t}: lower mismatch at u={u}, s={s}"
                );
                let exp_up = patch.evaluate(u, t + s * (1.0 - t));
                assert!(
                    (upper.evaluate(u, s) - exp_up).magnitude() < TOL,
                    "split_v t={t}: upper mismatch at u={u}, s={s}"
                );
            }
        }
    }
}

#[test]
fn double_split_u_then_v_reproduces_sub_rectangle() {
    // Splitting in u then v should isolate a sub-rectangle that still
    // re-evaluates to the original surface. Take the right-of-tu,
    // upper-of-tv corner patch and check it against the original over
    // [tu,1] × [tv,1].
    let patch = asymmetric_rational_patch();
    let (tu, tv) = (0.4_f64, 0.7_f64);
    let (_left, right) = patch.split_u(tu);
    let (_lower, corner) = right.split_v(tv);
    for &su in &grid() {
        for &sv in &grid() {
            // corner local (su,sv): u = tu + su·(1−tu) on the original,
            // v = tv + sv·(1−tv).
            let exp = patch.evaluate(tu + su * (1.0 - tu), tv + sv * (1.0 - tv));
            let got = corner.evaluate(su, sv);
            assert!(
                (got - exp).magnitude() < TOL,
                "double split: corner mismatch at su={su}, sv={sv}: {got:?} vs {exp:?}"
            );
        }
    }
}

#[test]
fn split_u_partitions_parent_domain_contiguously() {
    let patch = asymmetric_rational_patch();
    let t = 0.4;
    let (left, right) = patch.split_u(t);
    // Parent domain is partitioned: left ends where right begins, at the
    // parent parameter mid = domain.0 + t·(domain.1−domain.0).
    let (a, b) = patch.domain_u;
    let mid = a + t * (b - a);
    assert!((left.domain_u.0 - a).abs() < 1e-12, "left start = parent start");
    assert!((left.domain_u.1 - mid).abs() < 1e-12, "left end = mid");
    assert!((right.domain_u.0 - mid).abs() < 1e-12, "right start = mid");
    assert!((right.domain_u.1 - b).abs() < 1e-12, "right end = parent end");
    // v-domain untouched by a u-split.
    assert_eq!(left.domain_v, patch.domain_v);
    assert_eq!(right.domain_v, patch.domain_v);
}

#[test]
fn split_preserves_degree_and_grid_shape() {
    let patch = asymmetric_rational_patch();
    let (left, right) = patch.split_u(0.3);
    for half in [&left, &right] {
        assert_eq!(half.degree_u, patch.degree_u, "u-split keeps degree_u");
        assert_eq!(half.degree_v, patch.degree_v, "u-split keeps degree_v");
        assert_eq!(half.control_points.len(), patch.degree_u + 1);
        assert_eq!(half.control_points[0].len(), patch.degree_v + 1);
        assert_eq!(half.weights.len(), patch.degree_u + 1);
        assert_eq!(half.weights[0].len(), patch.degree_v + 1);
    }
}

#[test]
fn split_halves_share_boundary_curve_exactly() {
    // The u=t boundary: left's s=1 edge must equal right's s=0 edge for
    // every v (the shared split curve), so the two halves are watertight
    // in parameter space.
    let patch = asymmetric_rational_patch();
    let (left, right) = patch.split_u(0.55);
    for &v in &grid() {
        let on_left = left.evaluate(1.0, v);
        let on_right = right.evaluate(0.0, v);
        assert!(
            (on_left - on_right).magnitude() < TOL,
            "split boundary curve discontinuous at v={v}: {on_left:?} vs {on_right:?}"
        );
    }
}

#[test]
fn aabb_contains_surface_samples_for_asymmetric_patch() {
    let patch = asymmetric_rational_patch();
    let bb = patch.aabb().expect("non-empty patch has an aabb");
    for &u in &grid() {
        for &v in &grid() {
            let p = patch.evaluate(u, v);
            assert!(
                p.x >= bb.min.x - 1e-9
                    && p.x <= bb.max.x + 1e-9
                    && p.y >= bb.min.y - 1e-9
                    && p.y <= bb.max.y + 1e-9
                    && p.z >= bb.min.z - 1e-9
                    && p.z <= bb.max.z + 1e-9,
                "surface sample ({u},{v}) = {p:?} escapes control-net AABB {bb:?}"
            );
        }
    }
}
