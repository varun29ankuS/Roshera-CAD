//! EYE-PROFILE: dimensioned axial-profile (meridian) drawing for
//! axisymmetric solids — nozzles, revolved bodies, lofted bells.
//!
//! Where [`super::dimensioned`] (EYE-1) gives shaded multi-views with the
//! OVERALL bounding box, this gives the true engineering MERIDIAN: the radius
//! of the body as a function of axial station, drawn symmetric about the axis
//! centerline, with the FEATURES an engineer reads off it dimensioned — overall
//! length, max/min diameters (the throat), the end diameters (exit / chamber),
//! wall thickness for a hollow part, and the half-angle of the dominant sloped
//! segments.
//!
//! ## Meridian source
//! The meridian is read directly off the kernel's tessellated mesh: every
//! vertex is projected to `(s = axial, r = radial distance from the axis)` and
//! the radial envelope is recorded per axial station. This is honest (the
//! kernel's own geometry, in its tessellated form) and robust for ANY solid —
//! primitive, boolean, or freeform NURBS-loft. It is deliberately NOT an axial
//! cutting-plane section: a plane through the axis lies *in* the meridian
//! facets of a body of revolution, so a straddle-slice finds no crossing on the
//! lateral surface and recovers only the end caps. The radial-envelope read is
//! immune to that degeneracy.
//!
//! The axis of symmetry is detected from the geometry (the most axisymmetric
//! world direction, axis line on the dense mesh centroid), with a Z fall-back
//! when no axis dominates, or supplied by the caller.
//!
//! Output mirrors EYE-1: a PNG plus a structured table of measured values. The
//! image is the placed table; the JSON is authoritative.

use super::dimensioned::{
    draw_line_pub, draw_text_pub, fmt_num_pub, glyph_supported, text_width_pub,
};
use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::tessellation::{tessellate_solid, TessellationParams};
use crate::units::LengthUnit;
use serde::Serialize;

const BG: [u8; 3] = [250, 250, 250];
const WIDTH: usize = 900;
const HEIGHT: usize = 640;
/// Outline ink.
const OUTLINE: [u8; 3] = [25, 35, 60];
/// Centerline (chain-dash) ink.
const CENTERLINE: [u8; 3] = [150, 40, 40];
/// Dimension ink (extension/witness lines, dimension lines, text).
const DIM: [u8; 3] = [20, 20, 20];
/// Diameter-dimension ink (distinct so Ø dims read apart from linear ones).
const DIA: [u8; 3] = [150, 60, 0];

/// One measured feature dimension on the meridian profile.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileDimension {
    /// "overall_length" | "max_diameter" | "min_diameter" | "exit_diameter" |
    /// "base_diameter" | "wall_thickness" | "half_angle".
    pub kind: String,
    pub value: f64,
    /// "mm" for lengths, "deg" for angles.
    pub unit: String,
    /// Human label, e.g. "Ø10.40", "L 17.50", "∠ 12.3°".
    pub label: String,
    /// Axial station (distance along the axis from the profile's axial origin)
    /// the dimension is taken at, when meaningful (diameters); `None` for the
    /// overall length / angles that span a range.
    pub station: Option<f64>,
}

/// The full measured profile result: the rendered drawing plus the structured
/// dimension table and the detected axis (so the agent can reproduce the cut).
#[derive(Debug, Clone)]
pub struct ProfileFrame {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
    /// Document length-unit label for every reported length (e.g. `"mm"`).
    /// Mirrors the model's [`crate::units::LengthUnit`]; angles are degrees.
    pub units: &'static str,
    /// Detected (or supplied) axis of symmetry: a point on the axis + a unit
    /// direction.
    pub axis_origin: Point3,
    pub axis_dir: Vector3,
    /// `true` when the part is hollow (the cut shows an inner wall contour).
    pub hollow: bool,
    /// Measured feature dimensions.
    pub dimensions: Vec<ProfileDimension>,
}

impl ProfileFrame {
    pub fn to_png(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, self.width as u32, self.height as u32);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().map_err(|e| format!("png header: {e}"))?;
            w.write_image_data(&self.pixels)
                .map_err(|e| format!("png data: {e}"))?;
        }
        Ok(out)
    }

    /// Convenience accessor for a measured value by kind (first match).
    pub fn value_of(&self, kind: &str) -> Option<f64> {
        self.dimensions
            .iter()
            .find(|d| d.kind == kind)
            .map(|d| d.value)
    }
}

