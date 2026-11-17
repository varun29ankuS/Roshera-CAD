// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! BUG 1b-5 (R2) — degeneracy-robust `normal_at` / `closest_point` on a
//! NURBS patch with a COLLAPSED control row (a parametric pole).
//!
//! The mixed-corner cap (`operations/fillet.rs`) is a rational bi-quadratic
//! whose `u = u_max` row collapses to a single apex point. On that row the
//! `v`-derivative is identically zero, so the naive surface normal
//! `du × dv` is the zero vector: pre-fix `GeneralNurbsSurface::normal_at`
//! (the default `evaluate_full().normal`) returns `Err(DivisionByZero)`, and
//! `closest_point` — which called `evaluate_full` inside its Newton loop —
//! aborted with the same error whenever the footpoint reached the apex row.
//! The tessellation-soundness oracle reads every facet's winding against
//! `closest_point → normal_at`, so those failures HID the apex-adjacent
//! facets (an unresolvable normal is treated as "agreeing") or read a
//! spurious ~114° normal on the few that resolved just off the pole.
//!
//! These fixtures pin the robust behaviour on a minimal collapsed-apex patch
//! of the same class: a well-defined LIMITING normal at the pole (never an
//! error), an unchanged normal away from the pole, and an oracle composite
//! (`closest_point` then `normal_at`) that succeeds in the apex region.

use geometry_engine::math::nurbs::NurbsSurface;
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::primitives::surface::{GeneralNurbsSurface, Surface};

/// A rational bi-quadratic patch whose `u = 1` row collapses to a single
/// apex point — the same degenerate class as the mixed-corner cap. Curved in
/// both directions so the limiting normal at the pole is a genuine tangent
/// plane, not an artefact of a flat patch.
fn collapsed_apex_patch() -> GeneralNurbsSurface {
    let apex = Point3::new(0.0, 1.0, 1.4);
    let control_points = vec![
        vec![
            Point3::new(-1.0, -1.0, 0.0),
            Point3::new(0.0, -1.0, 0.3),
            Point3::new(1.0, -1.0, 0.0),
        ],
        vec![
            Point3::new(-1.0, 0.0, 0.6),
            Point3::new(0.0, 0.0, 1.0),
            Point3::new(1.0, 0.0, 0.6),
        ],
        vec![apex, apex, apex], // collapsed apex row (u = 1): dv ≡ 0 here
    ];
    let weights = vec![vec![1.0; 3]; 3];
    let knots_u = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    let knots_v = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    let nurbs = NurbsSurface::new(control_points, weights, knots_u, knots_v, 2, 2)
        .expect("collapsed-apex bi-quadratic patch is a valid NURBS surface");
    GeneralNurbsSurface { nurbs }
}

/// The apex is at u = 1; `normal_at` there must return the LIMITING normal,
/// never an error. Pre-fix (default `evaluate_full().normal`) `du × dv = 0`
/// on the collapsed row ⇒ `Err(DivisionByZero)`.
#[test]
fn normal_at_collapsed_apex_row_returns_limiting_normal() {
    let s = collapsed_apex_patch();

    let n_apex = s.normal_at(1.0, 0.5);
    assert!(
        n_apex.is_ok(),
        "normal_at on the collapsed apex row must return the limiting normal, \
         not Err (pre-fix du×dv = 0 ⇒ DivisionByZero); got {n_apex:?}"
    );
    let n_apex = n_apex.expect("apex normal ok");

    // Continuity: the pole normal must match the normal a hair inside the
    // domain (the surface normal is continuous up to the pole).
    let n_near = s
        .normal_at(1.0 - 1e-3, 0.5)
        .expect("normal just inside the pole is well-conditioned");
    let deg = n_apex.dot(&n_near).clamp(-1.0, 1.0).acos().to_degrees();
    assert!(
        deg < 2.0,
        "apex limiting normal must agree with the just-inside normal (<2°); \
         got {deg:.3}° (apex={n_apex:?} near={n_near:?})"
    );

    // And it must be a genuine unit vector.
    assert!(
        (n_apex.magnitude() - 1.0).abs() < 1e-9,
        "limiting normal must be unit length; got |n|={}",
        n_apex.magnitude()
    );
}

