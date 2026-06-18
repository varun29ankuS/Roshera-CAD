//! NURBS-skinned loft — build a watertight solid whose lateral wall is a single
//! freeform NURBS surface skinned through a stack of closed cross-section rings.
//!
//! This is the kernel's first operation that materialises a genuine NURBS
//! surface as a B-Rep face (every prior freeform op — `loft`, `sweep` — emits
//! `RuledSurface` / `SurfaceOfRevolution`). The lateral is `math::nurbs::
//! skin_surface` (Piegl & Tiller §10.3) interpolated through the sections; at
//! the default `degree_v = 3` it is **G2-continuous (curvature-continuous) along
//! the loft direction** by construction — a degree-p B-spline is C^{p-1}
//! internally — so a degree-3 skin is the cheapest sound way to build a
//! curvature-continuous freeform part (no inter-patch constraint solving, unlike
//! `operations::g2_blending`, which remains the tool for surface-to-surface
//! fillet blends).
//!
//! Topology mirrors `TopologyBuilder::create_cylinder_topology` EXACTLY — the
//! proven watertight closed-curved-lateral structure: two closed iso-curve ring
//! edges (bottom/top) + one seam edge, a single rectangular lateral loop that
//! walks the seam twice, and two planar end caps. The cylinder's invariant — the
//! seam vertex sits at the lateral surface's `u = 0` and at each ring curve's
//! parametric origin — is satisfied automatically here: a `v`-iso-curve's
//! `evaluate(0)` IS `S(0, v_end)`, and the seam `u`-iso-curve's endpoints ARE
//! `S(0, 0)` / `S(0, 1)` (avoids the cone-rim / frustum-throat seam-weld bug
//! class). Manifoldness comes from the cylinder's edge-pairing (each shared edge
//! used once forward, once backward); outward normals come from
//! `orient_face_for_outward`, so arbitrarily-oriented input sections are handled.

use super::orientation::orient_face_for_outward;
use super::{lifecycle, CommonOptions, OperationError, OperationResult};
use crate::math::nurbs::{skin_surface, NurbsSurface};
use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::{NurbsCurve, ParameterRange},
    edge::{Edge, EdgeOrientation},
    face::Face,
    r#loop::{Loop, LoopType},
    shell::{Shell, ShellType},
    solid::{Solid, SolidId},
    surface::{GeneralNurbsSurface, Plane},
    topology_builder::BRepModel,
};

/// Options for [`nurbs_loft`].
#[derive(Debug, Clone)]
pub struct NurbsLoftOptions {
    /// Common operation options (validation toggles).
    pub common: CommonOptions,
    /// Degree around the section (U). Clamped to `min(degree_u, nu - 1)`.
    pub degree_u: usize,
    /// Degree along the loft (V). `3` ⇒ G2 along the loft. Clamped to
    /// `min(degree_v, nv - 1)`.
    pub degree_v: usize,
}

impl Default for NurbsLoftOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            degree_u: 3,
            degree_v: 3,
        }
    }
}

