//! B-Rep → polyhedral-cone bridge for contact determination (CD).
//!
//! This is where the pure cone algebra in [`crate::math::polyhedral_cone`]
//! finally touches a real solid. It builds the *first-order directional
//! structure* of a solid's boundary — the **normal cone** and **tangent cone**
//! at a vertex or along an edge — from the **exact** outward surface normals of
//! the faces meeting there (Crozet, *Smooth-BRep Contact Determination*, Ch. 3).
//!
//! The cone is exact, not tessellated: finitely many faces meet at a boundary
//! feature, and each contributes exactly one outward normal read straight off
//! its supporting surface (`Surface::normal_at`, exact even for NURBS). So the
//! "polyhedral cone" is an *exact* description of where the boundary can be
//! touched — the polyhedral structure is in the directional algebra, never in a
//! lossy approximation of the geometry.
//!
//! On top of the cones sit the two CD primitives the LMD search needs:
//!
//! * [`is_lmd_critical_direction`] — the critical-point gate: a separation
//!   direction is admissible only if it is an outward normal of *both* features
//!   (Crozet Eq. 1.23).
//! * [`features_can_contact`] — feature-pair culling: two features can produce a
//!   contact at all iff one's normal cone meets the *reflection* of the other's.
//!
//! Everything here is read-only; the model is interrogated, never mutated.

use crate::math::polyhedral_cone::{ConeIntersectionResult, PolyhedralCone};
use crate::math::vector3::{Point3, Vector3};
use crate::primitives::edge::EdgeId;
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::primitives::vertex::VertexId;

/// Outward unit normal of `face_id` at the 3D point `p` lying on (or nearest to)
/// it.
///
/// Reads the supporting surface's normal at the parameter closest to `p` and
/// flips it when the face is oriented `Backward` relative to its surface, so the
/// result always points *out of the solid*. Exact for analytic surfaces and for
/// NURBS (no tessellation). Returns `None` if the face or its surface is missing
/// or the surface cannot produce a unit normal there.
pub fn face_outward_normal_at(model: &BRepModel, face_id: FaceId, p: &Point3) -> Option<Vector3> {
    let face = model.faces.get(face_id)?;
    let surface = model.surfaces.get(face.surface_id)?;
    let (u, v) = surface.closest_point(p, model.tolerance()).ok()?;
    let n = surface.normal_at(u, v).ok()?.normalize().ok()?;
    // `orientation.sign()` is +1 for Forward (surface normal already outward),
    // −1 for Backward (surface normal points into the solid and must flip).
    Some(n * face.orientation.sign())
}

/// The **normal cone** of a vertex: the conic hull of the outward normals of the
/// faces meeting at it — the exact first-order directional structure of the
/// corner. A direction lies in this cone iff it is an outward normal of the
/// boundary at that vertex.
///
/// Returns `None` if the vertex or solid is unknown, or no incident face yields
/// a usable normal.
pub fn vertex_normal_cone(
    model: &BRepModel,
    solid_id: SolidId,
    vertex_id: VertexId,
) -> Option<PolyhedralCone> {
    let p = vertex_point(model, vertex_id)?;
    let mut normals = Vec::new();
    for face_id in solid_face_ids(model, solid_id) {
        if face_touches_vertex(model, face_id, vertex_id) {
            if let Some(n) = face_outward_normal_at(model, face_id, &p) {
                normals.push(n);
            }
        }
    }
    if normals.is_empty() {
        return None;
    }
    Some(PolyhedralCone::from_generators(&normals))
}

/// The **tangent cone** of a vertex: the polar of its normal cone — the set of
/// directions one can move while staying inside the solid to first order. For a
/// convex corner this is the intersection of the inward half-spaces of the
/// incident faces.
pub fn vertex_tangent_cone(
    model: &BRepModel,
    solid_id: SolidId,
    vertex_id: VertexId,
) -> Option<PolyhedralCone> {
    Some(vertex_normal_cone(model, solid_id, vertex_id)?.polar())
}

