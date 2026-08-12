//! FIDELITY — "is the geometry you asked for the geometry you got?"
//!
//! Soundness measures TOPOLOGY: closed, manifold, oriented, non-self-
//! intersecting. It is silent on SHAPE. The motivating measurement is in
//! `tests/capability_probe_loft_sweep_pattern.rs`: a loft of two circles used
//! to densify its vertex correspondence to a hard floor of 8, producing an
//! octagonal frustum — a genuinely closed, genuinely manifold solid, certified
//! SOUND, carrying a 9.97% volume shortfall against the circular closed form.
//! Nothing in the kernel's verdict could see it, because nothing in the
//! kernel's verdict compares the RESULT to the REQUEST.
//!
//! This module is that comparison. It measures the built solid with machinery
//! that already exists (the tessellation the op already produced, the trimmed
//! face-area integral `queries::measure` already uses) and reports
//! requested / measured / relative deviation per named quantity.
//!
//! # Contract
//!
//! * A fidelity deviation NEVER flips `sound`. The topology of an octagonal
//!   frustum is genuinely sound; saying otherwise would be a different lie.
//!   [`FidelityReport::fidelity_ok`] is its own verdict, disclosed beside the
//!   certificate, never folded into it.
//! * A quantity that cannot be measured is ABSENT with a stated reason
//!   ([`FidelityReport::gaps`]), never reported as `0.0`. A fabricated zero
//!   would read as "requested 7, got 0" — a louder lie than silence.
//! * The report is a function of the REQUEST and the RESULT together, so it
//!   deliberately does NOT live on [`crate::primitives::provenance::ValidityCertificate`]:
//!   that certificate is memoized per-solid and computed from the solid alone,
//!   and a request-dependent field on it would make the same cached answer
//!   depend on who asked.
//!
//! # What "measured" means, per op class
//!
//! | op        | quantities                              | measured from                          |
//! |-----------|-----------------------------------------|----------------------------------------|
//! | cylinder  | radius, height                          | tessellation extents about the axis    |
//! | box       | width, depth, height                    | tessellation extents along u, v, u×v   |
//! | revolve   | meridian max radius, meridian extent    | tessellation extents about the axis    |
//! | loft      | end-cap cross-section areas             | tessellated cap triangles (see below)  |
//!
//! The loft row is deliberately NOT the trimmed face-area integral
//! `queries::measure` uses for ordinary planar faces. That was the first
//! choice and was MEASURED not to work on this shape — a `nurbs_loft` cap's
//! outer loop is one closed periodic-NURBS edge, so the integral refuses —
//! and [`mesh_cross_section_area`] carries the observation in full, pinned by
//! `loft_cap_faces_are_found_but_the_trimmed_integral_is_not_the_path`. A
//! header that named the integral here would be this module's own kind of lie.
//!
//! Extremal statistics (max radial distance, projected span) are used for the
//! swept/extruded classes rather than means: a tessellation vertex lies ON the
//! true surface, so an extremum is unbiased by faceting while a mean is not.
//! Using a mean would make a perfect cylinder report a spurious few-tenths-of-
//! a-percent "deviation" that is pure discretization.
//!
//! The loft's quantity is cross-section AREA rather than a radius because area
//! is the statistic the motivating defect actually moved: an octagon inscribed
//! in its circle has every vertex exactly ON the circle (a radius probe reads
//! it as perfect) while enclosing 9.97% less area.

use crate::math::{Point3, Vector3};
use crate::primitives::solid::SolidId;
use crate::primitives::surface::Plane;
use crate::primitives::topology_builder::BRepModel;
use crate::tessellation::TriangleMesh;
use serde::{Deserialize, Serialize};

/// Default relative-deviation band beyond which a result stops being "the
/// geometry that was asked for".
///
/// 2% is the budget the kernel's own capability probe
/// (`tests/capability_probe_loft_sweep_pattern.rs`) already asserts for
/// loft/sweep against their analytic closed forms, so this band is that
/// existing, measured standard rather than a fresh invention. It is a PRODUCT
/// decision, not a law of geometry: the raw requested/measured numbers ride
/// along on every quantity so a caller with a tighter contract can apply its
/// own band without re-measuring.
///
/// Calibration points it separates: an analytic primitive measures at ~1e-15,
/// the fixed loft residual at 0.19%, the octagon defect at 9.97%.
pub const DEFAULT_FIDELITY_TOLERANCE: f64 = 0.02;

