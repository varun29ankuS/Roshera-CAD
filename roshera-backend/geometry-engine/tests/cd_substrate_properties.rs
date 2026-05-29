//! Property-based tests for the CD-substrate differential-geometry and
//! Bézier-decomposition features.
//!
//! Two recently-shipped surfaces are exercised:
//!
//! 1. `Surface::fundamental_forms_at` + the `FundamentalForms` curvature
//!    accessors (`gaussian_curvature`, `mean_curvature`) on the analytic
//!    primitives `Plane`, `Cylinder`, `Sphere`. The closed-form curvatures of
//!    these surfaces are known exactly, so a `proptest`-generated radius and a
//!    `proptest`-generated interior `(u, v)` parameter give a strong oracle:
//!      - Sphere of radius `R`:    `K = 1/R²`, `|H| = 1/R`.
//!      - Cylinder of radius `R`:  `K = 0`,    `|H| = 1/(2R)`.
//!      - Plane:                   `K = 0`,    `H = 0`, and `L = M = N = 0`.
//!    `K` is orientation-independent (invariant under a normal flip); `H` is
//!    not, so the cylinder/sphere mean-curvature checks take `|H|`.
//!
//! 2. `NurbsSurface::to_bezier_patches` round-trip: a random rational
//!    degree-2×2 surface with one interior knot per direction must decompose
//!    into Bézier patches that reproduce the parent surface pointwise (within
//!    `1e-6`) and whose `(domain_u, domain_v)` rectangles tile the full
//!    `[0, 1] × [0, 1]` parameter square contiguously with no gaps or overlap.
//!
//! References: do Carmo, *Differential Geometry of Curves and Surfaces* §3;
//! Piegl & Tiller, *The NURBS Book* §4–5.

use geometry_engine::math::nurbs::NurbsSurface;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::surface::{Cylinder, Plane, Sphere, Surface};

use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Tolerances
// ---------------------------------------------------------------------------

/// Closed-form analytic-curvature comparison tolerance. The fundamental forms
/// are computed in double precision from exact analytic derivatives, so the
/// only error is round-off in the dot products and the `(LN−M²)/(EG−F²)`
/// quotient. `1e-7` holds across the proptest radius/param envelope.
const ANALYTIC_TOL: f64 = 1e-7;

/// Bézier-decomposition round-trip tolerance. Knot insertion to full
/// multiplicity is exact in exact arithmetic; `1e-6` absorbs the accumulated
/// floating-point error of the Oslo/Boehm insertion and the rational
/// Bernstein re-evaluation.
const DECOMP_TOL: f64 = 1e-6;

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

/// Positive radius envelope shared by the sphere and cylinder tests. Lower
/// bound `0.5` keeps `1/R²` (the sphere's `K`) bounded so the absolute-error
/// tolerance is meaningful; upper bound `50.0` keeps curvatures well above the
/// degenerate-form epsilon.
fn arb_radius() -> impl Strategy<Value = f64> {
    0.5_f64..=50.0
}

/// A parameter strictly inside `(0, 1)`, used to interpolate into an open
/// parameter interval `(lo, hi)` and stay clear of the periodic seam / pole
/// where a surface's parametrization degenerates.
fn arb_unit_open() -> impl Strategy<Value = f64> {
    0.02_f64..=0.98
}

/// One control-point coordinate. Bounded so random nets stay numerically
/// well-conditioned for the decomposition round-trip.
fn arb_coord() -> impl Strategy<Value = f64> {
    -10.0_f64..=10.0
}

/// A strictly-positive NURBS weight. The lower bound keeps the rational
/// denominator away from zero; the range spans an order of magnitude so the
/// patch is genuinely rational (not a disguised polynomial).
fn arb_weight() -> impl Strategy<Value = f64> {
    0.2_f64..=5.0
}

/// Map a `(0,1)` fraction into the open interval `(lo, hi)`.
fn lerp_open(lo: f64, hi: f64, t: f64) -> f64 {
    lo + (hi - lo) * t
}

// ---------------------------------------------------------------------------
// Sphere — K = 1/R², |H| = 1/R, K orientation-independent
// ---------------------------------------------------------------------------

