//! #70 — chamfer chain terminating against an existing fillet's cylindrical
//! surface (the "chamfer crosses fillet" case).
//!
//! # The configuration
//!
//! After a fillet consumes an edge, the edges that used to meet the filleted
//! corner are trimmed back to the fillet's boundary: their endpoint vertex
//! now sits where two planar faces and the fillet's cylindrical face meet.
//! Chamfering such a trimmed edge means the chamfer wall must *terminate
//! against the fillet surface* — there is no third planar face to host the
//! straight cap chord the legacy V-side splice inserts. The legacy chord
//! left the rail endpoint at the raw perpendicular offset, which lands
//! INSIDE the fillet scar (the region the fillet already removed), so the
//! adjacent planar face's boundary loop crossed itself — the pinned #70
//! self-overlap.
//!
//! # The analytic termination
//!
//! Everything about the correct termination is closed-form:
//!
//! * Each chamfer **rail** (the offset line on an adjacent planar face) is
//!   extended/trimmed to its intersection with that face's *fillet-boundary
//!   edge* — a line×line solve on the tangent-contact side and a
//!   line×circle solve on the cap-arc side.
//! * The chamfer **wall plane** `W` cuts the fillet cylinder
//!   (axis `a`, radius `r`) in an **ellipse** with centre at the axis⁠∩⁠plane
//!   point, semi-minor `r` along `(n × a)/‖n × a‖`, and semi-major
//!   `r / |n·a|` along the in-plane direction perpendicular to it (`n` the
//!   unit wall normal). This is the classical planar conic section of a
//!   quadric cylinder (Patrikalakis & Maekawa 2002, *Shape Interrogation*,
//!   §5; ISO 10303-42 treats it as an exact `ELLIPSE`). The arc of that
//!   ellipse between the two corrected rail endpoints is the V-side cap —
//!   shared manifold-2 between the chamfer wall and the retrimmed fillet
//!   face. No sampled/NURBS approximation is involved; the edge carries the
//!   kernel's analytic [`Ellipse`] curve.
//!
//! The treatment mirrors OCCT's `ChFiKPart` philosophy (closed-form blend
//! parts read directly off the supporting analytic surfaces) — the same
//! lineage as `CylindricalFillet::from_analytic_kpart`.
//!
//! # Topology
//!
//! The standard [`super::edge_blend_topology::splice_blend_edge`] surgery
//! already performs the correct re-stitch **once the geometry is right**:
//!
//! * the F1/F2 neighbour-edge rewires retrim the shared fillet-boundary
//!   edges in place (both the planar face and the fillet face reference the
//!   same edge, and both want the same retrim — the shared-edge retrim that
//!   is fatal for the #72 bowtie is exactly correct here);
//! * the third-face lookup at the crossing endpoint resolves to the fillet
//!   face, and the cap insertion closes its loop with the elliptical arc.
//!
//! So this module only (1) *plans* the crossing pre-surgery (detection +
//! feasibility gates + corrected rail endpoints + the ellipse arc), and
//! (2) *applies* the plan by overriding the rail endpoints (before the wall
//! face is built) and swapping the V-side cap edge's straight chord for the
//! elliptical arc (after it is built, before the splice runs).
//!
//! # Feasibility (typed refusals)
//!
//! The crossing termination exists iff each rail meets the fillet's live
//! boundary transversally. Degenerate configurations refuse with
//! [`BlendFailure::FilletCrossingInfeasible`] *before any mutation* (the
//! chamfer entry point is transactional): setback ≥ fillet zone (`d ≥ r`),
//! tangential grazing (`d == r`), wall plane parallel to the fillet axis,
//! non-planar walls, and sliver spans.

// Reason for `#![allow(clippy::indexing_slicing)]`: every index below is
// bounded by an explicit length check at the enclosing scope (offset
// polylines are validated `len() >= 2` at entry; `last = len() - 1`
// derivations follow) — the same canonical idiom `edge_blend_topology.rs`
// and `fillet_surfaces.rs` use.
#![allow(clippy::indexing_slicing)]