/// One requested-vs-measured pair.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FidelityQuantity {
    /// Which dimension this is (`radius`, `height`, `cap_area_bottom`, …).
    pub name: String,
    /// The number the REQUEST carried.
    pub requested: f64,
    /// The number the BUILT solid actually carries.
    pub measured: f64,
    /// `|measured − requested| / |requested|`, or the absolute difference when
    /// the requested value is (near) zero — never a division by ~0. This is the
    /// magnitude the tolerance band is judged against.
    pub relative_deviation: f64,
    /// The same number WITH ITS SIGN: `(measured − requested) / |requested|`.
    ///
    /// The direction is the diagnosis, and dropping it loses the distinction
    /// that matters most. NEGATIVE means the kernel built LESS than was asked
    /// for — the octagon class, where a circular request came back as an
    /// inscribed polygon enclosing 9.97% less area, and the class that costs
    /// material and function. POSITIVE means it built MORE — typically a smooth
    /// interpolation running outside a coarsely-sampled request, which is the
    /// operation doing what it documents. Both are worth disclosing and they
    /// are not the same fact.
    pub signed_relative_deviation: f64,
    /// How `measured` was obtained. Stated so a caller can judge the number
    /// instead of trusting it.
    pub method: String,
}

impl FidelityQuantity {
    /// Build a quantity, computing the relative deviation honestly.
    pub fn new(name: &str, requested: f64, measured: f64, method: &str) -> Self {
        // A requested value of ~0 has no meaningful RELATIVE deviation; fall
        // back to the absolute difference rather than dividing by ~0 and
        // reporting an infinity that reads like a catastrophic failure.
        let denom = requested.abs();
        let signed_relative_deviation = if denom > 1e-12 {
            (measured - requested) / denom
        } else {
            measured - requested
        };
        Self {
            name: name.to_string(),
            requested,
            measured,
            relative_deviation: signed_relative_deviation.abs(),
            signed_relative_deviation,
            method: method.to_string(),
        }
    }
}

/// A quantity the request DOES carry but the result could not be measured
/// against. Absent with a reason — never a fabricated zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FidelityGap {
    pub name: String,
    pub reason: String,
}

/// The op-level fidelity verdict: every measurable requested dimension against
/// what was built.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FidelityReport {
    /// The operation this describes (`cylinder`, `box`, `revolve`, `loft`).
    pub op: String,
    /// The band `fidelity_ok` is judged against (see
    /// [`DEFAULT_FIDELITY_TOLERANCE`]).
    pub tolerance: f64,
    /// Every quantity that WAS measured.
    pub quantities: Vec<FidelityQuantity>,
    /// Every quantity that was NOT, and why.
    pub gaps: Vec<FidelityGap>,
}

impl FidelityReport {
    /// Empty report for `op`, at the default band.
    pub fn new(op: &str) -> Self {
        Self {
            op: op.to_string(),
            tolerance: DEFAULT_FIDELITY_TOLERANCE,
            quantities: Vec::new(),
            gaps: Vec::new(),
        }
    }

    /// Record a measured quantity.
    pub fn measured(&mut self, name: &str, requested: f64, measured: f64, method: &str) {
        self.quantities
            .push(FidelityQuantity::new(name, requested, measured, method));
    }

    /// Record a quantity that could not be measured, and why.
    pub fn gap(&mut self, name: &str, reason: &str) {
        self.gaps.push(FidelityGap {
            name: name.to_string(),
            reason: reason.to_string(),
        });
    }