/// Build the dimensioned axial-profile drawing for `solid_id`.
///
/// `axis_override` forces the symmetry axis (origin + direction); when `None`
/// the axis is detected from the geometry. Returns `None` when the solid is
/// absent, tessellates empty, has no axial extent, or the axis is degenerate —
/// all caller-visible conditions, not errors (mirrors
/// `render_dimensioned_multiview`).
pub fn render_axial_profile(
    model: &BRepModel,
    solid_id: SolidId,
    axis_override: Option<(Point3, Vector3)>,
    tolerance: Tolerance,
) -> Option<ProfileFrame> {
    model.solids.get(solid_id)?;

    // Document length unit governs the unit string + formatting on the drawing
    // (the kernel geometry stays in its native millimetre modelling unit).
    let unit = model.document_unit();
    let ulabel = unit.label();

    let (axis_origin, axis_dir) = match axis_override {
        Some((o, d)) => (o, d.normalize().ok()?),
        None => detect_symmetry_axis(model, solid_id)?,
    };

    // The meridian of an axisymmetric body is its radius as a function of axial
    // station: profile(s) = { r(p) : p on the surface, axial(p) = s }. We read
    // it directly off the kernel's tessellated mesh by projecting every vertex
    // to (s = axial, r = radial distance from the axis) and recording the
    // radial envelope per axial station. This is the honest, robust source for
    // ANY solid (primitive / boolean / freeform NURBS-loft): it is the kernel's
    // own geometry, and unlike an axial section plane it never grazes the
    // lateral surface (a plane through the axis lies IN the meridian facets, so
    // a straddle-slice finds no crossing there — see the design note). The
    // `in_plane`/`plane_normal` basis only frames the 2D drawing.
    let in_plane = axis_dir.perpendicular().normalize().ok()?;
    let plane_normal = axis_dir.cross(&in_plane).normalize().ok()?;

    let solid = model.solids.get(solid_id)?;
    // Fine mesh: the meridian dimensions are MEASURED off this mesh, so the
    // facet chord error must be well under the few-percent dimension tolerance.
    let mesh = tessellate_solid(solid, model, &TessellationParams::fine());
    if mesh.vertices.is_empty() {
        return None;
    }

    // (s, r) for a world point: axial coordinate along the axis + perpendicular
    // distance from the axis line through `axis_origin`.
    let to_sr = |p: &Point3| -> (f64, f64) {
        let w = Vector3::new(
            p.x - axis_origin.x,
            p.y - axis_origin.y,
            p.z - axis_origin.z,
        );
        let s = w.dot(&axis_dir);
        let perp = Vector3::new(
            w.x - axis_dir.x * s,
            w.y - axis_dir.y * s,
            w.z - axis_dir.z * s,
        );
        (s, perp.magnitude())
    };

    let s_min = mesh
        .vertices
        .iter()
        .map(|v| to_sr(&v.position).0)
        .fold(f64::INFINITY, f64::min);
    let s_max = mesh
        .vertices
        .iter()
        .map(|v| to_sr(&v.position).0)
        .fold(f64::NEG_INFINITY, f64::max);
    let length = s_max - s_min;
    if !(length > 1e-9) {
        return None;
    }

    // Per axial bin: outer = max radius (the outer wall), inner = min radius
    // (the inner wall when hollow; ≈ the outer radius for a solid body, since
    // mid-body has no interior surface vertices).
    const BINS: usize = 400;
    let mut outer = vec![f64::NEG_INFINITY; BINS];
    let mut inner = vec![f64::INFINITY; BINS];
    let bin_of = |s: f64| -> usize {
        let b = ((s - s_min) / length * (BINS as f64 - 1.0)).round() as i64;
        b.clamp(0, BINS as i64 - 1) as usize
    };
    for v in &mesh.vertices {
        let (s, r) = to_sr(&v.position);
        let bi = bin_of(s);
        if r > outer[bi] {
            outer[bi] = r;
        }
        if r < inner[bi] {
            inner[bi] = r;
        }
    }
    // Fill any empty interior bins by nearest-neighbour from filled bins so the
    // envelope (and the drawn outline) is continuous even where the mesh is
    // sparse at a station.
    {
        let mut last = None;
        for bi in 0..BINS {
            if outer[bi].is_finite() && outer[bi] > 0.0 {
                last = Some(bi);
            } else if let Some(l) = last {
                outer[bi] = outer[l];
                inner[bi] = inner[l];
            }
        }
        let mut next = None;
        for bi in (0..BINS).rev() {
            if outer[bi].is_finite() && outer[bi] > 0.0 {
                next = Some(bi);
            } else if let Some(nx) = next {
                outer[bi] = outer[nx];
                inner[bi] = inner[nx];
            }
        }
    }

    // The part is hollow when, over a meaningful span, the inner radius sits
    // well below the outer radius (a wall gap that is not just numerical).
    let mut hollow_bins = 0usize;
    let mut filled_bins = 0usize;
    let mut max_outer = 0.0_f64;
    for bi in 0..BINS {
        if outer[bi].is_finite() && outer[bi] > 0.0 {
            filled_bins += 1;
            if outer[bi] > max_outer {
                max_outer = outer[bi];
            }
        }
    }
    let wall_eps = (max_outer * 0.02).max(1e-4);
    for bi in 0..BINS {
        if outer[bi].is_finite() && inner[bi].is_finite() && outer[bi] - inner[bi] > wall_eps {
            // A real bore only counts when the inner radius is itself > 0 (an
            // open lumen), not the solid axis line (inner ≈ 0).
            if inner[bi] > wall_eps {
                hollow_bins += 1;
            }
        }
    }
    let hollow = filled_bins > 0 && hollow_bins as f64 > 0.15 * filled_bins as f64;

    // ── Measured feature dimensions ──────────────────────────────────────────
    let mut dims: Vec<ProfileDimension> = Vec::new();

    dims.push(ProfileDimension {
        kind: "overall_length".into(),
        value: length,
        unit: ulabel.into(),
        label: format!("L {} {}", fmt_num_pub(length), ulabel),
        station: None,
    });

    // Outer-radius profile as (station, r) over filled bins.
    let station_of = |bi: usize| -> f64 { bi as f64 / (BINS as f64 - 1.0) * length };
    let outer_samples: Vec<(f64, f64)> = (0..BINS)
        .filter(|&bi| outer[bi].is_finite() && outer[bi] > 0.0)
        .map(|bi| (station_of(bi), outer[bi]))
        .collect();
    if outer_samples.is_empty() {
        return None;
    }

    // Max diameter (and where).
    let (max_st, max_r) =
        outer_samples
            .iter()
            .copied()
            .fold((0.0_f64, f64::NEG_INFINITY), |acc, (s, r)| {
                if r > acc.1 {
                    (s, r)
                } else {
                    acc
                }
            });
    dims.push(ProfileDimension {
        kind: "max_diameter".into(),
        value: 2.0 * max_r,
        unit: ulabel.into(),
        label: format!("Ø{} {}", fmt_num_pub(2.0 * max_r), ulabel),
        station: Some(max_st),
    });

    // Min diameter (throat) — the narrowest INTERIOR station (exclude the very
    // ends, whose radius is the cut rim, so a converging-diverging bell's
    // throat is found, not an end). Require it to be a genuine local minimum
    // strictly less than both end radii.
    let interior: Vec<(f64, f64)> = outer_samples
        .iter()
        .copied()
        .filter(|&(s, _)| s > length * 0.05 && s < length * 0.95)
        .collect();
    if let Some((thr_st, thr_r)) =
        interior
            .iter()
            .copied()
            .reduce(|acc, (s, r)| if r < acc.1 { (s, r) } else { acc })
    {
        let end_r0 = outer_samples.first().map(|&(_, r)| r).unwrap_or(thr_r);
        let end_r1 = outer_samples.last().map(|&(_, r)| r).unwrap_or(thr_r);
        if thr_r < end_r0 - wall_eps && thr_r < end_r1 - wall_eps {
            dims.push(ProfileDimension {
                kind: "min_diameter".into(),
                value: 2.0 * thr_r,
                unit: ulabel.into(),
                label: format!("Ø{} {}", fmt_num_pub(2.0 * thr_r), ulabel),
                station: Some(thr_st),
            });
        }
    }

    // End diameters: base (station 0) and exit (station = length). Average a
    // few bins at each end for robustness against a single noisy rim sample.
    let end_radius = |from_start: bool| -> Option<f64> {
        let mut rs: Vec<f64> = outer_samples
            .iter()
            .filter(|&&(s, _)| {
                if from_start {
                    s <= length * 0.04
                } else {
                    s >= length * 0.96
                }
            })
            .map(|&(_, r)| r)
            .collect();
        if rs.is_empty() {
            return None;
        }
        rs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        Some(rs[rs.len() / 2])
    };
    if let Some(r) = end_radius(true) {
        dims.push(ProfileDimension {
            kind: "base_diameter".into(),
            value: 2.0 * r,
            unit: ulabel.into(),
            label: format!("Ø{} {}", fmt_num_pub(2.0 * r), ulabel),
            station: Some(0.0),
        });
    }
    if let Some(r) = end_radius(false) {
        dims.push(ProfileDimension {
            kind: "exit_diameter".into(),
            value: 2.0 * r,
            unit: ulabel.into(),
            label: format!("Ø{} {}", fmt_num_pub(2.0 * r), ulabel),
            station: Some(length),
        });
    }

    // Wall thickness (hollow only): median outer−inner over the bins that have
    // a real lumen.
    if hollow {
        let mut walls: Vec<f64> = (0..BINS)
            .filter(|&bi| {
                outer[bi].is_finite()
                    && inner[bi].is_finite()
                    && inner[bi] > wall_eps
                    && outer[bi] - inner[bi] > wall_eps
            })
            .map(|bi| outer[bi] - inner[bi])
            .collect();
        if !walls.is_empty() {
            walls.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let t = walls[walls.len() / 2];
            dims.push(ProfileDimension {
                kind: "wall_thickness".into(),
                value: t,
                unit: ulabel.into(),
                label: format!("t {} {}", fmt_num_pub(t), ulabel),
                station: None,
            });
        }
    }

    // Dominant cone/bell half-angle: the slope of the longest straight-ish run
    // of the outer profile. Half-angle = atan(dr/ds) measured off the axis.
    if let Some(angle) = dominant_half_angle(&outer_samples) {
        dims.push(ProfileDimension {
            kind: "half_angle".into(),
            value: angle,
            unit: "deg".into(),
            label: format!("∠ {:.1}°", angle),
            station: None,
        });
    }

    // ── Draw the dimensioned profile ─────────────────────────────────────────
    let pixels = draw_profile(s_min, length, max_outer, &outer, &inner, hollow, &dims);

    Some(ProfileFrame {
        width: WIDTH,
        height: HEIGHT,
        pixels,
        units: ulabel,
        axis_origin,
        axis_dir,
        hollow,
        dimensions: dims,
    })
}

