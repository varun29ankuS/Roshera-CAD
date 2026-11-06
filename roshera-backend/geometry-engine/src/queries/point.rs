//! Point spatial queries (#14 slice 1) — classify + nearest.
//!
//! The composable spatial-query core's point primitive, built on the analytic
//! ray-cast: `classify_point` answers inside / outside / on, and
//! `nearest_on_solid` returns the closest boundary point with the face it lies
//! on (recoverable). Sound: classification is exact ray-parity along a generic
//! direction over analytic surface crossings; "on" is exact surface distance —
//! never a tessellation lookup. A point inside a through-hole reads OUTSIDE the
//! solid, because the hole is genuinely not material.

use super::raycast::raycast_all;
use crate::math::{Point3, Vector3};
use crate::primitives::edge::EdgeId;
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::primitives::vertex::VertexId;

/// Where a point sits relative to a solid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointClass {
    Inside,
    Outside,
    /// Within `tol` of the boundary surface.
    On,
}

/// Closest point on the solid's boundary to `p`, as `(face_id, surface_point,
/// distance)`. Minimizes over face interiors AND trim edges (edge endpoints
/// included, so vertices are covered too): a query point whose nearest feature
/// is an edge or a corner — the entire Voronoi wedge outside a box corner, for
/// example — gets the exact edge/vertex distance. (Slice 1 considered face
/// interiors only, so those wedges returned `None`, which a dense sampler then
/// silently recorded as `+inf`.)
pub fn nearest_on_solid(
    model: &BRepModel,
    solid_id: SolidId,
    p: Point3,
) -> Option<(FaceId, Point3, f64)> {
    nearest_boundary(model, solid_id, p).map(|n| (n.face, n.point, n.distance))
}

/// Which boundary feature a nearest point lies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NearestFeature {
    /// The interior of a face.
    Face,
    /// The interior of a trim edge.
    Edge(EdgeId),
    /// A vertex (an edge's endpoint).
    Vertex(VertexId),
}

/// The closest boundary point of a solid to a query point, with the feature
/// it lies on (so the side can be decided from the boundary's own normals).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NearestBoundary {
    /// A face owning the nearest point (the face itself, or the first face
    /// walking the nearest edge / vertex) — keeps the answer recoverable.
    pub face: FaceId,
    /// The nearest boundary point.
    pub point: Point3,
    /// Its distance from the query point.
    pub distance: f64,
    /// Face interior, edge or vertex.
    pub feature: NearestFeature,
}