    /// The worst measured deviation, or `None` when nothing was measured.
    pub fn worst(&self) -> Option<&FidelityQuantity> {
        self.quantities.iter().max_by(|a, b| {
            a.relative_deviation
                .partial_cmp(&b.relative_deviation)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    /// Every measured quantity is inside the band — or `None` when NOTHING was
    /// measured.
    ///
    /// The `None` is the whole point and it is deliberately not a `bool`. A
    /// report with no measured quantities has no verdict to give: `false` would
    /// assert a defect nobody observed, and `true` would be a green boolean
    /// sitting over an unmeasured quantity — which is precisely the
    /// "certified sound at 9.97%" pattern this module exists to end, in
    /// miniature. A thin client that keys off a boolean and never reads
    /// `quantities` would swallow the lie; an `Option` makes it impossible to
    /// read the verdict without meeting the absence.
    pub fn fidelity_ok(&self) -> Option<bool> {
        if self.quantities.is_empty() {
            return None;
        }
        Some(
            self.quantities
                .iter()
                .all(|q| q.relative_deviation <= self.tolerance),
        )
    }

    /// True when the report carries nothing at all — the caller should omit the
    /// whole block rather than emit an empty one.
    pub fn is_empty(&self) -> bool {
        self.quantities.is_empty() && self.gaps.is_empty()
    }
}

// ─── Measurement primitives (existing machinery only) ───────────────────────

/// Extents of a tessellated solid measured in an axis frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisExtents {
    /// Smallest projection of any mesh vertex onto the axis.
    pub axial_min: f64,
    /// Largest projection of any mesh vertex onto the axis.
    pub axial_max: f64,
    /// Largest perpendicular distance of any mesh vertex from the axis line.
    pub max_radial: f64,
}

impl AxisExtents {
    /// Length of the solid along the axis.
    pub fn axial_span(&self) -> f64 {
        self.axial_max - self.axial_min
    }
}

fn delta(a: Point3, b: Point3) -> Vector3 {
    Vector3::new(a.x - b.x, a.y - b.y, a.z - b.z)
}

/// Measure a tessellated solid in the frame of a requested axis.
///
/// Every mesh vertex lies ON the built surface (the tessellator samples the
/// surface; it never invents points off it), so `max_radial` and the axial
/// span are exact readings of the built geometry rather than faceting
/// artifacts. `None` when the mesh is empty or the axis is degenerate —
/// callers turn that into a stated gap, never a zero.
pub fn mesh_axis_extents(
    mesh: &TriangleMesh,
    origin: Point3,
    axis: Vector3,
) -> Option<AxisExtents> {
    let n = axis.normalize().ok()?;
    let mut axial_min = f64::INFINITY;
    let mut axial_max = f64::NEG_INFINITY;
    let mut max_radial: f64 = 0.0;
    for v in &mesh.vertices {
        let d = delta(v.position, origin);
        let t = d.dot(&n);
        if !t.is_finite() {
            continue;
        }
        axial_min = axial_min.min(t);
        axial_max = axial_max.max(t);
        let radial = Vector3::new(d.x - n.x * t, d.y - n.y * t, d.z - n.z * t).magnitude();
        if radial.is_finite() {
            max_radial = max_radial.max(radial);
        }
    }
    if !axial_min.is_finite() || !axial_max.is_finite() {
        return None;
    }
    Some(AxisExtents {
        axial_min,
        axial_max,
        max_radial,
    })
}

/// A requested cross-section ring reduced to its plane and enclosed area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RingPlane {
    pub centroid: Point3,
    /// Unit normal from Newell's formula (robust for near-degenerate rings).
    pub normal: Vector3,
    /// Enclosed area of the ring polygon.
    pub area: f64,
}

/// Fit the plane of a closed ring of points and compute its enclosed area.
///
/// Newell's formula: the summed cross products of consecutive edge vectors
/// about the centroid give twice the signed area vector, whose magnitude is
/// twice the polygon area and whose direction is the plane normal. Exact for a
/// planar polygon and stable for a slightly non-planar one. `None` for a ring
/// of fewer than three points or one enclosing no area.
pub fn ring_plane(points: &[Point3]) -> Option<RingPlane> {
    if points.len() < 3 {
        return None;
    }
    let count = points.len() as f64;
    let mut cx = 0.0;
    let mut cy = 0.0;
    let mut cz = 0.0;
    for p in points {
        cx += p.x;
        cy += p.y;
        cz += p.z;
    }
    let centroid = Point3::new(cx / count, cy / count, cz / count);

    let mut nx = 0.0;
    let mut ny = 0.0;
    let mut nz = 0.0;
    for (i, p) in points.iter().enumerate() {
        let q = points.get((i + 1) % points.len())?;
        let a = delta(*p, centroid);
        let b = delta(*q, centroid);
        let c = a.cross(&b);
        nx += c.x;
        ny += c.y;
        nz += c.z;
    }
    let raw = Vector3::new(nx, ny, nz);
    let twice_area = raw.magnitude();
    if !(twice_area.is_finite() && twice_area > 1e-15) {
        return None;
    }
    let normal = raw.normalize().ok()?;
    Some(RingPlane {
        centroid,
        normal,
        area: twice_area / 2.0,
    })
}

/// Faces of `solid_id` whose supporting surface is a PLANE coincident with the
/// given plane (parallel normals in either sense — a cap's outward normal may
/// point away from the requested ring's winding — and coincident origin).
///
/// Exposed because "which faces are the cap" is the load-bearing step of the
/// loft measurement and a caller that gets a stated gap deserves to be able to
/// ask why.
pub fn planar_faces_in_plane(
    model: &BRepModel,
    solid_id: SolidId,
    origin: Point3,
    normal: Vector3,
    distance_tol: f64,
) -> Vec<u32> {
    let Ok(unit) = normal.normalize() else {
        return Vec::new();
    };
    let Some(solid) = model.solids.get(solid_id) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for shell_id in solid.all_shells() {
        let Some(shell) = model.shells.get(shell_id) else {
            continue;
        };
        for &face_id in &shell.faces {
            let Some(face) = model.faces.get(face_id) else {
                continue;
            };
            let Some(surface) = model.surfaces.get(face.surface_id) else {
                continue;
            };
            let Some(plane) = surface.as_any().downcast_ref::<Plane>() else {
                continue;
            };
            let Ok(pn) = plane.normal.normalize() else {
                continue;
            };
            if pn.dot(&unit).abs() > 0.999_99
                && delta(plane.origin, origin).dot(&unit).abs() <= distance_tol
            {
                out.push(face_id);
            }
        }
    }
    out
}

/// Area of the built cross-section lying in the given plane, read off the
/// tessellation the operation already produced.
///
/// The cap IS a face of the solid, and `TriangleMesh::face_map` says which
/// triangles came from it, so summing those triangles' areas measures the
/// cross-section the kernel actually built. The face's own trimmed area
/// integral ([`crate::primitives::face::Face::area`]) was the first choice and
/// was MEASURED not to work here: a `nurbs_loft` cap's outer loop is ONE closed
/// periodic-NURBS edge — a single self-closing edge, so the loop walk sees one
/// distinct vertex and the integral refuses with
/// `InvalidParameter("Loop has fewer than 3 vertices")`. The cap face itself is
/// found without trouble; only the integral declines
/// (`loft_cap_faces_are_found_but_the_trimmed_integral_is_not_the_path` pins
/// both halves of that observation). The tessellated reading is
/// chord-controlled, so its own discretization error is orders of magnitude
/// below the fidelity band it feeds: a 64-point loft ring measured 0.0949%
/// against its requested area, where the same ring at the historical 8-vertex
/// density measured 10.86%.
///
/// `None` when no face lies in that plane or no triangle came from one — the
/// caller states that as a gap rather than reporting a fabricated 0.
pub fn mesh_cross_section_area(
    model: &BRepModel,
    mesh: &TriangleMesh,
    solid_id: SolidId,
    origin: Point3,
    normal: Vector3,
    distance_tol: f64,
) -> Option<f64> {
    let faces = planar_faces_in_plane(model, solid_id, origin, normal, distance_tol);
    if faces.is_empty() {
        return None;
    }
    let mut total = 0.0;
    let mut counted = 0usize;
    for (index, tri) in mesh.triangles.iter().enumerate() {
        let Some(face_id) = mesh.face_map.get(index) else {
            continue;
        };
        if !faces.contains(face_id) {
            continue;
        }
        let (Some(a), Some(b), Some(c)) = (
            mesh.vertices.get(tri[0] as usize),
            mesh.vertices.get(tri[1] as usize),
            mesh.vertices.get(tri[2] as usize),
        ) else {
            continue;
        };
        let area = delta(b.position, a.position)
            .cross(&delta(c.position, a.position))
            .magnitude()
            / 2.0;
        if area.is_finite() {
            total += area;
            counted += 1;
        }
    }
    if counted == 0 {
        None
    } else {
        Some(total)
    }
}

// ─── Per-op reports ─────────────────────────────────────────────────────────

/// Cylinder primitive: requested radius/height against the built solid.
///
/// THE CALIBRATION CASE. An analytic cylinder is exactly what was asked for, so
/// this must read ~0 deviation. A non-trivial number here means the statistic
/// is wrong, not the kernel.
pub fn cylinder_fidelity(
    mesh: &TriangleMesh,
    center: Point3,
    axis: Vector3,
    radius: f64,
    height: f64,
) -> FidelityReport {
    let mut report = FidelityReport::new("cylinder");
    match mesh_axis_extents(mesh, center, axis) {
        Some(e) => {
            report.measured(
                "radius",
                radius,
                e.max_radial,
                "largest perpendicular distance of a tessellation vertex from the requested axis",
            );
            report.measured(
                "height",
                height,
                e.axial_span(),
                "span of the tessellation projected onto the requested axis",
            );
        }
        None => report.gap(
            "radius,height",
            "the built solid tessellated to no vertices, or the requested axis is degenerate — \
             extents about the axis are undefined, so nothing is compared",
        ),
    }
    report
}

/// Box primitive: requested width/depth/height against the built solid,
/// measured in the box's OWN frame (u, v, u×v) rather than a world bbox, so an
/// arbitrarily-posed box is judged on what was asked for.
pub fn box_fidelity(
    mesh: &TriangleMesh,
    center: Point3,
    u_axis: Vector3,
    v_axis: Vector3,
    width: f64,
    depth: f64,
    height: f64,
) -> FidelityReport {
    let mut report = FidelityReport::new("box");
    let w_axis = u_axis.cross(&v_axis);
    let along = |dir: Vector3| mesh_axis_extents(mesh, center, dir).map(|e| e.axial_span());
    let spans = (along(u_axis), along(v_axis), along(w_axis));
    match spans {
        (Some(u), Some(v), Some(w)) => {
            let method = "span of the tessellation projected onto the requested frame axis";
            report.measured("width", width, u, method);
            report.measured("depth", depth, v, method);
            report.measured("height", height, w, method);
        }
        _ => report.gap(
            "width,depth,height",
            "the built solid tessellated to no vertices, or the requested u/v frame is \
             degenerate (u parallel to v) — spans in the box frame are undefined",
        ),
    }
    report
}

/// Revolve: the requested MERIDIAN's extents against the built solid of
/// revolution.
///
/// A meridian point is `[r, z]` — radius from the axis, height along it. The
/// two facts a revolution must preserve are the largest radius the profile
/// reaches and the axial length it spans; both survive a partial `angle_deg`
/// (the extreme point is still swept) and a bore (which only removes material
/// inside `r_max`). Spans, not absolute positions, are compared: where the
/// profile sits along the axis is placement, not fidelity.
pub fn revolve_fidelity(
    mesh: &TriangleMesh,
    axis_origin: Point3,
    axis_direction: Vector3,
    profile: &[(f64, f64)],
) -> FidelityReport {
    let mut report = FidelityReport::new("revolve");
    if profile.is_empty() {
        report.gap(
            "meridian_max_radius,meridian_axial_extent",
            "no sampled [r,z] meridian was supplied, so the request carries no extents to \
             compare against",
        );
        return report;
    }
    let mut r_max = f64::NEG_INFINITY;
    let mut z_min = f64::INFINITY;
    let mut z_max = f64::NEG_INFINITY;
    for (r, z) in profile {
        r_max = r_max.max(*r);
        z_min = z_min.min(*z);
        z_max = z_max.max(*z);
    }
    if !(r_max.is_finite() && z_min.is_finite() && z_max.is_finite()) {
        report.gap(
            "meridian_max_radius,meridian_axial_extent",
            "the sampled meridian carries a non-finite [r,z] point",
        );
        return report;
    }
    match mesh_axis_extents(mesh, axis_origin, axis_direction) {
        Some(e) => {
            report.measured(
                "meridian_max_radius",
                r_max,
                e.max_radial,
                "largest perpendicular distance of a tessellation vertex from the revolve axis",
            );
            report.measured(
                "meridian_axial_extent",
                z_max - z_min,
                e.axial_span(),
                "span of the tessellation projected onto the revolve axis",
            );
        }
        None => report.gap(
            "meridian_max_radius,meridian_axial_extent",
            "the built solid tessellated to no vertices, or the requested axis is degenerate",
        ),
    }
    report
}

/// Loft: requested cross-section rings against the cross-sections actually
/// built. THE MOTIVATING CASE.
///
/// Only the FIRST and LAST sections are measured, and that is a property of the
/// operation rather than a shortcut: those two rings become planar CAP FACES of
/// the solid, so each has a directly measurable trimmed area. Intermediate
/// sections are interior isoparms of one skinned surface — there is no face to
/// integrate and no honest measurement without building a sectioning pass this
/// slice does not have. They are reported as stated gaps.
///
/// The compared quantity is enclosed AREA, because area is what the motivating
/// defect moved: an octagon inscribed in its circle has every vertex exactly on
/// the circle (a radius probe calls it perfect) while enclosing 9.97% less.
///
/// READ THE SIGN. The comparison is symmetric and the two directions are
/// different facts:
///
/// * `signed_relative_deviation < 0` — the kernel built LESS cross-section than
///   the request describes. This is the octagon class: a densification or
///   correspondence collapse that inscribes a polygon inside the requested
///   curve. It is the class worth alarming about.
/// * `signed_relative_deviation > 0` — the kernel built MORE. On `nurbs_loft`
///   this is the ordinary consequence of its documented behaviour: the U
///   direction is a PERIODIC CUBIC interpolated through the ring points, so a
///   coarsely-sampled ring is rounded out into the smooth curve it was sampling
///   from. Measured on this path, a ring is essentially the circle at BOTH
///   densities (r=7 → built 153.84 at 64 points, 153.65 at 8, against the true
///   circle's 153.94); the 10.86% at 8 points comes from the REQUESTED octagon
///   shrinking, not from the kernel deviating. That is still a real fidelity
///   fact — the caller asked for an octagonal section and got a round one — but
///   it is a sampling report, not a kernel defect, and the sign is what says so.
///
/// Consequence a caller should know: requested-N-gon-vs-built-circle crosses the
/// 2% band at roughly N = 20, so a 16-point ring reads ≈2.6% and a 12-point ring
/// ≈4.7%. Rings that coarse ARE materially not the shape they name, and the
/// block says so with the number and the direction attached.
pub fn loft_fidelity(
    model: &BRepModel,
    mesh: &TriangleMesh,
    solid_id: SolidId,
    sections: &[Vec<Point3>],
    distance_tol: f64,
) -> FidelityReport {
    let mut report = FidelityReport::new("loft");
    if sections.len() < 2 {
        report.gap(
            "cap_area",
            "fewer than two sections were requested — no end caps exist to measure",
        );
        return report;
    }
    let last = sections.len() - 1;
    for (index, label) in [(0usize, "bottom"), (last, "top")] {
        let name = format!("cap_area_{label}");
        let Some(section) = sections.get(index) else {
            report.gap(&name, "requested section index is absent from the request");
            continue;
        };
        let Some(ring) = ring_plane(section) else {
            report.gap(
                &name,
                "the requested ring has fewer than three points or encloses no area, so it \
                 defines no cross-section to compare against",
            );
            continue;
        };
        match mesh_cross_section_area(
            model,
            mesh,
            solid_id,
            ring.centroid,
            ring.normal,
            distance_tol,
        ) {
            Some(area) => report.measured(
                &name,
                ring.area,
                area,
                "summed area of the tessellation triangles belonging to the built solid's \
                 planar cap face in the requested section's own plane",
            ),
            None => report.gap(
                &name,
                "no planar face of the built solid lies in this section's plane (the cap was \
                 not built there, or its trimmed area integral failed) — nothing measurable to \
                 compare, so no number is reported",
            ),
        }
    }
    if last > 1 {
        report.gap(
            "interior_section_areas",
            format!(
                "{} interior section(s) are isoparms of the skinned surface, not faces — this \
                 slice measures only the two end caps, which are real faces with a trimmed area",
                last - 1
            )
            .as_str(),
        );
    }
    report
}

#[cfg(test)]
mod tests {
    // Reason: unit tests — panicking IS the failure mechanism; the workspace
    // production deny stands.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn relative_deviation_falls_back_to_absolute_at_zero_request() {
        let q = FidelityQuantity::new("r", 0.0, 0.25, "test");
        assert_eq!(q.relative_deviation, 0.25);
        assert!(q.relative_deviation.is_finite());
    }

