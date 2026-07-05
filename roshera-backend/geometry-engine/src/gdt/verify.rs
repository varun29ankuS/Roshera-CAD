//! Kernel-verified GD&T conformance — the differentiator.
//!
//! The moat is not the tolerance vocabulary (every CAD system has that); it is
//! that **the kernel measures the actual geometry against the tolerance zone and
//! reports pass/fail from the B-Rep itself** — GD&T that cannot lie. A
//! flatness callout is verified against the real face's analytic surface; a
//! diameter ± is verified by reading the actual cylindrical face's radius. No
//! verdict is asserted by the caller, and nothing fakes a pass.
//!
//! ## Honesty contract (load-bearing)
//!
//! Verification returns a [`Conformance`] tri-state, never a bare bool:
//! * [`Conformance::InSpec`] / [`Conformance::OutOfSpec`] are reported ONLY when
//!   the kernel actually measured the geometry.
//! * [`Conformance::NotYetVerified`] is reported for a tolerance whose
//!   verification this phase does not implement. It is NEVER substituted by a
//!   silent pass. A verifier that always passes is worthless; this contract is
//!   what makes the verdict trustworthy.
//!
//! ## Measurement doctrine: ANALYTIC-FIRST, never the display mesh
//!
//! Spec C, section 1: verdicts are *"evaluated against the exact B-Rep, never a
//! mesh, never an estimate"*. The measurement source, in strict priority order:
//!
//! 1. **Exact surface reads.** When the toleranced feature's surface is
//!    analytic, its parameters ARE the measurement: a [`Cylinder`]'s
//!    `origin`/`axis`/`radius` are the exact bore axis and size (zero error); a
//!    [`Plane`]'s `normal` (outward-oriented via the face orientation sign) is
//!    the exact face direction. Nothing is fitted, so nothing is biased.
//! 2. **Analytic parameter sampling.** Where an extent or a form band must be
//!    measured over the feature (zone widths, flatness), points are evaluated
//!    ON the analytic surface via its parameterisation —
//!    `surface.point_at(u, v)` over the face's trimmed UV domain, and
//!    `curve.point_at(t)` along boundary edges — never harvested from a
//!    tessellation. Analytic samples lie exactly on the surface, so a perfect
//!    primitive measures exactly 0 form error (to float precision, not to a
//!    mesh-chord bound).
//! 3. **Least-squares fits ([`fit`])** are reserved for genuinely freeform
//!    (NURBS) targets, fed with analytic parameter-grid samples; the fit
//!    residual is reported with the verdict as the measurement's uncertainty.
//!
//! The display/export tessellation is NEVER consulted: it is a chordal
//! approximation with non-uniform vertex spacing whose centroid bias corrupts
//! fitted frames (up to r·Δθ ≈ 0.06 mm on a Ø6 bore — larger than the
//! tolerances a position callout typically adjudicates). It also made every
//! evaluation tessellate the entire owning solid — a per-eval perf hazard the
//! analytic path eliminates.

use serde::Serialize;
use std::collections::HashSet;

use crate::gdt::drf::{resolve_datum, solid_face_ids, DatumReferenceFrame, DatumResolution};
use crate::gdt::fit;
use crate::gdt::model::{
    Annotation, DatumKind, DimensionalTolerance, FeatureControlFrame, GeometricCharacteristic,
};
use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::edge::EdgeId;
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::surface::{Cylinder, Plane, Sphere};
use crate::primitives::topology_builder::BRepModel;

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
    /// RMS residual of the least-squares fit behind the measurement (mm), when
    /// a point fit was performed (form family). `None` for exact analytic
    /// reads (dimensional size) and for unverified checks. A large residual
    /// means the feature is poorly described by the fitted primitive and the
    /// verdict should be read with lower confidence.
    pub fit_residual: Option<f64>,
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
            fit_residual: None,
            detail: detail.into(),
        }
    }

    /// `in_spec` accessor matching the task's `ConformanceResult { ... in_spec }`
    /// vocabulary: `Some(true)` in-spec, `Some(false)` out, `None` unverified.
    pub fn in_spec(&self) -> Option<bool> {
        self.verdict.measured()
    }
}

// ---------------------------------------------------------------------------
// Analytic sampling — the ONLY point sources in this module.
//
// All three helpers evaluate the exact B-Rep (surface/curve parameterisations
// and topological vertices). None of them touches the tessellation.
// ---------------------------------------------------------------------------

/// Quantise a coordinate to 1e-9 mm for duplicate detection. Duplicate samples
/// (a closed surface's seam evaluates to identical points at u = 0 and
/// u = 2π; shared loop vertices repeat across edges) bias every
/// centroid-seeded fit toward the duplicated locus, so grid/boundary samples
/// are deduplicated before fitting — the same bias fix the earlier
/// mesh-vertex path applied at the seam, carried over to the analytic path.
fn quantise(x: f64) -> i64 {
    // Saturating cast is fine: coordinates beyond ±9.2e9 mm are outside any
    // sane model space and would collapse to a single sentinel key.
    (x * 1e9).round() as i64
}