/// [`nearest_on_solid`] with the nearest FEATURE reported (face interior,
/// edge, or vertex) — what the pseudonormal sign test needs.
pub fn nearest_boundary(
    model: &BRepModel,
    solid_id: SolidId,
    p: Point3,
) -> Option<NearestBoundary> {
    let solid = model.solids.get(solid_id)?;
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    shells.extend_from_slice(&solid.peer_shells);

    let mut best: Option<NearestBoundary> = None;
    // Edges to try after the face pass, each attributed to the first face that
    // walks it (deterministic: shell/face/loop order is deterministic), so an
    // edge-nearest answer stays recoverable to a real face.
    let mut edge_owner: Vec<(u32, FaceId)> = Vec::new();
    let mut seen_edges = std::collections::HashSet::new();

    for shell_id in shells {
        let shell = match model.shells.get(shell_id) {
            Some(s) => s,
            None => continue,
        };
        for &face_id in &shell.faces {
            let face = match model.faces.get(face_id) {
                Some(f) => f,
                None => continue,
            };
            // Collect this face's boundary edges for the edge pass.
            let mut loops = vec![face.outer_loop];
            loops.extend_from_slice(&face.inner_loops);
            for loop_id in loops {
                if let Some(lp) = model.loops.get(loop_id) {
                    for &eid in &lp.edges {
                        if seen_edges.insert(eid) {
                            edge_owner.push((eid, face_id));
                        }
                    }
                }
            }
            let surface = match model.surfaces.get(face.surface_id) {
                Some(s) => s,
                None => continue,
            };
            let (u, v) = match surface.closest_point(&p, model.tolerance()) {
                Ok(uv) => uv,
                Err(_) => continue,
            };
            if !crate::tessellation::surface::point_inside_face_uv(u, v, face, model) {
                continue;
            }
            let sp = match surface.point_at(u, v) {
                Ok(sp) => sp,
                Err(_) => continue,
            };
            let d = (p - sp).magnitude();
            if best.map(|b| d < b.distance).unwrap_or(true) {
                best = Some(NearestBoundary {
                    face: face_id,
                    point: sp,
                    distance: d,
                    feature: NearestFeature::Face,
                });
            }
        }
    }

    // Edge pass: exact nearest on each boundary edge, clamped to the edge's
    // parameter range; the range endpoints are always candidates, which covers
    // vertices. For periodic curves the clamp is conservative — the endpoint
    // candidates keep it correct at seams.
    for (eid, face_id) in edge_owner {
        let edge = match model.edges.get(eid) {
            Some(e) => e,
            None => continue,
        };
        let curve = match model.curves.get(edge.curve_id) {
            Some(c) => c,
            None => continue,
        };
        let (t0, t1) = {
            let (a, b) = (edge.param_range.start, edge.param_range.end);
            (a.min(b), a.max(b))
        };
        let mut candidates = [t0, t1, t0];
        if let Ok((t, _)) = curve.closest_point(&p, model.tolerance()) {
            candidates[2] = t.clamp(t0, t1);
        }
        for t in candidates {
            let cp = match curve.point_at(t) {
                Ok(cp) => cp,
                Err(_) => continue,
            };
            let d = (p - cp).magnitude();
            if best.map(|b| d < b.distance).unwrap_or(true) {
                // A parameter at the edge's range end is its vertex.
                let feature = if t == edge.param_range.start {
                    NearestFeature::Vertex(edge.start_vertex)
                } else if t == edge.param_range.end {
                    NearestFeature::Vertex(edge.end_vertex)
                } else {
                    NearestFeature::Edge(eid)
                };
                best = Some(NearestBoundary {
                    face: face_id,
                    point: cp,
                    distance: d,
                    feature,
                });
            }
        }
    }
    best
}