/// Half-angle (degrees, off the axis) of the longest near-straight run of the
/// outer-radius profile. Returns `None` when no run spans enough stations or
/// the dominant run is effectively axis-parallel (a cylinder, half-angle ~0).
fn dominant_half_angle(outer_samples: &[(f64, f64)]) -> Option<f64> {
    if outer_samples.len() < 8 {
        return None;
    }
    // Fit slope over a sliding window and keep the window with the largest
    // |dr/ds| that is also a consistent straight run (low residual). A simple,
    // robust proxy: take the steepest monotonic run of meaningful length.
    let n = outer_samples.len();
    let total_s = outer_samples[n - 1].0 - outer_samples[0].0;
    if total_s <= 1e-9 {
        return None;
    }
    let min_span = total_s * 0.25;
    let mut best_angle = 0.0_f64;
    let mut found = false;
    let mut i = 0;
    while i < n - 1 {
        // Grow a monotonic-slope-sign run from i.
        let mut j = i + 1;
        let s0 = outer_samples[i].0;
        let r0 = outer_samples[i].1;
        let sign = (outer_samples[j].1 - r0).signum();
        while j + 1 < n {
            let dr = outer_samples[j + 1].1 - outer_samples[j].1;
            if dr.signum() != sign && dr.abs() > 1e-6 {
                break;
            }
            j += 1;
        }
        let span = outer_samples[j].0 - s0;
        if span >= min_span {
            let dr = outer_samples[j].1 - r0;
            let angle = (dr.abs() / span).atan().to_degrees();
            if angle > best_angle {
                best_angle = angle;
                found = true;
            }
        }
        i = j.max(i + 1);
    }
    if found && best_angle > 1.0 {
        Some(best_angle)
    } else {
        None
    }
}