use super::diagnostics::BlendFailure;
use super::edge_blend_topology::{outer_shell_faces_at_vertex, BlendEdgeSurgery};
use super::{OperationError, OperationResult};
use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::{Arc, Curve, Ellipse, Line, ParameterRange},
    edge::{EdgeId, EdgeOrientation},
    face::FaceId,
    fillet_surfaces::CylindricalFillet,
    solid::SolidId,
    topology_builder::BRepModel,
    vertex::VertexId,
};

/// One planned fillet-crossing termination at a single chamfered-edge
/// endpoint. Built pre-surgery by [`plan_fillet_crossings`]; consumed
/// post-face-construction by [`apply_crossing_caps`].
#[derive(Debug)]
pub(crate) struct CrossingPlan {
    /// The chamfered edge's endpoint vertex on the fillet boundary.
    pub vertex: VertexId,
    /// The fillet face the chamfer terminates against.
    pub fillet_face: FaceId,
    /// `true` when this plan terminates the `t = 1` end of the chamfer
    /// data (offset index `last` → `v_t1_end`/`v_t2_end` → `cap_v1`);
    /// `false` for the `t = 0` end (`cap_v0`).
    pub at_t1_end: bool,
    /// The exact plane×cylinder ellipse, with `range` already set to the
    /// live arc span `[t_start, t_end]`.
    pub ellipse: Ellipse,
    /// Arc parameter range (increasing; span < π).
    pub t_start: f64,
    pub t_end: f64,
}

/// Geometric slack used for on-curve / on-surface membership checks:
/// an order of magnitude above the kernel distance tolerance, well under
/// any feature size the blend pipeline accepts.
fn membership_tol() -> f64 {
    Tolerance::default().distance() * 10.0
}

/// Minimum feature size for the crossing arc: chords / rail-to-boundary
/// clearances below this are treated as tangential grazing and refused.
fn sliver_tol() -> f64 {
    Tolerance::default().distance() * 100.0
}

fn refuse(vertex: VertexId, fillet_face: FaceId, reason: impl Into<String>) -> OperationError {
    OperationError::BlendFailed(Box::new(BlendFailure::FilletCrossingInfeasible {
        vertex,
        fillet_face,
        reason: reason.into(),
    }))
}

fn vpos(model: &BRepModel, v: VertexId) -> Option<Point3> {
    model.vertices.get(v).map(|vx| Point3::from(vx.position))
}

/// Plan the fillet-crossing terminations for one open chamfered edge.
///
/// Inspects both endpoints. For an endpoint whose third face (the unique
/// outer-shell face at the vertex besides the chamfer's two adjacent
/// faces) is a straight-spine [`CylindricalFillet`], computes the
/// analytic termination, MUTATES the rail offset polylines so the rails
/// end exactly on the fillet boundary, and returns the plan. Endpoints
/// that do not match the crossing configuration are left untouched
/// (legacy V-side treatment). Geometrically infeasible crossings refuse
/// typed — the chamfer entry point's transactional wrapper guarantees
/// the model is restored.
#[allow(clippy::too_many_arguments)]
pub(crate) fn plan_fillet_crossings(
    model: &BRepModel,
    solid_id: SolidId,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    offsets1: &mut [Point3],
    offsets2: &mut [Point3],
    partial_corner_vertices: &[VertexId],
) -> OperationResult<Vec<CrossingPlan>> {
    let edge = match model.edges.get(edge_id) {
        Some(e) => e,
        None => return Ok(Vec::new()),
    };
    if edge.is_loop() || offsets1.len() < 2 || offsets2.len() < 2 {
        return Ok(Vec::new());
    }

    // Curve-parameter → vertex mapping respects `Edge::orientation`,
    // mirroring the miter-override convention in `compute_chamfer_offsets`.
    let (v_at_t0, v_at_t1) = if edge.orientation.is_forward() {
        (edge.start_vertex, edge.end_vertex)
    } else {
        (edge.end_vertex, edge.start_vertex)
    };

    let mut plans = Vec::new();
    for (v, at_t1_end) in [(v_at_t0, false), (v_at_t1, true)] {
        if let Some(plan) = plan_one_endpoint(
            model,
            solid_id,
            edge_id,
            face1_id,
            face2_id,
            offsets1,
            offsets2,
            v,
            at_t1_end,
            partial_corner_vertices,
        )? {
            plans.push(plan);
        }
    }
    Ok(plans)
}

