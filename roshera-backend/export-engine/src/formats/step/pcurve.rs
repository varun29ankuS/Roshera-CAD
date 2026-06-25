//! Parameter-space curve (pcurve) construction for STEP export.
//!
//! ## Why this module exists
//!
//! A STEP `ADVANCED_FACE` trims its surface with an `EDGE_LOOP` of
//! `ORIENTED_EDGE`s; each edge references an `EDGE_CURVE` carrying a *3D*
//! curve. ISO 10303-42 permits — and OpenCascade in practice *requires* for
//! robustness — that the edge ALSO carry the curve's image in the surface's
//! 2D `(u, v)` parameter space (a "pcurve"). When no pcurve is present a STEP
//! reader must REPROJECT the 3D edge back onto the face's surface to recover
//! the trim. For an analytic surface that inverse projection is
//! well-conditioned, but for a **periodic / seam** surface (a closed lofted
//! NURBS lateral) the projection is ambiguous AT THE SEAM — a seam point maps
//! to BOTH `u = u_min` and `u = u_max` — so OpenCascade's reprojection picks a
//! branch heuristically and, for some rotations of the same geometry, picks
//! the wrong one. The face then fails `BRepCheck` with
//! `BRepCheck_UnorientableShape` and is dropped/garbled in FreeCAD.
//!
//! Emitting explicit pcurves removes the reprojection step entirely: the
//! reader uses the supplied `(u, v)` curve verbatim, so the seam branch is
//! never guessed. This module computes those pcurves from the live
//! [`BRepModel`] (which still owns the analytic [`Surface`] objects and the
//! full face↔edge topology that the serialisable snapshot flattens away).
//!
//! ## What is produced
//!
//! For every edge that bounds at least one **non-planar parametric** face we
//! produce an [`EdgePcurves`] entry keyed by the edge's deterministic export
//! UUID (see [`super::super::ros_snapshot`]'s `id_to_uuid`). The entry holds
//! one pcurve per `(face, side)` the edge bounds:
//!
//! - A **regular** edge on a parametric face contributes ONE pcurve on that
//!   face → the writer wraps the 3D curve in a `SURFACE_CURVE`.
//! - A **seam** edge (an edge that appears TWICE in the same loop of a
//!   periodic face — it bounds the surface at both `u = u_min` and
//!   `u = u_max`) contributes TWO pcurves on the SAME surface → the writer
//!   wraps the 3D curve in a `SEAM_CURVE` carrying both.
//!
//! Planar faces are intentionally excluded: their inverse projection is exact
//! and unambiguous, so a reader reconstructs the trim flawlessly without a
//! pcurve, and a plane has no canonical bounded `(u, v)` frame to anchor one
//! to without exporting its placement (added cost, zero robustness gain).
//!
//! ## Source of the 2D curve
//!
//! The pcurve geometry is sourced as follows, in priority order:
//!
//! 1. **Analytic** — a seam edge on a u-periodic surface is, by the loft's
//!    construction (and STEP's own seam definition), the iso-`u` line
//!    `u = const` over the full `v` span. We emit those two iso-`u` lines
//!    directly. Likewise an iso-`v` ring edge (start/end project to the same
//!    `v`) is the iso-`v` line. These are exact, not fitted.
//! 2. **Projected** — any other edge is sampled along its 3D curve, each
//!    sample inverse-projected onto the surface via [`Surface::closest_point`],
//!    and the resulting `(u, v)` polyline emitted as a degree-1
//!    `B_SPLINE_CURVE_WITH_KNOTS` in parameter space. A degree-1 (polyline)
//!    fit is chord-exact at the samples and needs no curve-fitting solve, so
//!    it cannot introduce a spurious wiggle the reader would reject.
//!
//! Each pcurve is validated by lifting it back to 3D (`surface.point_at(u, v)`)
//! and comparing against the 3D curve; an entry whose lift error exceeds a
//! generous tolerance is DROPPED rather than emitted, so a bad pcurve can
//! never make a previously-readable face worse (the reader falls back to
//! reprojection for that one edge).

use std::collections::{HashMap, HashSet};

use geometry_engine::math::{Point2, Tolerance};
use geometry_engine::primitives::surface::{Surface, SurfaceType};
use geometry_engine::primitives::topology_builder::BRepModel;
use uuid::Uuid;

use crate::formats::ros_snapshot::id_to_uuid;