/// Sample points ON the face's analytic surface: an `n × n` parameter grid
/// evaluated with `surface.point_at(u, v)`. Deduplicated (seam-safe). Never
/// consults the tessellation.
///
/// Domain selection: the surface's own `parameter_bounds()` is authoritative
/// wherever it is finite (a finite Cylinder knows u ∈ [0, 2π], v ∈ [0, h]; a
/// NURBS patch knows its knot domain). `face.uv_bounds` is the fallback for
/// unbounded surfaces (an infinite Plane) — it cannot be trusted as the
/// primary source because `Face::new` defaults it to [0, 1]² and several
/// builders never overwrite it (a 1-radian sliver of a full cylinder would
/// wreck any fit's conditioning). Every grid sample lies exactly ON the
/// analytic surface either way, so for FORM measurement the domain choice
/// affects only coverage, never membership.
fn analytic_surface_grid(model: &BRepModel, face: FaceId, samples_per_dim: usize) -> Vec<Point3> {
    let mut pts = Vec::new();
    let Some(f) = model.faces.get(face) else {
        return pts;
    };
    let Some(surface) = model.surfaces.get(f.surface_id) else {
        return pts;
    };
    let ((su0, su1), (sv0, sv1)) = surface.parameter_bounds();
    let [fu0, fu1, fv0, fv1] = f.uv_bounds;
    let (u0, u1) = if su0.is_finite() && su1.is_finite() {
        (su0, su1)
    } else {
        (fu0, fu1)
    };
    let (v0, v1) = if sv0.is_finite() && sv1.is_finite() {
        (sv0, sv1)
    } else {
        (fv0, fv1)
    };
    if !(u0.is_finite() && u1.is_finite() && v0.is_finite() && v1.is_finite()) {
        return pts;
    }
    let n = samples_per_dim.max(2);
    let mut seen: HashSet<(i64, i64, i64)> = HashSet::new();
    for i in 0..n {
        let u = u0 + (u1 - u0) * (i as f64) / ((n - 1) as f64);
        for j in 0..n {
            let v = v0 + (v1 - v0) * (j as f64) / ((n - 1) as f64);
            if let Ok(p) = surface.point_at(u, v) {
                if seen.insert((quantise(p.x), quantise(p.y), quantise(p.z))) {
                    pts.push(p);
                }
            }
        }
    }
    pts
}

/// Sample the face's BOUNDARY by walking every edge of every loop (outer +
/// inner) along its analytic 3D curve. Used for zone-extent measurements on
/// planar faces: the extremes of a linear functional over a planar face lie on
/// its boundary, and straight edges attain them exactly at the sampled
/// endpoints (curved boundary edges are covered by the interior samples of the
/// fixed per-edge density). Duplicates are harmless for a max−min spread, so
/// no dedup pass is needed here.
fn face_boundary_points(model: &BRepModel, face: FaceId, samples_per_edge: usize) -> Vec<Point3> {
    let mut pts = Vec::new();
    let Some(f) = model.faces.get(face) else {
        return pts;
    };
    let mut loop_ids = vec![f.outer_loop];
    loop_ids.extend(f.inner_loops.iter().copied());
    let n = samples_per_edge.max(2);
    for lid in loop_ids {
        let Some(l) = model.loops.get(lid) else {
            continue;
        };
        for &eid in &l.edges {
            let Some(e) = model.edges.get(eid) else {
                continue;
            };
            let Some(curve) = model.curves.get(e.curve_id) else {
                continue;
            };
            let r = e.param_range;
            for i in 0..n {
                let t = r.start + (r.end - r.start) * (i as f64) / ((n - 1) as f64);
                if let Ok(p) = curve.point_at(t) {
                    pts.push(p);
                }
            }
        }
    }
    pts
}

/// All distinct B-Rep vertex positions of a solid (outer + inner shells) —
/// exact topological data, used for the documented part-corner completion of
/// under-constrained datum reference frames.
fn solid_vertex_points(model: &BRepModel, solid: SolidId) -> Vec<Point3> {
    let mut seen = HashSet::new();
    let mut pts = Vec::new();
    for fid in solid_face_ids(model, solid) {
        let Some(f) = model.faces.get(fid) else {
            continue;
        };
        let mut loop_ids = vec![f.outer_loop];
        loop_ids.extend(f.inner_loops.iter().copied());
        for lid in loop_ids {
            let Some(l) = model.loops.get(lid) else {
                continue;
            };
            for &eid in &l.edges {
                let Some(e) = model.edges.get(eid) else {
                    continue;
                };
                for vid in [e.start_vertex, e.end_vertex] {
                    if seen.insert(vid) {
                        if let Some(v) = model.vertices.get(vid) {
                            pts.push(v.point());
                        }
                    }
                }
            }
        }
    }
    pts
}

/// The minimum coordinate of a solid's B-Rep vertices along `dir` — the
/// "part corner" coordinate in that direction. `None` for a solid with no
/// vertices (nothing to complete a frame from).
fn part_corner_coordinate(model: &BRepModel, solid: SolidId, dir: &Vector3) -> Option<f64> {
    let pts = solid_vertex_points(model, solid);
    pts.iter()
        .map(|p| p.dot(dir))
        .fold(None, |acc: Option<f64>, c| {
            Some(match acc {
                Some(a) => a.min(c),
                None => c,
            })
        })
}