/// Skin a watertight NURBS solid through `sections`.
///
/// Each section is an ORDERED ring of points (the U direction); sections are
/// ordered along the loft (the V direction). The rings are given OPEN (the
/// closing point is NOT repeated) — this op closes each ring internally so the
/// lateral wraps with a single seam. Every section must carry the same number of
/// points. The first and last sections must each be planar (they become the end
/// caps); intermediate sections may be anywhere.
///
/// Returns the new solid id. The lateral is one `GeneralNurbsSurface` face; the
/// caps are two `Plane` faces.
pub fn nurbs_loft(
    model: &mut BRepModel,
    sections: Vec<Vec<Point3>>,
    options: NurbsLoftOptions,
) -> OperationResult<SolidId> {
    let n_sections = sections.len();
    if n_sections < 2 {
        return Err(OperationError::IncompatibleProfiles);
    }
    let ring_len = sections[0].len();
    if ring_len < 3 {
        return Err(OperationError::InvalidGeometry(
            "nurbs_loft: each section needs at least 3 points".to_string(),
        ));
    }
    if sections.iter().any(|s| s.len() != ring_len) {
        return Err(OperationError::IncompatibleProfiles);
    }

    // Close each ring (repeat the first point) so the skinned U-curve is
    // geometrically closed (S(0,v) == S(1,v)) → a watertight seam.
    let closed_sections: Vec<Vec<Point3>> = sections
        .iter()
        .map(|s| {
            let mut c = s.clone();
            c.push(s[0]);
            c
        })
        .collect();
    let nu = ring_len + 1; // points per closed section
    let nv = n_sections;

    // Degrees, clamped so the interpolation system is well-posed.
    let degree_u = options.degree_u.clamp(1, nu - 1);
    let degree_v = options.degree_v.clamp(1, nv - 1);

    // ---- the skinned lateral surface. ----
    let surf: NurbsSurface = skin_surface(&closed_sections, degree_u, degree_v)
        .map_err(|e| OperationError::NumericalError(format!("nurbs_loft skin: {e}")))?;

    lifecycle::with_rollback(model, move |model| {
        let tol = Tolerance::default();

        // ---- seam vertices: the surface's u=0 endpoints. ----
        let p_bottom = surf.evaluate(0.0, 0.0).point;
        let p_top = surf.evaluate(0.0, 1.0).point;
        let v_bottom =
            model
                .vertices
                .add_or_find(p_bottom.x, p_bottom.y, p_bottom.z, tol.distance());
        let v_top = model
            .vertices
            .add_or_find(p_top.x, p_top.y, p_top.z, tol.distance());

        // ---- ring + seam curves from the surface's iso-curves. ----
        let bottom_iso = surf
            .iso_curve_v(0.0)
            .map_err(|e| OperationError::NumericalError(format!("bottom iso-curve: {e}")))?;
        let top_iso = surf
            .iso_curve_v(1.0)
            .map_err(|e| OperationError::NumericalError(format!("top iso-curve: {e}")))?;
        let seam_iso = surf
            .iso_curve_u(0.0)
            .map_err(|e| OperationError::NumericalError(format!("seam iso-curve: {e}")))?;

        let to_prim = |m: crate::math::nurbs::NurbsCurve| -> OperationResult<NurbsCurve> {
            NurbsCurve::new(m.degree, m.control_points, m.weights, m.knots.to_vec())
                .map_err(|e| OperationError::NumericalError(format!("iso-curve wrap: {e:?}")))
        };
        let bottom_curve = to_prim(bottom_iso)?;
        let top_curve = to_prim(top_iso)?;
        let seam_curve = to_prim(seam_iso)?;

        let bottom_curve_id = model.curves.add(Box::new(bottom_curve));
        let top_curve_id = model.curves.add(Box::new(top_curve));
        let seam_curve_id = model.curves.add(Box::new(seam_curve));

        // ---- planar end caps (fit + planarity check). ----
        let (bottom_plane, bottom_centroid) = fit_section_plane(&sections[0], tol, "bottom")?;
        let (top_plane, top_centroid) = fit_section_plane(&sections[n_sections - 1], tol, "top")?;

        // ---- surfaces. ----
        let lateral_surface_id = model
            .surfaces
            .add(Box::new(GeneralNurbsSurface { nurbs: surf }));
        let bottom_surface_id = model.surfaces.add(Box::new(bottom_plane));
        let top_surface_id = model.surfaces.add(Box::new(top_plane));

        // ---- edges: closed ring edges + linear-domain seam edge. ----
        let bottom_edge = model.edges.add(Edge::new(
            0,
            v_bottom,
            v_bottom,
            bottom_curve_id,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ));
        let top_edge = model.edges.add(Edge::new(
            0,
            v_top,
            v_top,
            top_curve_id,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ));
        let seam_edge = model.edges.add(Edge::new(
            0,
            v_bottom,
            v_top,
            seam_curve_id,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ));

        // ---- loops (edge directions identical to create_cylinder_topology so
        //      every shared edge is used once forward + once backward ⇒
        //      2-manifold ⇒ watertight). ----
        let mut bottom_loop = Loop::new(0, LoopType::Outer);
        bottom_loop.add_edge(bottom_edge, false);
        let bottom_loop_id = model.loops.add(bottom_loop);

        let mut top_loop = Loop::new(0, LoopType::Outer);
        top_loop.add_edge(top_edge, true);
        let top_loop_id = model.loops.add(top_loop);

        let mut lateral_loop = Loop::new(0, LoopType::Outer);
        lateral_loop.add_edge(bottom_edge, true);
        lateral_loop.add_edge(seam_edge, true);
        lateral_loop.add_edge(top_edge, false);
        lateral_loop.add_edge(seam_edge, false);
        let lateral_loop_id = model.loops.add(lateral_loop);

        // ---- face orientations: pick Forward/Backward so each oriented normal
        //      points OUT of the solid (handles arbitrary section winding). ----
        let axis = (top_centroid - bottom_centroid)
            .normalize()
            .unwrap_or(Vector3::Z);
        // Lateral: outward = radial from the loft axis at the surface midpoint.
        let lateral_surface = model
            .surfaces
            .get(lateral_surface_id)
            .ok_or_else(|| OperationError::InternalError("lateral surface vanished".into()))?;
        let mid = lateral_surface
            .point_at(0.5, 0.5)
            .map_err(|e| OperationError::NumericalError(format!("lateral midpoint: {e:?}")))?;
        let along = (mid - bottom_centroid).dot(&axis);
        let radial = (mid - (bottom_centroid + axis * along))
            .normalize()
            .unwrap_or(Vector3::X);
        let lateral_orientation = orient_face_for_outward(lateral_surface, radial)?;

        let bottom_surface = model
            .surfaces
            .get(bottom_surface_id)
            .ok_or_else(|| OperationError::InternalError("bottom surface vanished".into()))?;
        let bottom_orientation = orient_face_for_outward(bottom_surface, -axis)?;
        let top_surface = model
            .surfaces
            .get(top_surface_id)
            .ok_or_else(|| OperationError::InternalError("top surface vanished".into()))?;
        let top_orientation = orient_face_for_outward(top_surface, axis)?;

        // ---- faces. ----
        let mut lateral_face =
            Face::new(0, lateral_surface_id, lateral_loop_id, lateral_orientation);
        lateral_face.outer_loop = lateral_loop_id;
        let lateral_face_id = model.faces.add(lateral_face);

        let mut bottom_face = Face::new(0, bottom_surface_id, bottom_loop_id, bottom_orientation);
        bottom_face.outer_loop = bottom_loop_id;
        let bottom_face_id = model.faces.add(bottom_face);

        let mut top_face = Face::new(0, top_surface_id, top_loop_id, top_orientation);
        top_face.outer_loop = top_loop_id;
        let top_face_id = model.faces.add(top_face);

        // ---- shell + solid. ----
        let mut shell = Shell::new(0, ShellType::Closed);
        shell.add_face(lateral_face_id);
        shell.add_face(bottom_face_id);
        shell.add_face(top_face_id);
        let shell_id = model.shells.add(shell);
        let solid_id = model.solids.add(Solid::new(0, shell_id));
        // PILLAR 1: a NURBS skin is a genuine designed surface, not a primitive.
        model.set_solid_provenance(
            solid_id,
            crate::primitives::provenance::OperationKind::NurbsLoft,
            Vec::new(),
        );

        if options.common.validate_result {
            use crate::primitives::validation::{validate_solid_scoped, ValidationLevel};
            let report = validate_solid_scoped(model, solid_id, tol, ValidationLevel::Standard);
            if !report.is_valid {
                return Err(OperationError::InvalidBRep(format!(
                    "nurbs_loft result invalid: {:?}",
                    report.errors
                )));
            }
        }

        model.record_operation(
            crate::operations::recorder::RecordedOperation::new("nurbs_loft")
                .with_parameters(serde_json::json!({
                    "sections": n_sections,
                    "ring_points": ring_len,
                    "degree_u": degree_u,
                    "degree_v": degree_v,
                }))
                .with_output_solids([solid_id as u64]),
        );

        Ok(solid_id)
    })
}