/// Detect the axis of symmetry of an axisymmetric solid: a point ON the axis +
/// a unit direction.
///
/// Method:
///   1. Score axisymmetry about each world axis through the edge-sample
///      centroid by binning points along the candidate and measuring the
///      radial-distance spread within each bin (low spread ⇒ a body of
///      revolution about that axis). Pick the best, with +Z as the fall-back
///      when none clears the confidence threshold.
///   2. Locate the axis LINE: the edge-sample centroid is biased off-axis
///      because the construction seam adds samples at one azimuth, so instead
///      use the dense, azimuthally-uniform MESH-vertex centroid projected onto
///      the cross-axis plane — for a body of revolution that lands on the true
///      axis (the seam bias washes out).
fn detect_symmetry_axis(model: &BRepModel, solid_id: SolidId) -> Option<(Point3, Vector3)> {
    let pts = edge_sample_points(model, solid_id);
    let edge_c = centroid(&pts).unwrap_or(Point3::new(0.0, 0.0, 0.0));

    let candidates = [
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
    ];
    let axis_dir = if pts.len() < 8 {
        Vector3::new(0.0, 0.0, 1.0)
    } else {
        let mut best: Option<(f64, Vector3)> = None;
        for axis in candidates {
            if let Some(s) = axisymmetry_score(&pts, edge_c, axis) {
                if best.map_or(true, |(b, _)| s < b) {
                    best = Some((s, axis));
                }
            }
        }
        match best {
            Some((s, axis)) if s < 0.08 => axis,
            _ => Vector3::new(0.0, 0.0, 1.0),
        }
    };

    // Axis line position from the dense mesh centroid (unbiased by the seam).
    let origin = match model.solids.get(solid_id) {
        Some(solid) => {
            let mesh = tessellate_solid(solid, model, &TessellationParams::coarse());
            mesh_centroid(&mesh).unwrap_or(edge_c)
        }
        None => edge_c,
    };
    Some((origin, axis_dir.normalize().ok()?))
}