/// Spread (max − min) of the points projected onto `dir`. Zero for an empty
/// set (callers guard for sufficient points before measuring).
fn projected_spread(pts: &[Point3], dir: &Vector3) -> f64 {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in pts {
        let s = p.dot(dir);
        lo = lo.min(s);
        hi = hi.max(s);
    }
    (hi - lo).max(0.0)
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

/// Default analytic parameter-grid density for face form measurements: 32 × 32
/// (≤ 1024 points) — dense enough to expose real form error on a freeform
/// patch, cheap enough for ambient re-evaluation, and O(face) not O(solid).
const FORM_GRID_SAMPLES: usize = 32;

/// Default per-edge sample count for boundary-extent measurements.
const BOUNDARY_EDGE_SAMPLES: usize = 33;

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
        // Exact analytic read — no point fit was performed.
        fit_residual: None,
        detail: format!("measured diameter {diameter:.6} mm; limits [{lower:.6}, {upper:.6}]"),
    }
}

/// Verify a form FCF against a face (flatness / cylindricity) or edge
/// (circularity / straightness). The face/edge selector is chosen by the
/// characteristic. Datum-referenced / non-form characteristics report
/// `NotYetVerified`.
///
/// Sampling is ANALYTIC: an on-surface parameter grid (`surface.point_at`),
/// never tessellation vertices — a perfect analytic primitive measures exactly
/// 0 form error, and a distorted freeform patch measures its true band.
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

    match fcf.characteristic {
        GeometricCharacteristic::Flatness => {
            let pts = analytic_surface_grid(model, face, FORM_GRID_SAMPLES);
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
            let mut sum_sq = 0.0;
            for p in &pts {
                let d = plane.signed_distance(*p);
                lo = lo.min(d);
                hi = hi.max(d);
                sum_sq += d * d;
            }
            let error = (hi - lo).max(0.0);
            let rms = (sum_sq / pts.len() as f64).sqrt();
            verdict_form(name, zone, error, Some(rms))
        }
        GeometricCharacteristic::Cylindricity => {
            let pts = analytic_surface_grid(model, face, FORM_GRID_SAMPLES);
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
            let mut sum_sq = 0.0;
            for p in &pts {
                let d = fit::cylinder_radial_deviation(&cyl, *p);
                lo = lo.min(d);
                hi = hi.max(d);
                sum_sq += d * d;
            }
            let error = (hi - lo).max(0.0);
            let rms = (sum_sq / pts.len() as f64).sqrt();
            verdict_form(name, zone, error, Some(rms))
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
            let mut sum_sq = 0.0;
            for p in &pts {
                let d = fit::circle_radial_deviation(&circle, *p);
                lo = lo.min(d);
                hi = hi.max(d);
                sum_sq += d * d;
            }
            let error = (hi - lo).max(0.0);
            let rms = (sum_sq / pts.len() as f64).sqrt();
            verdict_form(name, zone, error, Some(rms))
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
            let mut max_perp = 0.0_f64;
            let mut sum_sq = 0.0;
            for p in &pts {
                let d = line.distance(*p);
                max_perp = max_perp.max(d);
                sum_sq += d * d;
            }
            let error = 2.0 * max_perp;
            let rms = (sum_sq / pts.len() as f64).sqrt();
            verdict_form(name, zone, error, Some(rms))
        }
        other => ConformanceResult::not_verified(
            name,
            zone,
            format!("{} is not an edge form characteristic", other.mnemonic()),
        ),
    }
}

/// Build the in/out verdict for a measured form error against a zone.
fn verdict_form(name: &str, zone: f64, error: f64, fit_residual: Option<f64>) -> ConformanceResult {
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
        fit_residual,
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
/// analytically sampled surface points (an on-surface parameter grid) at a
/// given per-dimension density — e.g. to assert a measurement independently.
pub fn face_surface_samples(
    model: &BRepModel,
    face: FaceId,
    samples_per_dim: usize,
) -> Vec<Point3> {
    analytic_surface_grid(model, face, samples_per_dim)
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
// the form family (wired through the existing `verify_form_on_face` path),
// perpendicularity, parallelism, and position RFS. Every code path that cannot
// produce a real measurement returns `Conforms::NotEvaluable` with a reason —
// never a fabricated pass, never a fabricated measurement.
//
// Zone semantics summary (full derivations at each evaluator):
//
// Parallelism:      W = spread of (p − o_d)·n_d over the face
//                   (two planes PARALLEL to datum A, t apart).
//
// Perpendicularity: W = spread of p·u* over the face, where
//                   u* = normalize(n_t − (n_t·n_d)·n_d)
//                   (two planes PERPENDICULAR to datum A; u* is the zone
//                   normal that minimises the width for a planar face).
//
// Position RFS:     measured = 2·√(Δx² + Δy²) of the EXACT bore axis at the
//                   feature's axial mid-height vs the basic position in the
//                   DRF frame (see `evaluate_position` for the frame and its
//                   documented part-corner completion).

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
    /// dimension is missing, or the geometry cannot be measured. The `reason`
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
    /// Measurement uncertainty from fitting, in mm. `Some(0.0)` when the
    /// measurement is an EXACT analytic surface read (no fit performed — the
    /// surface parameters are the geometry, zero error). For freeform targets
    /// it is the RMS residual of the least-squares fit to the analytic
    /// samples; a large residual means the feature is poorly described by the
    /// fitted primitive and the verdict carries that uncertainty.
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

    fn measured(
        characteristic: &str,
        tolerance_mm: f64,
        measured_mm: f64,
        fit_residual_mm: f64,
        datum_status: Vec<DatumStatus>,
    ) -> Self {
        Self {
            characteristic: characteristic.to_string(),
            tolerance_mm,
            measured_mm: Some(measured_mm),
            conforms: if measured_mm <= tolerance_mm {
                Conforms::InSpec
            } else {
                Conforms::OutOfSpec
            },
            fit_residual_mm: Some(fit_residual_mm),
            datum_status,
        }
    }
}

/// Convert a form-family [`ConformanceResult`] (the pre-existing datum-free
/// verification path) into a Task-2 [`Verdict`] — the form family is WIRED
/// THROUGH `verify_form_on_face`, not re-implemented, so there is exactly one
/// copy of the form math to maintain.
fn verdict_from_form(r: ConformanceResult) -> Verdict {
    let conforms = match r.verdict {
        Conformance::InSpec => Conforms::InSpec,
        Conformance::OutOfSpec => Conforms::OutOfSpec,
        Conformance::NotYetVerified => Conforms::not_evaluable(r.detail.clone()),
    };
    Verdict {
        characteristic: r.characteristic,
        tolerance_mm: r.tolerance,
        measured_mm: r.actual,
        conforms,
        fit_residual_mm: r.fit_residual,
        datum_status: Vec::new(),
    }
}

/// Which orientation characteristic is being evaluated. A closed enum chosen
/// by `evaluate_fcf`'s dispatch so `evaluate_orientation` has no unreachable
/// "defensive" arm that could fabricate a measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OrientationKind {
    Perpendicularity,
    Parallelism,
}