/// Regression guard: away from the pole, `normal_at` is byte-for-byte the old
/// `du × dv` normal — the robust path must not perturb well-conditioned
/// queries (the overwhelming majority).
#[test]
fn normal_at_interior_matches_du_cross_dv() {
    let s = collapsed_apex_patch();
    for &(u, v) in &[(0.25, 0.25), (0.5, 0.5), (0.3, 0.8), (0.75, 0.4)] {
        let e = s.nurbs.evaluate_derivatives(u, v, 1, 1);
        let du = e.du.expect("du");
        let dv = e.dv.expect("dv");
        let expected = du.cross(&dv).normalize().expect("well-conditioned");
        let got = s.normal_at(u, v).expect("interior normal ok");
        let d = (got - expected).magnitude();
        assert!(
            d < 1e-12,
            "interior normal_at({u},{v}) must equal du×dv normalised; delta={d:.3e}"
        );
    }
}

/// The soundness-oracle composite: for a point in the apex region,
/// `closest_point` must succeed (pre-fix its Newton loop hit `evaluate_full`
/// on the collapsed row and returned `Err`), and the subsequent `normal_at`
/// at that footpoint must also succeed. This is exactly what
/// `harness::watertight::analytic_facet_*` runs per facet.
#[test]
fn closest_point_then_normal_at_survives_apex_region() {
    let s = collapsed_apex_patch();
    let apex = Point3::new(0.0, 1.0, 1.4);
    let tol = Tolerance::default();

    // A query point straight "above" the apex — the apex is the topmost point
    // of the patch, so its true footpoint IS the apex (u = 1), forcing the
    // Newton search onto the collapsed row.
    let probe = apex + Vector3::new(0.0, 0.0, 0.6);

    let foot = s.closest_point(&probe, tol);
    assert!(
        foot.is_ok(),
        "closest_point in the apex region must not error on the collapsed row \
         (pre-fix evaluate_full in the Newton loop ⇒ Err); got {foot:?}"
    );
    let (u, v) = foot.expect("foot ok");

    let n = s.normal_at(u, v);
    assert!(
        n.is_ok(),
        "normal_at at the apex-region footpoint must resolve; got {n:?}"
    );

    // The footpoint must actually be near the apex (u close to 1), i.e. the
    // search converged to the pole rather than bailing early elsewhere.
    let foot_pos = s.point_at(u, v).expect("foot position");
    assert!(
        (foot_pos - apex).magnitude() < 0.35,
        "apex-region footpoint must land near the apex; got {foot_pos:?} (u={u:.4})"
    );
}

/// The POINT at a pole is well defined -- the apex itself -- so `point_at`
/// there must return it. Before, `point_at` went through `evaluate_full`,
/// which normalised `du x dv` and failed on the zero cross product: every
/// point on the pole row was an error (CADGenBench part 104: a NURBS face
/// whose outline touches its pole could not be projected, and meshed to
/// nothing).
#[test]
fn point_at_on_the_collapsed_row_is_the_apex() {
    let s = collapsed_apex_patch();
    for v in [0.0, 0.3, 0.5, 1.0] {
        let p = s
            .point_at(1.0, v)
            .unwrap_or_else(|e| panic!("point_at(1, {v}) on the pole: {e:?}"));
        assert!(
            (p - Point3::new(0.0, 1.0, 1.4)).magnitude() < 1e-12,
            "the pole row is the apex: {p:?}"
        );
    }
}

/// `evaluate_full` on the pole returns the point, its partials (one of them
/// zero, honestly) and the LIMITING normal -- never an error.
#[test]
fn evaluate_full_on_the_collapsed_row_carries_the_limiting_normal() {
    let s = collapsed_apex_patch();
    let e = s
        .evaluate_full(1.0, 0.5)
        .unwrap_or_else(|err| panic!("evaluate_full on the pole: {err:?}"));
    let limit = s.normal_at(1.0, 0.5).expect("limiting normal");
    assert!(
        (e.normal - limit).magnitude() < 1e-12,
        "{:?} vs {limit:?}",
        e.normal
    );
    assert!(e.dv.magnitude() < 1e-12, "dv is zero on the collapsed row");
}

