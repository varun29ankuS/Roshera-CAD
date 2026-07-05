//! Datum Reference Frame (DRF) designation and storage — the kernel half of
//! ASME Y14.5 datum reference frames.
//!
//! A **datum reference frame** (DRF) is the coordinate system a geometric
//! tolerance is measured *relative to*. It is established by designating real
//! features on the part — "face A is datum A, bore axis B is datum B" — and
//! pinning those designations by [`PersistentId`] so they survive regeneration
//! and boolean operations that change transient [`FaceId`]s.
//!
//! ## Storage discipline (mirrors the existing sidecar pattern)
//!
//! `drf: HashMap<SolidId, DatumReferenceFrame>` lives on [`BRepModel`] as a
//! sidecar — beside, not inside, the SoA topology stores. It is:
//!
//! * **Serde-persisted** with the model (via the `BRepModel` derive).
//! * **Cleared with geometry** (`clear_geometry` empties it because a DRF is
//!   bound to topology being discarded).
//! * **Snapshot-safe** because the PID maps it keys through ARE snapshotted.
//!
//! ## Honesty contract
//!
//! * [`designate_datum`] refuses non-qualifying surfaces (NURBS, cone, etc.)
//!   with a typed [`GdtError`] variant — the kernel cannot lie about what
//!   establishes a datum.
//! * [`resolve_datum`] performs a PID→FaceId lookup at call time; a consumed
//!   or missing face returns [`DatumResolution::Dangling`], never a stale
//!   verdict from cached geometry.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::gdt::model::{Datum, DatumKind};
use crate::math::{Point3, Vector3};
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::surface::{Cylinder, Plane};
use crate::primitives::topology_builder::BRepModel;

/// Typed errors from datum designation. Each variant carries a message
/// actionable for both agent and user — no silent failures.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum GdtError {
    /// The face is not part of the specified solid (Spec-A face∈solid
    /// membership discipline: the kernel must not designate a datum on a
    /// face that belongs to a different solid).
    #[error("face {face} is not a member of solid {solid}")]
    FaceNotInSolid { solid: SolidId, face: FaceId },

    /// The face carries no PersistentId; the datum cannot be pinned durably.
    /// Wire the operation that created the face for PID assignment first.
    #[error("face {face} has no PersistentId — cannot create a durable datum anchor")]
    FaceHasNoPersistentId { face: FaceId },

    /// The surface kind cannot establish a datum under ASME Y14.5 (e.g. a
    /// NURBS blob, a cone, a torus). Only planar → Plane datum and cylindrical
    /// → Axis datum are supported.
    #[error(
        "surface kind '{kind}' cannot establish a datum; supported kinds: Plane (→ datum plane), Cylinder (→ datum axis)"
    )]
    UnsupportedSurfaceKind { kind: String },

    /// A datum with this label is already registered in this solid's DRF.
    /// Labels must be unique within a frame (A, B, C are the canonical set).
    #[error(
        "datum label '{label}' is already designated in solid {solid}'s datum reference frame"
    )]
    DuplicateLabel { solid: SolidId, label: String },

    /// The specified solid does not exist in the model.
    #[error("solid {solid} does not exist in the model")]
    UnknownSolid { solid: SolidId },
}

/// The result of resolving a [`Datum`] at the current model state.
///
/// A datum is pinned by PID, not by transient face id, so it is possible for
/// the face to have been consumed by a later boolean (or a failed operation)
/// and its PID removed from the inverse map. In that case the datum is
/// **dangling**: the geometry it was anchored to no longer exists. A dangling
/// datum must NEVER produce a stale geometry snapshot — it silently blocks
/// evaluation instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DatumResolution {
    /// The datum feature is live: the PID still maps to an existing face.
    /// The reported `origin` and `direction` are derived from that face's
    /// current analytic surface at resolve time — never cached.
    Live {
        /// A point on the datum feature (plane origin or cylinder axis base).
        origin: Point3,
        /// The datum direction: plane normal (pointing out of the solid) or
        /// cylinder axis unit vector.
        direction: Vector3,
    },
    /// The datum's source face no longer exists in the model (consumed by a
    /// boolean, or the model was cleared). Do not evaluate tolerances that
    /// reference this datum.
    Dangling,
}

