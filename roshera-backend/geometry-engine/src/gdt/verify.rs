//! Kernel-verified GD&T conformance — the differentiator.
//!
//! The moat is not the tolerance vocabulary (every CAD system has that); it is
//! that **the kernel measures the actual geometry against the tolerance zone and
//! reports pass/fail from the B-Rep itself** — GD&T that cannot lie. A
//! flatness callout is verified by fitting the real face's sampled points to a
//! best-fit plane and computing the worst deviation; a diameter ± is verified
//! by measuring the actual cylindrical face's radius. No verdict is asserted by
//! the caller, and nothing fakes a pass.
//!
//! ## Honesty contract (load-bearing)
//!
//! Verification returns a [`Conformance`] tri-state, never a bare bool:
//! * [`Conformance::InSpec`] / [`Conformance::OutOfSpec`] are reported ONLY when
//!   the kernel actually measured the geometry.
//! * [`Conformance::NotYetVerified`] is reported for a tolerance whose
//!   verification this phase does not implement (orientation, location, runout,
//!   profile — anything that needs a datum reference frame; and unresolved fit
//!   classes). It is NEVER substituted by a silent pass. A verifier that always
//!   passes is worthless; this contract is what makes the verdict trustworthy.
//!
//! ## What is verified this phase (all datum-free)
//!
//! * **Dimensional** — diameter of a cylindrical face (and other size measures
//!   the kernel can extract analytically) vs nominal ± tolerance.
//! * **Flatness** — max |signed distance| of a planar face's sampled points
//!   from their best-fit plane.
//! * **Circularity** — radial band of a circular edge's sampled points about
//!   their best-fit circle.
//! * **Cylindricity** — radial band of a cylindrical face's sampled points
//!   about the best-fit cylinder.
//! * **Straightness** — perpendicular band of a linear edge's sampled points
//!   about their best-fit line.
//!
//! ## Measurement source
//!
//! Form deviation is measured from the fine tessellation of the face/edge (the
//! same mesh the kernel uses for export), sampled densely enough to expose real
//! form error. A perfect analytic primitive tessellates to points that lie on
//! its ideal surface, so it measures ~0 deviation; a distorted or coarsely
//! faceted feature measures the true worst error. Dimensional size is taken from
//! the analytic surface parameters directly (exact), not the mesh.

use serde::Serialize;

use crate::gdt::fit;
use crate::gdt::model::{
    Annotation, DimensionalTolerance, FeatureControlFrame, GeometricCharacteristic,
};
use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::edge::EdgeId;
use crate::primitives::face::FaceId;
use crate::primitives::surface::{Cylinder, Sphere};
use crate::primitives::topology_builder::BRepModel;
use crate::tessellation::{tessellate_solid, TessellationParams};

/// The verdict of a conformance check. A tri-state, never a bare bool, so an
/// unimplemented characteristic is reported honestly rather than as a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "verdict")]
pub enum Conformance {
    /// The kernel measured the geometry and it is within the tolerance.
    InSpec,
    /// The kernel measured the geometry and it violates the tolerance.
    OutOfSpec,
    /// The kernel does NOT yet verify this characteristic/feature combination.
    /// Carries a machine-readable reason. Never substituted by a pass.
    NotYetVerified,
}

impl Conformance {
    /// `Some(true/false)` when an actual measurement was made; `None` when the
    /// check was not verified.
    pub fn measured(self) -> Option<bool> {
        match self {
            Conformance::InSpec => Some(true),
            Conformance::OutOfSpec => Some(false),
            Conformance::NotYetVerified => None,
        }
    }
}

/// A single conformance result: what was checked, the tolerance asked for, the
/// actual measured value, the deviation, and the verdict. Every field is
/// computed from the geometry except the requested tolerance.
#[derive(Debug, Clone, Serialize)]
pub struct ConformanceResult {
    /// Human/agent label of the characteristic (e.g. "FLATNESS", "DIAMETER").
    pub characteristic: String,
    /// The requested tolerance value (zone width / size band) in mm.
    pub tolerance: f64,
    /// The kernel-measured actual value. For form: the measured form error. For
    /// dimensional: the measured size. `None` when not measured.
    pub actual: Option<f64>,
    /// For dimensional checks: the measured size's deviation from nominal.
    /// For form checks: the measured form error (== actual). `None` when not
    /// measured.
    pub deviation: Option<f64>,
    /// The verdict.
    pub verdict: Conformance,
    /// Machine-readable explanation, especially for `NotYetVerified`.
    pub detail: String,
}