/// The **normal cone** along an edge: the conic hull of the outward normals of
/// the faces sharing it, evaluated at the edge's mid-point. For a smooth (G1)
/// edge the two normals coincide and this collapses to a single ray; for a
/// convex crease it is the dihedral wedge between the two face normals.
///
/// Returns `None` if the edge or solid is unknown, or no incident face yields a
/// usable normal.
pub fn edge_normal_cone(
    model: &BRepModel,
    solid_id: SolidId,
    edge_id: EdgeId,
) -> Option<PolyhedralCone> {
    let mid = edge_midpoint(model, edge_id)?;
    let mut normals = Vec::new();
    for face_id in solid_face_ids(model, solid_id) {
        if face_uses_edge(model, face_id, edge_id) {
            if let Some(n) = face_outward_normal_at(model, face_id, &mid) {
                normals.push(n);
            }
        }
    }
    if normals.is_empty() {
        return None;
    }
    Some(PolyhedralCone::from_generators(&normals))
}

/// The **tangent cone** along an edge: the polar of its normal cone.
pub fn edge_tangent_cone(
    model: &BRepModel,
    solid_id: SolidId,
    edge_id: EdgeId,
) -> Option<PolyhedralCone> {
    Some(edge_normal_cone(model, solid_id, edge_id)?.polar())
}

/// The **critical-point gate** (Crozet Eq. 1.23).
///
/// `d` is a candidate unit separation direction pointing *from* feature A *to*
/// feature B. The footpoint pair can be a local minimum-distance critical point
/// only if `d` is an outward normal of A (so A's boundary recedes from B along
/// `d`) and `-d` is an outward normal of B. Equivalently: `d ∈ N_A` and
/// `-d ∈ N_B`, where `N_•` are the (possibly dilated) normal cones.
///
/// For curved features pass cones already widened with
/// [`PolyhedralCone::dilate`] so the single-direction test conservatively covers
/// the whole patch.
pub fn is_lmd_critical_direction(
    d: &Vector3,
    normal_cone_a: &PolyhedralCone,
    normal_cone_b: &PolyhedralCone,
) -> bool {
    normal_cone_a.contains(d) && normal_cone_b.contains(&(-*d))
}

/// **Feature-pair culling.** Can features A and B touch at all, given only their
/// normal cones? A contact needs a direction `d` with `d ∈ N_A` and `-d ∈ N_B`;
/// such a `d` exists iff `N_A` meets the reflected cone `-N_B`. When this returns
/// `false` the pair can be discarded before any (expensive) LMD search.
pub fn features_can_contact(
    normal_cone_a: &PolyhedralCone,
    normal_cone_b: &PolyhedralCone,
) -> bool {
    matches!(
        normal_cone_a.intersects(&normal_cone_b.negated()),
        ConeIntersectionResult::Overlapping
    )
}

// ---------------------------------------------------------------------------
// Topology walks (private)
// ---------------------------------------------------------------------------

/// 3D position of a vertex as a [`Point3`].
fn vertex_point(model: &BRepModel, vertex_id: VertexId) -> Option<Point3> {
    let v = model.vertices.get(vertex_id)?;
    Some(Vector3::new(v.position[0], v.position[1], v.position[2]))
}

/// Mid-point of an edge — the curve evaluated at its mid-parameter when the
/// curve is available, else the average of the two endpoint vertices (exact for
/// the straight-edge case).
fn edge_midpoint(model: &BRepModel, edge_id: EdgeId) -> Option<Point3> {
    let edge = model.edges.get(edge_id)?;
    let t_mid = 0.5 * (edge.param_range.start + edge.param_range.end);
    if let Some(curve) = model.curves.get(edge.curve_id) {
        if let Ok(p) = curve.point_at(t_mid) {
            return Some(p);
        }
    }
    let a = vertex_point(model, edge.start_vertex)?;
    let b = vertex_point(model, edge.end_vertex)?;
    Some((a + b) * 0.5)
}

/// All face ids in a solid (outer shell plus any void shells).
fn solid_face_ids(model: &BRepModel, solid_id: SolidId) -> Vec<FaceId> {
    let mut out = Vec::new();
    let Some(solid) = model.solids.get(solid_id) else {
        return out;
    };
    let mut shell_ids = vec![solid.outer_shell];
    shell_ids.extend(solid.inner_shells.iter().copied());
    for sid in shell_ids {
        if let Some(shell) = model.shells.get(sid) {
            out.extend(shell.faces.iter().copied());
        }
    }
    out
}

/// Does any boundary loop of `face_id` reference `vertex_id`?
fn face_touches_vertex(model: &BRepModel, face_id: FaceId, vertex_id: VertexId) -> bool {
    face_edge_ids(model, face_id)
        .into_iter()
        .any(|eid| match model.edges.get(eid) {
            Some(edge) => edge.start_vertex == vertex_id || edge.end_vertex == vertex_id,
            None => false,
        })
}