impl OrientationKind {
    fn mnemonic(self) -> &'static str {
        match self {
            OrientationKind::Perpendicularity => "PERPENDICULARITY",
            OrientationKind::Parallelism => "PARALLELISM",
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

/// Look up a resolved Live datum's geometry from the collected statuses.
/// The caller has already run [`resolve_fcf_datums`], so a non-Live entry here
/// is an internal inconsistency — reported honestly, never defaulted.
fn live_datum_geometry(statuses: &[DatumStatus], label: &str) -> Option<(Point3, Vector3)> {
    statuses
        .iter()
        .find(|s| s.label == label)
        .and_then(|s| match &s.resolution {
            DatumResolution::Live { origin, direction } => Some((*origin, *direction)),
            DatumResolution::Dangling => None,
        })
}

/// Evaluate a geometric [`Annotation`] on a face against a [`DatumReferenceFrame`],
/// returning a certified [`Verdict`].
///
/// This is the single public entry point for Task 2 datum-referenced
/// evaluation. It first enforces the Spec-A membership discipline — the
/// toleranced face must belong to `solid` — then dispatches:
/// - **Form family (flatness, cylindricity)**: wired through the existing
///   [`verify_form_on_face`] path (single copy of the form math).
/// - **Perpendicularity / Parallelism**: exact analytic plane read for planar
///   targets; analytic-grid fit only for freeform targets.
/// - **Position RFS**: the EXACT cylinder axis read at the feature's axial
///   mid-height vs the basic position in the documented DRF frame.
///
/// Any non-evaluable condition (dangling datum, missing basic, foreign face,
/// unsupported target kind) is reported as `Conforms::NotEvaluable` — never a
/// silent pass, never a fabricated measurement.
pub fn evaluate(
    model: &BRepModel,
    solid: SolidId,
    face: FaceId,
    annotation: &Annotation,
    drf: &DatumReferenceFrame,
) -> Verdict {
    let measure_tol = Tolerance::from_distance(1e-9);

    let (name, tol) = match annotation {
        Annotation::Dimensional(_) => ("DIMENSIONAL", 0.0),
        Annotation::Geometric(fcf) => (fcf.characteristic.mnemonic(), fcf.tolerance_value),
    };

    // Spec-A membership discipline: the `solid` parameter is a contract. A
    // face outside the solid must not be measured against its DRF (the same
    // check `designate_datum` enforces at designation time).
    if !solid_face_ids(model, solid).contains(&face) {
        return Verdict::not_evaluable(
            name,
            tol,
            format!("face {face} is not a member of solid {solid} — evaluation refused"),
        );
    }

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

    match fcf.characteristic {
        // Datum-free form family on a face: wire the EXISTING form path
        // through (one copy of the math; `verify_form_on_face` measures both
        // flatness and cylindricity).
        GeometricCharacteristic::Flatness | GeometricCharacteristic::Cylindricity => {
            if !fcf.datum_refs.is_empty() {
                return Verdict::not_evaluable(
                    name,
                    tol,
                    format!(
                        "{} is a datum-free form characteristic; remove datum references",
                        name
                    ),
                );
            }
            verdict_from_form(verify_form_on_face(model, face, fcf, measure_tol))
        }

        GeometricCharacteristic::Straightness | GeometricCharacteristic::Circularity => {
            Verdict::not_evaluable(
                name,
                tol,
                "this characteristic verifies against an EDGE; use verify_form_on_edge",
            )
        }

        GeometricCharacteristic::Perpendicularity => evaluate_orientation(
            model,
            solid,
            face,
            fcf,
            OrientationKind::Perpendicularity,
            drf,
            measure_tol,
        ),
        GeometricCharacteristic::Parallelism => evaluate_orientation(
            model,
            solid,
            face,
            fcf,
            OrientationKind::Parallelism,
            drf,
            measure_tol,
        ),

        GeometricCharacteristic::Position => evaluate_position(model, solid, face, fcf, drf),

        other => Verdict::not_evaluable(
            name,
            tol,
            format!(
                "{} evaluation is not yet implemented in this phase (Task 2 scope: \
                 form family, Perpendicularity, Parallelism, Position RFS)",
                other.mnemonic()
            ),
        ),
    }
}

/// The toleranced target's direction + measurement points, read analytically.
struct OrientedTarget {
    /// Unit direction characterising the feature: the OUTWARD plane normal
    /// (exact for analytic planes; best-fit for freeform).
    normal: Vector3,
    /// Points ON the feature used for the zone-width spread.
    points: Vec<Point3>,
    /// 0.0 for an exact analytic read; RMS fit residual for freeform.
    fit_residual: f64,
}

/// Evaluate perpendicularity or parallelism of a (nominally planar) face
/// against a primary Plane datum.
///
/// ## Zone semantics — ASME Y14.5-2018 §9 (orientation), derived on paper
///
/// Let the datum plane have outward unit normal `n_d` and contain `o_d`; let
/// the toleranced face have outward unit normal `n_t` and points `{p_i}`.
///
/// ### Parallelism
///
/// Zone: two planes PARALLEL to datum A (normal `n_d`), `t` apart, containing
/// the entire surface. The width actually occupied is the spread of the
/// points' heights over the datum:
///
/// ```text
///   s_i = (p_i − o_d) · n_d
///   W   = max(s_i) − min(s_i)
/// ```
///
/// A perfect parallel planar face has constant height → W = 0 exactly. A face
/// tilted by α about an axis ⊥ n_d with extent L in the tilt direction gives
/// W = L·sin α. The face's own flatness error correctly folds into W (the
/// zone must contain the whole surface). Hand-verified fixture: 50 mm plate
/// rotated 0.5° about Y over a −Z datum → z′ = −x·sin α + 5·cos α,
/// x ∈ [−25, 25] → W = 50·sin(0.5°) = 0.4363224 mm, attained at the two
/// boundary edges x = ±25 (extremes of a linear functional over a planar face
/// lie on its boundary, which the analytic boundary sampling hits exactly).
///
/// ### Perpendicularity
///
/// Zone: two parallel planes PERPENDICULAR to datum A, `t` apart, containing
/// the entire surface. The zone planes' common normal `u` must satisfy
/// `u · n_d = 0`; the measured value is the minimal spread over admissible
/// zone orientations:
///
/// ```text
///   W = min_{u ⊥ n_d}  [ max_i(p_i·u) − min_i(p_i·u) ]
/// ```
///
/// For a planar face the minimising direction is the fit normal's
/// in-datum-plane component:
///
/// ```text
///   u* = normalize( n_t − (n_t·n_d)·n_d )
/// ```
///
/// Proof sketch: put in-face coordinates (a, b) with e₁ = n_d × u* (in-face,
/// ⊥ n_d) and e₂ the in-face direction in the u*–n_d plane; then
/// p·u(θ) = cos θ·cos α·b + sin θ·a where α is the tilt from square. The
/// spread |cos θ|·(b-extent)·cos α + |sin θ|·(a-extent) is minimised at
/// θ = 0, i.e. u = u*. Consequences, both verified by fixtures:
/// * a PERFECT perpendicular face has n_t ⊥ n_d → u* = n_t → every point
///   projects identically → W = 0 exactly (a point-projection approach along
///   u* does yield 0 — the previous formula's contrary claim was false);
/// * a face tilted by α measures W = H·tan α where H is the surface's extent
///   ALONG the datum normal direction (for the boolean-cut fixture: full
///   plate thickness T → W = T·tan α = 10 × 0.003 = 0.030000 mm exactly).
///   The width scales with the extent along n_d — NOT with the lateral
///   footprint (the old `2·R_lat·sin α` formula measured the wrong extent:
///   3× over on a 30 mm-wide face, under on a tall narrow one — a certified
///   false accept; see the review of commit 8fd3f17).
///
/// Degenerate case: a face PARALLEL to the datum (n_t ∥ n_d) has no
/// projection to orient the zone from — honestly NotEvaluable (the callout is
/// almost certainly mis-authored; parallelism is the meaningful check).
///
/// ## Measurement source (analytic-first)
///
/// * Planar target: `n_t` is read EXACTLY from the Plane surface (oriented
///   outward by the face orientation sign, as `resolve_datum` does), and the
///   spread points are analytic boundary samples (extremes of a linear
///   functional over a planar face lie on its boundary). Fit residual 0.0.
/// * Cylindrical target: orientation of an AXIS is a different zone
///   construction (a cylinder about the datum direction) — honest refusal in
///   Task 2 rather than a meaningless plane fit through cylinder samples.
/// * Freeform target: analytic parameter-grid samples, `fit_plane` for the
///   normal, RMS residual reported; the spread uses the on-surface grid so
///   the zone contains the true surface.
fn evaluate_orientation(
    model: &BRepModel,
    solid: SolidId,
    face: FaceId,
    fcf: &FeatureControlFrame,
    kind: OrientationKind,
    drf: &DatumReferenceFrame,
    measure_tol: Tolerance,
) -> Verdict {
    let name = kind.mnemonic();
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

    // Datum geometry (guaranteed Live by resolve_fcf_datums).
    let Some((datum_origin, datum_normal)) =
        live_datum_geometry(&datum_statuses, &primary_ref.label)
    else {
        return Verdict::not_evaluable_with_status(
            name,
            tol,
            format!("datum '{}' did not resolve Live", primary_ref.label),
            datum_statuses,
        );
    };

    // ---- Analytic-first target read ---------------------------------------
    let target = {
        let Some(f) = model.faces.get(face) else {
            return Verdict::not_evaluable_with_status(
                name,
                tol,
                "unknown face id",
                datum_statuses,
            );
        };
        let Some(surface) = model.surfaces.get(f.surface_id) else {
            return Verdict::not_evaluable_with_status(
                name,
                tol,
                "face has no surface",
                datum_statuses,
            );
        };

        if let Some(plane) = surface.as_any().downcast_ref::<Plane>() {
            // EXACT read: the Plane surface's normal, oriented outward.
            let normal = plane.normal * f.orientation.sign();
            let points = face_boundary_points(model, face, BOUNDARY_EDGE_SAMPLES);
            if points.len() < 3 {
                return Verdict::not_evaluable_with_status(
                    name,
                    tol,
                    "insufficient boundary samples on the toleranced planar face",
                    datum_statuses,
                );
            }
            OrientedTarget {
                normal,
                points,
                fit_residual: 0.0,
            }
        } else if surface.as_any().downcast_ref::<Cylinder>().is_some() {
            return Verdict::not_evaluable_with_status(
                name,
                tol,
                "orientation of a cylindrical feature's axis is not implemented in \
                 Task 2 — the toleranced target must be a planar face",
                datum_statuses,
            );
        } else {
            // Freeform fallback: on-surface analytic parameter grid + plane fit.
            let points = analytic_surface_grid(model, face, FORM_GRID_SAMPLES);
            if points.len() < 3 {
                return Verdict::not_evaluable_with_status(
                    name,
                    tol,
                    "insufficient analytic samples on the toleranced face",
                    datum_statuses,
                );
            }
            let Some(fit_plane) = fit::fit_plane(&points, measure_tol) else {
                return Verdict::not_evaluable_with_status(
                    name,
                    tol,
                    "plane fit on the toleranced freeform face failed",
                    datum_statuses,
                );
            };
            let rms = (points
                .iter()
                .map(|p| fit_plane.signed_distance(*p).powi(2))
                .sum::<f64>()
                / points.len() as f64)
                .sqrt();
            OrientedTarget {
                normal: fit_plane.normal,
                points,
                fit_residual: rms,
            }
        }
    };

    // ---- Zone width ---------------------------------------------------------
    let measured = match kind {
        OrientationKind::Parallelism => {
            // W = spread of (p − o_d)·n_d — subtracting the datum origin keeps
            // the projections small (numerical hygiene); the spread itself is
            // origin-invariant.
            let shifted: Vec<Point3> = target.points.iter().map(|p| *p - datum_origin).collect();
            projected_spread(&shifted, &datum_normal)
        }
        OrientationKind::Perpendicularity => {
            let u_raw = target.normal - datum_normal * target.normal.dot(&datum_normal);
            let u_star = match u_raw.normalize() {
                Ok(u) => u,
                Err(_) => {
                    return Verdict::not_evaluable_with_status(
                        name,
                        tol,
                        format!(
                            "the toleranced face is parallel to datum '{}' — a \
                             perpendicularity zone cannot be oriented from its normal \
                             (did you mean PARALLELISM?)",
                            primary_ref.label
                        ),
                        datum_statuses,
                    );
                }
            };
            projected_spread(&target.points, &u_star)
        }
    };

    Verdict::measured(name, tol, measured, target.fit_residual, datum_statuses)
}

/// Evaluate position RFS for a cylindrical bore/boss face.
///
/// ## Zone semantics (ASME Y14.5-2018 §10, RFS)
///
/// The tolerance zone is a cylinder of diameter `t`, coaxial with the true
/// position determined by the DRF and the basic dimensions. The measured
/// deviation is diametral: `2·√(Δx² + Δy²)`, where (Δx, Δy) is the offset of
/// the feature's axis from the basic position in the DRF's X′/Y′ plane.
///
/// ## Measurement source (exact — zero fitting error)
///
/// The target must be a [`Cylinder`] face; its surface parameters ARE the
/// feature: `cyl.origin`/`cyl.axis` give the exact axis line. The axis
/// position is taken **at the feature's axial mid-height**: with the face's
/// trimmed axial range `v ∈ [v₀, v₁]` (the Cylinder parameterisation is
/// `p(u, v) = origin + radial(u) + axis·v`),
///
/// ```text
///   q_mid = origin + axis · (v₀ + v₁)/2
/// ```
///
/// For a bore square to the datum this equals the intercept at any height;
/// for a tilted bore the mid-height point is the location that minimises the
/// worst end-deviation (the two ends deviate symmetrically about it). The
/// full over-the-feature-length zone check (both ends inside the cylinder) is
/// an MMC/LMC-era refinement, deferred with this documented convention.
///
/// ## DRF frame construction — derived, never from surface parameterisation
///
/// Y14.5 degree-of-freedom analysis for the supported frames (primary datum
/// plane normal `n_A =: Z′`):
///
/// * a primary plane A pins translation along Z′ and rotations about X′/Y′;
/// * a secondary plane B (not parallel to A) pins translation along X′ and
///   rotation about Z′;
/// * translation along Y′ needs a tertiary datum — absent one, the kernel
///   completes it from the PART CORNER (below), a documented convention.
///
/// **Two Plane datums (A | B):**
/// ```text
///   Z′ = n_A                                  (outward datum normal)
///   X′ = normalize( (−n_B) − ((−n_B)·Z′)·Z′ ) (secondary's INWARD normal
///                                              projected into the datum
///                                              plane — so basics are
///                                              positive distances INTO the
///                                              material from datum B)
///   Y′ = Z′ × X′
///   x₀ = X′-coordinate of the A∩B intersection LINE, solved from the two
///        plane equations p·n_A = d_A, p·n_B = d_B with d = o·n (invariant
///        under re-parameterisation of the plane surfaces — never the
///        arbitrary `plane.origin` representative point):
///          c = n_A·n_B,  a = (d_A − c·d_B)/(1 − c²),  b = (d_B − c·d_A)/(1 − c²)
///          p₀ = a·n_A + b·n_B   (the point of A∩B in the span of the normals)
///          x₀ = p₀·X′           (constant along the line, since Y′ ⊥ X′)
///   y₀ = part-corner completion of the FREE Y′ translation:
///          y₀ = min over the solid's B-Rep vertices of v·Y′
/// ```
/// Parallel datum planes cannot pin X′ → honest refusal naming the
/// degeneracy.
///
/// **Single Plane datum (A):** X′/Y′ take the documented world-axis seed —
/// `X′` = the world X axis projected into the datum plane (world Y seed when
/// `|n_A·X̂| > 0.9`), `Y′ = Z′ × X′` — and BOTH in-plane origin coordinates
/// come from the part corner: `x₀ = min v·X′`, `y₀ = min v·Y′`.
///
/// **The `basic [x, y]` meaning** (also documented on
/// [`FeatureControlFrame::basic`]): millimetres from (x₀, y₀) along (X′, Y′).
/// For the canonical plate-with-datums-on-left/bottom setup this reads
/// exactly like the drawing: distances from the part edges into the material.
fn evaluate_position(
    model: &BRepModel,
    solid: SolidId,
    face: FaceId,
    fcf: &FeatureControlFrame,
    drf: &DatumReferenceFrame,
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

    let primary_ref = match fcf.datum_refs.first() {
        Some(r) => r,
        None => {
            return Verdict::not_evaluable_with_status(
                name,
                tol,
                "position requires at least one datum reference",
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
                 coaxiality frame construction, not implemented in Task 2",
                primary_ref.label
            ),
            datum_statuses,
        );
    }

    let Some((primary_origin, primary_normal)) =
        live_datum_geometry(&datum_statuses, &primary_ref.label)
    else {
        return Verdict::not_evaluable_with_status(
            name,
            tol,
            format!("datum '{}' is not Live", primary_ref.label),
            datum_statuses,
        );
    };

    // ---- DRF frame (see the doc comment derivation) ------------------------
    let z_prime = primary_normal;

    let (x_prime, y_prime, x0, y0) = if fcf.datum_refs.len() >= 2 {
        // Secondary datum path: X′ from datum B, x₀ from the A∩B line.
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
                     datum pairs is not implemented in Task 2",
                    secondary_ref.label
                ),
                datum_statuses,
            );
        }

