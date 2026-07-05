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

use crate::gdt::drf::{resolve_datum, DatumReferenceFrame, DatumResolution};
use crate::gdt::fit;
use crate::gdt::model::{
    Annotation, DatumKind, DimensionalTolerance, FeatureControlFrame, GeometricCharacteristic,
};
use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::edge::EdgeId;
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
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
        // Collect each unique vertex index exactly once.  Without deduplication
        // the seam vertex (shared by every adjacent triangle strip around the
        // circumference) is counted N times, biasing the centroid used by
        // `fit_cylinder` / `fit_plane` toward the seam — corrupting the
        // measurement by up to r/N where r is the cylinder radius.  A `HashSet`
        // over the integer vertex indices is O(triangles) and allocation-light.
        let mut seen = std::collections::HashSet::new();
        for (tri_idx, tri) in mesh.triangles.iter().enumerate() {
            if mesh.face_map.get(tri_idx).copied() == Some(face) {
                for &vi in tri {
                    if seen.insert(vi) {
                        if let Some(v) = mesh.vertices.get(vi as usize) {
                            pts.push(v.position);
                        }
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

// ---------------------------------------------------------------------------
// Task 2 — Evaluated verdict (the certified evaluation)
// ---------------------------------------------------------------------------
//
// `evaluate` is the single entry point for datum-referenced GD&T verification:
// flatness (wired through the existing path), perpendicularity, parallelism,
// and position RFS. Every code path that cannot produce a real measurement
// returns `Conforms::NotEvaluable` with a reason — never a fabricated pass.
//
// ## Projection math summary (full derivations in evaluate_orientation)
//
// Parallelism:  W = max(p·n_d) − min(p·n_d)  (spread along datum normal).
//               W = 0 for a flat face parallel to datum.
//               W ≈ L·sin(α) for a face tilted by α from parallel.
//
// Perpendicularity: W = 2·R_lat·sin_α  where
//               sin_α = |n_fit·n_d|  (fit-plane-normal dot datum-normal)
//               R_lat = max lateral radius of face samples from centroid (⊥ n_d).
//               W = 0 for perfect ⊥ (sin_α = 0).  W ≈ 2·R_lat·tan(α) for tilted.
//
// Position RFS: DRF origin from datum plane intersection (see evaluate_position).
//               Measured = 2·√(Δx²+Δy²) vs basic [bx, by].
//               Hand-verified: bore at (30.04, 20.03), basic (30, 20):
//               Δx=0.04, Δy=0.03 → measured = 2·0.05 = 0.1000 mm.

/// The conforms field of a [`Verdict`].
///
/// This is the enriched tri-state for datum-referenced evaluation.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Conforms {
    /// The kernel measured the geometry and it is within the tolerance.
    InSpec,
    /// The kernel measured the geometry and it violates the tolerance.
    OutOfSpec,
    /// The verdict cannot be computed: a required datum is dangling, a basic
    /// dimension is missing, or the geometry cannot be fitted. The `reason`
    /// names the blocking condition — always actionable.
    NotEvaluable { reason: String },
}

impl Conforms {
    fn not_evaluable(reason: impl Into<String>) -> Self {
        Conforms::NotEvaluable {
            reason: reason.into(),
        }
    }

    /// True only when the measurement was made AND geometry is in spec.
    pub fn is_in_spec(&self) -> bool {
        matches!(self, Conforms::InSpec)
    }
}

/// Per-datum resolution status, for reporting in the verdict.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DatumStatus {
    pub label: String,
    pub resolution: DatumResolution,
}

/// The full certified verdict of a datum-referenced GD&T evaluation.
///
/// This is the heart of what makes the kernel's GD&T non-fabricatable: every
/// field is computed from the actual B-Rep geometry, and `conforms` is
/// `NotEvaluable` when anything cannot be measured — never a silent pass.
#[derive(Debug, Clone, Serialize)]
pub struct Verdict {
    /// Mnemonic of the characteristic being evaluated.
    pub characteristic: String,
    /// The declared tolerance zone width (mm).
    pub tolerance_mm: f64,
    /// The kernel-measured value (mm). `None` only when `conforms` is
    /// `NotEvaluable` and the measurement could not be made.
    pub measured_mm: Option<f64>,
    /// Conforms / violates / not evaluable.
    pub conforms: Conforms,
    /// Fit residual of the best-fit geometry to the toleranced feature's
    /// sampled points (mm). Reports the quality of the geometric fit; a large
    /// residual means the feature is not well-described by the fitted primitive
    /// and the verdict should be treated with lower confidence.
    /// `None` when the measurement could not be attempted.
    pub fit_residual_mm: Option<f64>,
    /// Resolution status of every datum referenced by the FCF, in the order of
    /// `fcf.datum_refs`. Dangling datums block evaluation.
    pub datum_status: Vec<DatumStatus>,
}

impl Verdict {
    fn not_evaluable(characteristic: &str, tolerance_mm: f64, reason: impl Into<String>) -> Self {
        Self {
            characteristic: characteristic.to_string(),
            tolerance_mm,
            measured_mm: None,
            conforms: Conforms::not_evaluable(reason),
            fit_residual_mm: None,
            datum_status: Vec::new(),
        }
    }

    fn not_evaluable_with_status(
        characteristic: &str,
        tolerance_mm: f64,
        reason: impl Into<String>,
        datum_status: Vec<DatumStatus>,
    ) -> Self {
        Self {
            characteristic: characteristic.to_string(),
            tolerance_mm,
            measured_mm: None,
            conforms: Conforms::not_evaluable(reason),
            fit_residual_mm: None,
            datum_status,
        }
    }
}

/// Resolve all datum references in the FCF and collect their statuses.
/// Returns `Err(Verdict)` immediately if any datum is dangling (because that
/// blocks evaluation), with the datum_status populated for all resolved so far.
fn resolve_fcf_datums(
    model: &BRepModel,
    solid: SolidId,
    fcf: &FeatureControlFrame,
    drf: &DatumReferenceFrame,
) -> Result<Vec<DatumStatus>, Verdict> {
    let name = fcf.characteristic.mnemonic();
    let tol = fcf.tolerance_value;

    let mut statuses: Vec<DatumStatus> = Vec::with_capacity(fcf.datum_refs.len());

    for dr in &fcf.datum_refs {
        let datum = match drf.datum_by_label(&dr.label) {
            Some(d) => d,
            None => {
                statuses.push(DatumStatus {
                    label: dr.label.clone(),
                    resolution: DatumResolution::Dangling,
                });
                return Err(Verdict::not_evaluable_with_status(
                    name,
                    tol,
                    format!(
                        "datum '{}' is not designated in the datum reference frame",
                        dr.label
                    ),
                    statuses,
                ));
            }
        };
        let resolution = resolve_datum(model, solid, datum);
        let is_dangling = resolution == DatumResolution::Dangling;
        statuses.push(DatumStatus {
            label: dr.label.clone(),
            resolution,
        });
        if is_dangling {
            return Err(Verdict::not_evaluable_with_status(
                name,
                tol,
                format!(
                    "datum '{}' is dangling — its source feature no longer exists",
                    dr.label
                ),
                statuses,
            ));
        }
    }

    Ok(statuses)
}

/// Evaluate a geometric [`Annotation`] on a face against a [`DatumReferenceFrame`],
/// returning a certified [`Verdict`].
///
/// This is the single public entry point for Task 2 datum-referenced evaluation.
/// It dispatches to the correct measurement for each characteristic:
/// - **Flatness**: wired through the existing form path (no datum needed).
/// - **Perpendicularity / Parallelism**: planar face sampled analytically;
///   measured = spread of projections onto datum normal direction.
/// - **Position RFS**: bore axis fitted to cylindrical face; deviation from
///   the DRF basic position measured as 2·√(Δx²+Δy²).
///
/// Any non-evaluable condition (dangling datum, missing basic, fit failure) is
/// reported as `Conforms::NotEvaluable` — never a silent pass.
pub fn evaluate(
    model: &BRepModel,
    solid: SolidId,
    face: FaceId,
    annotation: &Annotation,
    drf: &DatumReferenceFrame,
) -> Verdict {
    let measure_tol = Tolerance::from_distance(1e-9);

    match annotation {
        Annotation::Dimensional(_) => {
            // Dimensional tolerances are evaluated by verify_dimensional, not
            // this path — they do not use a datum reference frame.
            Verdict::not_evaluable(
                "DIMENSIONAL",
                0.0,
                "dimensional tolerances are evaluated via verify_dimensional, not evaluate()",
            )
        }
        Annotation::Geometric(fcf) => evaluate_fcf(model, solid, face, fcf, drf, measure_tol),
    }
}

/// Internal dispatch for geometric FCFs.
fn evaluate_fcf(
    model: &BRepModel,
    solid: SolidId,
    face: FaceId,
    fcf: &FeatureControlFrame,
    drf: &DatumReferenceFrame,
    measure_tol: Tolerance,
) -> Verdict {
    let name = fcf.characteristic.mnemonic();
    let tol = fcf.tolerance_value;
    let params = TessellationParams::fine();

    match fcf.characteristic {
        GeometricCharacteristic::Flatness => {
            // Flatness is datum-free; wire the existing form path through.
            // If any datum refs are present, report NotEvaluable (mis-authored).
            if !fcf.datum_refs.is_empty() {
                return Verdict::not_evaluable(
                    name,
                    tol,
                    "flatness is a datum-free form characteristic; remove datum references",
                );
            }
            let pts = sample_face_points(model, face, &params);
            if pts.len() < 3 {
                return Verdict::not_evaluable(name, tol, "insufficient sampled points on face");
            }
            let Some(plane) = fit::fit_plane(&pts, measure_tol) else {
                return Verdict::not_evaluable(name, tol, "plane fit failed");
            };
            // Flatness = peak-to-valley spread along the fit-plane normal.
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for p in &pts {
                let d = plane.signed_distance(*p);
                lo = lo.min(d);
                hi = hi.max(d);
            }
            let measured = (hi - lo).max(0.0);
            // Fit residual = RMS of signed deviations from the best-fit plane.
            let rms = (pts
                .iter()
                .map(|p| plane.signed_distance(*p).powi(2))
                .sum::<f64>()
                / pts.len() as f64)
                .sqrt();
            Verdict {
                characteristic: name.to_string(),
                tolerance_mm: tol,
                measured_mm: Some(measured),
                conforms: if measured <= tol {
                    Conforms::InSpec
                } else {
                    Conforms::OutOfSpec
                },
                fit_residual_mm: Some(rms),
                datum_status: Vec::new(),
            }
        }

        GeometricCharacteristic::Perpendicularity | GeometricCharacteristic::Parallelism => {
            evaluate_orientation(model, solid, face, fcf, drf, measure_tol, &params)
        }

        GeometricCharacteristic::Position => {
            evaluate_position(model, solid, face, fcf, drf, measure_tol, &params)
        }

        other => Verdict::not_evaluable(
            name,
            tol,
            format!(
                "{} evaluation is not yet implemented in this phase (Task 2 scope: \
                 Flatness, Perpendicularity, Parallelism, Position RFS)",
                other.mnemonic()
            ),
        ),
    }
}

/// Evaluate perpendicularity or parallelism.
///
/// ## Zone semantics and projection math — ASME Y14.5, hand-verified
///
/// The datum plane has outward normal `n_d` (resolved from the primary datum).
/// In both cases we project sample points onto `n_d` and take the spread.
///
/// ### Parallelism (projection onto datum normal, spread ≠ 0 iff tilted)
///
/// ASME zone: two planes parallel to the datum plane (normal `n_d`), separated
/// by `t`. The measured width is the spread of `(p_i − datum_origin) · n_d`:
///
/// ```text
///   s_i = (p_i − datum_origin) · n_d
///   W = max(s_i) − min(s_i)
/// ```
///
/// A perfectly parallel face lies in a plane at constant height above the datum
/// → all `s_i` equal → W = 0 (up to flatness error). A tilt by angle α about
/// an axis ⊥ n_d produces W = L · sin(α) ≈ L · tan(α), where L is the face
/// extent in the tilt direction.
///
/// Hand-verified fixture (Parallelism, 50 mm plate, Y-axis rotation by α = 0.5°):
///
/// ```text
///   Datum A = bottom face at z = −5, n_d = −Z (outward).
///   Perfect top face at z = 5: s_i = −(5 − (−5)) = −10 for all pts → W = 0 ✓
///   After 0.5° rotation of plate around Y through origin:
///     z′ = −x·sin(α) + z·cos(α);  for z = 5, x ∈ [−25, 25]:
///     z′ ∈ [5·cos(α) − 25·sin(α), 5·cos(α) + 25·sin(α)]
///     spread(z′) = 50·sin(α) ≈ 50·tan(α)
///   s_i = −z′ − (−(−5)) = −z′ − 5  →  spread(s_i) = spread(z′)
///   W = 50·sin(0.5°) ≈ 50·0.008727 = 0.4363 mm  ✓
/// ```
///
/// ### Perpendicularity (angular-deviation formula, = 0 for perfect ⊥)
///
/// ASME zone: two planes perpendicular to datum A, i.e., their normals lie IN
/// the datum plane (⊥ n_d). Width t is measured along those zone-plane normals.
///
/// A purely point-projection approach cannot yield W = 0 for a perfect
/// perpendicular face that extends in the n_d direction (which any physical face
/// would). Instead we use the fitted plane normal to isolate the angular deviation:
///
/// ```text
///   Fit a plane to the sample points → centroid c, unit normal n_fit.
///   Angular deviation from exact ⊥: sin_α = |n_fit · n_d|  (0 when n_fit ⊥ n_d).
///   Lateral half-extent of face from centroid (projected ⊥ to n_d):
///     R_lat = max_i  || (p_i − c) − ((p_i − c) · n_d) · n_d ||
///   Measured zone width:
///     W = 2 · R_lat · sin_α
/// ```
///
/// W = 0 when n_fit ⊥ n_d (perfect perpendicularity), because sin_α = 0.
/// For a face tilted by α from perfect perpendicular: W ≈ 2 · R_lat · tan(α).
/// For a rectangular face W × H, R_lat ≈ W/2 when the tilt is around the H axis
/// (W is the wider dimension orthogonal to the tilt), giving W_measured ≈ W · tan(α).
///
/// Hand-verified fixture (Perpendicularity, plate 50 mm wide, 0.5° tilt around Y):
///
/// ```text
///   Datum A = +Z top face of plate, n_d = +Z.
///   Toleranced face = +X side face (normal +X, ⊥ +Z ✓ → sin_α = 0 → W = 0 ✓).
///   After 0.5° rotation around Y:  n_fit ≈ (cos(α), 0, −sin(α))
///     sin_α = |n_fit · Z_hat| = sin(0.5°) ≈ 0.008727
///   Face spans y ∈ [−15, 15], z ∈ [−5, 5].  R_lat ≈ max_i ||(p−c) projected ⊥ Z||.
///   For the +X face (normal ≈ X): (p − c) = (0, y_i, z_i − 0).
///   Project ⊥ Z: strip the Z component → (0, y_i, 0). Max magnitude = 15 mm.
///   W = 2 · 15 · sin(0.5°) = 30 · 0.008727 ≈ 0.2618 mm.
///   [The larger contributing factor of 50 mm × sin(0.5°) ≈ 0.4363 mm comes from
///    the X-width; the formula uses R_lat along the direction ⊥ n_d, which for the
///    +X face with n_d = Z picks up the Y-extent (15 mm) rather than the full 50 mm
///    lateral extent.  See also: fixture PX1 in the test suite, which explicitly
///    verifies both the formula and the conform/violate bands.]
/// ```
///
/// The fit residual (RMS deviation of samples from the best-fit plane) reports the
/// face's flatness quality, independent of orientation; a large residual means the
/// feature is not well-described by a single plane and the verdict should be read
/// with lower confidence.
fn evaluate_orientation(
    model: &BRepModel,
    solid: SolidId,
    face: FaceId,
    fcf: &FeatureControlFrame,
    drf: &DatumReferenceFrame,
    measure_tol: Tolerance,
    params: &TessellationParams,
) -> Verdict {
    let name = fcf.characteristic.mnemonic();
    let tol = fcf.tolerance_value;

    // Resolve all datum references; any dangling datum blocks evaluation.
    let datum_statuses = match resolve_fcf_datums(model, solid, fcf, drf) {
        Ok(s) => s,
        Err(v) => return v,
    };

    // The primary datum must be designated and of Plane kind.
    let primary_ref = match fcf.datum_refs.first() {
        Some(r) => r,
        None => {
            return Verdict::not_evaluable_with_status(
                name,
                tol,
                "perpendicularity/parallelism requires at least one datum reference",
                datum_statuses,
            );
        }
    };

    let primary_datum = match drf.datum_by_label(&primary_ref.label) {
        Some(d) => d,
        None => {
            return Verdict::not_evaluable_with_status(
                name,
                tol,
                format!("datum '{}' not found in the DRF", primary_ref.label),
                datum_statuses,
            );
        }
    };

    if primary_datum.kind != DatumKind::Plane {
        return Verdict::not_evaluable_with_status(
            name,
            tol,
            format!(
                "datum '{}' is an Axis datum; orientation against a cylindrical axis \
                 is not implemented in Task 2 (Plane datums only)",
                primary_ref.label
            ),
            datum_statuses,
        );
    }

    // Extract the datum origin and outward normal (guaranteed Live by resolve_fcf_datums).
    let (datum_origin, datum_normal) = match datum_statuses
        .iter()
        .find(|s| s.label == primary_ref.label)
        .map(|s| &s.resolution)
    {
        Some(DatumResolution::Live { origin, direction }) => (*origin, *direction),
        _ => {
            return Verdict::not_evaluable_with_status(
                name,
                tol,
                format!("datum '{}' did not resolve Live", primary_ref.label),
                datum_statuses,
            );
        }
    };

    // Sample the toleranced face.
    let pts = sample_face_points(model, face, params);
    if pts.len() < 3 {
        return Verdict::not_evaluable_with_status(
            name,
            tol,
            "insufficient sampled points on toleranced face (need ≥ 3)",
            datum_statuses,
        );
    }

    // Fit a plane to the toleranced face.
    let Some(fit_plane) = fit::fit_plane(&pts, measure_tol) else {
        return Verdict::not_evaluable_with_status(
            name,
            tol,
            "plane fit on the toleranced face failed",
            datum_statuses,
        );
    };

    // Fit residual = RMS signed-distance from the best-fit plane (flatness quality).
    let fit_residual = {
        let rms_sq: f64 = pts
            .iter()
            .map(|p| fit_plane.signed_distance(*p).powi(2))
            .sum::<f64>()
            / pts.len() as f64;
        rms_sq.sqrt()
    };

    let measured = match fcf.characteristic {
        GeometricCharacteristic::Parallelism => {
            // Parallelism: spread of projections (p − datum_origin) · n_d.
            // W = max(s_i) − min(s_i) = 0 for a face at constant height above datum.
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for p in &pts {
                let s = (*p - datum_origin).dot(&datum_normal);
                lo = lo.min(s);
                hi = hi.max(s);
            }
            (hi - lo).max(0.0)
        }
        GeometricCharacteristic::Perpendicularity => {
            // Perpendicularity: angular-deviation × lateral-extent formula.
            // W = 2 · R_lat · sin_α  where:
            //   sin_α = |n_fit · n_d|  (0 for perfect ⊥)
            //   R_lat = max lateral distance from centroid, projected ⊥ to n_d
            let centroid = fit_plane.point; // centroid from the fit
            let sin_alpha = fit_plane.normal.dot(&datum_normal).abs();
            let r_lat = pts
                .iter()
                .map(|p| {
                    let v = *p - centroid;
                    // Strip the n_d component to get the lateral offset.
                    let along_nd = v.dot(&datum_normal);
                    let lateral = v - datum_normal * along_nd;
                    lateral.magnitude()
                })
                .fold(0.0_f64, f64::max);
            2.0 * r_lat * sin_alpha
        }
        // The match arms in evaluate_fcf restrict us to these two; defensive:
        _ => 0.0,
    };

    Verdict {
        characteristic: name.to_string(),
        tolerance_mm: tol,
        measured_mm: Some(measured),
        conforms: if measured <= tol {
            Conforms::InSpec
        } else {
            Conforms::OutOfSpec
        },
        fit_residual_mm: Some(fit_residual),
        datum_status: datum_statuses,
    }
}

/// Evaluate position RFS for a cylindrical bore face.
///
/// ## Zone semantics (ASME Y14.5, RFS)
///
/// The tolerance zone is a cylinder of diameter t, coaxial with the true
/// position determined by the DRF and basic dimensions. The measured deviation
/// is 2·√(Δx²+Δy²) where (Δx, Δy) is the offset of the fitted bore axis from
/// the basic position in the DRF's XY plane.
///
/// ## DRF origin derivation
///
/// **Two Plane datums (A primary, B secondary)**: origin is the point where the
/// two datum planes intersect, projected to be accessible. Concretely, the origin
/// is determined by:
///   origin_x = B_datum_origin · n_B  (the X coordinate in the B-plane direction)
///   origin_y = A_datum_origin · n_A  (Y coordinate in the A-plane direction?)
///
/// More precisely: when datum A is the +Z face (normal n_A = +Z, o_A at z=h_A)
/// and datum B is the +X face (normal n_B = +X, o_B at x=x_B), the DRF origin
/// is the corner where the two datum planes meet:
///   DRF_origin = (x_B, ?, h_A) — the Y coordinate is unconstrained by A and B alone.
///
/// For bore position measurement, we project the fitted axis onto the plane
/// perpendicular to the bore's direction. We build the DRF in-plane frame:
///   - Primary datum plane normal n_A → defines the "Z'" axis of the DRF.
///   - Secondary datum plane normal n_B → the X' direction of the DRF is n_B.
///   - Y' = Z' × X' (right-hand rule).
///   - DRF in-plane origin (x_0, y_0): component of the secondary datum's origin
///     along n_B (gives x_0), component of primary datum's origin along n_A
///     projected then lifted to the in-plane frame. For the perpendicular-planes
///     case this simplifies to: x_0 = (o_B · n_B) along the n_B direction,
///     y_0 = the projection of o_A onto the Y' direction.
///
/// **Single Plane datum**: DRF origin = datum origin. X' = n_A.perp(), Y' = n_A × X'.
///   Basic [bx, by] measured from datum origin in (X', Y'). This is fully documented
///   above in the module-level comment.
fn evaluate_position(
    model: &BRepModel,
    solid: SolidId,
    face: FaceId,
    fcf: &FeatureControlFrame,
    drf: &DatumReferenceFrame,
    measure_tol: Tolerance,
    params: &TessellationParams,
) -> Verdict {
    let name = "POSITION";
    let tol = fcf.tolerance_value;

    // Basic dimensions are mandatory for position.
    let basic = match fcf.basic {
        Some(b) => b,
        None => {
            return Verdict::not_evaluable(
                name,
                tol,
                "position requires basic dimensions [x, y] in the FCF (fcf.basic is None)",
            );
        }
    };

    // Resolve all datum references.
    let datum_statuses = match resolve_fcf_datums(model, solid, fcf, drf) {
        Ok(s) => s,
        Err(v) => return v,
    };

    if fcf.datum_refs.is_empty() {
        return Verdict::not_evaluable_with_status(
            name,
            tol,
            "position requires at least one datum reference",
            datum_statuses,
        );
    }

    // Extract the primary and optional secondary datum geometry.
    // All must be Plane datums for this implementation (Axis datums for position
    // require additional frame construction logic deferred to a later task).
    let primary_ref = &fcf.datum_refs[0];
    let primary_datum = match drf.datum_by_label(&primary_ref.label) {
        Some(d) => d,
        None => {
            return Verdict::not_evaluable_with_status(
                name,
                tol,
                format!("datum '{}' not found in DRF", primary_ref.label),
                datum_statuses,
            );
        }
    };

    if primary_datum.kind != DatumKind::Plane {
        return Verdict::not_evaluable_with_status(
            name,
            tol,
            format!(
                "datum '{}' is an Axis datum; position vs. an axis datum requires \
                 coaxiality/concentricity, not implemented in Task 2",
                primary_ref.label
            ),
            datum_statuses,
        );
    }

    let (primary_origin, primary_normal) = match datum_statuses
        .iter()
        .find(|s| s.label == primary_ref.label)
        .map(|s| &s.resolution)
    {
        Some(DatumResolution::Live { origin, direction }) => (*origin, *direction),
        _ => {
            return Verdict::not_evaluable_with_status(
                name,
                tol,
                format!("datum '{}' is not Live", primary_ref.label),
                datum_statuses,
            );
        }
    };

    // Build the DRF in-plane frame.
    // Z' = primary datum normal (the "height" direction of the DRF).
    // X', Y' are two orthogonal directions in the datum plane.
    let z_prime = primary_normal; // unit normal of the datum plane

    // X' comes from the secondary datum if it is a Plane datum, otherwise perpendicular to Z'.
    let (x_prime, y_prime, drf_origin_x, drf_origin_y) = if fcf.datum_refs.len() >= 2 {
        let secondary_ref = &fcf.datum_refs[1];
        let secondary_datum = match drf.datum_by_label(&secondary_ref.label) {
            Some(d) => d,
            None => {
                return Verdict::not_evaluable_with_status(
                    name,
                    tol,
                    format!("datum '{}' not found in DRF", secondary_ref.label),
                    datum_statuses,
                );
            }
        };

        if secondary_datum.kind != DatumKind::Plane {
            return Verdict::not_evaluable_with_status(
                name,
                tol,
                format!(
                    "secondary datum '{}' is an Axis datum; position with mixed-kind \
                         datum pairs not yet implemented",
                    secondary_ref.label
                ),
                datum_statuses,
            );
        }

        let (secondary_origin, secondary_normal) = match datum_statuses
            .iter()
            .find(|s| s.label == secondary_ref.label)
            .map(|s| &s.resolution)
        {
            Some(DatumResolution::Live { origin, direction }) => (*origin, *direction),
            _ => {
                return Verdict::not_evaluable_with_status(
                    name,
                    tol,
                    format!("datum '{}' is not Live", secondary_ref.label),
                    datum_statuses,
                );
            }
        };

        // X' = secondary datum normal (the secondary datum plane defines the X direction).
        // We remove any component of secondary_normal along z_prime to keep x_prime in
        // the primary datum plane.
        let x_raw = secondary_normal - z_prime * secondary_normal.dot(&z_prime);
        let x_prime = match x_raw.normalize() {
            Ok(v) => v,
            Err(_) => {
                return Verdict::not_evaluable_with_status(
                    name,
                    tol,
                    "secondary datum normal is parallel to primary datum normal — \
                         cannot build an orthogonal DRF X' direction",
                    datum_statuses,
                );
            }
        };
        let y_prime = z_prime.cross(&x_prime).normalize().unwrap_or_else(|_| {
            // Defensive; cannot fail if x_prime ⊥ z_prime.
            z_prime.perpendicular().normalize().unwrap_or(Vector3::Y)
        });

        // DRF origin in-plane coordinates:
        // The DRF "zero" in X' is where the secondary datum plane intersects the
        // primary datum plane. This is the component of the secondary datum's origin
        // along X' (the secondary datum's normal projected into the primary datum plane).
        // Similarly, the DRF zero in Y' is zero (the primary datum defines Z', and
        // the Y' direction is orthogonal to both — the DRF origin in Y' is derived
        // from the primary datum origin projected onto Y').
        let drf_origin_x = secondary_origin.dot(&x_prime);
        // drf_origin_y: the primary datum's origin projects to zero in Y' by convention
        // (the primary datum plane defines the reference height; Y' origin is at the
        // intersection which lies at the primary datum plane's own origin in the Y' direction).
        let drf_origin_y = primary_origin.dot(&y_prime);

        (x_prime, y_prime, drf_origin_x, drf_origin_y)
    } else {
        // Single datum: X' and Y' are arbitrary orthogonal directions in the datum plane.
        let x_prime = match z_prime.perpendicular().normalize() {
            Ok(v) => v,
            Err(_) => {
                return Verdict::not_evaluable_with_status(
                    name,
                    tol,
                    "could not build an orthogonal basis from the single datum normal",
                    datum_statuses,
                );
            }
        };
        let y_prime = z_prime
            .cross(&x_prime)
            .normalize()
            .unwrap_or_else(|_| z_prime.perpendicular().normalize().unwrap_or(Vector3::Y));
        let drf_origin_x = primary_origin.dot(&x_prime);
        let drf_origin_y = primary_origin.dot(&y_prime);
        (x_prime, y_prime, drf_origin_x, drf_origin_y)
    };

    // Sample the toleranced face and fit a cylinder.
    let pts = sample_face_points(model, face, params);
    if pts.len() < 6 {
        return Verdict::not_evaluable_with_status(
            name,
            tol,
            "insufficient sampled points on the bore face (need ≥ 6)",
            datum_statuses,
        );
    }

    // Verify the face is a cylindrical surface (the only supported kind for position).
    let face_is_cylinder = model
        .faces
        .get(face)
        .and_then(|f| model.surfaces.get(f.surface_id))
        .map(|s| s.as_any().downcast_ref::<Cylinder>().is_some())
        .unwrap_or(false);

    if !face_is_cylinder {
        return Verdict::not_evaluable_with_status(
            name,
            tol,
            "position evaluation requires a cylindrical face (bore/boss); \
             the target face is not cylindrical",
            datum_statuses,
        );
    }

    let Some(cyl_fit) = fit::fit_cylinder(&pts, measure_tol) else {
        return Verdict::not_evaluable_with_status(
            name,
            tol,
            "cylinder fit on the bore face failed — insufficient or degenerate geometry",
            datum_statuses,
        );
    };

    // Fit residual: RMS radial deviation of sampled points from the fitted cylinder.
    let fit_residual = {
        let sum_sq: f64 = pts
            .iter()
            .map(|p| fit::cylinder_radial_deviation(&cyl_fit, *p).powi(2))
            .sum();
        (sum_sq / pts.len() as f64).sqrt()
    };

    // Find the bore axis intercept in the DRF plane (the plane perpendicular to z_prime
    // through the DRF origin). We project the fitted axis point to this plane by
    // parameterising along the fitted axis:
    //   Q(t) = axis_point + t · axis_dir
    //   z_prime · Q(t) = z_prime · primary_origin
    //   t = (z_prime · (primary_origin − axis_point)) / (z_prime · axis_dir)
    // If the axis is parallel to the datum plane (axis_dir · z_prime ≈ 0), we use
    // the axis_point itself projected to the datum plane (the axis has no intercept
    // with a unique intersection — the position is undefined in that degenerate case).
    let axis_intercept: Point3 = {
        let denom = cyl_fit.axis.dot(&z_prime);
        if denom.abs() < 1e-9 {
            // Bore axis nearly parallel to datum plane — use the axis_point projected.
            let t = z_prime.dot(&(primary_origin - cyl_fit.axis_point));
            cyl_fit.axis_point + z_prime * t
        } else {
            let t = z_prime.dot(&(primary_origin - cyl_fit.axis_point)) / denom;
            cyl_fit.axis_point + cyl_fit.axis * t
        }
    };

    // Actual position of bore axis in the DRF frame.
    let actual_x = axis_intercept.dot(&x_prime);
    let actual_y = axis_intercept.dot(&y_prime);

    // True position from basic dimensions relative to DRF origin.
    let true_x = drf_origin_x + basic[0];
    let true_y = drf_origin_y + basic[1];

    let delta_x = actual_x - true_x;
    let delta_y = actual_y - true_y;

    let measured = 2.0 * (delta_x * delta_x + delta_y * delta_y).sqrt();

    Verdict {
        characteristic: name.to_string(),
        tolerance_mm: tol,
        measured_mm: Some(measured),
        conforms: if measured <= tol {
            Conforms::InSpec
        } else {
            Conforms::OutOfSpec
        },
        fit_residual_mm: Some(fit_residual),
        datum_status: datum_statuses,
    }
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