/// When the apex is the grid node nearest the query, every node of its row is
/// that same point: the global footpoint search used to take three copies of
/// it as its seeds, and the solver cannot leave a pole (its metric is
/// degenerate there), so the APEX came back as the footpoint (CADGenBench
/// part 229: 0.23 mm off, read as an 85 deg facet-normal deviation). The
/// seeds are distinct points, so the true footpoint is found.
#[test]
fn closest_point_is_not_trapped_at_the_apex_when_it_is_the_nearest_grid_node() {
    // A flat triangle: P(u, v) = (1 - u) (10 v, 10, 0), the u = 1 row
    // collapsed to the origin.
    let apex = Point3::new(0.0, 0.0, 0.0);
    let nurbs = NurbsSurface::new(
        vec![
            vec![Point3::new(0.0, 10.0, 0.0), Point3::new(10.0, 10.0, 0.0)],
            vec![apex, apex],
        ],
        vec![vec![1.0; 2]; 2],
        vec![0.0, 0.0, 1.0, 1.0],
        vec![0.0, 0.0, 1.0, 1.0],
        1,
        1,
    )
    .expect("a valid bilinear patch");
    let patch = GeneralNurbsSurface { nurbs };
    // 0.5 from the apex (the nearest grid node); its footpoint is on the
    // v = 1 side (the line y = x), 0.1 / sqrt(2) away.
    let query = Point3::new(0.4, 0.3, 0.0);
    let (u, v) = patch
        .closest_point(&query, Tolerance::default())
        .expect("projects");
    let found = patch.point_at(u, v).expect("evaluates").distance(&query);
    let exact = 0.1 / 2f64.sqrt();
    assert!(
        (found - exact).abs() < 1e-6,
        "footpoint at distance {found:.6} (u {u:.4}, v {v:.4}); the true one is {exact:.6}"
    );
}

/// A closed surface's seam is one point at two parameters, u_min and u_max.
/// When the nearest grid nodes are on the seam, both copies must be seeds: a
/// query just short of the seam on the u_max side is reached only from the
/// u_max copy, because the solver does not wrap round the seam. Keeping one
/// copy of each point dropped it, and the footpoint stuck at u_min, 0.2 mm
/// off (CADGenBench part 147: a NURBS face's boundary points lost, leaving
/// open edges against its cone neighbour).
#[test]
fn closest_point_reaches_a_point_just_short_of_a_closed_surfaces_seam() {
    // A radius-10 cylinder, z 0..10, u round a rational full circle that
    // starts and ends at (10, 0, z).
    let h = std::f64::consts::FRAC_1_SQRT_2;
    let ring = [
        (1.0, 0.0, 1.0),
        (1.0, 1.0, h),
        (0.0, 1.0, 1.0),
        (-1.0, 1.0, h),
        (-1.0, 0.0, 1.0),
        (-1.0, -1.0, h),
        (0.0, -1.0, 1.0),
        (1.0, -1.0, h),
        (1.0, 0.0, 1.0),
    ];
    let control_points = ring
        .iter()
        .map(|&(x, y, _)| {
            vec![
                Point3::new(10.0 * x, 10.0 * y, 0.0),
                Point3::new(10.0 * x, 10.0 * y, 10.0),
            ]
        })
        .collect();
    let weights = ring.iter().map(|&(_, _, w)| vec![w, w]).collect();
    let nurbs = NurbsSurface::new(
        control_points,
        weights,
        vec![
            0.0, 0.0, 0.0, 0.25, 0.25, 0.5, 0.5, 0.75, 0.75, 1.0, 1.0, 1.0,
        ],
        vec![0.0, 0.0, 1.0, 1.0],
        2,
        1,
    )
    .expect("a valid rational cylinder");
    let cylinder = GeneralNurbsSurface { nurbs };
    // On the surface, 0.02 rad before the seam going round the u direction.
    let a = -0.02f64;
    let query = Point3::new(10.0 * a.cos(), 10.0 * a.sin(), 5.05);
    let (u, v) = cylinder
        .closest_point(&query, Tolerance::default())
        .expect("projects");
    let found = cylinder.point_at(u, v).expect("evaluates").distance(&query);
    assert!(
        found < 1e-6,
        "the query is on the surface, but its footpoint is {found:.6} away (u {u:.5}, v {v:.5})"
    );
}