/// Unweighted mean of mesh vertex positions — dense and azimuthally uniform, so
/// for a body of revolution it lies on the symmetry axis.
fn mesh_centroid(mesh: &crate::tessellation::mesh::TriangleMesh) -> Option<Point3> {
    if mesh.vertices.is_empty() {
        return None;
    }
    let (mut x, mut y, mut z) = (0.0, 0.0, 0.0);
    for v in &mesh.vertices {
        x += v.position.x;
        y += v.position.y;
        z += v.position.z;
    }
    let n = mesh.vertices.len() as f64;
    Some(Point3::new(x / n, y / n, z / n))
}

/// Mean coefficient of variation of the radial distance within axial bins —
/// 0 for a perfect body of revolution about `axis`. `None` when the points
/// don't span the axis at all.
fn axisymmetry_score(pts: &[Point3], center: Point3, axis: Vector3) -> Option<f64> {
    let axis = axis.normalize().ok()?;
    let proj = |p: &Point3| -> f64 {
        (p.x - center.x) * axis.x + (p.y - center.y) * axis.y + (p.z - center.z) * axis.z
    };
    let radial = |p: &Point3| -> f64 {
        let w = Vector3::new(p.x - center.x, p.y - center.y, p.z - center.z);
        let along = w.dot(&axis);
        let perp = Vector3::new(
            w.x - axis.x * along,
            w.y - axis.y * along,
            w.z - axis.z * along,
        );
        perp.magnitude()
    };
    let a_min = pts.iter().map(proj).fold(f64::INFINITY, f64::min);
    let a_max = pts.iter().map(proj).fold(f64::NEG_INFINITY, f64::max);
    let span = a_max - a_min;
    if span <= 1e-9 {
        return None;
    }
    const NB: usize = 24;
    let mut sum: Vec<f64> = vec![0.0; NB];
    let mut sumsq: Vec<f64> = vec![0.0; NB];
    let mut cnt: Vec<usize> = vec![0; NB];
    for p in pts {
        let t = ((proj(p) - a_min) / span * (NB as f64 - 1.0)).round() as usize;
        let t = t.min(NB - 1);
        let r = radial(p);
        sum[t] += r;
        sumsq[t] += r * r;
        cnt[t] += 1;
    }
    let mut cv_sum = 0.0;
    let mut bins = 0usize;
    for b in 0..NB {
        if cnt[b] >= 3 {
            let m = sum[b] / cnt[b] as f64;
            if m > 1e-9 {
                let var = (sumsq[b] / cnt[b] as f64 - m * m).max(0.0);
                cv_sum += var.sqrt() / m;
                bins += 1;
            }
        }
    }
    if bins == 0 {
        return None;
    }
    Some(cv_sum / bins as f64)
}

/// Exact edge-curve sample points for the whole solid (curves, not the mesh).
fn edge_sample_points(model: &BRepModel, solid_id: SolidId) -> Vec<Point3> {
    let mut pts = Vec::new();
    let solid = match model.solids.get(solid_id) {
        Some(s) => s,
        None => return pts,
    };
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    let mut seen_edges = std::collections::HashSet::new();
    for sh in shells {
        let shell = match model.shells.get(sh) {
            Some(s) => s,
            None => continue,
        };
        for &fid in &shell.faces {
            let face = match model.faces.get(fid) {
                Some(f) => f,
                None => continue,
            };
            let mut loops = vec![face.outer_loop];
            loops.extend_from_slice(&face.inner_loops);
            for lid in loops {
                let lp = match model.loops.get(lid) {
                    Some(l) => l,
                    None => continue,
                };
                for &eid in &lp.edges {
                    if !seen_edges.insert(eid) {
                        continue;
                    }
                    let edge = match model.edges.get(eid) {
                        Some(e) => e,
                        None => continue,
                    };
                    let curve = match model.curves.get(edge.curve_id) {
                        Some(c) => c,
                        None => continue,
                    };
                    let r = edge.param_range;
                    for k in 0..=32 {
                        let t = r.start + (r.end - r.start) * (k as f64 / 32.0);
                        if let Ok(p) = curve.point_at(t) {
                            pts.push(p);
                        }
                    }
                }
            }
        }
    }
    pts
}

fn centroid(pts: &[Point3]) -> Option<Point3> {
    if pts.is_empty() {
        return None;
    }
    let (mut x, mut y, mut z) = (0.0, 0.0, 0.0);
    for p in pts {
        x += p.x;
        y += p.y;
        z += p.z;
    }
    let n = pts.len() as f64;
    Some(Point3::new(x / n, y / n, z / n))
}