impl ConformanceResult {
    /// Convenience for the common "this is not verified" path.
    fn not_verified(characteristic: &str, tolerance: f64, detail: impl Into<String>) -> Self {
        Self {
            characteristic: characteristic.to_string(),
            tolerance,
            actual: None,
            deviation: None,
            verdict: Conformance::NotYetVerified,
            detail: detail.into(),
        }
    }

    /// `in_spec` accessor matching the task's `ConformanceResult { ... in_spec }`
    /// vocabulary: `Some(true)` in-spec, `Some(false)` out, `None` unverified.
    pub fn in_spec(&self) -> Option<bool> {
        self.verdict.measured()
    }
}

/// Sample the points belonging to a single face from the solid's fine
/// tessellation. Returns the deduplicated mesh-vertex positions whose triangles
/// map to `face`. The tessellation `face_map[i]` gives the `FaceId` of triangle
/// `i`; we collect the three vertices of every matching triangle.
fn sample_face_points(model: &BRepModel, face: FaceId, params: &TessellationParams) -> Vec<Point3> {
    let mut pts = Vec::new();
    // Find the solid that owns this face, tessellate it, and filter.
    for (_sid, solid) in model.solids.iter() {
        let mut owns = false;
        for sh in solid.all_shells() {
            if let Some(shell) = model.shells.get(sh) {
                if shell.faces.contains(&face) {
                    owns = true;
                    break;
                }
            }
        }
        if !owns {
            continue;
        }
        let mesh = tessellate_solid(solid, model, params);
        for (tri_idx, tri) in mesh.triangles.iter().enumerate() {
            if mesh.face_map.get(tri_idx).copied() == Some(face) {
                for &vi in tri {
                    if let Some(v) = mesh.vertices.get(vi as usize) {
                        pts.push(v.position);
                    }
                }
            }
        }
        break;
    }
    pts
}

/// Sample points along a single edge by walking its 3D curve over the parameter
/// range at a fixed sample count proportional to the requested resolution.
fn sample_edge_points(model: &BRepModel, edge: EdgeId, samples: usize) -> Vec<Point3> {
    let mut pts = Vec::new();
    let Some(e) = model.edges.get(edge) else {
        return pts;
    };
    let Some(curve) = model.curves.get(e.curve_id) else {
        return pts;
    };
    let r = e.param_range;
    let n = samples.max(2);
    for i in 0..n {
        let t = r.start + (r.end - r.start) * (i as f64) / ((n - 1) as f64);
        if let Ok(p) = curve.point_at(t) {
            pts.push(p);
        }
    }
    pts
}

/// Verify a dimensional (size) tolerance against a feature. This phase measures
/// the DIAMETER of a cylindrical face (the common bore/boss size callout) from
/// the analytic surface directly. Other size measures report `NotYetVerified`.
pub fn verify_dimensional(
    model: &BRepModel,
    face: FaceId,
    tol: &DimensionalTolerance,
) -> ConformanceResult {
    // A fit class has no resolved numeric envelope — honest non-verdict.
    let (lower, upper) = match tol.limit_range() {
        Some(r) => r,
        None => {
            return ConformanceResult::not_verified(
                "DIAMETER",
                0.0,
                "fit-class tolerance: ISO 286 grade table not yet resolved",
            );
        }
    };
    let tolerance_band = upper - lower;

    let Some(f) = model.faces.get(face) else {
        return ConformanceResult::not_verified("DIAMETER", tolerance_band, "unknown face id");
    };
    let Some(surface) = model.surfaces.get(f.surface_id) else {
        return ConformanceResult::not_verified("DIAMETER", tolerance_band, "face has no surface");
    };

    // Measure the actual diameter from the analytic surface.
    let actual_diameter = match surface.type_name() {
        "Cylinder" => surface
            .as_any()
            .downcast_ref::<Cylinder>()
            .map(|c| 2.0 * c.radius),
        "Sphere" => surface
            .as_any()
            .downcast_ref::<Sphere>()
            .map(|s| 2.0 * s.radius),
        _ => None,
    };

    let Some(diameter) = actual_diameter else {
        return ConformanceResult::not_verified(
            "DIAMETER",
            tolerance_band,
            format!(
                "size measurement for a '{}' face not implemented (cylindrical/spherical only this phase)",
                surface.type_name()
            ),
        );
    };

    let in_spec = diameter >= lower && diameter <= upper;
    ConformanceResult {
        characteristic: "DIAMETER".to_string(),
        tolerance: tolerance_band,
        actual: Some(diameter),
        deviation: Some(diameter - tol.nominal),
        verdict: if in_spec {
            Conformance::InSpec
        } else {
            Conformance::OutOfSpec
        },
        detail: format!("measured diameter {diameter:.6} mm; limits [{lower:.6}, {upper:.6}]"),
    }
}

