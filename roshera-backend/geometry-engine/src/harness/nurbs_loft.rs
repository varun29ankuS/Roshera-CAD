//! NURBS-loft correctness harness (NURBS-LOFT verification).
//!
//! `operations::nurbs_loft` is the kernel's first operation that materialises a
//! genuine NURBS surface as a B-Rep face. This harness pins its contract with a
//! generative sweep of section stacks (straight tube, cone, barrel, ogive,
//! twisted, lobed-freeform) against the FULL invariant bundle:
//!
//! 1. **Valid B-Rep** — `validate_solid_scoped` (mesh-independent).
//! 2. **Watertight** — the universal [`crate::harness::watertight::is_watertight`]
//!    oracle at export density (the cap↔lateral seam must weld).
//! 3. **NURBS lateral present** — the wall is a real `GeneralNurbsSurface`, not
//!    a ruled/planar degradation.
//! 4. **Skin interpolation** — the built lateral surface passes through every
//!    input section point (the defining property of a skinned/lofted surface).
//! 5. **G2 along the loft** — at `degree_v >= 3` the lateral is curvature-
//!    continuous in V: C2 across every interior V-knot (so the part is a sound
//!    G2 freeform, not just G1).
//!
//! Checks 4–5 are run DIRECTLY on the surface extracted from the built solid, so
//! the harness verifies the geometry the kernel actually stored — not a
//! re-derivation.

use crate::harness::watertight::is_watertight;
use crate::math::{Point3, Tolerance, Vector3};
use crate::operations::nurbs_loft::{nurbs_loft, NurbsLoftOptions};
use crate::primitives::surface::GeneralNurbsSurface;
use crate::primitives::topology_builder::BRepModel;
use crate::primitives::validation::{validate_solid_scoped, ValidationLevel};

/// Outcome of the NURBS-loft invariant bundle.
#[derive(Debug, Clone)]
pub struct NurbsLoftCheck {
    pub built: bool,
    pub valid_brep: bool,
    pub watertight: bool,
    pub has_nurbs_lateral: bool,
    /// The built lateral surface passes through every input section point.
    pub interpolates_sections: bool,
    /// degree_v >= 3 and C2 across every interior V-knot (curvature-continuous).
    pub g2_in_v: bool,
    pub all_hold: bool,
}

impl NurbsLoftCheck {
    fn failed() -> Self {
        Self {
            built: false,
            valid_brep: false,
            watertight: false,
            has_nurbs_lateral: false,
            interpolates_sections: false,
            g2_in_v: false,
            all_hold: false,
        }
    }
}

/// Build a NURBS loft through `sections` (each an OPEN ring of equal point
/// count) and run the full invariant bundle. `interp_tol` is the absolute
/// tolerance for the skin-interpolation check.
pub fn nurbs_loft_invariants(
    sections: Vec<Vec<Point3>>,
    degree_u: usize,
    degree_v: usize,
    interp_tol: f64,
) -> NurbsLoftCheck {
    let n_sections = sections.len();
    let ring_len = sections.first().map(|s| s.len()).unwrap_or(0);
    let mut model = BRepModel::new();
    let options = NurbsLoftOptions {
        degree_u,
        degree_v,
        ..Default::default()
    };
    let solid = match nurbs_loft(&mut model, sections.clone(), options) {
        Ok(s) => s,
        Err(_) => return NurbsLoftCheck::failed(),
    };

    let valid_brep = validate_solid_scoped(
        &model,
        solid,
        Tolerance::default(),
        ValidationLevel::Standard,
    )
    .is_valid;
    let watertight = is_watertight(&mut model, solid, 0.05, 1e-3);

    // Extract the lateral NURBS surface from the built solid.
    let mut lateral: Option<crate::math::nurbs::NurbsSurface> = None;
    if let Some(sol) = model.solids.get(solid) {
        let mut shells = vec![sol.outer_shell];
        shells.extend_from_slice(&sol.inner_shells);
        'outer: for sh in shells {
            if let Some(shell) = model.shells.get(sh) {
                for &fid in &shell.faces {
                    if let Some(face) = model.faces.get(fid) {
                        if let Some(surf) = model.surfaces.get(face.surface_id) {
                            if let Some(g) = surf.as_any().downcast_ref::<GeneralNurbsSurface>() {
                                lateral = Some(g.nurbs.clone());
                                break 'outer;
                            }
                        }
                    }
                }
            }
        }
    }
    let has_nurbs_lateral = lateral.is_some();

    let (interpolates_sections, g2_in_v) = match &lateral {
        Some(surf) => (
            skin_interpolates(surf, &sections, ring_len, n_sections, interp_tol),
            g2_continuous_in_v(surf),
        ),
        None => (false, false),
    };

    let all_hold =
        valid_brep && watertight && has_nurbs_lateral && interpolates_sections && g2_in_v;
    NurbsLoftCheck {
        built: true,
        valid_brep,
        watertight,
        has_nurbs_lateral,
        interpolates_sections,
        g2_in_v,
        all_hold,
    }
}