/// Straightness probe for a rail polyline: every interior sample within
/// `tol` of the endpoint chord. The crossing termination moves a rail
/// endpoint ALONG its carrier line, which is only exact for straight rails.
fn rail_is_straight(samples: &[Point3], tol: f64) -> bool {
    let n = samples.len();
    if n < 3 {
        return true;
    }
    let a = samples[0];
    let b = samples[n - 1];
    let dir = b - a;
    let len2 = dir.magnitude_squared();
    if len2 < tol * tol {
        return false;
    }
    samples.iter().all(|&p| {
        let t = (p - a).dot(&dir) / len2;
        let proj = a + dir * t;
        (p - proj).magnitude() <= tol
    })
}

/// The unique loop edge of `face_id` (other than `chamfer_edge`) incident
/// to `v`, required to also appear in `fillet_face`'s outer loop — i.e.
/// the shared boundary edge between the planar face and the fillet face.
fn shared_boundary_edge_at(
    model: &BRepModel,
    face_id: FaceId,
    fillet_face: FaceId,
    chamfer_edge: EdgeId,
    v: VertexId,
) -> Option<EdgeId> {
    let face = model.faces.get(face_id)?;
    let lp = model.loops.get(face.outer_loop)?;
    let mut found: Option<EdgeId> = None;
    for &eid in &lp.edges {
        if eid == chamfer_edge {
            continue;
        }
        let e = model.edges.get(eid)?;
        if e.start_vertex == v || e.end_vertex == v {
            if found.is_some() {
                return None; // over-connected vertex — not the clean crossing form
            }
            found = Some(eid);
        }
    }
    let candidate = found?;
    // Must be manifold-shared with the fillet face.
    let ff = model.faces.get(fillet_face)?;
    let flp = model.loops.get(ff.outer_loop)?;
    if flp.edges.contains(&candidate) {
        Some(candidate)
    } else {
        None
    }
}

