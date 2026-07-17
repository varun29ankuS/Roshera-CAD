//! Mate connectors + the document-level mate taxonomy (kinematic-assembly
//! campaign, Slices 1–2).
//!
//! # The mate-connector model
//!
//! A mate is ONE relationship between two coordinate FRAMES, not a stack of
//! primitive constraints (Onshape's design; spec 2026-07-16 §3.2). A
//! [`MateConnector`] is a frame bound to a PLACE on an instance — the
//! labeller discipline ("a NAME bound to a PLACE, kept proven") applied to
//! kinematics. A single `Revolute` mate declares the joint's full DOF
//! signature in one call: the mate IS the joint.
//!
//! # The durability ladder
//!
//! [`ConnectorAnchor`] names WHERE the frame is anchored, best-first:
//!
//! 1. `FacePid` — durable identity (survives re-extrude; #11 lineage),
//! 2. `Label`  — durable name (label → PID → assertion re-verified),
//! 3. `Fingerprint` — best-effort geometric re-resolve,
//! 4. `RawFrame` — explicit coordinates (datum-style; the engine's
//!    `mate_anchor` probe is the last line of defence against fabricated
//!    joints).
//!
//! The stored [`ConnectorFrame`] is PART-LOCAL and is RE-DERIVED from the
//! anchored feature at every resolve — an edited bore MOVES its axis and
//! the mate follows. A resolve failure marks the mate STALE (surfaced,
//! never silently re-anchored — the labeller's `Holds | Stale` contract).
//!
//! # Division of labour
//!
//! These are DOCUMENT types: pure serde data on [`super::instancing::
//! InstancedAssembly`], no solver. The assembly-engine crate owns the
//! residuals/DOF mathematics over *resolved* frames; the api-server maps
//! document mates into the engine's `SolveInput` view at solve time.

use crate::math::{Point3, Vector3};
use crate::primitives::face::FaceId;
use crate::primitives::surface::{Cylinder, Plane, Sphere};
use crate::primitives::topology_builder::BRepModel;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable identifier of a mate connector within an assembly document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MateConnectorId(pub Uuid);

impl MateConnectorId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for MateConnectorId {
    fn default() -> Self {
        Self::new()
    }
}

/// Stable identifier of a mate within an assembly document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DocMateId(pub Uuid);

impl DocMateId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for DocMateId {
    fn default() -> Self {
        Self::new()
    }
}

/// A right-handed coordinate frame in the PART-LOCAL space of the instance
/// the connector sits on: `z_axis` is the primary direction (face normal /
/// bore axis), `x_axis` the secondary (in-plane major direction / axis
/// reference direction). `y = z × x` is implied.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ConnectorFrame {
    pub origin: [f64; 3],
    pub z_axis: [f64; 3],
    pub x_axis: [f64; 3],
}

/// WHERE a connector's frame is anchored — the durability ladder (module
/// doc). The variant IS the provenance the certificate reports per mate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConnectorAnchor {
    /// Durable face identity: the frame derives from the face resolved via
    /// `face_by_pid`. Survives re-extrude / replay (#11 lineage).
    FacePid { pid: u128 },
    /// Durable NAME: label → PID → assertion, re-verified on every resolve
    /// (`Holds | Stale`). The strongest agent-ergonomic anchor.
    Label { name: String },
    /// Best-effort geometric identity, used when the target face has no PID
    /// yet (fillet/chamfer/pattern faces — #11 slices 40-E/F pending). The
    /// certificate reports the DEGRADED provenance honestly.
    Fingerprint {
        /// Part-local representative point (face centroid).
        position: [f64; 3],
        /// Representative outward normal, when the face has one.
        normal: Option<[f64; 3]>,
        /// Representative radius (cylindrical/conical faces).
        radius: Option<f64>,
        /// Representative size (face area) — coarse identity signal.
        size: Option<f64>,
    },
    /// Explicit coordinates, no geometry binding (datum-style). The
    /// engine-side `mate_anchor` probe is the anti-fabrication check.
    RawFrame,
}

/// The provenance rung a connector currently stands on — reported in every
/// solve/certify response so anchor degradation is visible, never hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnchorProvenance {
    Pid,
    Label,
    Fingerprint,
    Raw,
}

impl ConnectorAnchor {
    pub fn provenance(&self) -> AnchorProvenance {
        match self {
            ConnectorAnchor::FacePid { .. } => AnchorProvenance::Pid,
            ConnectorAnchor::Label { .. } => AnchorProvenance::Label,
            ConnectorAnchor::Fingerprint { .. } => AnchorProvenance::Fingerprint,
            ConnectorAnchor::RawFrame => AnchorProvenance::Raw,
        }
    }
}