/// The skinned surface must pass through every input section point. With the
/// closing point appended (the op closes each ring) the data sits at
/// `u = k/(nu-1)`, `v = j/(nv-1)` for `nu = ring_len + 1`, `nv = n_sections`.
fn skin_interpolates(
    surf: &crate::math::nurbs::NurbsSurface,
    sections: &[Vec<Point3>],
    ring_len: usize,
    n_sections: usize,
    tol: f64,
) -> bool {
    if ring_len < 3 || n_sections < 2 {
        return false;
    }
    let nu = ring_len + 1;
    for (j, section) in sections.iter().enumerate() {
        for k in 0..nu {
            let want = section[k % ring_len]; // wrap: closing point == first
            let u = k as f64 / (nu - 1) as f64;
            let v = j as f64 / (n_sections - 1) as f64;
            let got = surf.evaluate(u, v).point;
            if (got - want).magnitude() > tol {
                return false;
            }
        }
    }
    true
}

/// G2 along the loft (V): degree >= 3 AND the second V-derivative is continuous
/// across every interior V-knot. A degree-p B-spline is C^{p-1}, so degree 3 ⇒
/// C2 ⇒ curvature-continuous; this verifies it numerically at each interior knot
/// (a relative jump test, scale-free).
fn g2_continuous_in_v(surf: &crate::math::nurbs::NurbsSurface) -> bool {
    if surf.degree_v < 3 {
        return false;
    }
    let knots = surf.knots_v.to_vec();
    // Interior knots: strictly between the clamped end values, deduplicated.
    let (lo, hi) = (knots[0], knots[knots.len() - 1]);
    let mut interior: Vec<f64> = Vec::new();
    for &t in &knots {
        if t > lo + 1e-9 && t < hi - 1e-9 && !interior.iter().any(|&x| (x - t).abs() < 1e-9) {
            interior.push(t);
        }
    }
    if interior.is_empty() {
        // Single span ⇒ a polynomial patch, C-infinity internally ⇒ G2.
        return true;
    }
    let eps = (hi - lo) * 1e-5;
    for &t in &interior {
        let below = surf
            .evaluate_derivatives(0.5, t - eps, 0, 2)
            .dvv
            .unwrap_or(Vector3::ZERO);
        let above = surf
            .evaluate_derivatives(0.5, t + eps, 0, 2)
            .dvv
            .unwrap_or(Vector3::ZERO);
        let scale = below.magnitude() + above.magnitude();
        if (below - above).magnitude() > 1e-2 * scale.max(1.0) {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// section generators (private)
// ---------------------------------------------------------------------------

/// A circular ring of `n` points (open — not closed) at height `z`, radius `r`.
fn circle_ring(n: usize, r: f64, z: f64) -> Vec<Point3> {
    (0..n)
        .map(|i| {
            let a = i as f64 * std::f64::consts::TAU / n as f64;
            Point3::new(r * a.cos(), r * a.sin(), z)
        })
        .collect()
}

/// A lobed (non-circular) ring, optionally twisted by `phase`.
fn lobed_ring(n: usize, base: f64, lobe: f64, lobes: f64, phase: f64, z: f64) -> Vec<Point3> {
    (0..n)
        .map(|i| {
            let a = i as f64 * std::f64::consts::TAU / n as f64;
            let r = base + lobe * (lobes * a + phase).cos();
            Point3::new(r * a.cos(), r * a.sin(), z)
        })
        .collect()
}

/// The generative section-stack sweep used by the harness tests: a labelled set
/// of profiles spanning the shapes the loft must handle.
pub fn sweep_cases() -> Vec<(&'static str, Vec<Vec<Point3>>)> {
    vec![
        (
            "straight_tube",
            (0..4)
                .map(|j| circle_ring(16, 4.0, 2.0 * j as f64))
                .collect(),
        ),
        (
            "cone",
            (0..4)
                .map(|j| circle_ring(16, 5.0 - 0.8 * j as f64, 2.0 * j as f64))
                .collect(),
        ),
        (
            "barrel",
            vec![
                circle_ring(20, 2.0, 0.0),
                circle_ring(20, 3.0, 1.5),
                circle_ring(20, 3.5, 3.0),
                circle_ring(20, 3.0, 4.5),
                circle_ring(20, 2.0, 6.0),
            ],
        ),
        (
            "ogive",
            vec![
                circle_ring(18, 4.0, 0.0),
                circle_ring(18, 3.6, 2.0),
                circle_ring(18, 2.6, 4.0),
                circle_ring(18, 1.2, 6.0),
            ],
        ),
        (
            "twisted_lobed",
            (0..5)
                .map(|j| {
                    let t = j as f64;
                    lobed_ring(24, 3.0, 0.6, 3.0, 0.4 * t, 1.5 * t)
                })
                .collect(),
        ),
        (
            "freeform_scaled",
            vec![
                lobed_ring(24, 2.0, 0.4, 3.0, 0.0, 0.0),
                lobed_ring(24, 2.6, 0.5, 3.0, 0.0, 2.0),
                lobed_ring(24, 2.2, 0.45, 3.0, 0.0, 4.0),
                lobed_ring(24, 1.8, 0.35, 3.0, 0.0, 6.0),
            ],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every shape in the generative sweep must build a valid, watertight NURBS
    /// solid whose lateral interpolates the sections and is G2 along the loft.
    #[test]
    fn nurbs_loft_sweep_holds_all_invariants() {
        for (label, sections) in sweep_cases() {
            let c = nurbs_loft_invariants(sections, 3, 3, 1e-5);
            assert!(c.built, "[{label}] failed to build");
            assert!(c.valid_brep, "[{label}] B-Rep invalid: {c:?}");
            assert!(c.watertight, "[{label}] not watertight: {c:?}");
            assert!(c.has_nurbs_lateral, "[{label}] no NURBS lateral: {c:?}");
            assert!(
                c.interpolates_sections,
                "[{label}] skin does not interpolate the sections: {c:?}"
            );
            assert!(c.g2_in_v, "[{label}] not G2 along the loft: {c:?}");
            assert!(c.all_hold, "[{label}] invariant bundle failed: {c:?}");
        }
    }

    /// A degree-2 loft is only G1 (C1) along V — the harness's G2 check must
    /// REJECT it, proving the check has teeth (it isn't vacuously true).
    #[test]
    fn nurbs_loft_degree2_is_not_g2() {
        let sections: Vec<Vec<Point3>> = (0..4)
            .map(|j| circle_ring(16, 4.0, 2.0 * j as f64))
            .collect();
        let c = nurbs_loft_invariants(sections, 3, 2, 1e-5);
        // Still a valid watertight NURBS solid...
        assert!(
            c.built && c.valid_brep && c.watertight && c.has_nurbs_lateral,
            "{c:?}"
        );
        // ...but NOT G2 (degree_v = 2), so the bundle must not pass.
        assert!(!c.g2_in_v, "degree-2 loft wrongly reported G2: {c:?}");
        assert!(
            !c.all_hold,
            "degree-2 loft wrongly passed the bundle: {c:?}"
        );
    }
}