/// Verify a form FCF against a face (flatness / cylindricity) or edge
/// (circularity / straightness). The face/edge selector is chosen by the
/// characteristic. Datum-referenced / non-form characteristics report
/// `NotYetVerified`.
pub fn verify_form_on_face(
    model: &BRepModel,
    face: FaceId,
    fcf: &FeatureControlFrame,
    measure_tol: Tolerance,
) -> ConformanceResult {
    let name = fcf.characteristic.mnemonic();
    let zone = fcf.tolerance_value;

    if !fcf.datum_refs.is_empty() {
        return ConformanceResult::not_verified(
            name,
            zone,
            "datum-referenced verification not implemented this phase",
        );
    }

    let params = TessellationParams::fine();
    match fcf.characteristic {
        GeometricCharacteristic::Flatness => {
            let pts = sample_face_points(model, face, &params);
            if pts.len() < 3 {
                return ConformanceResult::not_verified(
                    name,
                    zone,
                    "insufficient sampled points on face",
                );
            }
            let Some(plane) = fit::fit_plane(&pts, measure_tol) else {
                return ConformanceResult::not_verified(name, zone, "plane fit failed");
            };
            // Flatness = total band = max(+dev) − min(−dev); for a single
            // best-fit plane this is the peak-to-valley spread, which is the
            // ISO 1101 flatness zone width.
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for p in &pts {
                let d = plane.signed_distance(*p);
                lo = lo.min(d);
                hi = hi.max(d);
            }
            let error = (hi - lo).max(0.0);
            verdict_form(name, zone, error)
        }
        GeometricCharacteristic::Cylindricity => {
            let pts = sample_face_points(model, face, &params);
            if pts.len() < 6 {
                return ConformanceResult::not_verified(
                    name,
                    zone,
                    "insufficient sampled points on cylindrical face",
                );
            }
            let Some(cyl) = fit::fit_cylinder(&pts, measure_tol) else {
                return ConformanceResult::not_verified(name, zone, "cylinder fit failed");
            };
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for p in &pts {
                let d = fit::cylinder_radial_deviation(&cyl, *p);
                lo = lo.min(d);
                hi = hi.max(d);
            }
            let error = (hi - lo).max(0.0);
            verdict_form(name, zone, error)
        }
        GeometricCharacteristic::Straightness | GeometricCharacteristic::Circularity => {
            ConformanceResult::not_verified(
                name,
                zone,
                "this characteristic verifies against an EDGE; use verify_form_on_edge",
            )
        }
        other => ConformanceResult::not_verified(
            name,
            zone,
            format!(
                "{} requires a datum reference frame (orientation/location/runout/profile)",
                other.mnemonic()
            ),
        ),
    }
}