proptest! {
    /// Gaussian curvature of a sphere of radius `R` is `1/R²` everywhere,
    /// independent of the surface normal's orientation.
    #[test]
    fn sphere_gaussian_curvature_is_inverse_r_squared(
        r in arb_radius(),
        fu in arb_unit_open(),
        fv in arb_unit_open(),
    ) {
        let sphere = Sphere::new(Point3::new(0.0, 0.0, 0.0), r)
            .expect("strategy guarantees positive radius");
        let ((u_lo, u_hi), (v_lo, v_hi)) = sphere.parameter_bounds();
        let u = lerp_open(u_lo, u_hi, fu);
        let v = lerp_open(v_lo, v_hi, fv);

        let forms = sphere
            .fundamental_forms_at(u, v)
            .expect("interior (u,v) is a regular point of the sphere");
        let k = forms
            .gaussian_curvature()
            .expect("regular point has a defined Gaussian curvature");

        let expected = 1.0 / (r * r);
        prop_assert!(
            (k - expected).abs() < ANALYTIC_TOL,
            "K={} expected {} (R={}, u={}, v={})",
            k, expected, r, u, v
        );
        // K = k1*k2 > 0 for a sphere regardless of the chosen normal.
        prop_assert!(k > 0.0, "sphere K must be positive, got {}", k);
    }
}

proptest! {
    /// Mean curvature magnitude of a sphere of radius `R` is `1/R`
    /// everywhere. Its sign follows the chosen normal, so the test compares
    /// `|H|`.
    #[test]
    fn sphere_mean_curvature_magnitude_is_inverse_r(
        r in arb_radius(),
        fu in arb_unit_open(),
        fv in arb_unit_open(),
    ) {
        let sphere = Sphere::new(Point3::new(0.0, 0.0, 0.0), r)
            .expect("strategy guarantees positive radius");
        let ((u_lo, u_hi), (v_lo, v_hi)) = sphere.parameter_bounds();
        let u = lerp_open(u_lo, u_hi, fu);
        let v = lerp_open(v_lo, v_hi, fv);

        let forms = sphere
            .fundamental_forms_at(u, v)
            .expect("interior (u,v) is a regular point of the sphere");
        let h = forms
            .mean_curvature()
            .expect("regular point has a defined mean curvature");

        let expected = 1.0 / r;
        prop_assert!(
            (h.abs() - expected).abs() < ANALYTIC_TOL,
            "|H|={} expected {} (R={}, u={}, v={})",
            h.abs(), expected, r, u, v
        );
    }
}

// ---------------------------------------------------------------------------
// Cylinder — K = 0, |H| = 1/(2R)
// ---------------------------------------------------------------------------

proptest! {
    /// A cylinder is developable: its Gaussian curvature is identically zero.
    #[test]
    fn cylinder_gaussian_curvature_is_zero(
        r in arb_radius(),
        fu in arb_unit_open(),
        fv in arb_unit_open(),
    ) {
        let cyl = Cylinder::new(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            r,
        )
        .expect("strategy guarantees positive radius");

        // u sweeps the periodic angle (0, 2π); v is the unbounded axial
        // coordinate — pick a finite interval to interpolate into.
        let ((u_lo, u_hi), _) = cyl.parameter_bounds();
        let u = lerp_open(u_lo, u_hi, fu);
        let v = lerp_open(-25.0, 25.0, fv);

        let forms = cyl
            .fundamental_forms_at(u, v)
            .expect("interior (u,v) is a regular point of the cylinder");
        let k = forms
            .gaussian_curvature()
            .expect("regular point has a defined Gaussian curvature");

        prop_assert!(
            k.abs() < ANALYTIC_TOL,
            "cylinder K={} expected 0 (R={}, u={}, v={})",
            k, r, u, v
        );
    }
}

proptest! {
    /// Mean curvature magnitude of a cylinder of radius `R` is `1/(2R)`.
    #[test]
    fn cylinder_mean_curvature_magnitude_is_half_inverse_r(
        r in arb_radius(),
        fu in arb_unit_open(),
        fv in arb_unit_open(),
    ) {
        let cyl = Cylinder::new(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            r,
        )
        .expect("strategy guarantees positive radius");

        let ((u_lo, u_hi), _) = cyl.parameter_bounds();
        let u = lerp_open(u_lo, u_hi, fu);
        let v = lerp_open(-25.0, 25.0, fv);

        let forms = cyl
            .fundamental_forms_at(u, v)
            .expect("interior (u,v) is a regular point of the cylinder");
        let h = forms
            .mean_curvature()
            .expect("regular point has a defined mean curvature");

        let expected = 1.0 / (2.0 * r);
        prop_assert!(
            (h.abs() - expected).abs() < ANALYTIC_TOL,
            "cylinder |H|={} expected {} (R={}, u={}, v={})",
            h.abs(), expected, r, u, v
        );
    }
}

// ---------------------------------------------------------------------------
// Plane — K = 0, H = 0, L = M = N = 0
// ---------------------------------------------------------------------------