/// The boundary's ANGLE-WEIGHTED PSEUDONORMAL at a nearest point (Bærentzen &
/// Aanæs, "Signed distance computation using the angle weighted
/// pseudonormal", IEEE TVCG 2005): the outward face normal on a face
/// interior; the sum of the adjacent faces' unit outward normals on an edge;
/// the sum of the incident faces' normals weighted by each face's corner
/// angle at a vertex. Its defining property: for ANY point `p` whose nearest
/// boundary point is `q`, `(p − q) · n(q)` is negative exactly when `p` is
/// inside — the side follows from the boundary's own geometry, for every
/// surface type, with no ray. `None` when no normal can be evaluated there.
pub fn boundary_pseudonormal(
    model: &BRepModel,
    solid_id: SolidId,
    nearest: &NearestBoundary,
) -> Option<Vector3> {
    let q = nearest.point;
    let tol = model.tolerance();
    // Outward unit normal of `face_id` at the boundary point `q`.
    let outward = |face_id: FaceId| -> Option<Vector3> {
        let face = model.faces.get(face_id)?;
        let surface = model.surfaces.get(face.surface_id)?;
        let (u, v) = surface.closest_point(&q, tol).ok()?;
        (surface.normal_at(u, v).ok()? * face.orientation.sign())
            .normalize()
            .ok()
    };
    // Every face of the solid with the loop-edge lists it walks.
    let solid = model.solids.get(solid_id)?;
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    shells.extend_from_slice(&solid.peer_shells);
    let faces_walking = |pred: &dyn Fn(EdgeId) -> bool| -> Vec<(FaceId, Vec<(EdgeId, bool)>)> {
        let mut out = Vec::new();
        for &sh in &shells {
            let Some(shell) = model.shells.get(sh) else {
                continue;
            };
            for &fid in &shell.faces {
                let Some(face) = model.faces.get(fid) else {
                    continue;
                };
                let mut uses: Vec<(EdgeId, bool)> = Vec::new();
                for lid in std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied())
                {
                    if let Some(lp) = model.loops.get(lid) {
                        for (k, &eid) in lp.edges.iter().enumerate() {
                            if pred(eid) {
                                uses.push((eid, lp.orientations.get(k).copied().unwrap_or(true)));
                            }
                        }
                    }
                }
                if !uses.is_empty() {
                    out.push((fid, uses));
                }
            }
        }
        out
    };

    let sum = match nearest.feature {
        NearestFeature::Face => outward(nearest.face)?,
        NearestFeature::Edge(eid) => {
            let mut n = Vector3::ZERO;
            for (fid, uses) in faces_walking(&|e| e == eid) {
                // A seam edge is walked twice by the same face: count it once
                // per use, the same direction either way.
                if let Some(nf) = outward(fid) {
                    n = n + nf * uses.len() as f64;
                }
            }
            n
        }
        NearestFeature::Vertex(vid) => {
            let incident = |e: EdgeId| {
                model
                    .edges
                    .get(e)
                    .is_some_and(|ed| ed.start_vertex == vid || ed.end_vertex == vid)
            };
            let mut n = Vector3::ZERO;
            for (fid, uses) in faces_walking(&incident) {
                let Some(nf) = outward(fid) else {
                    continue;
                };
                // Corner angle of this face at the vertex: between its two
                // incident edges' tangents leaving the vertex, in the face's
                // tangent plane. Any other configuration (a single closed
                // edge, a seam vertex) weights the face equally.
                let leaving = |eid: EdgeId| -> Option<Vector3> {
                    let ed = model.edges.get(eid)?;
                    let curve = model.curves.get(ed.curve_id)?;
                    let at_start = ed.start_vertex == vid;
                    let t = if at_start {
                        ed.param_range.start
                    } else {
                        ed.param_range.end
                    };
                    let d1 = curve.evaluate(t).ok()?.derivative1;
                    let d = if at_start { d1 } else { d1 * -1.0 };
                    (d - nf * d.dot(&nf)).normalize().ok()
                };
                let weight = if uses.len() == 2 {
                    match (leaving(uses[0].0), leaving(uses[1].0)) {
                        (Some(a), Some(b)) => a.dot(&b).clamp(-1.0, 1.0).acos(),
                        _ => 1.0,
                    }
                } else {
                    1.0
                };
                n = n + nf * weight;
            }
            n
        }
    };
    sum.normalize().ok()
}

/// `Some(true)` when `p` is inside the solid, decided by the pseudonormal at
/// its nearest boundary point; `None` when that cannot be evaluated (the
/// caller then falls back to ray parity).
pub(crate) fn inside_by_pseudonormal(
    model: &BRepModel,
    solid_id: SolidId,
    p: Point3,
    nearest: &NearestBoundary,
) -> Option<bool> {
    let n = boundary_pseudonormal(model, solid_id, nearest)?;
    let s = (p - nearest.point).dot(&n);
    // A side test needs a side: exactly on the tangent plane (a point ON the
    // boundary, or numerically so) is not decided here.
    (s.abs() > 1e-12 * nearest.distance.max(1.0)).then_some(s < 0.0)
}

/// Classify `p`: `On` if within `tol` of the boundary, else `Inside`/`Outside`
/// by the PSEUDONORMAL at its nearest boundary point
/// ([`boundary_pseudonormal`]) — exact for every surface type.
///
/// It used ray parity alone, and the ray solver intersects only Plane,
/// Sphere, Cylinder and Cone: a ray through a torus (a fillet round a circular
/// edge), a NURBS face or a ruled wall (an extruded sketch arc) was not
/// counted, so those solids classified wrong (measured: a point 3 deep inside
/// a torus read Outside). Parity remains only as the fallback where no
/// pseudonormal can be evaluated.
pub fn classify_point(model: &BRepModel, solid_id: SolidId, p: Point3, tol: f64) -> PointClass {
    let nearest = nearest_boundary(model, solid_id, p);
    if let Some(n) = &nearest {
        if n.distance <= tol {
            return PointClass::On;
        }
    }
    let inside = nearest
        .and_then(|n| inside_by_pseudonormal(model, solid_id, p, &n))
        .unwrap_or_else(|| inside_by_parity(model, solid_id, p));
    if inside {
        PointClass::Inside
    } else {
        PointClass::Outside
    }
}