/// A coordinate frame bound to a PLACE on an instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MateConnector {
    pub id: MateConnectorId,
    /// The instance this connector belongs to.
    pub instance: super::instancing::InstanceId,
    /// WHERE the frame is anchored (durability ladder).
    pub anchor: ConnectorAnchor,
    /// The frame derived from the anchored feature at the LAST resolve,
    /// in part-local coordinates. Re-derived on every solve for
    /// `FacePid`/`Label`/`Fingerprint` anchors; authoritative as stored
    /// for `RawFrame`.
    pub frame: ConnectorFrame,
    /// Radius of the anchored feature when it is cylindrical/conical —
    /// consumed by `Tangent` mates. `None` for planar anchors.
    #[serde(default)]
    pub radius: Option<f64>,
}

/// The document-level mate taxonomy (spec §3.2). Joint-school primary,
/// dimensional overlays secondary, DOF couplings third, honest-refuse tail.
/// Limits are first-class joint parameters `(min, max)`; enforcement with
/// at-limit facts is campaign Slice 5 — the fields are part of the document
/// contract from day one so timelines never need a migration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DocMateKind {
    /// 0 DOF — the true rigid lock (bolt pattern).
    Fastened,
    /// 1 rotational DOF about the connector z.
    Revolute {
        limits: Option<(f64, f64)>,
    },
    /// 1 translational DOF along the connector z.
    Slider {
        limits: Option<(f64, f64)>,
    },
    /// 1 rot + 1 trans about/along z.
    Cylindrical {
        rot_limits: Option<(f64, f64)>,
        trans_limits: Option<(f64, f64)>,
    },
    /// 2 trans + 1 rot in the connector plane.
    Planar,
    /// 3 rotational DOF about the connector origin.
    Ball,
    /// 1 rot (pin) + 1 trans (slot). `slot_dir_x` picks the slot direction:
    /// frame-A x (true) or y (false).
    PinSlot {
        slot_dir_x: bool,
        limits: Option<(f64, f64)>,
    },
    // ── Dimensional overlays ──────────────────────────────────────────
    /// Signed offset along frame-A z between the origins. Rank 1.
    Distance {
        value: f64,
    },
    /// Angle between the two z axes. Rank 1.
    Angle {
        value: f64,
    },
    /// z axes parallel (or antiparallel). Rank 2.
    Parallel,
    /// Frame-B feature (cylinder/sphere, via connector `radius`) tangent to
    /// frame-A plane. Rank 1. Pairs beyond plane↔cylinder/sphere REFUSE.
    Tangent,
    // ── DOF couplings between EXISTING joints (Onshape "relations") ──
    /// Couples the rotation parameters of two Revolute/Cylindrical mates:
    /// `ratio·Δθ₁ + Δθ₂ = 0` measured from `at`.
    GearRatio {
        ratio: f64,
    },
    /// Couples a Revolute rotation to a Slider translation:
    /// `pinion_radius·Δθ − Δs = 0` measured from `at`.
    RackPinion {
        pinion_radius: f64,
    },
    /// Couples the two DOF WITHIN one Cylindrical mate into a helix:
    /// `Δs − lead·Δθ/2π = 0` measured from `at`.
    Screw {
        lead: f64,
    },
    // ── Honest-refuse set (typed; solver refuses with a fact, never a
    //    silent zero-DOF lie — the #19 `is_numerically_enforced` contract)
    Cam,
    Path,
    Symmetric,
}

/// A mate: ONE relationship between two connector frames (+ optional
/// coupling references for the relation kinds).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocMate {
    pub id: DocMateId,
    pub kind: DocMateKind,
    pub a: MateConnectorId,
    pub b: MateConnectorId,
    /// For coupling kinds (`GearRatio`/`RackPinion`/`Screw`): the mates
    /// whose joint parameters are coupled — two for gear/rack-pinion, one
    /// for screw. Empty for every geometric kind.
    #[serde(default)]
    pub couples: Vec<DocMateId>,
    /// Joint-parameter values of the coupled mates AT DECLARATION (the
    /// coupling's reference configuration), captured by the caller when
    /// the mate is created. Empty for geometric kinds.
    #[serde(default)]
    pub at: Vec<f64>,
}

// ── Frame derivation from anchored faces ────────────────────────────────

/// A connector frame derived from a face, plus the feature radius when the
/// face is cylindrical/spherical (consumed by Tangent mates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DerivedFrame {
    pub frame: ConnectorFrame,
    pub radius: Option<f64>,
}