        let Some((secondary_origin, secondary_normal)) =
            live_datum_geometry(&datum_statuses, &secondary_ref.label)
        else {
            return Verdict::not_evaluable_with_status(
                name,
                tol,
                format!("datum '{}' is not Live", secondary_ref.label),
                datum_statuses,
            );
        };

        // X′ = the secondary datum's INWARD normal projected into the primary
        // datum plane, so basics measure positive INTO the material.
        let inward = -secondary_normal;
        let x_raw = inward - z_prime * inward.dot(&z_prime);
        let x_prime = match x_raw.normalize() {
            Ok(v) => v,
            Err(_) => {
                return Verdict::not_evaluable_with_status(
                    name,
                    tol,
                    format!(
                        "secondary datum '{}' is parallel to primary datum '{}' — \
                         parallel planes cannot pin the X′ direction; choose a \
                         non-parallel secondary datum",
                        secondary_ref.label, primary_ref.label
                    ),
                    datum_statuses,
                );
            }
        };
        let y_prime = match z_prime.cross(&x_prime).normalize() {
            Ok(v) => v,
            Err(_) => {
                return Verdict::not_evaluable_with_status(
                    name,
                    tol,
                    "degenerate DRF: Z′ × X′ is not a valid direction",
                    datum_statuses,
                );
            }
        };