/// Render the meridian + centerline + dimensions into an RGB framebuffer.
///
/// Layout: the axis runs horizontally; radius up/down (the section is drawn
/// symmetric about the centerline). Linear dims (length, end diameters) sit
/// below/right with witness lines; the throat diameter is called out at its
/// station; the overall length spans the bottom.
fn draw_profile(
    _s_min: f64,
    length: f64,
    max_outer: f64,
    outer: &[f64],
    inner: &[f64],
    hollow: bool,
    dims: &[ProfileDimension],
) -> Vec<u8> {
    let mut px = vec![0u8; WIDTH * HEIGHT * 3];
    for c in px.chunks_exact_mut(3) {
        c.copy_from_slice(&BG);
    }

    // World → pixel. Leave generous margins for dimension annotations.
    let margin_l = 90.0;
    let margin_r = 150.0;
    let margin_t = 70.0;
    let margin_b = 110.0;
    let draw_w = WIDTH as f64 - margin_l - margin_r;
    let draw_h = HEIGHT as f64 - margin_t - margin_b;
    let full_r = max_outer.max(1e-6) * 2.0; // diameter spans the height
    let sx = draw_w / length.max(1e-9);
    let sy = draw_h / full_r;
    let scale = sx.min(sy);
    let cx0 = margin_l;
    let cy_axis = margin_t + draw_h * 0.5; // centerline y
                                           // station s (0..length) → x; radius r → y (up = -r, mirrored down = +r).
    let to_x = |s: f64| -> f64 { cx0 + s * scale };
    let nb = outer.len();
    let station_of = |bi: usize| -> f64 { bi as f64 / (nb as f64 - 1.0) * length };

    // Build ordered outer profile polyline (top half) over filled bins.
    let mut top: Vec<(f64, f64)> = Vec::new(); // (x, y)
    let mut bot: Vec<(f64, f64)> = Vec::new();
    let mut inner_top: Vec<(f64, f64)> = Vec::new();
    let mut inner_bot: Vec<(f64, f64)> = Vec::new();
    for bi in 0..nb {
        if outer[bi].is_finite() && outer[bi] > 0.0 {
            let x = to_x(station_of(bi));
            top.push((x, cy_axis - outer[bi] * scale));
            bot.push((x, cy_axis + outer[bi] * scale));
            if hollow && inner[bi].is_finite() && inner[bi] > 1e-6 {
                inner_top.push((x, cy_axis - inner[bi] * scale));
                inner_bot.push((x, cy_axis + inner[bi] * scale));
            }
        }
    }

    // Draw outer outline (top + bottom) and close the ends.
    let stroke_poly = |px: &mut [u8], poly: &[(f64, f64)], color: [u8; 3]| {
        for w in poly.windows(2) {
            draw_line_pub(px, WIDTH, HEIGHT, w[0].0, w[0].1, w[1].0, w[1].1, color);
        }
    };
    stroke_poly(&mut px, &top, OUTLINE);
    stroke_poly(&mut px, &bot, OUTLINE);
    if let (Some(&t0), Some(&b0)) = (top.first(), bot.first()) {
        draw_line_pub(&mut px, WIDTH, HEIGHT, t0.0, t0.1, b0.0, b0.1, OUTLINE);
    }
    if let (Some(&t1), Some(&b1)) = (top.last(), bot.last()) {
        draw_line_pub(&mut px, WIDTH, HEIGHT, t1.0, t1.1, b1.0, b1.1, OUTLINE);
    }
    if hollow {
        stroke_poly(&mut px, &inner_top, OUTLINE);
        stroke_poly(&mut px, &inner_bot, OUTLINE);
    }

    // Centerline (chain-dash) the full width.
    draw_chain_dash(
        &mut px,
        to_x(0.0) - 24.0,
        cy_axis,
        to_x(length) + 24.0,
        cy_axis,
        CENTERLINE,
    );

    // Title.
    draw_text_pub(
        &mut px,
        WIDTH,
        HEIGHT,
        margin_l,
        18.0,
        "AXIAL PROFILE",
        DIM,
        2,
    );

    // ── Dimension annotations ────────────────────────────────────────────────
    // Overall length: a dimension line below the part with end witness lines.
    let dim_y = cy_axis + (max_outer * scale) + 56.0;
    let lx0 = to_x(0.0);
    let lx1 = to_x(length);
    draw_line_pub(&mut px, WIDTH, HEIGHT, lx0, cy_axis, lx0, dim_y + 8.0, DIM);
    draw_line_pub(&mut px, WIDTH, HEIGHT, lx1, cy_axis, lx1, dim_y + 8.0, DIM);
    draw_dim_line(&mut px, lx0, dim_y, lx1, dim_y, DIM);
    if let Some(d) = dims.iter().find(|d| d.kind == "overall_length") {
        let lbl = render_label(&d.label);
        let tw = text_width_pub(&lbl, 2);
        draw_text_pub(
            &mut px,
            WIDTH,
            HEIGHT,
            (lx0 + lx1) * 0.5 - tw * 0.5,
            dim_y + 6.0,
            &lbl,
            DIM,
            2,
        );
    }

    // Diameter callouts at their stations: a vertical witness line across the
    // full diameter at the station, then a short leader up to a label parked
    // above the part. Labels are de-overlapped along three stacked rows above
    // the outline so leaders stay short and read against the body, not all
    // bunched into one corner.
    let th = 7.0 * 2.0; // glyph height at scale 2
    let top_band = margin_t - 4.0;
    let mut placed: Vec<(f64, f64, f64, f64)> = Vec::new(); // x0,y0,x1,y1 boxes
    let mut dia_dims: Vec<&ProfileDimension> = dims
        .iter()
        .filter(|d| {
            matches!(
                d.kind.as_str(),
                "max_diameter" | "min_diameter" | "exit_diameter" | "base_diameter"
            ) && d.station.is_some()
        })
        .collect();
    // Draw left-to-right so the de-overlap reads in station order.
    dia_dims.sort_by(|a, b| {
        a.station
            .unwrap_or(0.0)
            .partial_cmp(&b.station.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for d in dia_dims {
        let r_at = d.value * 0.5;
        let st = d.station.unwrap_or(0.0);
        let x = to_x(st);
        let yt = cy_axis - r_at * scale;
        let yb = cy_axis + r_at * scale;
        // Full-diameter witness across the section at the station.
        draw_dim_line_v(&mut px, x, yt, x, yb, DIA);
        let lbl = render_label(&d.label);
        let tw = text_width_pub(&lbl, 2);
        // Park the label above the part; nudge into 3 stacked rows to avoid
        // collisions, then leader from the outer wall up to it.
        let mut lx = (x - tw * 0.5).clamp(4.0, WIDTH as f64 - tw - 4.0);
        let mut ly = top_band - th;
        let mut row = 0;
        while row < 3 {
            let b = (lx, ly, lx + tw, ly + th);
            let hit = placed
                .iter()
                .any(|q| !(b.2 < q.0 || b.0 > q.2 || b.3 < q.1 || b.1 > q.3));
            if !hit {
                break;
            }
            ly -= th + 6.0;
            row += 1;
        }
        if row == 3 {
            // Could not find a clear row above; shift horizontally instead.
            lx = (lx + tw + 8.0).min(WIDTH as f64 - tw - 4.0);
            ly = top_band - th;
        }
        placed.push((lx, ly, lx + tw, ly + th));
        draw_line_pub(&mut px, WIDTH, HEIGHT, x, yt, lx + tw * 0.5, ly + th, DIA);
        draw_text_pub(&mut px, WIDTH, HEIGHT, lx, ly, &lbl, DIA, 2);
    }

    // Wall thickness + half-angle: stacked text, bottom-left.
    let mut info_y = HEIGHT as f64 - 40.0;
    for d in dims {
        if d.kind == "wall_thickness" || d.kind == "half_angle" {
            let lbl = render_label(&d.label);
            draw_text_pub(&mut px, WIDTH, HEIGHT, margin_l, info_y, &lbl, DIM, 2);
            info_y += 22.0;
        }
    }

    px
}

/// A dimension line with simple arrowheads at both ends (horizontal).
fn draw_dim_line(px: &mut [u8], x0: f64, y0: f64, x1: f64, y1: f64, color: [u8; 3]) {
    draw_line_pub(px, WIDTH, HEIGHT, x0, y0, x1, y1, color);
    // Arrowheads (pointing outward toward the witness lines).
    for (xe, dir) in [(x0, 1.0), (x1, -1.0)] {
        draw_line_pub(px, WIDTH, HEIGHT, xe, y0, xe + dir * 7.0, y0 - 4.0, color);
        draw_line_pub(px, WIDTH, HEIGHT, xe, y0, xe + dir * 7.0, y0 + 4.0, color);
    }
    let _ = y1;
}

/// A vertical dimension line with arrowheads at both ends.
fn draw_dim_line_v(px: &mut [u8], x0: f64, y0: f64, x1: f64, y1: f64, color: [u8; 3]) {
    draw_line_pub(px, WIDTH, HEIGHT, x0, y0, x1, y1, color);
    for (ye, dir) in [(y0, 1.0), (y1, -1.0)] {
        draw_line_pub(px, WIDTH, HEIGHT, x0, ye, x0 - 4.0, ye + dir * 7.0, color);
        draw_line_pub(px, WIDTH, HEIGHT, x0, ye, x0 + 4.0, ye + dir * 7.0, color);
    }
    let _ = x1;
}

/// ISO chain-dash centerline: long–short–long dash pattern.
fn draw_chain_dash(px: &mut [u8], x0: f64, y: f64, x1: f64, _y1: f64, color: [u8; 3]) {
    let total = x1 - x0;
    if total <= 0.0 {
        return;
    }
    let long = 14.0;
    let short = 3.0;
    let gap = 4.0;
    let mut x = x0;
    let mut long_dash = true;
    while x < x1 {
        let seg = if long_dash { long } else { short };
        let xe = (x + seg).min(x1);
        draw_line_pub(px, WIDTH, HEIGHT, x, y, xe, y, color);
        x = xe + gap;
        long_dash = !long_dash;
    }
}

/// Keep only glyphs the 5×7 overlay font can actually draw. The font now
/// carries the true engineering symbols (Ø diameter, ∠ angle, ° degree) plus
/// the unit letters, so the pretty label renders verbatim; any glyph still
/// outside the font is dropped rather than substituted (the structured table
/// keeps the authoritative label regardless).
fn render_label(s: &str) -> String {
    s.chars().filter(|&c| glyph_supported(c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::nurbs_loft::{nurbs_loft, NurbsLoftOptions};

    /// Build a ring of `n` points of radius `r` centred on the axis at height
    /// `z` (axis = +Z, world XY ring).
    fn ring(r: f64, z: f64, n: usize) -> Vec<Point3> {
        (0..n)
            .map(|k| {
                let a = std::f64::consts::TAU * (k as f64) / (n as f64);
                Point3::new(r * a.cos(), r * a.sin(), z)
            })
            .collect()
    }

    /// A lofted bell-nozzle meridian: chamber Ø6 at z0, converging to a throat
    /// Ø2 near z6.8, diverging to an exit Ø10.4 at z17.5. Built as a NURBS loft
    /// through axisymmetric rings.
    fn nozzle(model: &mut BRepModel) -> SolidId {
        // (radius, z) contour stations — chamber → throat → exit.
        let stations = [
            (3.0, 0.0),
            (3.0, 2.0),
            (2.4, 4.0),
            (1.6, 5.6),
            (1.0, 6.8), // throat Ø2.0
            (1.6, 8.5),
            (2.6, 11.0),
            (3.7, 14.0),
            (5.2, 17.5), // exit Ø10.4
        ];
        let sections: Vec<Vec<Point3>> = stations.iter().map(|&(r, z)| ring(r, z, 48)).collect();
        nurbs_loft(
            model,
            sections,
            NurbsLoftOptions {
                degree_u: 3,
                degree_v: 3,
                ..Default::default()
            },
        )
        .expect("nozzle loft")
    }

    #[test]
    fn detects_z_axis_for_nozzle() {
        let mut m = BRepModel::new();
        let s = nozzle(&mut m);
        let (_o, axis) = detect_symmetry_axis(&m, s).expect("axis");
        // Axis must be (anti)parallel to +Z.
        assert!(
            axis.z.abs() > 0.95,
            "expected ~Z axis, got ({:.3},{:.3},{:.3})",
            axis.x,
            axis.y,
            axis.z
        );
    }

    #[test]
    fn nozzle_profile_measured_dims_match() {
        let mut m = BRepModel::new();
        let s = nozzle(&mut m);
        let frame =
            render_axial_profile(&m, s, None, Tolerance::default()).expect("profile must render");

        let overall = frame.value_of("overall_length").expect("length");
        let max_d = frame.value_of("max_diameter").expect("max dia");
        let min_d = frame.value_of("min_diameter").expect("throat dia");
        let exit_d = frame.value_of("exit_diameter").expect("exit dia");
        let base_d = frame.value_of("base_diameter").expect("base dia");

        // Tolerances a few % (faceting + bin resolution + skin overshoot).
        let approx = |got: f64, want: f64, rel: f64, what: &str| {
            assert!(
                (got - want).abs() <= want * rel + 0.15,
                "{what}: measured {got:.3} vs expected {want:.3} (rel {rel})"
            );
        };
        approx(overall, 17.5, 0.04, "overall length");
        approx(max_d, 10.4, 0.06, "max diameter (= exit)");
        approx(exit_d, 10.4, 0.06, "exit diameter");
        approx(base_d, 6.0, 0.08, "base/chamber diameter");
        approx(min_d, 2.0, 0.12, "throat diameter");

        // PNG encodes and is non-trivial.
        let png = frame.to_png().expect("png");
        assert!(png.len() > 2000, "png too small: {}", png.len());
    }

    #[test]
    #[ignore = "writes a PNG for manual inspection"]
    fn emit_nozzle_profile_png() {
        let mut m = BRepModel::new();
        let s = nozzle(&mut m);
        let frame = render_axial_profile(&m, s, None, Tolerance::default()).expect("profile");
        let png = frame.to_png().expect("png");
        std::fs::write("../_nozzle_profile.png", &png).expect("write");
        eprintln!(
            "wrote ../_nozzle_profile.png ({} bytes); dims: {:?}",
            png.len(),
            frame
                .dimensions
                .iter()
                .map(|d| (d.kind.clone(), d.value))
                .collect::<Vec<_>>()
        );
    }
}