    #[test]
    fn the_sign_distinguishes_built_smaller_from_built_larger() {
        // The octagon class: a circular request came back inscribed.
        let short = FidelityQuantity::new("cap_area", 100.0, 90.03, "test");
        assert!(
            short.signed_relative_deviation < 0.0,
            "built LESS than requested must read negative"
        );
        // Interpolation overshoot: a coarse request came back rounded out.
        let over = FidelityQuantity::new("cap_area", 100.0, 110.0, "test");
        assert!(over.signed_relative_deviation > 0.0);
        // The magnitude the band judges is sign-free in both cases.
        assert!((short.relative_deviation - 0.0997).abs() < 1e-9);
        assert!((over.relative_deviation - 0.1).abs() < 1e-9);
    }

    #[test]
    fn ring_plane_of_a_unit_square_reads_area_one() {
        let square = vec![
            Point3::new(0.0, 0.0, 3.0),
            Point3::new(1.0, 0.0, 3.0),
            Point3::new(1.0, 1.0, 3.0),
            Point3::new(0.0, 1.0, 3.0),
        ];
        let r = ring_plane(&square).expect("planar square has a ring plane");
        assert!((r.area - 1.0).abs() < 1e-12, "area {}", r.area);
        assert!((r.normal.z.abs() - 1.0).abs() < 1e-12);
        assert!((r.centroid.z - 3.0).abs() < 1e-12);
    }