        // x₀ from the A∩B intersection line (plane-equation solve; invariant
        // under surface re-parameterisation).
        let d_a = primary_origin.dot(&primary_normal);
        let d_b = secondary_origin.dot(&secondary_normal);
        let c = primary_normal.dot(&secondary_normal);
        let denom = 1.0 - c * c;
        if denom.abs() < 1e-12 {
            return Verdict::not_evaluable_with_status(
                name,
                tol,
                format!(
                    "datum planes '{}' and '{}' are parallel — no intersection line \
                     to anchor the DRF origin",
                    primary_ref.label, secondary_ref.label
                ),
                datum_statuses,
            );
        }
        let a = (d_a - c * d_b) / denom;
        let b = (d_b - c * d_a) / denom;
        let p0 = primary_normal * a + secondary_normal * b;
        let x0 = p0.dot(&x_prime);

        // y₀ = part-corner completion of the Y′ translation A|B leaves free.
        let Some(y0) = part_corner_coordinate(model, solid, &y_prime) else {
            return Verdict::not_evaluable_with_status(
                name,
                tol,
                "the solid has no B-Rep vertices to complete the DRF origin from",
                datum_statuses,
            );
        };

        (x_prime, y_prime, x0, y0)
    } else {
        // Single-datum path: documented world-axis seed + part-corner origin.
        let seed = if z_prime.dot(&Vector3::X).abs() < 0.9 {
            Vector3::X
        } else {
            Vector3::Y
        };
        let x_raw = seed - z_prime * seed.dot(&z_prime);
        let x_prime = match x_raw.normalize() {
            Ok(v) => v,
            Err(_) => {
                return Verdict::not_evaluable_with_status(
                    name,
                    tol,
                    "could not build an in-plane X′ axis from the datum normal",
                    datum_statuses,
                );
            }
        };
        let y_prime = match z_prime.cross(&x_prime).normalize() {
            Ok(v) => v,
            Err(_) => {
                return Verdict::not_evaluable_with_status(
                    name,
                    tol,
                    "degenerate DRF: Z′ × X′ is not a valid direction",
                    datum_statuses,
                );
            }
        };
        let Some(x0) = part_corner_coordinate(model, solid, &x_prime) else {
            return Verdict::not_evaluable_with_status(
                name,
                tol,
                "the solid has no B-Rep vertices to complete the DRF origin from",
                datum_statuses,
            );
        };
        let Some(y0) = part_corner_coordinate(model, solid, &y_prime) else {
            return Verdict::not_evaluable_with_status(
                name,
                tol,
                "the solid has no B-Rep vertices to complete the DRF origin from",
                datum_statuses,
            );
        };
        (x_prime, y_prime, x0, y0)
    };

    // ---- Exact analytic feature read ---------------------------------------
    let Some(f) = model.faces.get(face) else {
        return Verdict::not_evaluable_with_status(name, tol, "unknown face id", datum_statuses);
    };
    let Some(surface) = model.surfaces.get(f.surface_id) else {
        return Verdict::not_evaluable_with_status(
            name,
            tol,
            "face has no surface",
            datum_statuses,
        );
    };
    let Some(cyl) = surface.as_any().downcast_ref::<Cylinder>() else {
        return Verdict::not_evaluable_with_status(
            name,
            tol,
            "position evaluation requires a cylindrical face (bore/boss); \
             the target face is not cylindrical",
            datum_statuses,
        );
    };

    // The exact axis, evaluated at the feature's axial mid-height (see the
    // doc comment): Cylinder parameterisation puts v along the axis.
    let v_mid = 0.5 * (f.uv_bounds[2] + f.uv_bounds[3]);
    let q_mid = cyl.origin + cyl.axis * v_mid;

    let actual_x = q_mid.dot(&x_prime);
    let actual_y = q_mid.dot(&y_prime);

    // True position from basic dimensions relative to the derived DRF origin.
    let true_x = x0 + basic[0];
    let true_y = y0 + basic[1];

    let delta_x = actual_x - true_x;
    let delta_y = actual_y - true_y;

    // Diametral deviation (RFS): 2·√(Δx² + Δy²).
    // Hand-verified: bore drilled at (30.04, 20.03) vs basic (30, 20) from a
    // zero corner → Δ = (0.04, 0.03) → measured = 2·0.05 = 0.100000 mm.
    let measured = 2.0 * (delta_x * delta_x + delta_y * delta_y).sqrt();

    // Exact analytic read — no fit was performed, so the measurement carries
    // zero fitting uncertainty.
    Verdict::measured(name, tol, measured, 0.0, datum_statuses)
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
        assert!(r.fit_residual.is_none());
    }

    #[test]
    fn fit_class_dimensional_is_not_verified() {
        // No geometry needed: a fit-class tolerance has no resolved envelope.
        let dim = DimensionalTolerance::fit(10.0, "H7");
        assert!(matches!(dim.bound, ToleranceBound::Fit(_)));
        assert_eq!(dim.limit_range(), None);
    }

    #[test]
    fn form_conversion_preserves_tri_state() {
        // NotYetVerified from the form path must surface as NotEvaluable with
        // the same reason text — never as a pass.
        let r = ConformanceResult::not_verified("FLATNESS", 0.05, "insufficient sampled points");
        let v = verdict_from_form(r);
        match &v.conforms {
            Conforms::NotEvaluable { reason } => {
                assert!(reason.contains("insufficient"), "reason preserved");
            }
            other => panic!("expected NotEvaluable, got {other:?}"),
        }
        assert!(v.measured_mm.is_none());
    }
}