/// A datum reference frame stored per solid: an ordered set of datum
/// designations (A, B, C…) each pinned to a feature by [`PersistentId`].
///
/// The struct is deliberately thin — it holds only the compact designations.
/// Resolution (PID→live face→analytic geometry) happens on demand in
/// [`resolve_datum`], so no stale geometry is cached here.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DatumReferenceFrame {
    /// Ordered datum designations. The index within this vec reflects the
    /// primary / secondary / tertiary precedence per Y14.5, but labels are
    /// what the FCF references ("A", "B", "C").
    pub datums: Vec<Datum>,
}

impl DatumReferenceFrame {
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up a datum by label, e.g. `"A"`.
    pub fn datum_by_label(&self, label: &str) -> Option<&Datum> {
        self.datums.iter().find(|d| d.label == label)
    }

    /// True when no datums have been designated.
    pub fn is_empty(&self) -> bool {
        self.datums.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Helper: collect all FaceIds belonging to a solid (outer + inner shells).
// This is a module-private copy of the idiom used across queries/cd.rs,
// queries/features.rs, etc. — there is no shared free function yet.
// ---------------------------------------------------------------------------
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

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Designate a face as a datum feature in the specified solid's DRF.
///
/// # Behaviour
///
/// 1. Verifies the solid exists.
/// 2. Verifies the face belongs to that solid (Spec-A membership).
/// 3. Verifies the face has a [`PersistentId`] (durable anchor).
/// 4. Inspects the face's analytic surface:
///    - [`Plane`] → [`DatumKind::Plane`]
///    - [`Cylinder`] → [`DatumKind::Axis`]
///    - anything else → [`GdtError::UnsupportedSurfaceKind`]
/// 5. Checks the label is unique within this solid's DRF.
/// 6. Appends the datum and returns it.
///
/// The geometry (normal/axis) is NOT cached in the datum — it is resolved at
/// read time by [`resolve_datum`] so stale geometry is impossible.
pub fn designate_datum(
    model: &mut BRepModel,
    solid: SolidId,
    label: &str,
    face: FaceId,
) -> Result<Datum, GdtError> {
    // 1. Solid must exist.
    if model.solids.get(solid).is_none() {
        return Err(GdtError::UnknownSolid { solid });
    }

    // 2. Face must belong to this solid (Spec-A discipline).
    let face_ids = solid_face_ids(model, solid);
    if !face_ids.contains(&face) {
        return Err(GdtError::FaceNotInSolid { solid, face });
    }

    // 3. Face must have a PersistentId.
    let pid = model
        .face_pid(face)
        .ok_or(GdtError::FaceHasNoPersistentId { face })?;

    // 4. Determine datum kind from the surface type.
    let kind = {
        let face_data = model
            .faces
            .get(face)
            .ok_or(GdtError::FaceNotInSolid { solid, face })?;
        let surface = model.surfaces.get(face_data.surface_id).ok_or_else(|| {
            GdtError::UnsupportedSurfaceKind {
                kind: "missing".to_string(),
            }
        })?;

        if surface.as_any().downcast_ref::<Plane>().is_some() {
            DatumKind::Plane
        } else if surface.as_any().downcast_ref::<Cylinder>().is_some() {
            DatumKind::Axis
        } else {
            return Err(GdtError::UnsupportedSurfaceKind {
                kind: surface.type_name().to_string(),
            });
        }
    };

    // 5. Label must be unique within this solid's DRF.
    let drf = model.drf.entry(solid).or_default();
    if drf.datum_by_label(label).is_some() {
        return Err(GdtError::DuplicateLabel {
            solid,
            label: label.to_string(),
        });
    }

    // 6. Append and return.
    let datum = Datum::new(label, kind, pid);
    drf.datums.push(datum.clone());
    Ok(datum)
}

/// Resolve a [`Datum`] to its current live geometry, or report
/// [`DatumResolution::Dangling`] if the source face is gone.
///
/// # Geometry derivation at resolve time (never cached)
///
/// * **Plane datum**: `origin` = plane origin, `direction` = plane normal.
///   The face orientation is applied: a [`FaceOrientation::Backward`] face
///   has its surface normal pointing INTO the solid, so the datum direction is
///   flipped to point outward. This matches the convention used by `cd.rs`'s
///   `face_outward_normal_at`.
/// * **Axis datum**: `origin` = cylinder axis base (origin field),
///   `direction` = cylinder axis unit vector. The orientation flip is not
///   applied to axis datums because cylinder axes are unsigned (the axis
///   direction is the same regardless of face orientation).
/// * **Point datum**: not yet designated by `designate_datum`; would resolve
///   similarly when added.
/// NOTE (single-solid scope): PID lookup is model-wide — a face that
/// migrates to ANOTHER solid across a boolean still resolves Live for
/// the original DRF. Sound while DRFs are used single-solid (Tasks 1-3);
/// assembly scoping must add a face-in-solid cross-check.
pub fn resolve_datum(model: &BRepModel, _solid: SolidId, datum: &Datum) -> DatumResolution {
    // PID→FaceId lookup. Missing → Dangling.
    let Some(face_id) = model.face_by_pid(datum.feature) else {
        return DatumResolution::Dangling;
    };

    // Face must still exist in the face store.
    let Some(face_data) = model.faces.get(face_id) else {
        return DatumResolution::Dangling;
    };

    let Some(surface) = model.surfaces.get(face_data.surface_id) else {
        return DatumResolution::Dangling;
    };

    match datum.kind {
        DatumKind::Plane => {
            let Some(plane) = surface.as_any().downcast_ref::<Plane>() else {
                return DatumResolution::Dangling;
            };
            // Apply face orientation: Backward → flip the normal so it always
            // points outward. `FaceOrientation::sign()` is +1 (Forward) or -1
            // (Backward).
            let direction = plane.normal * face_data.orientation.sign();
            DatumResolution::Live {
                origin: plane.origin,
                direction,
            }
        }
        DatumKind::Axis => {
            let Some(cyl) = surface.as_any().downcast_ref::<Cylinder>() else {
                return DatumResolution::Dangling;
            };
            DatumResolution::Live {
                origin: cyl.origin,
                direction: cyl.axis,
            }
        }
        DatumKind::Point => {
            // Point datum designation not yet implemented in `designate_datum`;
            // arriving here would mean a Point datum was constructed manually.
            // Resolve honestly as Dangling until the surface interrogation for
            // point features is defined.
            DatumResolution::Dangling
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Point3, Vector3};
    use crate::primitives::persistent_id::PersistentId;
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    fn faces_of(m: &BRepModel, s: SolidId) -> Vec<FaceId> {
        let solid = m.solids.get(s).expect("solid exists");
        let mut shells = vec![solid.outer_shell];
        shells.extend_from_slice(&solid.inner_shells);
        let mut out = Vec::new();
        for sh in shells {
            if let Some(shell) = m.shells.get(sh) {
                out.extend_from_slice(&shell.faces);
            }
        }
        out
    }

    /// Find a planar face of `solid` whose normal is aligned with `axis`
    /// (0=X, 1=Y, 2=Z) at coordinate `coord`.
    fn planar_face_at(m: &BRepModel, solid: SolidId, axis: usize, coord: f64) -> Option<FaceId> {
        for fid in faces_of(m, solid) {
            let face = m.faces.get(fid)?;
            let surf = m.surfaces.get(face.surface_id)?;
            if let Some(p) = surf.as_any().downcast_ref::<Plane>() {
                let n = [p.normal.x, p.normal.y, p.normal.z];
                let o = [p.origin.x, p.origin.y, p.origin.z];
                let others_ok = (0..3).filter(|&i| i != axis).all(|i| n[i].abs() < 1e-6);
                if n[axis].abs() > 0.99 && (o[axis] - coord).abs() < 1e-6 && others_ok {
                    return Some(fid);
                }
            }
        }
        None
    }

    /// Find the cylindrical face of `solid`.
    fn cylinder_face(m: &BRepModel, solid: SolidId) -> Option<FaceId> {
        faces_of(m, solid).into_iter().find(|&fid| {
            m.faces
                .get(fid)
                .and_then(|f| m.surfaces.get(f.surface_id))
                .map(|s| s.as_any().downcast_ref::<Cylinder>().is_some())
                .unwrap_or(false)
        })
    }

    // -----------------------------------------------------------------------
    // Unit: DatumReferenceFrame helpers
    // -----------------------------------------------------------------------

    #[test]
    fn drf_lookup_by_label() {
        let pid = PersistentId::root(b"test-face");
        let mut drf = DatumReferenceFrame::new();
        assert!(drf.is_empty());
        drf.datums.push(Datum::new("A", DatumKind::Plane, pid));
        assert_eq!(
            drf.datum_by_label("A").map(|d| d.kind),
            Some(DatumKind::Plane)
        );
        assert!(drf.datum_by_label("B").is_none());
        assert!(!drf.is_empty());
    }

    #[test]
    fn drf_round_trips_through_json() {
        let pid = PersistentId::root(b"rtt");
        let mut drf = DatumReferenceFrame::new();
        drf.datums.push(Datum::new("A", DatumKind::Axis, pid));
        let json = serde_json::to_string(&drf).expect("serialize");
        let back: DatumReferenceFrame = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(drf, back);
    }

    // -----------------------------------------------------------------------
    // Unit: designate_datum refusals
    // -----------------------------------------------------------------------

    #[test]
    fn designate_unknown_solid_refuses() {
        let mut m = BRepModel::new();
        let phantom_solid: SolidId = 9999;
        let phantom_face: FaceId = 0;
        let err = designate_datum(&mut m, phantom_solid, "A", phantom_face)
            .expect_err("must refuse unknown solid");
        assert!(matches!(err, GdtError::UnknownSolid { .. }), "{err:?}");
    }

    #[test]
    fn designate_face_not_in_solid_refuses() {
        let mut m = BRepModel::new();
        m.set_event_key(Some("box-a".into()));
        let solid_a = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(10.0, 10.0, 10.0)
            .expect("box-a"));
        m.set_event_key(Some("box-b".into()));
        let solid_b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(10.0, 10.0, 10.0)
            .expect("box-b"));
        m.set_event_key(None);

        // Grab a face that belongs to solid_b and try to designate it on solid_a.
        let foreign_face = *m
            .solids
            .get(solid_b)
            .and_then(|s| m.shells.get(s.outer_shell))
            .map(|sh| sh.faces.first().expect("shell has faces"))
            .expect("solid_b shell");

        let err = designate_datum(&mut m, solid_a, "A", foreign_face)
            .expect_err("must refuse face from another solid");
        assert!(matches!(err, GdtError::FaceNotInSolid { .. }), "{err:?}");
    }

    #[test]
    fn designate_face_with_no_pid_refuses() {
        let mut m = BRepModel::new();
        // Create a box WITHOUT setting an event key so PIDs are seeded normally
        // by `assign_primitive_pids` — but we need a face that has NO pid.
        // Build the box, then manually strip its pid from the sidecar.
        m.set_event_key(Some("box".into()));
        let solid = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(10.0, 10.0, 10.0)
            .expect("box"));
        m.set_event_key(None);

        let top_face = planar_face_at(&m, solid, 2, 5.0).expect("+Z face");
        // Strip the PID so the face appears unregistered.
        let pid_of_top = m.face_pid(top_face).expect("face had pid");
        m.face_pids.remove(&top_face);
        m.pid_to_face.remove(&pid_of_top);

        let err = designate_datum(&mut m, solid, "A", top_face)
            .expect_err("must refuse face with no PID");
        assert!(
            matches!(err, GdtError::FaceHasNoPersistentId { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn designate_duplicate_label_refuses() {
        let mut m = BRepModel::new();
        m.set_event_key(Some("plate".into()));
        let solid = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 10.0, 5.0)
            .expect("plate"));
        m.set_event_key(None);

        let top = planar_face_at(&m, solid, 2, 2.5).expect("+Z face");
        let bottom = planar_face_at(&m, solid, 2, -2.5).expect("-Z face");

        designate_datum(&mut m, solid, "A", top).expect("first A ok");
        let err = designate_datum(&mut m, solid, "A", bottom)
            .expect_err("duplicate label must be refused");
        assert!(matches!(err, GdtError::DuplicateLabel { .. }), "{err:?}");
    }

    #[test]
    fn designate_sphere_face_refuses() {
        let mut m = BRepModel::new();
        m.set_event_key(Some("sphere".into()));
        let solid = sid(TopologyBuilder::new(&mut m)
            .create_sphere_3d(Point3::ORIGIN, 10.0)
            .expect("sphere"));
        m.set_event_key(None);

        // The sphere has only one face (the spherical surface).
        let sphere_face = *m
            .solids
            .get(solid)
            .and_then(|s| m.shells.get(s.outer_shell))
            .map(|sh| sh.faces.first().expect("sphere has a face"))
            .expect("solid shell");

        let err = designate_datum(&mut m, solid, "A", sphere_face)
            .expect_err("sphere face must be refused");
        assert!(
            matches!(err, GdtError::UnsupportedSurfaceKind { .. }),
            "{err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Unit: designate_datum successes
    // -----------------------------------------------------------------------

    #[test]
    fn plate_face_designates_as_plane_datum() {
        let mut m = BRepModel::new();
        m.set_event_key(Some("plate".into()));
        let solid = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(50.0, 30.0, 10.0)
            .expect("plate"));
        m.set_event_key(None);

        let top = planar_face_at(&m, solid, 2, 5.0).expect("+Z face");
        let datum = designate_datum(&mut m, solid, "A", top).expect("designate A");

        assert_eq!(datum.label, "A");
        assert_eq!(datum.kind, DatumKind::Plane);

        // The DRF on the solid must now carry exactly one datum.
        let drf = m.drf.get(&solid).expect("DRF stored for solid");
        assert_eq!(drf.datums.len(), 1);
        assert_eq!(drf.datums[0].label, "A");
    }

    #[test]
    fn cylinder_face_designates_as_axis_datum() {
        let mut m = BRepModel::new();
        m.set_event_key(Some("pin".into()));
        let solid = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, 5.0, 20.0)
            .expect("pin"));
        m.set_event_key(None);

        let lat = cylinder_face(&m, solid).expect("lateral cyl face");
        let datum = designate_datum(&mut m, solid, "B", lat).expect("designate B");

        assert_eq!(datum.label, "B");
        assert_eq!(datum.kind, DatumKind::Axis);
    }

    // -----------------------------------------------------------------------
    // Unit: resolve_datum
    // -----------------------------------------------------------------------

    #[test]
    fn live_plane_datum_resolves_with_correct_normal() {
        let mut m = BRepModel::new();
        m.set_event_key(Some("plate".into()));
        let solid = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(50.0, 30.0, 10.0)
            .expect("plate"));
        m.set_event_key(None);

        let top = planar_face_at(&m, solid, 2, 5.0).expect("+Z face");
        let datum = designate_datum(&mut m, solid, "A", top).expect("designate");

        match resolve_datum(&m, solid, &datum) {
            DatumResolution::Live {
                origin: _,
                direction,
            } => {
                // The +Z face of a centred box: normal is either (0,0,+1)
                // (Forward orientation) or (0,0,-1) (Backward, pointing in).
                // The resolver applies the orientation sign, so the result
                // is the OUTWARD normal — for the top face that is (0,0,+1).
                assert!(
                    (direction.z.abs() - 1.0).abs() < 1e-9,
                    "direction should be aligned with Z, got {direction:?}"
                );
            }
            DatumResolution::Dangling => panic!("expected Live, got Dangling"),
        }
    }