/// Ray parity (odd crossings = inside) along a generic direction — the
/// fallback side test. Correct only when every face crossed is one the ray
/// solver intersects (Plane / Sphere / Cylinder / Cone).
pub(crate) fn inside_by_parity(model: &BRepModel, solid_id: SolidId, p: Point3) -> bool {
    let dir = Vector3::new(0.5212, 0.3389, 0.7831);
    raycast_all(model, solid_id, p, dir).len() % 2 == 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Matrix4, Vector3};
    use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use crate::operations::{transform_solid, TransformOptions};
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    #[test]
    fn box_classify_inside_outside_on() {
        // Box 20³ centred at origin (each half-extent 10).
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("box"));
        assert_eq!(
            classify_point(&m, b, Point3::ZERO, 1e-6),
            PointClass::Inside
        );
        assert_eq!(
            classify_point(&m, b, Point3::new(50.0, 0.0, 0.0), 1e-6),
            PointClass::Outside
        );
        // A point exactly on the +Z face.
        assert_eq!(
            classify_point(&m, b, Point3::new(0.0, 0.0, 10.0), 1e-5),
            PointClass::On
        );
        // nearest: a point above the top is nearest the +Z face at distance 5.
        let (_, sp, d) = nearest_on_solid(&m, b, Point3::new(0.0, 0.0, 15.0)).expect("nearest");
        assert!((d - 5.0).abs() < 1e-6, "distance {d} != 5");
        assert!((sp.z - 10.0).abs() < 1e-6, "nearest on top face z=10");
    }

    #[test]
    fn cylinder_and_sphere_classify() {
        let mut m = BRepModel::new();
        let c = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::ZERO, Vector3::Z, 10.0, 30.0)
            .expect("cyl"));
        assert_eq!(
            classify_point(&m, c, Point3::new(0.0, 0.0, 15.0), 1e-6),
            PointClass::Inside
        );
        assert_eq!(
            classify_point(&m, c, Point3::new(20.0, 0.0, 15.0), 1e-6),
            PointClass::Outside
        );

        let mut m2 = BRepModel::new();
        let s = sid(TopologyBuilder::new(&mut m2)
            .create_sphere_3d(Point3::ZERO, 12.0)
            .expect("sphere"));
        assert_eq!(
            classify_point(&m2, s, Point3::ZERO, 1e-6),
            PointClass::Inside
        );
        assert_eq!(
            classify_point(&m2, s, Point3::new(0.0, 0.0, 12.0), 1e-5),
            PointClass::On
        );
        assert_eq!(
            classify_point(&m2, s, Point3::new(0.0, 0.0, 20.0), 1e-6),
            PointClass::Outside
        );
    }

    #[test]
    fn point_in_through_hole_is_outside_the_solid() {
        // A plate with a Ø20 through-bore. A point on the bore AXIS is inside
        // the HOLE, i.e. OUTSIDE the material — the soundness check: the query
        // sees the hole as genuinely not part of the solid.
        let mut m = BRepModel::new();
        let plate = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(50.0, 50.0, 16.0)
            .expect("plate"));
        let bore = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, -20.0), Vector3::Z, 10.0, 80.0)
            .expect("bore"));
        let part = boolean_operation(
            &mut m,
            plate,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("bore");
        // On the bore axis, mid-plate → inside the hole → OUTSIDE the solid.
        assert_eq!(
            classify_point(&m, part, Point3::ZERO, 1e-6),
            PointClass::Outside,
            "a point in the through-hole is outside the material"
        );
        // In the material (between bore wall r=10 and plate edge) → Inside.
        assert_eq!(
            classify_point(&m, part, Point3::new(20.0, 0.0, 0.0), 1e-6),
            PointClass::Inside,
            "a point in the plate material is inside"
        );
    }

    /// REGRESSION (found by the independent `boolean_sdf_oracle` falsifier,
    /// `tests/boolean_sdf_oracle.rs`): a bare, translated, radius-6 cylinder
    /// with NO boolean involved — `classify_point` alone misclassified two
    /// off-axis points near its top rim, in BOTH directions (a false
    /// `Inside` and a false `Outside`).
    ///
    /// Cylinder: created at base `(0,0,-15)`, axis `+Z`, radius 6, height 30
    /// (so local `v = z + 15`, height range `v ∈ [0, 30]`), then translated
    /// by `(8,0,0)` — axis line is `x=8, y=0`, spanning `z ∈ [-15, 15]`.
    ///
    /// Point `(-4, -6, -9)`: radial distance from the axis line is
    /// `sqrt((-4-8)^2 + (-6-0)^2) = sqrt(144+36) = sqrt(180) = 6*sqrt(5)
    /// ≈ 13.416`, which exceeds the radius (6) on its own — genuinely
    /// `Outside`, independent of height, since `z=-9` is comfortably inside
    /// `[-15, 15]` anyway.
    ///
    /// ROOT CAUSE: `classify_point`'s ray-parity (`raycast_all` with the
    /// fixed `PARITY_DIR`) casts a ray from this point and must cross the
    /// solid boundary an EVEN number of times. The real crossings are: (1)
    /// the lateral wall at `t≈12.10` (entering, valid — local `v≈15.48`,
    /// inside `[0,30]`) and (2) the top cap disk at `t≈30.64` (exiting,
    /// valid). That is 2 — correctly even/`Outside`. But the SAME ray also
    /// solves a second root on the lateral surface's quadratic at
    /// `t≈30.78`, landing at local `v≈30.11` — just PAST the real top rim
    /// (`v=30`). `Cylinder::closest_point` (designed for NEAREST-POINT
    /// projection, where clamping to the finite height is correct) clamped
    /// that `v` back down to exactly `30.0` — precisely on the trimmed
    /// face's own UV-rectangle boundary — and the winding-number trim test
    /// accepted it as a genuine hit due to floating-point noise at that
    /// boundary. That fabricated a 3rd crossing (odd, wrongly `Inside`).
    /// `(8, -2, 9)` shows the same defect producing the opposite flip (a
    /// spurious accepted hit turning a true 1-crossing `Inside` into an
    /// even 2-crossing `Outside`) — both were reproduced directly against
    /// `raycast_all`'s returned hit list before this fix, one extra
    /// `RayHit` on face `Cylinder` at local `v` just over 30 in each case.
    ///
    /// Fix: `Surface::exact_uv` (new trait method) gives the raycast path
    /// the TRUE, unclamped `(u, v)` of a point already known to lie exactly
    /// on the analytic surface, instead of reusing `closest_point`'s
    /// nearest-projection clamp — so a lateral-surface root past the real
    /// rim is correctly reported as out-of-trim-range and rejected.
    #[test]
    fn translated_cylinder_point_beyond_rim_classifies_correctly() {
        let mut m = BRepModel::new();
        let cyl = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, -15.0), Vector3::Z, 6.0, 30.0)
            .expect("cylinder"));
        transform_solid(
            &mut m,
            cyl,
            Matrix4::translation(8.0, 0.0, 0.0),
            TransformOptions::default(),
        )
        .expect("translate cylinder off-axis");

        assert_eq!(
            classify_point(&m, cyl, Point3::new(-4.0, -6.0, -9.0), 1e-6),
            PointClass::Outside,
            "radial distance from axis is 6*sqrt(5) ~= 13.416 > radius 6 -> genuinely outside"
        );
        assert_eq!(
            classify_point(&m, cyl, Point3::new(8.0, -2.0, 9.0), 1e-6),
            PointClass::Inside,
            "radial distance from axis is 2 < radius 6, z=9 in [-15,15] -> genuinely inside"
        );
    }
}