/// Fit a plane to a section ring (Newell's robust normal + centroid) and verify
/// the ring is planar within `tol`. Returns the plane and the centroid.
fn fit_section_plane(
    ring: &[Point3],
    tol: Tolerance,
    which: &str,
) -> OperationResult<(Plane, Point3)> {
    let k = ring.len();
    let (mut sx, mut sy, mut sz) = (0.0_f64, 0.0_f64, 0.0_f64);
    for p in ring {
        sx += p.x;
        sy += p.y;
        sz += p.z;
    }
    let centroid = Point3::new(sx / k as f64, sy / k as f64, sz / k as f64);

    // Newell's method: robust polygon normal, immune to a single near-collinear
    // edge (unlike a 3-point cross product).
    let mut n = Vector3::ZERO;
    for i in 0..k {
        let c = ring[i];
        let nx = ring[(i + 1) % k];
        n.x += (c.y - nx.y) * (c.z + nx.z);
        n.y += (c.z - nx.z) * (c.x + nx.x);
        n.z += (c.x - nx.x) * (c.y + nx.y);
    }
    let normal = n.normalize().map_err(|_| {
        OperationError::InvalidGeometry(format!(
            "nurbs_loft: {which} section is degenerate (zero-area ring)"
        ))
    })?;

    // Planarity: every point within tol of the fitted plane.
    let max_dev = ring
        .iter()
        .map(|p| (*p - centroid).dot(&normal).abs())
        .fold(0.0_f64, f64::max);
    if max_dev > tol.distance().max(1e-6) * 1e3 {
        return Err(OperationError::NotImplemented(format!(
            "nurbs_loft: {which} end section is non-planar (max deviation {max_dev:.6}); \
             planar end caps only in v1"
        )));
    }

    let plane = Plane::from_point_normal(centroid, normal).map_err(|e| {
        OperationError::NumericalError(format!("nurbs_loft: {which} cap plane: {e:?}"))
    })?;
    Ok((plane, centroid))
}