    #[test]
    fn live_axis_datum_resolves_with_cylinder_axis() {
        let mut m = BRepModel::new();
        m.set_event_key(Some("pin".into()));
        let solid = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, 5.0, 20.0)
            .expect("pin"));
        m.set_event_key(None);

        let lat = cylinder_face(&m, solid).expect("lateral face");
        let datum = designate_datum(&mut m, solid, "B", lat).expect("designate B");

        match resolve_datum(&m, solid, &datum) {
            DatumResolution::Live {
                origin: _,
                direction,
            } => {
                // Cylinder was created with axis = Z.
                assert!(
                    (direction.dot(&Vector3::Z) - 1.0).abs() < 1e-9,
                    "axis datum direction should be Z, got {direction:?}"
                );
            }
            DatumResolution::Dangling => panic!("expected Live, got Dangling"),
        }
    }

    #[test]
    fn dangling_pid_resolves_as_dangling() {
        let mut m = BRepModel::new();
        m.set_event_key(Some("plate".into()));
        let solid = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 20.0, 5.0)
            .expect("plate"));
        m.set_event_key(None);

        let top = planar_face_at(&m, solid, 2, 2.5).expect("+Z face");
        let datum = designate_datum(&mut m, solid, "A", top).expect("designate A");
        let pid = datum.feature;

        // Simulate the face being consumed: remove it from the PID inverse map.
        m.pid_to_face.remove(&pid);

        assert_eq!(
            resolve_datum(&m, solid, &datum),
            DatumResolution::Dangling,
            "a removed PID must resolve as Dangling"
        );
    }

    #[test]
    fn drf_cleared_with_geometry() {
        let mut m = BRepModel::new();
        m.set_event_key(Some("plate".into()));
        let solid = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(10.0, 10.0, 10.0)
            .expect("plate"));
        m.set_event_key(None);

        let top = planar_face_at(&m, solid, 2, 5.0).expect("+Z face");
        designate_datum(&mut m, solid, "A", top).expect("designate A");
        assert!(!m.drf.is_empty(), "DRF was stored");

        m.clear_geometry();
        assert!(m.drf.is_empty(), "DRF must be cleared with geometry");
    }
}