/// Exact analytic centroid of a face, with the boundary-vertex mean as the
/// fallback when the centroid integral declines (annular caps) — the same
/// two-step the labeller's anchor uses. `&mut` because stats computation
/// warms a per-face cache.
fn face_centroid(model: &mut BRepModel, face: FaceId) -> Option<Point3> {
    let BRepModel {
        faces,
        loops,
        vertices,
        edges,
        curves,
        surfaces,
        ..
    } = model;
    if let Some(f) = faces.get_mut(face) {
        if let Ok(stats) = f.compute_stats(loops, vertices, edges, curves, surfaces) {
            return Some(stats.centroid);
        }
    }
    model.face_boundary_mean(face)
}

/// Face surface area via the stats cache (fingerprint size signal).
fn face_area(model: &mut BRepModel, face: FaceId) -> Option<f64> {
    let BRepModel {
        faces,
        loops,
        vertices,
        edges,
        curves,
        surfaces,
        ..
    } = model;
    faces
        .get_mut(face)?
        .compute_stats(loops, vertices, edges, curves, surfaces)
        .ok()
        .map(|s| s.area)
}

/// Derive the connector frame a face MEANS, read from its exact analytic
/// surface (never inferred from the mesh — the relational-query
/// discipline):
///
///   * planar face       → origin = face centroid, z = normal, x = u_dir
///   * cylindrical face  → origin = face centroid projected onto the axis,
///                         z = axis, x = ref_dir; radius carried
///   * spherical face    → origin = centre, z = north, x = ref_dir;
///                         radius carried
///
/// `None` for faces without an analytic frame (cones/tori/NURBS today) —
/// the caller REFUSES or degrades, never guesses.
pub fn derive_frame_for_face(model: &mut BRepModel, face: FaceId) -> Option<DerivedFrame> {
    let surface_id = model.faces.get(face)?.surface_id;
    // Read the analytic parameters first (immutable borrow), then the
    // centroid (mutable, cache-warming).
    enum Kind {
        Plane {
            normal: Vector3,
            u_dir: Vector3,
        },
        Cyl {
            origin: Point3,
            axis: Vector3,
            ref_dir: Vector3,
            radius: f64,
        },
        Sph {
            center: Point3,
            north: Vector3,
            ref_dir: Vector3,
            radius: f64,
        },
    }
    let kind = {
        let surf = model.surfaces.get(surface_id)?;
        if let Some(p) = surf.as_any().downcast_ref::<Plane>() {
            Kind::Plane {
                normal: p.normal.normalize().ok()?,
                u_dir: p.u_dir.normalize().ok()?,
            }
        } else if let Some(c) = surf.as_any().downcast_ref::<Cylinder>() {
            Kind::Cyl {
                origin: c.origin,
                axis: c.axis.normalize().ok()?,
                ref_dir: c.ref_dir.normalize().ok()?,
                radius: c.radius,
            }
        } else if let Some(s) = surf.as_any().downcast_ref::<Sphere>() {
            Kind::Sph {
                center: s.center,
                north: s.north_dir.normalize().ok()?,
                ref_dir: s.ref_dir.normalize().ok()?,
                radius: s.radius,
            }
        } else {
            return None;
        }
    };
    match kind {
        Kind::Plane { normal, u_dir } => {
            let origin = face_centroid(model, face)?;
            // Re-orthonormalize x against z (u_dir is ⊥ by construction,
            // but keep the invariant explicit).
            let x = (u_dir - normal * u_dir.dot(&normal)).normalize().ok()?;
            Some(DerivedFrame {
                frame: ConnectorFrame {
                    origin: [origin.x, origin.y, origin.z],
                    z_axis: [normal.x, normal.y, normal.z],
                    x_axis: [x.x, x.y, x.z],
                },
                radius: None,
            })
        }
        Kind::Cyl {
            origin,
            axis,
            ref_dir,
            radius,
        } => {
            // Origin = the face centroid's foot on the axis — the axial
            // MIDDLE of the actual face, so a re-extruded (longer) bore
            // moves its connector origin with it.
            let c = face_centroid(model, face)?;
            let foot = origin + axis * (c - origin).dot(&axis);
            Some(DerivedFrame {
                frame: ConnectorFrame {
                    origin: [foot.x, foot.y, foot.z],
                    z_axis: [axis.x, axis.y, axis.z],
                    x_axis: [ref_dir.x, ref_dir.y, ref_dir.z],
                },
                radius: Some(radius),
            })
        }
        Kind::Sph {
            center,
            north,
            ref_dir,
            radius,
        } => Some(DerivedFrame {
            frame: ConnectorFrame {
                origin: [center.x, center.y, center.z],
                z_axis: [north.x, north.y, north.z],
                x_axis: [ref_dir.x, ref_dir.y, ref_dir.z],
            },
            radius: Some(radius),
        }),
    }
}