/// Does any boundary loop of `face_id` contain `edge_id`?
fn face_uses_edge(model: &BRepModel, face_id: FaceId, edge_id: EdgeId) -> bool {
    face_edge_ids(model, face_id).contains(&edge_id)
}

/// Edge ids across a face's outer and inner loops.
fn face_edge_ids(model: &BRepModel, face_id: FaceId) -> Vec<EdgeId> {
    let mut out = Vec::new();
    let Some(face) = model.faces.get(face_id) else {
        return out;
    };
    let mut loop_ids = vec![face.outer_loop];
    loop_ids.extend(face.inner_loops.iter().copied());
    for lid in loop_ids {
        if let Some(lp) = model.loops.get(lid) {
            out.extend(lp.edges.iter().copied());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const X: Vector3 = Vector3::X;
    const Y: Vector3 = Vector3::Y;
    const Z: Vector3 = Vector3::Z;

    /// Build a 2×2×2 box centred at the origin (corners at ±1) and return the
    /// model plus its solid id.
    fn unit_box() -> (BRepModel, SolidId) {
        use crate::primitives::topology_builder::TopologyBuilder;
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("box creation succeeds");
        let solid_id = model
            .solids
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("box has a solid");
        (model, solid_id)
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    /// The vertex id whose position is closest to `target`.
    fn vertex_at(model: &BRepModel, target: Vector3) -> VertexId {
        model
            .vertices
            .iter()
            .min_by(|(_, va), (_, vb)| {
                let da = (Vector3::new(va.position[0], va.position[1], va.position[2]) - target)
                    .magnitude();
                let db = (Vector3::new(vb.position[0], vb.position[1], vb.position[2]) - target)
                    .magnitude();
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(id, _)| id)
            .expect("box has vertices")
    }

    /// An edge id whose two endpoints are closest to `a` and `b` (either order).
    fn edge_between(model: &BRepModel, a: Vector3, b: Vector3) -> EdgeId {
        let va = vertex_at(model, a);
        let vb = vertex_at(model, b);
        model
            .edges
            .iter()
            .find(|(_, e)| {
                (e.start_vertex == va && e.end_vertex == vb)
                    || (e.start_vertex == vb && e.end_vertex == va)
            })
            .map(|(id, _)| id)
            .expect("edge between the two corners exists")
    }

    fn ray(g: Vector3) -> PolyhedralCone {
        PolyhedralCone::from_generators(&[g])
    }

    // -- vertex normal cone ------------------------------------------------

    #[test]
    fn corner_normal_cone_is_the_positive_octant() {
        let (model, solid) = unit_box();
        let v = vertex_at(&model, Vector3::new(1.0, 1.0, 1.0));
        let cone = vertex_normal_cone(&model, solid, v).expect("corner has a normal cone");

        // Three faces meet → a pointed rank-3 cone: 3 generators, 3 supports.
        assert_eq!(cone.generators().len(), 3, "three incident faces");
        assert_eq!(cone.supports().len(), 3, "pointed cone has three supports");

        // It is the (+,+,+) octant: contains the three axes and the diagonal,
        // excludes the opposite directions.
        assert!(cone.contains(&X) && cone.contains(&Y) && cone.contains(&Z));
        assert!(cone.contains(&Vector3::new(1.0, 1.0, 1.0)));
        assert!(!cone.contains(&(-X)) && !cone.contains(&(-Y)) && !cone.contains(&(-Z)));
        assert!(!cone.contains(&Vector3::new(-1.0, -1.0, -1.0)));
    }

    #[test]
    fn every_corner_points_its_normal_cone_outward() {
        let (model, solid) = unit_box();
        // All eight sign combinations of (±1,±1,±1).
        for &sx in &[-1.0_f64, 1.0] {
            for &sy in &[-1.0_f64, 1.0] {
                for &sz in &[-1.0_f64, 1.0] {
                    let corner = Vector3::new(sx, sy, sz);
                    let v = vertex_at(&model, corner);
                    let cone =
                        vertex_normal_cone(&model, solid, v).expect("corner normal cone exists");
                    assert_eq!(cone.generators().len(), 3);
                    assert_eq!(cone.supports().len(), 3);
                    // Normal cone points away from centre: contains the outward
                    // diagonal, not the inward one.
                    let outward = corner; // centre is the origin
                    assert!(
                        cone.contains(&outward),
                        "normal cone must contain the outward diagonal at {corner:?}"
                    );
                    assert!(
                        !cone.contains(&(-outward)),
                        "normal cone must exclude the inward diagonal at {corner:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn corner_tangent_cone_is_the_polar_octant() {
        let (model, solid) = unit_box();
        let corner = Vector3::new(1.0, 1.0, 1.0);
        let v = vertex_at(&model, corner);
        let tangent = vertex_tangent_cone(&model, solid, v).expect("tangent cone exists");

        // Feasible directions from the (+,+,+) corner point back into the box:
        // the (−,−,−) octant. The direction to the box centre is admissible;
        // the outward diagonal is not.
        assert!(tangent.contains(&Vector3::new(-1.0, -1.0, -1.0)));
        assert!(!tangent.contains(&Vector3::new(1.0, 1.0, 1.0)));

        // Polarity: tangent == polar(normal), checked structurally.
        let normal = vertex_normal_cone(&model, solid, v).expect("normal cone exists");
        assert_eq!(tangent.generators().len(), normal.supports().len());
    }

    // -- edge normal cone --------------------------------------------------

    #[test]
    fn box_edge_normal_cone_is_a_two_face_wedge() {
        let (model, solid) = unit_box();
        // Vertical edge at x=+1, y=+1 (between the two z corners). Faces x=+1
        // (normal +X) and y=+1 (normal +Y) meet there.
        let edge = edge_between(
            &model,
            Vector3::new(1.0, 1.0, 1.0),
            Vector3::new(1.0, 1.0, -1.0),
        );
        let cone = edge_normal_cone(&model, solid, edge).expect("edge has a normal cone");

        assert_eq!(cone.generators().len(), 2, "two faces meet at the edge");
        // The wedge spanned by +X and +Y: contains their bisector, excludes the
        // out-of-plane axis and the opposite directions.
        assert!(cone.contains(&X) && cone.contains(&Y));
        assert!(cone.contains(&Vector3::new(1.0, 1.0, 0.0)));
        assert!(!cone.contains(&Z) && !cone.contains(&(-Z)));
        assert!(!cone.contains(&(-X)) && !cone.contains(&(-Y)));
    }

    // -- critical-point gate ----------------------------------------------

    #[test]
    fn opposed_faces_pass_the_critical_gate_along_their_normal() {
        // A's +X face vs B's −X face: separation A→B is +X.
        let a = ray(X);
        let b = ray(-X);
        assert!(is_lmd_critical_direction(&X, &a, &b));
        // Any other direction fails — it is not an outward normal of A.
        assert!(!is_lmd_critical_direction(&Y, &a, &b));
        assert!(!is_lmd_critical_direction(&(-X), &a, &b));
    }

    #[test]
    fn critical_gate_is_symmetric_under_reversal() {
        let (model, solid) = unit_box();
        let na = vertex_normal_cone(
            &model,
            solid,
            vertex_at(&model, Vector3::new(1.0, 1.0, 1.0)),
        )
        .expect("normal cone");
        let nb = vertex_normal_cone(
            &model,
            solid,
            vertex_at(&model, Vector3::new(-1.0, -1.0, -1.0)),
        )
        .expect("normal cone");
        // d admissible for (A,B) ⟺ −d admissible for (B,A).
        let d = Vector3::new(1.0, 1.0, 1.0)
            .normalize()
            .expect("nonzero direction");
        assert_eq!(
            is_lmd_critical_direction(&d, &na, &nb),
            is_lmd_critical_direction(&(-d), &nb, &na)
        );
    }

    // -- feature-pair culling ---------------------------------------------

    #[test]
    fn opposed_faces_can_contact_aligned_faces_cannot() {
        // +X face vs −X face: they face each other → contact possible.
        assert!(features_can_contact(&ray(X), &ray(-X)));
        // +X face vs +X face: both point the same way → no face-to-face contact.
        assert!(!features_can_contact(&ray(X), &ray(X)));
    }

    #[test]
    fn opposite_box_corners_can_mate_same_corner_cannot() {
        let (model, solid) = unit_box();
        let pos = vertex_normal_cone(
            &model,
            solid,
            vertex_at(&model, Vector3::new(1.0, 1.0, 1.0)),
        )
        .expect("normal cone");
        let neg = vertex_normal_cone(
            &model,
            solid,
            vertex_at(&model, Vector3::new(-1.0, -1.0, -1.0)),
        )
        .expect("normal cone");
        // The (+,+,+) corner's outward octant and the (−,−,−) corner's outward
        // octant are reflections of one another → a mating direction exists.
        assert!(features_can_contact(&pos, &neg));
        // A corner cannot mate face-to-face with a copy of itself.
        assert!(!features_can_contact(&pos, &pos));
    }

    #[test]
    fn culling_matches_explicit_gate_witness() {
        // features_can_contact is exactly "∃ d: gate(d, A, B)". Cross-check the
        // headline case: when culling says yes, the constructive direction works.
        let a = ray(X);
        let b = ray(-X);
        assert!(features_can_contact(&a, &b));
        assert!(
            is_lmd_critical_direction(&X, &a, &b),
            "the witness direction reported by the geometry passes the gate"
        );
        // And when culling says no, no axis-aligned witness exists.
        let c = ray(X);
        assert!(!features_can_contact(&a, &c));
        for d in [X, -X, Y, -Y, Z, -Z] {
            assert!(!is_lmd_critical_direction(&d, &a, &c));
        }
    }

    // -- outward-normal sanity --------------------------------------------

    #[test]
    fn face_normals_on_a_box_point_outward() {
        let (model, solid) = unit_box();
        // Sample each face by its incident corner; the normal must have a
        // positive component along the outward corner direction.
        let corner = Vector3::new(1.0, 1.0, 1.0);
        for face_id in solid_face_ids(&model, solid) {
            if face_touches_vertex(&model, face_id, vertex_at(&model, corner)) {
                let n = face_outward_normal_at(&model, face_id, &corner)
                    .expect("box face yields a normal");
                assert!(approx(n.magnitude(), 1.0), "normal is unit length");
                assert!(
                    n.dot(&corner) > 0.0,
                    "face normal at corner {corner:?} points outward (n = {n:?})"
                );
            }
        }
    }

    // -- property tests ----------------------------------------------------

    use proptest::prelude::*;

    fn unit_vec() -> impl Strategy<Value = Vector3> {
        (-1.0_f64..1.0, -1.0_f64..1.0, -1.0_f64..1.0).prop_filter_map("nonzero", |(x, y, z)| {
            Vector3::new(x, y, z).normalize().ok()
        })
    }

    proptest! {
        /// The critical-point gate is reversal-symmetric for any cones and any
        /// direction: gate(d, A, B) ⟺ gate(−d, B, A). This is the defining
        /// symmetry of a footpoint pair (swap the roles of the two solids and
        /// flip the connecting direction).
        #[test]
        fn gate_reversal_symmetry(d in unit_vec(), g1 in unit_vec(), g2 in unit_vec()) {
            let a = PolyhedralCone::from_generators(&[g1]);
            let b = PolyhedralCone::from_generators(&[g2]);
            prop_assert_eq!(
                is_lmd_critical_direction(&d, &a, &b),
                is_lmd_critical_direction(&(-d), &b, &a)
            );
        }

        /// Culling never rejects a real contact: if some direction passes the
        /// gate, `features_can_contact` must return `true`. (Soundness — culling
        /// is conservative, it only discards pairs that genuinely cannot touch.)
        #[test]
        fn culling_never_drops_a_gated_pair(d in unit_vec(), g1 in unit_vec(), g2 in unit_vec()) {
            let a = PolyhedralCone::from_generators(&[g1]);
            let b = PolyhedralCone::from_generators(&[g2]);
            if is_lmd_critical_direction(&d, &a, &b) {
                prop_assert!(features_can_contact(&a, &b));
            }
        }

        /// Culling is symmetric in its two features: a pair can contact
        /// regardless of which feature is named first.
        #[test]
        fn culling_is_symmetric(g1 in unit_vec(), g2 in unit_vec()) {
            let a = PolyhedralCone::from_generators(&[g1]);
            let b = PolyhedralCone::from_generators(&[g2]);
            prop_assert_eq!(
                features_can_contact(&a, &b),
                features_can_contact(&b, &a)
            );
        }
    }

    // Keep the `ConeIntersectionResult` import meaningful even if the matches!
    // in `features_can_contact` is the only other user.
    #[test]
    fn intersection_result_is_in_scope() {
        let overlapping = ray(X).intersects(&ray(X));
        assert!(matches!(overlapping, ConeIntersectionResult::Overlapping));
    }
}