/// Number of interior samples taken along a projected edge when fitting its
/// parameter-space polyline. 24 segments resolve a full-turn ring or a
/// strongly-curved spine to well under the surface's own tessellation chord
/// error; the seam/ring analytic paths bypass sampling entirely.
const PROJECTION_SAMPLES: usize = 24;

/// Maximum permitted lift error (3D distance between the edge's 3D curve and
/// the surface point produced by evaluating the pcurve) for an emitted
/// pcurve, as a fraction of the model's bounding diagonal, plus a small
/// absolute floor. A pcurve exceeding this is dropped.
const LIFT_REL_TOL: f64 = 1e-3;
const LIFT_ABS_TOL: f64 = 1e-4;

/// A 2D curve in a surface's `(u, v)` parameter space.
#[derive(Debug, Clone, PartialEq)]
pub enum Pcurve2d {
    /// Straight segment in parameter space — used for analytic iso-`u`
    /// (seam) and iso-`v` (ring) lines.
    Line {
        /// Parameter-space start `(u, v)`.
        start: Point2,
        /// Parameter-space end `(u, v)`.
        end: Point2,
    },
    /// Degree-1 B-spline (polyline) in parameter space — used for projected
    /// edges. `points` are the ordered `(u, v)` samples; the writer emits a
    /// clamped uniform degree-1 `B_SPLINE_CURVE_WITH_KNOTS`.
    Polyline {
        /// Ordered `(u, v)` samples the polyline interpolates.
        points: Vec<Point2>,
    },
}

/// One pcurve attached to a specific exported surface.
#[derive(Debug, Clone)]
pub struct FacePcurve {
    /// Export UUID of the face's surface (matches the snapshot's surface
    /// key, so the writer resolves it to the already-emitted surface id).
    pub surface_uuid: Uuid,
    /// The parameter-space curve on that surface.
    pub curve: Pcurve2d,
}

/// The pcurve(s) carried by one edge, and how the writer must wrap them.
#[derive(Debug, Clone)]
pub enum EdgePcurves {
    /// A non-seam edge: one pcurve on one parametric face. The writer emits a
    /// `SURFACE_CURVE` with `.PCURVE_S1.` master representation.
    Surface(FacePcurve),
    /// A seam edge on a periodic surface: two pcurves on the SAME surface,
    /// one at the low-`u` boundary and one at the high-`u` boundary. The
    /// writer emits a `SEAM_CURVE` carrying both.
    Seam {
        /// Pcurve at the `u = u_min` boundary.
        low: FacePcurve,
        /// Pcurve at the `u = u_max` boundary.
        high: FacePcurve,
    },
}

/// Map from edge export-UUID to its computed pcurve(s).
pub type PcurveMap = HashMap<Uuid, EdgePcurves>;

/// The full pcurve-export payload threaded into the STEP writer.
///
/// Besides the per-edge pcurves, this carries the **periodicity metadata** the
/// writer needs to emit the closed/periodic flags ISO 10303-42 (and, in
/// practice, OpenCascade) requires. Without the `closed` flags set, a reader
/// reads a u-periodic surface's iso-curves with a COLLAPSED (zero-length) 3D
/// parameter range and manufactures degenerate edges → `UnorientableShape`,
/// EVEN with correct pcurves present. The flags + pcurves together are the
/// fix.
#[derive(Debug, Default)]
pub struct PcurveExport {
    /// Per-edge parameter-space curves, keyed by edge export-UUID.
    pub pcurves: PcurveMap,
    /// Export-UUIDs of surfaces closed/periodic in U.
    pub periodic_u_surfaces: HashSet<Uuid>,
    /// Export-UUIDs of surfaces closed/periodic in V.
    pub periodic_v_surfaces: HashSet<Uuid>,
    /// Export-UUIDs of 3D curves that are geometrically CLOSED (start point ==
    /// end point) — the loft's iso-`v` ring curves. The writer sets their
    /// `closed_curve` flag so a reader reads a non-degenerate range.
    pub closed_curves: HashSet<Uuid>,
}

impl PcurveExport {
    /// Whether the export carries any pcurve data at all (false for a pure
    /// analytic/box model — the writer then behaves exactly as before).
    pub fn is_empty(&self) -> bool {
        self.pcurves.is_empty()
            && self.periodic_u_surfaces.is_empty()
            && self.periodic_v_surfaces.is_empty()
            && self.closed_curves.is_empty()
    }
}