proptest! {
    /// A plane is flat: Gaussian curvature is identically zero.
    #[test]
    fn plane_gaussian_curvature_is_zero(
        z in -50.0_f64..=50.0,
        u in -50.0_f64..=50.0,
        v in -50.0_f64..=50.0,
    ) {
        let plane = Plane::xy(z);
        let forms = plane
            .fundamental_forms_at(u, v)
            .expect("every point of a plane is regular");
        let k = forms
            .gaussian_curvature()
            .expect("plane has a defined Gaussian curvature");
        prop_assert!(
            k.abs() < ANALYTIC_TOL,
            "plane K={} expected 0 (z={}, u={}, v={})",
            k, z, u, v
        );
    }
}

proptest! {
    /// A plane has zero mean curvature.
    #[test]
    fn plane_mean_curvature_is_zero(
        z in -50.0_f64..=50.0,
        u in -50.0_f64..=50.0,
        v in -50.0_f64..=50.0,
    ) {
        let plane = Plane::xy(z);
        let forms = plane
            .fundamental_forms_at(u, v)
            .expect("every point of a plane is regular");
        let h = forms
            .mean_curvature()
            .expect("plane has a defined mean curvature");
        prop_assert!(
            h.abs() < ANALYTIC_TOL,
            "plane H={} expected 0 (z={}, u={}, v={})",
            h, z, u, v
        );
    }
}

proptest! {
    /// A plane's second fundamental form vanishes: `L = M = N = 0`.
    #[test]
    fn plane_second_fundamental_form_is_zero(
        z in -50.0_f64..=50.0,
        u in -50.0_f64..=50.0,
        v in -50.0_f64..=50.0,
    ) {
        let plane = Plane::xy(z);
        let forms = plane
            .fundamental_forms_at(u, v)
            .expect("every point of a plane is regular");
        prop_assert!(
            forms.second.l.abs() < ANALYTIC_TOL
                && forms.second.m.abs() < ANALYTIC_TOL
                && forms.second.n.abs() < ANALYTIC_TOL,
            "plane II=(L={}, M={}, N={}) expected all 0 (z={}, u={}, v={})",
            forms.second.l, forms.second.m, forms.second.n, z, u, v
        );
    }
}

// ---------------------------------------------------------------------------
// Bézier decomposition round-trip
// ---------------------------------------------------------------------------

/// Build a random rational degree-2×2 NURBS surface with one interior knot per
/// direction (`knots = [0,0,0,0.5,1,1,1]`). A degree-2 knot vector of length 7
/// supports `n = knots.len() - degree - 1 = 4` control points per direction,
/// i.e. a 4×4 net.
///
/// Returns `Err` only if the kernel rejects an input the strategy guarantees is
/// valid — which would itself be a finding; callers `expect` on it.
fn build_random_surface(
    coords: &[[(f64, f64, f64); 4]; 4],
    weights: &[[f64; 4]; 4],
) -> Result<NurbsSurface, &'static str> {
    let control_points: Vec<Vec<Point3>> = coords
        .iter()
        .map(|row| {
            row.iter()
                .map(|&(x, y, z)| Point3::new(x, y, z))
                .collect()
        })
        .collect();
    let weight_grid: Vec<Vec<f64>> = weights.iter().map(|row| row.to_vec()).collect();

    // Clamped degree-2 knot vector with a single interior knot at 0.5.
    let knots = vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0];
    NurbsSurface::new(control_points, weight_grid, knots.clone(), knots, 2, 2)
}

/// Strategy for a 4×4 grid of control-point coordinates.
fn arb_net() -> impl Strategy<Value = [[(f64, f64, f64); 4]; 4]> {
    // 16 points × 3 coords. Build row-by-row to keep the type explicit.
    let row = || {
        [
            (arb_coord(), arb_coord(), arb_coord()),
            (arb_coord(), arb_coord(), arb_coord()),
            (arb_coord(), arb_coord(), arb_coord()),
            (arb_coord(), arb_coord(), arb_coord()),
        ]
    };
    [row(), row(), row(), row()]
}

/// Strategy for a 4×4 grid of positive weights.
fn arb_weights() -> impl Strategy<Value = [[f64; 4]; 4]> {
    let row = || [arb_weight(), arb_weight(), arb_weight(), arb_weight()];
    [row(), row(), row(), row()]
}