/// Verify an edge-based form FCF (circularity / straightness) against an edge.
pub fn verify_form_on_edge(
    model: &BRepModel,
    edge: EdgeId,
    fcf: &FeatureControlFrame,
    measure_tol: Tolerance,
    samples: usize,
) -> ConformanceResult {
    let name = fcf.characteristic.mnemonic();
    let zone = fcf.tolerance_value;
    if !fcf.datum_refs.is_empty() {
        return ConformanceResult::not_verified(
            name,
            zone,
            "datum-referenced verification not implemented this phase",
        );
    }
    let pts = sample_edge_points(model, edge, samples);
    match fcf.characteristic {
        GeometricCharacteristic::Circularity => {
            if pts.len() < 3 {
                return ConformanceResult::not_verified(
                    name,
                    zone,
                    "insufficient sampled points on edge",
                );
            }
            let Some(circle) = fit::fit_circle(&pts, measure_tol) else {
                return ConformanceResult::not_verified(name, zone, "circle fit failed");
            };
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for p in &pts {
                let d = fit::circle_radial_deviation(&circle, *p);
                lo = lo.min(d);
                hi = hi.max(d);
            }
            let error = (hi - lo).max(0.0);
            verdict_form(name, zone, error)
        }
        GeometricCharacteristic::Straightness => {
            if pts.len() < 2 {
                return ConformanceResult::not_verified(
                    name,
                    zone,
                    "insufficient sampled points on edge",
                );
            }
            let Some(line) = fit::fit_line(&pts, measure_tol) else {
                return ConformanceResult::not_verified(name, zone, "line fit failed");
            };
            // Straightness zone = diameter of the smallest cylinder enclosing the
            // axis/line elements = 2 × max perpendicular distance to the fit line.
            let max_perp = pts
                .iter()
                .map(|p| line.distance(*p))
                .fold(0.0_f64, f64::max);
            let error = 2.0 * max_perp;
            verdict_form(name, zone, error)
        }
        other => ConformanceResult::not_verified(
            name,
            zone,
            format!("{} is not an edge form characteristic", other.mnemonic()),
        ),
    }
}

/// Build the in/out verdict for a measured form error against a zone.
fn verdict_form(name: &str, zone: f64, error: f64) -> ConformanceResult {
    ConformanceResult {
        characteristic: name.to_string(),
        tolerance: zone,
        actual: Some(error),
        deviation: Some(error),
        verdict: if error <= zone {
            Conformance::InSpec
        } else {
            Conformance::OutOfSpec
        },
        detail: format!("measured form error {error:.6} mm against zone {zone:.6} mm"),
    }
}

/// Verify a single [`Annotation`] attached to a FACE, dispatching to the right
/// measurement. Edge-based characteristics on a face annotation report
/// `NotYetVerified` (the annotation is mis-targeted).
pub fn verify_face_annotation(
    model: &BRepModel,
    face: FaceId,
    annotation: &Annotation,
    measure_tol: Tolerance,
) -> ConformanceResult {
    match annotation {
        Annotation::Dimensional(dim) => verify_dimensional(model, face, dim),
        Annotation::Geometric(fcf) => verify_form_on_face(model, face, fcf, measure_tol),
    }
}

/// Helper exposed for callers/tests that want a plain `Vec<Point3>` of a face's
/// sampled surface points at a given quality (e.g. to assert a measurement
/// independently).
pub fn face_surface_samples(
    model: &BRepModel,
    face: FaceId,
    params: &TessellationParams,
) -> Vec<Point3> {
    sample_face_points(model, face, params)
}

/// Helper: a face's outward normal at its parametric midpoint (used by callers
/// to disambiguate planar faces; kept here so the GD&T layer is self-contained).
pub fn face_midpoint_normal(model: &BRepModel, face: FaceId) -> Option<Vector3> {
    let f = model.faces.get(face)?;
    let b = f.uv_bounds;
    let (u, v) = (0.5 * (b[0] + b[1]), 0.5 * (b[2] + b[3]));
    f.normal_at(u, v, &model.surfaces).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gdt::model::ToleranceBound;

    #[test]
    fn not_verified_carries_no_false_pass() {
        let r = ConformanceResult::not_verified("POSITION", 0.1, "needs datum");
        assert_eq!(r.verdict, Conformance::NotYetVerified);
        assert_eq!(r.in_spec(), None, "unverified must not look like a pass");
        assert!(r.actual.is_none());
    }

    #[test]
    fn fit_class_dimensional_is_not_verified() {
        // No geometry needed: a fit-class tolerance has no resolved envelope.
        let dim = DimensionalTolerance::fit(10.0, "H7");
        assert!(matches!(dim.bound, ToleranceBound::Fit(_)));
        assert_eq!(dim.limit_range(), None);
    }
}