/// Intersect the (straight) rail carrier line with the fillet-boundary
/// edge's curve. Returns the intersection point on the boundary curve.
///
/// * `Line` boundary (tangency contact seam): closed-form line×line
///   closest-approach solve; the gap must vanish.
/// * `Arc` boundary (fillet end-cap arc): the rail lies in the arc plane
///   (both live in the host planar face), so this is the planar
///   line×circle quadratic. Exactly one root may fall inside the edge's
///   live span; zero roots (or a vanishing discriminant) is the
///   `d ≥ r` / grazing family and refuses.
#[allow(clippy::too_many_arguments)]
fn rail_boundary_intersection(
    model: &BRepModel,
    boundary_edge: EdgeId,
    rail_origin: Point3,
    rail_dir: Vector3,
    v: VertexId,
    fillet_face: FaceId,
    side: &str,
) -> OperationResult<Point3> {
    let mtol = membership_tol();
    let stol = sliver_tol();
    let edge = model.edges.get(boundary_edge).ok_or_else(|| {
        OperationError::InvalidGeometry(format!("boundary edge {boundary_edge} missing"))
    })?;
    let curve = model.curves.get(edge.curve_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!("curve {} missing", edge.curve_id))
    })?;
    let range = edge.param_range;

    // Live endpoints of the boundary edge (one of them is `v` itself).
    let live_a = curve
        .point_at(range.start)
        .map_err(|e| OperationError::NumericalError(format!("boundary eval failed: {e:?}")))?;
    let live_b = curve
        .point_at(range.end)
        .map_err(|e| OperationError::NumericalError(format!("boundary eval failed: {e:?}")))?;

    // Accept a candidate point iff it projects onto the boundary curve
    // within tolerance, sits strictly inside the live span, and keeps a
    // non-sliver clearance from both live endpoints.
    let accept = |p: Point3| -> OperationResult<Option<Point3>> {
        let (t, proj) = curve
            .closest_point(&p, Tolerance::default())
            .map_err(|e| OperationError::NumericalError(format!("projection failed: {e:?}")))?;
        if (proj - p).magnitude() > mtol {
            return Ok(None);
        }
        let eps_t = (range.end - range.start).abs() * 1e-9;
        if t <= range.start.min(range.end) + eps_t || t >= range.start.max(range.end) - eps_t {
            return Ok(None);
        }
        if (proj - live_a).magnitude() <= stol || (proj - live_b).magnitude() <= stol {
            return Ok(None);
        }
        Ok(Some(proj))
    };

    // A boundary edge that is geometrically a straight segment: either an
    // exact `Line`, or a curve (the fillet's tangency contact curves are
    // stored as NURBS) whose live-range samples are collinear. Returns the
    // carrier (origin, direction) when linear.
    let linear_carrier = || -> Option<(Point3, Vector3)> {
        if let Some(line) = curve.as_any().downcast_ref::<Line>() {
            return Some((line.start, line.direction()));
        }
        let n = 5;
        let mut samples = Vec::with_capacity(n);
        for i in 0..n {
            let t = range.start + (range.end - range.start) * (i as f64) / ((n - 1) as f64);
            samples.push(curve.point_at(t).ok()?);
        }
        let a = samples[0];
        let b = samples[n - 1];
        let dir = b - a;
        let len2 = dir.magnitude_squared();
        if len2 < mtol * mtol {
            return None;
        }
        let collinear = samples.iter().all(|&p| {
            let t = (p - a).dot(&dir) / len2;
            (p - (a + dir * t)).magnitude() <= mtol
        });
        if collinear {
            Some((a, dir))
        } else {
            None
        }
    };

    if let Some((a0, w)) = linear_carrier() {
        // Two-line closest approach: rail P(s) = o + s·d, boundary
        // Q(t) = a + t·w. Solve the 2×2 normal system.
        let d = rail_dir;
        let dd = d.dot(&d);
        let dw = d.dot(&w);
        let ww = w.dot(&w);
        let det = dd * ww - dw * dw;
        if det.abs() < 1e-14 * dd.max(ww).max(1.0) {
            return Err(refuse(
                v,
                fillet_face,
                format!("{side}: rail is parallel to the fillet contact seam"),
            ));
        }
        let r0 = a0 - rail_origin;
        let s = (r0.dot(&d) * ww - r0.dot(&w) * dw) / det;
        let t = (r0.dot(&d) * dw - r0.dot(&w) * dd) / det;
        let p_rail = rail_origin + d * s;
        let p_line = a0 + w * t;
        if (p_rail - p_line).magnitude() > mtol {
            return Err(refuse(
                v,
                fillet_face,
                format!("{side}: rail does not meet the fillet contact seam (skew gap)"),
            ));
        }
        match accept(p_line)? {
            Some(p) => Ok(p),
            None => Err(refuse(
                v,
                fillet_face,
                format!(
                    "{side}: rail meets the contact seam outside its live span — \
                     the chamfer setback reaches past the fillet zone"
                ),
            )),
        }
    } else if let Some(arc) = curve.as_any().downcast_ref::<Arc>() {
        // Rail must lie in the arc plane (both live in the host face).
        let n = arc.normal;
        if (rail_origin - arc.center).dot(&n).abs() > mtol || rail_dir.dot(&n).abs() > 1e-6 {
            return Err(refuse(
                v,
                fillet_face,
                format!("{side}: rail is not coplanar with the fillet boundary arc"),
            ));
        }
        // |o + s·d − C|² = r², with d unit ⇒ s² + 2s·(o−C)·d + |o−C|²−r² = 0.
        let oc = rail_origin - arc.center;
        let half_b = oc.dot(&rail_dir);
        let c = oc.magnitude_squared() - arc.radius * arc.radius;
        let disc = half_b * half_b - c;
        let stol = sliver_tol();
        if disc <= stol * stol {
            return Err(refuse(
                v,
                fillet_face,
                format!(
                    "{side}: rail grazes the fillet boundary arc tangentially \
                     (chamfer distance equals the fillet zone width) — the \
                     crossing arc degenerates"
                ),
            ));
        }
        let sq = disc.sqrt();
        let mut hits = Vec::new();
        for s in [-half_b - sq, -half_b + sq] {
            if let Some(p) = accept(rail_origin + rail_dir * s)? {
                hits.push(p);
            }
        }
        match hits.as_slice() {
            [p] => Ok(*p),
            [] => Err(refuse(
                v,
                fillet_face,
                format!(
                    "{side}: rail clears the fillet boundary arc entirely — \
                     the chamfer distance is at or beyond the fillet radius zone"
                ),
            )),
            _ => Err(refuse(
                v,
                fillet_face,
                format!("{side}: rail crosses the fillet boundary arc twice (ambiguous)"),
            )),
        }
    } else {
        Err(refuse(
            v,
            fillet_face,
            format!(
                "{side}: fillet boundary curve type {:?} unsupported for \
                 crossing termination",
                curve.type_name()
            ),
        ))
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_one_endpoint(
    model: &BRepModel,
    solid_id: SolidId,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    offsets1: &mut [Point3],
    offsets2: &mut [Point3],
    v: VertexId,
    at_t1_end: bool,
    partial_corner_vertices: &[VertexId],
) -> OperationResult<Option<CrossingPlan>> {
    let mtol = membership_tol();

    if model.vertices.get(v).is_none() || partial_corner_vertices.contains(&v) {
        return Ok(None);
    }
    // Corners carrying prior blend marks or declared pending mixed-kind
    // corners belong to the CF-α/β corner machinery, not this path.
    if let Some(solid) = model.solids.get(solid_id) {
        if solid.vertex_blend_set(v).is_some()
            || solid.pending_mixed_kind_corners().contains_key(&v)
        {
            return Ok(None);
        }
    }

    // Exactly one third face at v, and it must be a straight-spine
    // cylindrical fillet bounded by planar F1/F2.
    let candidates = outer_shell_faces_at_vertex(model, solid_id, v, &[face1_id, face2_id])?;
    let [fillet_face] = candidates.as_slice() else {
        return Ok(None);
    };
    let fillet_face = *fillet_face;
    let Some(face) = model.faces.get(fillet_face) else {
        return Ok(None);
    };
    let Some(surface) = model.surfaces.get(face.surface_id) else {
        return Ok(None);
    };
    let Some(cf) = surface.as_any().downcast_ref::<CylindricalFillet>() else {
        return Ok(None);
    };
    let Some(spine) = cf.spine.as_any().downcast_ref::<Line>() else {
        return Ok(None); // curved-spine fillet — out of scope, legacy path
    };
    let planar = |fid: FaceId| {
        model
            .faces
            .get(fid)
            .and_then(|f| model.surfaces.get(f.surface_id))
            .map(|s| s.type_name() == "Plane")
            .unwrap_or(false)
    };
    if !planar(face1_id) || !planar(face2_id) {
        return Ok(None);
    }

    // The chamfered edge's loop neighbours at v must be the shared
    // planar↔fillet boundary edges — the geometric signature of the
    // crossing configuration.
    let Some(boundary1) = shared_boundary_edge_at(model, face1_id, fillet_face, edge_id, v) else {
        return Ok(None);
    };
    let Some(boundary2) = shared_boundary_edge_at(model, face2_id, fillet_face, edge_id, v) else {
        return Ok(None);
    };

    // Rails must be straight (their endpoints move along the carrier line).
    if !rail_is_straight(offsets1, mtol) || !rail_is_straight(offsets2, mtol) {
        return Err(refuse(
            v,
            fillet_face,
            "chamfer rails are not straight — crossing termination \
             supports straight-edge chamfers only",
        ));
    }

    let last1 = offsets1.len() - 1;
    let last2 = offsets2.len() - 1;
    let (end1, far1) = if at_t1_end { (last1, 0) } else { (0, last1) };
    let (end2, far2) = if at_t1_end { (last2, 0) } else { (0, last2) };

    // Cylinder frame from the straight spine.
    let axis = match (spine.end - spine.start).normalize() {
        Ok(a) => a,
        Err(_) => return Ok(None),
    };
    let c0 = spine.start;
    let radius = cf.radius;

    // Corrected rail endpoints on the fillet boundary.
    let dir1 = match (offsets1[end1] - offsets1[far1]).normalize() {
        Ok(d) => d,
        Err(_) => return Ok(None),
    };
    let dir2 = match (offsets2[end2] - offsets2[far2]).normalize() {
        Ok(d) => d,
        Err(_) => return Ok(None),
    };
    let e1 = rail_boundary_intersection(
        model,
        boundary1,
        offsets1[far1],
        dir1,
        v,
        fillet_face,
        "face1 rail",
    )?;
    let e2 = rail_boundary_intersection(
        model,
        boundary2,
        offsets2[far2],
        dir2,
        v,
        fillet_face,
        "face2 rail",
    )?;

    if (e1 - e2).magnitude() <= sliver_tol() {
        return Err(refuse(
            v,
            fillet_face,
            "crossing arc chord is a sliver — tangential grazing configuration",
        ));
    }

    // Wall plane through the two rails.
    let a_pt = offsets1[0];
    let b_pt = offsets1[last1];
    let mut n = (b_pt - a_pt).cross(&(offsets2[0] - a_pt));
    if n.magnitude_squared() < 1e-18 {
        n = (b_pt - a_pt).cross(&(offsets2[last2] - a_pt));
    }
    let n = match n.normalize() {
        Ok(n) => n,
        Err(_) => return Ok(None), // degenerate wall — legacy path decides
    };
    if (offsets2[last2] - a_pt).dot(&n).abs() > mtol || (offsets2[0] - a_pt).dot(&n).abs() > mtol {
        return Err(refuse(
            v,
            fillet_face,
            "chamfer wall is non-planar — crossing termination requires a planar wall",
        ));
    }
    let n_dot_a = n.dot(&axis);
    if n_dot_a.abs() < 1e-6 {
        return Err(refuse(
            v,
            fillet_face,
            "chamfer wall plane is parallel to the fillet axis — the \
             plane×cylinder section degenerates to lines",
        ));
    }

    // Both corrected endpoints must lie on the fillet cylinder.
    for (label, p) in [("face1 rail endpoint", e1), ("face2 rail endpoint", e2)] {
        let rel = p - c0;
        let radial = rel - axis * rel.dot(&axis);
        if (radial.magnitude() - radius).abs() > mtol {
            return Err(refuse(
                v,
                fillet_face,
                format!("{label} is off the fillet cylinder (defensive gate)"),
            ));
        }
    }

    // Exact plane×cylinder ellipse. Plane: n·p = k.
    let k = n.dot(&(a_pt - Point3::ORIGIN));
    let t_axis = (k - n.dot(&(c0 - Point3::ORIGIN))) / n_dot_a;
    let center = c0 + axis * t_axis;
    let minor_dir = match n.cross(&axis).normalize() {
        Ok(m) => m,
        Err(_) => {
            return Err(refuse(
                v,
                fillet_face,
                "wall normal parallel to fillet axis — degenerate section",
            ))
        }
    };
    let major_dir = match n.cross(&minor_dir).normalize() {
        Ok(q) => q,
        Err(_) => {
            return Err(refuse(v, fillet_face, "degenerate ellipse frame"));
        }
    };
    let semi_major = radius / n_dot_a.abs();
    let semi_minor = radius;

    // Angle of a point on the ellipse (must satisfy cos²+sin² = 1).
    let angle_of = |p: Point3| -> OperationResult<f64> {
        let rel = p - center;
        let c = rel.dot(&major_dir) / semi_major;
        let s = rel.dot(&minor_dir) / semi_minor;
        if (c * c + s * s - 1.0).abs() > 1e-6 {
            return Err(refuse(
                v,
                fillet_face,
                "rail endpoint is off the wall∩fillet ellipse (defensive gate)",
            ));
        }
        Ok(s.atan2(c).rem_euclid(std::f64::consts::TAU))
    };
    let t1 = angle_of(e1)?;
    let t2 = angle_of(e2)?;

    let ellipse = Ellipse::new(center, major_dir, minor_dir, semi_major, semi_minor)
        .map_err(|e| OperationError::NumericalError(format!("ellipse construction: {e:?}")))?;

    // Branch selection: of the two arcs between e1 and e2, exactly one
    // lies on the fillet face's live patch. Verify by inverting the
    // branch midpoint on the fillet surface (analytic for straight
    // spines) and demanding round-trip closure.
    let delta = (t2 - t1).rem_euclid(std::f64::consts::TAU);
    let candidates = [
        (t1, t1 + delta),                           // traverses e1 → e2
        (t2, t2 + (std::f64::consts::TAU - delta)), // traverses e2 → e1
    ];
    let on_live_patch = |t_mid: f64| -> bool {
        let rel_c = t_mid.cos();
        let rel_s = t_mid.sin();
        let pm = center + major_dir * (semi_major * rel_c) + minor_dir * (semi_minor * rel_s);
        // Exact chart inversion (straight spine — guaranteed by the
        // downcast gate above); round-trip closure is then a true
        // on-patch membership test, immune to iterative-projection
        // convergence luck.
        match cf.closest_point_analytic(&pm) {
            Some((u, w)) => match surface.point_at(u, w) {
                Ok(q) => (q - pm).magnitude() <= mtol,
                Err(_) => false,
            },
            None => false,
        }
    };
    let mut chosen: Option<(f64, f64)> = None;
    for (ts, te) in candidates {
        if te - ts >= std::f64::consts::PI - 1e-9 {
            continue; // the crossing arc is always the minor branch
        }
        if on_live_patch(0.5 * (ts + te)) {
            if chosen.is_some() {
                return Err(refuse(
                    v,
                    fillet_face,
                    "both ellipse branches lie on the fillet patch (ambiguous)",
                ));
            }
            chosen = Some((ts, te));
        }
    }
    let Some((t_start, t_end)) = chosen else {
        return Err(refuse(
            v,
            fillet_face,
            "no ellipse branch lies on the fillet's live patch — the wall \
             section leaves the fillet zone",
        ));
    };
    if t_end - t_start < 1e-6 {
        return Err(refuse(
            v,
            fillet_face,
            "crossing arc span is a sliver — tangential grazing configuration",
        ));
    }

    // Param-FORWARD contract. The cap edge's vertex pairing is fixed by
    // the surgery (`cap_v1`: v_t1_end → v_t2_end, i.e. e1 → e2;
    // `cap_v0`: v_t2_start → v_t1_start, i.e. e2 → e1), and every
    // downstream consumer (EdgeSampleCache emits samples in forward
    // parameter order; loop walkers pair `samples[0]` with the edge's
    // start vertex) assumes the curve parameter increases from the
    // start vertex. If the selected branch traverses the wrong way,
    // MIRROR the ellipse (negate the minor axis; t ↦ 2π − t maps the
    // same point set with reversed traversal) instead of emitting a
    // Backward-oriented edge the tessellation stack would walk inverted.
    let p_at_start = {
        let c = t_start.cos();
        let s = t_start.sin();
        center + major_dir * (semi_major * c) + minor_dir * (semi_minor * s)
    };
    let starts_at_e1 = (p_at_start - e1).magnitude() <= (p_at_start - e2).magnitude();
    let needs_e1_first = at_t1_end; // cap_v1 runs e1 → e2; cap_v0 runs e2 → e1
    let (mut ellipse, t_start, t_end) = if starts_at_e1 == needs_e1_first {
        (ellipse, t_start, t_end)
    } else {
        let mirrored = Ellipse::new(center, major_dir, -minor_dir, semi_major, semi_minor)
            .map_err(|e| OperationError::NumericalError(format!("ellipse mirror: {e:?}")))?;
        let span = t_end - t_start;
        let ts = (std::f64::consts::TAU - t_end).rem_euclid(std::f64::consts::TAU);
        (mirrored, ts, ts + span)
    };
    ellipse.range = ParameterRange::new(t_start, t_end);

    // Commit the corrected rail endpoints (linear re-interp keeps the
    // interior samples on the carrier line, mirroring the miter override).
    let relerp = |samples: &mut [Point3]| {
        let last = samples.len() - 1;
        if last >= 2 {
            let p0 = samples[0];
            let pn = samples[last];
            for (i, s) in samples.iter_mut().enumerate().take(last).skip(1) {
                let t = i as f64 / last as f64;
                *s = p0 + (pn - p0) * t;
            }
        }
    };
    offsets1[end1] = e1;
    offsets2[end2] = e2;
    relerp(offsets1);
    relerp(offsets2);

    Ok(Some(CrossingPlan {
        vertex: v,
        fillet_face,
        at_t1_end,
        ellipse,
        t_start,
        t_end,
    }))
}

/// Swap the straight V-side cap chord of each planned crossing for the
/// exact plane×cylinder elliptical arc. Runs after
/// `create_chamfer_face` (the cap edge and its vertices exist) and
/// before `splice_blend_edge` (nothing references the cap yet besides
/// the wall face's loop, which is orientation-flag based and unaffected
/// by the curve swap).
pub(crate) fn apply_crossing_caps(
    model: &mut BRepModel,
    surgery: &BlendEdgeSurgery,
    plans: &[CrossingPlan],
) -> OperationResult<()> {
    for plan in plans {
        let cap_edge_id = if plan.at_t1_end {
            surgery.cap_v1_edge
        } else {
            surgery.cap_v0_edge
        };
        let (cap_start, curve_id) = {
            let cap = model.edges.get(cap_edge_id).ok_or_else(|| {
                OperationError::InvalidGeometry(format!("cap edge {cap_edge_id} missing"))
            })?;
            (cap.start_vertex, cap.curve_id)
        };
        let start_pos = vpos(model, cap_start).ok_or_else(|| {
            OperationError::InvalidGeometry(format!("cap start vertex {cap_start} missing"))
        })?;

        let p_ts = plan
            .ellipse
            .point_at(plan.t_start)
            .map_err(|e| OperationError::NumericalError(format!("ellipse eval: {e:?}")))?;
        let p_te = plan
            .ellipse
            .point_at(plan.t_end)
            .map_err(|e| OperationError::NumericalError(format!("ellipse eval: {e:?}")))?;

        // The planner mirrors the ellipse so the arc is always
        // param-FORWARD from the cap's contract start vertex (the
        // tessellation cache and loop walkers pair `samples[0]` with
        // `start_vertex`). Verify that invariant held.
        let d_ts = (p_ts - start_pos).magnitude();
        let d_te = (p_te - start_pos).magnitude();
        if d_ts > membership_tol() {
            return Err(OperationError::InvalidGeometry(format!(
                "crossing cap at vertex {} (against fillet face {}) is not \
                 param-forward from its start vertex (t_start endpoint \
                 distance {:.3e}, t_end {:.3e})",
                plan.vertex, plan.fillet_face, d_ts, d_te
            )));
        }
        let orientation = EdgeOrientation::Forward;

        // Replace the cap's straight-chord curve IN PLACE (the Line was
        // created exclusively for this cap edge), then retarget the edge.
        let curve_slot = model.curves.get_mut(curve_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!("cap curve {curve_id} missing"))
        })?;
        *curve_slot = Box::new(plan.ellipse.clone());

        let cap = model.edges.get_mut(cap_edge_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!("cap edge {cap_edge_id} missing"))
        })?;
        cap.param_range = ParameterRange::new(plan.t_start, plan.t_end);
        cap.orientation = orientation;
        cap.invalidate_length_cache();
    }
    Ok(())
}