/// Build the pcurve export payload for `model`.
///
/// Walks every face; for the non-planar parametric ones it computes each
/// bounding edge's parameter-space image and records it under the edge's
/// export UUID, alongside the periodicity metadata. Returns an empty payload
/// when the model has no parametric faces (e.g. a pure box) — the writer then
/// behaves exactly as before.
pub fn build_pcurve_export(model: &BRepModel) -> PcurveExport {
    let mut map: PcurveMap = HashMap::new();
    let mut periodic_u_surfaces: HashSet<Uuid> = HashSet::new();
    let mut periodic_v_surfaces: HashSet<Uuid> = HashSet::new();
    let mut closed_curves: HashSet<Uuid> = HashSet::new();
    let tol = Tolerance::default();
    let lift_tol = lift_tolerance(model);

    for (_fid, face) in model.faces.iter() {
        let surface_id = face.surface_id;
        let Some(surface) = model.surfaces.get(surface_id) else {
            continue;
        };
        // Planar faces reproject exactly; skip them (see module docs).
        if surface.surface_type() == SurfaceType::Plane {
            continue;
        }
        let surface_uuid = id_to_uuid(surface_id as u64);
        if surface.is_periodic_u() {
            periodic_u_surfaces.insert(surface_uuid);
        }
        if surface.is_periodic_v() {
            periodic_v_surfaces.insert(surface_uuid);
        }

        // Gather every edge use across the face's loops, counting how many
        // times each edge id appears: a count of 2 within the SAME face marks
        // a seam edge (the closed lateral's seam is walked forward + backward
        // in the single rectangular loop).
        let mut loops: Vec<u32> = Vec::new();
        loops.push(face.outer_loop);
        loops.extend(face.inner_loops.iter().copied());

        let mut edge_uses: HashMap<u32, usize> = HashMap::new();
        for &lid in &loops {
            if let Some(loop_) = model.loops.get(lid) {
                for &eid in &loop_.edges {
                    *edge_uses.entry(eid).or_insert(0) += 1;
                }
            }
        }

        for (&eid, &count) in &edge_uses {
            let Some(edge) = model.edges.get(eid) else {
                continue;
            };
            let edge_uuid = id_to_uuid(eid as u64);
            // An edge already resolved on this face from a prior loop pass is
            // skipped (HashMap dedups by key); but an edge SHARED with another
            // face would also be keyed here — we only ever attach the pcurve
            // for the FIRST parametric face that claims the edge, which is the
            // surface the seam/ring analytic form is anchored to. A regular
            // shared edge (ring shared by lateral + planar cap) is attached to
            // the lateral here and the planar cap is skipped above, so there is
            // no conflict in the watertight loft topology.
            if map.contains_key(&edge_uuid) {
                continue;
            }

            let Some(curve) = model.curves.get(edge.curve_id) else {
                continue;
            };

            let is_seam = count >= 2 && surface.is_periodic_u();
            // A ring edge whose two endpoints are the SAME vertex is a CLOSED
            // iso-curve that wraps the periodic surface once (the loft's
            // bottom/top rings). Its parameter-space image is the full-`u`-span
            // iso-`v` line, NOT a projected polyline (whose closed endpoints
            // both collapse near the seam under inverse projection).
            let is_closed_ring = edge.start_vertex == edge.end_vertex;

            // A closed iso-`v` ring curve must declare its `closed_curve` flag
            // so a reader reads its FULL parameter range (an undeclared uniform
            // closed B-spline collapses to a zero-length 3D range → degenerate
            // edges → UnorientableShape). Record the curve's export-UUID.
            if is_closed_ring {
                closed_curves.insert(id_to_uuid(edge.curve_id as u64));
            }

            if is_seam {
                if let Some(pc) = build_seam_pcurves(surface, curve, lift_tol) {
                    let (low, high) = pc;
                    map.insert(
                        edge_uuid,
                        EdgePcurves::Seam {
                            low: FacePcurve {
                                surface_uuid,
                                curve: low,
                            },
                            high: FacePcurve {
                                surface_uuid,
                                curve: high,
                            },
                        },
                    );
                }
            } else if let Some(curve2d) =
                build_surface_pcurve(surface, curve, tol, lift_tol, is_closed_ring)
            {
                map.insert(
                    edge_uuid,
                    EdgePcurves::Surface(FacePcurve {
                        surface_uuid,
                        curve: curve2d,
                    }),
                );
            }
        }
    }

    PcurveExport {
        pcurves: map,
        periodic_u_surfaces,
        periodic_v_surfaces,
        closed_curves,
    }
}