/// Capture a face's geometric-identity fingerprint (position + normal +
/// radius + size) — the DEGRADED anchor used when the face has no PID yet
/// (fillet/chamfer/pattern faces, #11 slices 40-E/F pending).
pub fn fingerprint_for_face(model: &mut BRepModel, face: FaceId) -> Option<ConnectorAnchor> {
    let position = face_centroid(model, face)?;
    let size = face_area(model, face);
    let surface_id = model.faces.get(face)?.surface_id;
    let (normal, radius) = {
        let surf = model.surfaces.get(surface_id)?;
        if let Some(p) = surf.as_any().downcast_ref::<Plane>() {
            (p.normal.normalize().ok().map(|n| [n.x, n.y, n.z]), None)
        } else if let Some(c) = surf.as_any().downcast_ref::<Cylinder>() {
            (None, Some(c.radius))
        } else if let Some(s) = surf.as_any().downcast_ref::<Sphere>() {
            (None, Some(s.radius))
        } else {
            (None, None)
        }
    };
    Some(ConnectorAnchor::Fingerprint {
        position: [position.x, position.y, position.z],
        normal,
        radius,
        size,
    })
}

/// Best-effort re-resolution of a fingerprint anchor: the live face whose
/// identity matches within `pos_tol` (position) and 1% relative
/// radius/size. `None` when nothing matches (STALE) or the match is
/// AMBIGUOUS (two candidates — refusing beats guessing).
pub fn resolve_face_by_fingerprint(
    model: &mut BRepModel,
    position: [f64; 3],
    radius: Option<f64>,
    size: Option<f64>,
    pos_tol: f64,
) -> Option<FaceId> {
    let target = Point3::new(position[0], position[1], position[2]);
    let ids: Vec<FaceId> = model.faces.iter().map(|(id, _)| id).collect();
    let mut matched: Vec<FaceId> = Vec::new();
    for fid in ids {
        let Some(c) = face_centroid(model, fid) else {
            continue;
        };
        if (c - target).magnitude() > pos_tol {
            continue;
        }
        if let Some(r) = radius {
            let live_r = {
                let sid = match model.faces.get(fid) {
                    Some(f) => f.surface_id,
                    None => continue,
                };
                match model.surfaces.get(sid) {
                    Some(surf) => {
                        if let Some(cy) = surf.as_any().downcast_ref::<Cylinder>() {
                            Some(cy.radius)
                        } else {
                            surf.as_any().downcast_ref::<Sphere>().map(|sp| sp.radius)
                        }
                    }
                    None => None,
                }
            };
            match live_r {
                Some(lr) if (lr - r).abs() <= 0.01 * r.abs().max(1e-9) => {}
                _ => continue,
            }
        }
        if let Some(a) = size {
            match face_area(model, fid) {
                Some(la) if (la - a).abs() <= 0.01 * a.abs().max(1e-9) => {}
                _ => continue,
            }
        }
        matched.push(fid);
        if matched.len() > 1 {
            return None; // ambiguous — refuse, never guess
        }
    }
    matched.first().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_provenance_matches_variant() {
        assert_eq!(
            ConnectorAnchor::FacePid { pid: 7 }.provenance(),
            AnchorProvenance::Pid
        );
        assert_eq!(
            ConnectorAnchor::Label {
                name: "bore".into()
            }
            .provenance(),
            AnchorProvenance::Label
        );
        assert_eq!(
            ConnectorAnchor::Fingerprint {
                position: [0.0; 3],
                normal: None,
                radius: Some(3.0),
                size: None
            }
            .provenance(),
            AnchorProvenance::Fingerprint
        );
        assert_eq!(
            ConnectorAnchor::RawFrame.provenance(),
            AnchorProvenance::Raw
        );
    }

    #[test]
    fn doc_mate_serde_round_trips_with_defaults() {
        // Additive-serde contract: `couples`/`at` default empty, so mates
        // recorded without them keep replaying.
        let raw = serde_json::json!({
            "id": Uuid::nil(),
            "kind": { "Revolute": { "limits": null } },
            "a": Uuid::nil(),
            "b": Uuid::nil(),
        });
        let mate: DocMate = match serde_json::from_value(raw) {
            Ok(m) => m,
            Err(e) => {
                assert!(false, "mate without couples/at must parse: {e}");
                return;
            }
        };
        assert!(mate.couples.is_empty());
        assert!(mate.at.is_empty());
        let json = serde_json::to_string(&mate).unwrap_or_default();
        let back: Result<DocMate, _> = serde_json::from_str(&json);
        assert_eq!(back.ok(), Some(mate));
    }
}