proptest! {
    /// Every Bézier patch returned by `to_bezier_patches` reproduces the parent
    /// NURBS surface pointwise: for a random local `(s, t)`, the patch
    /// evaluated at `(s, t)` equals the parent evaluated at the mapped parent
    /// parameter `(domain.0 + s·Δ)`.
    #[test]
    fn bezier_decomposition_reproduces_parent_surface(
        net in arb_net(),
        weights in arb_weights(),
        s in 0.0_f64..=1.0,
        t in 0.0_f64..=1.0,
    ) {
        let surface = build_random_surface(&net, &weights)
            .expect("strategy yields a valid rational degree-2x2 surface");
        let patches = surface.to_bezier_patches();
        prop_assert!(!patches.is_empty(), "decomposition produced no patches");

        for patch in &patches {
            let parent_u = patch.domain_u.0 + s * (patch.domain_u.1 - patch.domain_u.0);
            let parent_v = patch.domain_v.0 + t * (patch.domain_v.1 - patch.domain_v.0);

            let from_patch = patch.evaluate(s, t);
            let from_parent = surface.evaluate(parent_u, parent_v).point;

            let d = from_patch.distance(&from_parent);
            prop_assert!(
                d < DECOMP_TOL,
                "patch/parent mismatch d={} at local (s={}, t={}) -> parent (u={}, v={}); \
                 patch={:?} parent={:?}",
                d, s, t, parent_u, parent_v, from_patch, from_parent
            );
        }
    }
}

proptest! {
    /// The patch domains tile `[0,1] × [0,1]` contiguously: with one interior
    /// knot at 0.5 in each direction the surface splits into a 2×2 grid of
    /// patches whose `domain_u`/`domain_v` are exactly `{(0,0.5),(0.5,1)}` in
    /// each axis, covering the full unit square with no gap and no overlap.
    #[test]
    fn bezier_decomposition_tiles_unit_square(
        net in arb_net(),
        weights in arb_weights(),
    ) {
        let surface = build_random_surface(&net, &weights)
            .expect("strategy yields a valid rational degree-2x2 surface");
        let patches = surface.to_bezier_patches();

        // One interior knot per direction => 2 segments per direction => 4 patches.
        prop_assert_eq!(
            patches.len(),
            4,
            "expected 2x2 = 4 patches, got {}",
            patches.len()
        );

        // Collect the distinct u-breakpoints and v-breakpoints from the patch
        // domains; they must be exactly {0.0, 0.5, 1.0} after sorting/dedup.
        let mut u_breaks: Vec<f64> = Vec::new();
        let mut v_breaks: Vec<f64> = Vec::new();
        for p in &patches {
            for &b in &[p.domain_u.0, p.domain_u.1] {
                if !u_breaks.iter().any(|x| (x - b).abs() < 1e-12) {
                    u_breaks.push(b);
                }
            }
            for &b in &[p.domain_v.0, p.domain_v.1] {
                if !v_breaks.iter().any(|x| (x - b).abs() < 1e-12) {
                    v_breaks.push(b);
                }
            }
        }
        u_breaks.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        v_breaks.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let expected = [0.0_f64, 0.5, 1.0];
        prop_assert_eq!(u_breaks.len(), 3, "u breakpoints: {:?}", u_breaks);
        prop_assert_eq!(v_breaks.len(), 3, "v breakpoints: {:?}", v_breaks);
        for (got, want) in u_breaks.iter().zip(expected.iter()) {
            prop_assert!(
                (got - want).abs() < 1e-9,
                "u break {} != {} (all: {:?})",
                got, want, u_breaks
            );
        }
        for (got, want) in v_breaks.iter().zip(expected.iter()) {
            prop_assert!(
                (got - want).abs() < 1e-9,
                "v break {} != {} (all: {:?})",
                got, want, v_breaks
            );
        }

        // Coverage: the four (domain_u, domain_v) rectangles must be exactly
        // the four cells of the 2x2 grid, each present once. This proves
        // contiguous tiling with no gap and no overlap.
        let cells = [(0.0, 0.5), (0.5, 1.0)];
        for &(uu_lo, uu_hi) in &cells {
            for &(vv_lo, vv_hi) in &cells {
                let count = patches
                    .iter()
                    .filter(|p| {
                        (p.domain_u.0 - uu_lo).abs() < 1e-9
                            && (p.domain_u.1 - uu_hi).abs() < 1e-9
                            && (p.domain_v.0 - vv_lo).abs() < 1e-9
                            && (p.domain_v.1 - vv_hi).abs() < 1e-9
                    })
                    .count();
                prop_assert_eq!(
                    count,
                    1,
                    "grid cell u({},{}) v({},{}) covered {} times",
                    uu_lo, uu_hi, vv_lo, vv_hi, count
                );
            }
        }
    }
}