/// Lift tolerance for this model: relative to the bounding diagonal with an
/// absolute floor, so the same threshold works for a millimetre part and a
/// metre part.
fn lift_tolerance(model: &BRepModel) -> f64 {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for (_, v) in model.vertices.iter() {
        for k in 0..3 {
            if v.position[k] < min[k] {
                min[k] = v.position[k];
            }
            if v.position[k] > max[k] {
                max[k] = v.position[k];
            }
        }
    }
    if !min[0].is_finite() || !max[0].is_finite() {
        return LIFT_ABS_TOL;
    }
    let diag =
        ((max[0] - min[0]).powi(2) + (max[1] - min[1]).powi(2) + (max[2] - min[2]).powi(2)).sqrt();
    (diag * LIFT_REL_TOL).max(LIFT_ABS_TOL)
}

/// Build the two seam pcurves for a periodic-u surface: the iso-`u` lines at
/// `u = u_min` and `u = u_max`, each spanning the full `v` range in the
/// direction the edge runs (start vertex → end vertex).
///
/// The seam edge's 3D curve runs from the surface's `S(u_*, v_start)` to
/// `S(u_*, v_end)`; for the loft lateral the seam iso-curve runs `v: 0 → 1`
/// (it is `iso_curve_u(0.0)`). We orient both pcurves the same way as the 3D
/// curve by checking which `v` endpoint the 3D start matches.
fn build_seam_pcurves(
    surface: &dyn Surface,
    curve: &dyn geometry_engine::primitives::curve::Curve,
    lift_tol: f64,
) -> Option<(Pcurve2d, Pcurve2d)> {
    let ((u_min, u_max), (v_min, v_max)) = surface.parameter_bounds();
    if !(u_max > u_min) || !(v_max > v_min) {
        return None;
    }

    // The 3D curve's parameter range maps t∈[0,1] across the seam; sample the
    // start point and decide v-direction by comparing to S(u_min, v_min) and
    // S(u_min, v_max).
    let start_3d = curve.point_at(curve.parameter_range().start).ok()?;
    let p_at_vmin = surface.point_at(u_min, v_min).ok()?;
    let p_at_vmax = surface.point_at(u_min, v_max).ok()?;
    let runs_up = start_3d.distance(&p_at_vmin) <= start_3d.distance(&p_at_vmax);

    let (v_a, v_b) = if runs_up {
        (v_min, v_max)
    } else {
        (v_max, v_min)
    };

    let low = Pcurve2d::Line {
        start: Point2::new(u_min, v_a),
        end: Point2::new(u_min, v_b),
    };
    let high = Pcurve2d::Line {
        start: Point2::new(u_max, v_a),
        end: Point2::new(u_max, v_b),
    };

    // Validate both branches by lifting back to 3D against the seam curve.
    // Because the seam is geometrically identical at u_min and u_max, both
    // must lift onto the SAME 3D curve.
    if !pcurve_lifts_ok(surface, curve, &low, lift_tol) {
        return None;
    }
    if !pcurve_lifts_ok(surface, curve, &high, lift_tol) {
        return None;
    }
    Some((low, high))
}