    #[test]
    fn ring_plane_refuses_a_degenerate_ring() {
        assert!(ring_plane(&[Point3::new(0.0, 0.0, 0.0)]).is_none());
        let collinear = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(2.0, 0.0, 0.0),
        ];
        assert!(ring_plane(&collinear).is_none());
    }

    #[test]
    fn a_report_with_nothing_measured_has_no_verdict_not_a_green_one() {
        let r = FidelityReport::new("cylinder");
        assert_eq!(
            r.fidelity_ok(),
            None,
            "a green boolean over an unmeasured quantity is the \
             certified-sound-at-9.97% pattern in miniature"
        );
        assert!(r.is_empty());
        assert!(r.worst().is_none());
    }

    #[test]
    fn a_gap_is_not_a_zero_and_still_yields_no_verdict() {
        let mut r = FidelityReport::new("revolve");
        r.gap("meridian_max_radius", "no sampled meridian supplied");
        assert!(r.quantities.is_empty(), "a gap must never mint a quantity");
        assert_eq!(r.gaps.len(), 1);
        assert!(!r.is_empty());
        assert_eq!(
            r.fidelity_ok(),
            None,
            "a stated gap explains the absence; it does not manufacture a pass"
        );
    }

    #[test]
    fn fidelity_ok_flips_only_beyond_the_band() {
        let mut r = FidelityReport::new("loft");
        r.measured("cap_area_bottom", 100.0, 101.5, "test");
        assert_eq!(r.fidelity_ok(), Some(true), "1.5% is inside the 2% band");
        r.measured("cap_area_top", 100.0, 90.03, "test");
        assert_eq!(r.fidelity_ok(), Some(false), "9.97% is outside the 2% band");
        let worst = r.worst().expect("a worst exists");
        assert_eq!(worst.name, "cap_area_top");
        assert!((worst.relative_deviation - 0.0997).abs() < 1e-9);
        assert!(
            worst.signed_relative_deviation < 0.0,
            "and it says the kernel built LESS than was asked for"
        );
    }
}