/// Build a single pcurve for a non-seam edge on a parametric surface.
///
/// First tries the analytic iso-`u`/iso-`v` line (a ring edge whose
/// endpoints share a `u` or `v` coordinate after projection); otherwise
/// falls back to a projected `(u, v)` polyline. Returns `None` if the result
/// cannot be lifted back onto the surface within tolerance.
fn build_surface_pcurve(
    surface: &dyn Surface,
    curve: &dyn geometry_engine::primitives::curve::Curve,
    tol: Tolerance,
    lift_tol: f64,
    is_closed_ring: bool,
) -> Option<Pcurve2d> {
    let range = curve.parameter_range();
    let (t0, t1) = (range.start, range.end);
    let ((u_min, u_max), (v_min, v_max)) = surface.parameter_bounds();

    // ── Closed iso-`v` ring on a u-periodic surface ──
    // The ring wraps the surface once, so its pcurve is the FULL-`u`-span
    // iso-`v` line `u: u_min → u_max` (or reversed) at the ring's constant
    // `v`. The `v` is found by projecting an INTERIOR sample (not the seam
    // endpoints, which inverse-project ambiguously to both u=u_min and
    // u=u_max). Orientation follows the edge: compare the projected `u` just
    // after `t0` against just before `t1`.
    if is_closed_ring && surface.is_periodic_u() {
        let p_mid = curve.point_at((t0 + t1) * 0.5).ok()?;
        let (_, v_ring) = surface.closest_point(&p_mid, tol).ok()?;

        // Direction: sample near the start and near the end (away from the
        // seam) and see whether u ascends or descends along the edge.
        let p_lo = curve.point_at(t0 + (t1 - t0) * 0.1).ok()?;
        let p_hi = curve.point_at(t0 + (t1 - t0) * 0.9).ok()?;
        let (u_lo, _) = surface.closest_point(&p_lo, tol).ok()?;
        let (u_hi, _) = surface.closest_point(&p_hi, tol).ok()?;
        let ascending = u_hi >= u_lo;

        let (u_a, u_b) = if ascending {
            (u_min, u_max)
        } else {
            (u_max, u_min)
        };
        let candidate = Pcurve2d::Line {
            start: Point2::new(u_a, v_ring),
            end: Point2::new(u_b, v_ring),
        };
        return if pcurve_lifts_ok(surface, curve, &candidate, lift_tol) {
            Some(candidate)
        } else {
            None
        };
    }

    // ── General (open) edge ── project a dense sample set to (u, v).
    let mut uv: Vec<Point2> = Vec::with_capacity(PROJECTION_SAMPLES + 1);
    for i in 0..=PROJECTION_SAMPLES {
        let t = t0 + (t1 - t0) * (i as f64) / (PROJECTION_SAMPLES as f64);
        let p = curve.point_at(t).ok()?;
        let (u, v) = surface.closest_point(&p, tol).ok()?;
        uv.push(Point2::new(u, v));
    }

    // Try the analytic iso-line: if every projected u (or every v) is constant
    // to a tight fraction of the parameter span, the pcurve IS that iso-line.
    let u_span = (u_max - u_min).abs().max(1e-12);
    let v_span = (v_max - v_min).abs().max(1e-12);
    let iso_tol = 1e-4;

    let u_const = uv.iter().all(|p| (p.x - uv[0].x).abs() <= iso_tol * u_span);
    let v_const = uv.iter().all(|p| (p.y - uv[0].y).abs() <= iso_tol * v_span);

    let candidate = if v_const && !u_const {
        // iso-v edge: a straight v=const line from first u to last u.
        Pcurve2d::Line {
            start: Point2::new(uv[0].x, uv[0].y),
            end: Point2::new(uv[uv.len() - 1].x, uv[0].y),
        }
    } else if u_const && !v_const {
        Pcurve2d::Line {
            start: Point2::new(uv[0].x, uv[0].y),
            end: Point2::new(uv[0].x, uv[uv.len() - 1].y),
        }
    } else {
        Pcurve2d::Polyline { points: uv }
    };

    if pcurve_lifts_ok(surface, curve, &candidate, lift_tol) {
        Some(candidate)
    } else {
        None
    }
}

/// Check that evaluating `pcurve` and lifting through `surface` stays within
/// `lift_tol` of the edge's 3D `curve` across a sweep of parameters.
fn pcurve_lifts_ok(
    surface: &dyn Surface,
    curve: &dyn geometry_engine::primitives::curve::Curve,
    pcurve: &Pcurve2d,
    lift_tol: f64,
) -> bool {
    let range = curve.parameter_range();
    let (t0, t1) = (range.start, range.end);
    const CHECKS: usize = 12;
    for i in 0..=CHECKS {
        let s = i as f64 / CHECKS as f64;
        let uv = eval_pcurve(pcurve, s);
        let Ok(on_surf) = surface.point_at(uv.x, uv.y) else {
            return false;
        };
        let t = t0 + (t1 - t0) * s;
        let Ok(on_curve) = curve.point_at(t) else {
            return false;
        };
        if on_surf.distance(&on_curve) > lift_tol {
            return false;
        }
    }
    true
}

/// Evaluate a pcurve at normalised `s ∈ [0, 1]`.
fn eval_pcurve(pcurve: &Pcurve2d, s: f64) -> Point2 {
    match pcurve {
        Pcurve2d::Line { start, end } => {
            let s = s.clamp(0.0, 1.0);
            Point2::new(
                start.x + s * (end.x - start.x),
                start.y + s * (end.y - start.y),
            )
        }
        Pcurve2d::Polyline { points } => {
            if points.is_empty() {
                return Point2::new(0.0, 0.0);
            }
            if points.len() == 1 {
                return points[0];
            }
            let s = s.clamp(0.0, 1.0);
            let seg = s * (points.len() - 1) as f64;
            let i = (seg.floor() as usize).min(points.len() - 2);
            let f = seg - i as f64;
            let a = points[i];
            let b = points[i + 1];
            Point2::new(a.x + f * (b.x - a.x), a.y + f * (b.y - a.y))
        }
    }
}
