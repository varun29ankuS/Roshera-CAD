//! Per-event certificate — the structured proof carried on a timeline event.
//!
//! Move 02 of the certified per-part timeline: turn the "cannot lie" property
//! into a fact stored on every history node. See
//! `Roshera-vault/.../2026-07-18-certified-timeline-design.md` §5.
//!
//! # The honesty contract (design §6), enforced by construction
//!
//! An [`EventCertificate`] is **per-event-type-honest**: a field is populated
//! only where it *exists* for that op class. The public constructors are the
//! only way to build one, and each hard-codes `None` for every field that does
//! not belong to its class — so it is structurally impossible to put a DOF on a
//! boolean or an Euler characteristic on a sketch:
//!
//! * [`EventCertificate::from_solid_certificate`] — box/cyl/sphere/boolean/
//!   fillet/chamfer/shell/extrude/revolve: `is_sound`, `euler_characteristic`,
//!   `volume`, `face_count`, `checks`. Never `dof`/`constrainedness`/`conflict`/
//!   `mates_satisfied`.
//! * [`EventCertificate::skipped_solid`] — a `fast: true` op that computed no
//!   certificate: `skipped = true`, `is_sound = None`, no `checks`. Volume and
//!   face_count are cheap and true, so they are kept (design §8).
//! * [`EventCertificate::for_sketch`] — `dof`, `constrainedness`, `conflict`.
//!   Never `euler_characteristic`/`volume`/`face_count`/`checks`.
//! * [`EventCertificate::for_assembly`] — `mates_satisfied`, `dof`, `conflict`.
//!
//! Rule 2 of the contract — **`is_sound` is the FULL
//! [`ValidityCertificate::is_sound`], never a subset of cheap checks** — is met
//! by delegating straight to that method; the projection never re-derives its
//! own verdict.

use serde::{Deserialize, Serialize};

use geometry_engine::primitives::provenance::ValidityCertificate;

use crate::types::EventMetadata;

/// The `EventMetadata::properties` key under which a serialized
/// [`EventCertificate`] is carried. Being part of the serialized event, a cert
/// stored here survives persistence and pure-replay boot unchanged.
pub const EVENT_CERTIFICATE_KEY: &str = "certificate";

/// The per-check breakdown behind a solid op's `is_sound` — the honest AND's
/// inputs, so a card can show WHAT was verified, not just the verdict. Mirrors
/// the soundness-relevant booleans of [`ValidityCertificate`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SolidCertChecks {
    /// `validate_solid_scoped` Standard topology verdict.
    pub brep_valid: bool,
    /// Mesh closes (no boundary edges) at the certification chord.
    pub watertight: bool,
    /// Every edge bordered by exactly two faces.
    pub manifold: bool,
    /// Consistently wound, correctly-oriented closed surface.
    pub oriented: bool,
    /// No two non-adjacent faces cross.
    pub self_intersection_free: bool,
}

/// A sketch/assembly op's constrainedness verdict — the three honest states.
/// Solid ops never carry this.
// Reason: `…Constrained` is the domain-standard vocabulary (it mirrors the
// kernel's `SketchConstrainedness`); renaming to satisfy the postfix lint
// would obscure the CAD meaning.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum Constrainedness {
    /// Rigidly constructed from the datum — zero residual freedom.
    WellConstrained,
    /// `free_dofs` structural degrees of freedom remain.
    UnderConstrained {
        /// Remaining free DOFs (> 0).
        free_dofs: u32,
    },
    /// `redundant` constraints exceed what the geometry needs; a subset may
    /// conflict (see [`EventCertificate::conflict`]).
    OverConstrained {
        /// Count of redundant constraints.
        redundant: u32,
    },
}

/// A minimal conflict witness — the ids of the constraints/mates in the
/// smallest set that cannot be satisfied together (a QuickXplain result).
/// Present only on an over-constrained-and-inconsistent sketch/assembly op.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictWitness {
    /// Ids in the minimal conflicting set. Empty is not a valid witness — omit
    /// the whole `conflict` field instead of carrying an empty one.
    pub conflicting: Vec<String>,
}

/// A projection of the kernel's proof for the geometry an event produced —
/// stored on the event, per-event-type-honest. Build one only through the
/// class constructors; every field that does not exist for the op class is
/// `None` (see the module docs for the honesty contract).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventCertificate {
    /// Present for solid-producing ops: the honest AND verdict
    /// ([`ValidityCertificate::is_sound`]). `None` on a skipped/`fast` op and
    /// on sketch/assembly ops (which carry `constrainedness` instead).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_sound: Option<bool>,
    /// Solid ops only: tessellated-mesh Euler characteristic (V − E + F).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub euler_characteristic: Option<i64>,
    /// Solid ops (incl. skipped): signed volume in model units³.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<f64>,
    /// Solid ops (incl. skipped): outer-shell face count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face_count: Option<usize>,
    /// Solid ops only: the per-check breakdown behind `is_sound`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks: Option<SolidCertChecks>,
    /// Sketch/assembly ops only: free degrees of freedom.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dof: Option<u32>,
    /// Sketch/assembly ops only: the constrainedness verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constrainedness: Option<Constrainedness>,
    /// Sketch/assembly ops only: minimal conflict witness, when inconsistent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict: Option<ConflictWitness>,
    /// Assembly ops only: `(satisfied, total)` mate counts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mates_satisfied: Option<(u32, u32)>,
    /// `true` when the op ran on the `fast: true` path and computed no
    /// certificate. An honest "not certified", never a fabricated green.
    pub skipped: bool,
}

impl EventCertificate {
    /// The all-`None` base every class constructor starts from. Private so the
    /// only reachable certificates are the honest per-class projections.
    fn empty() -> Self {
        Self {
            is_sound: None,
            euler_characteristic: None,
            volume: None,
            face_count: None,
            checks: None,
            dof: None,
            constrainedness: None,
            conflict: None,
            mates_satisfied: None,
            skipped: false,
        }
    }

    /// Project a solid op's proof from the kernel [`ValidityCertificate`] the
    /// op produced, plus the cheap structural facts the handler already
    /// computed alongside it (`volume`, `face_count` — `None` when unavailable).
    ///
    /// `is_sound` is [`ValidityCertificate::is_sound`] verbatim — the full AND,
    /// honesty-contract rule 2. No sketch/assembly field is ever set here.
    pub fn from_solid_certificate(
        cert: &ValidityCertificate,
        volume: Option<f64>,
        face_count: Option<usize>,
    ) -> Self {
        Self {
            is_sound: Some(cert.is_sound()),
            euler_characteristic: Some(cert.euler_characteristic),
            volume,
            face_count,
            checks: Some(SolidCertChecks {
                brep_valid: cert.brep_valid,
                watertight: cert.watertight,
                manifold: cert.manifold,
                oriented: cert.oriented,
                self_intersection_free: cert.self_intersection_free,
            }),
            ..Self::empty()
        }
    }

    /// A solid op that ran on the `fast: true` path: no certificate was
    /// computed, so `skipped = true` and `is_sound`/`checks` stay `None`.
    /// Volume and face_count are cheap and true, so they are retained (§8).
    pub fn skipped_solid(volume: Option<f64>, face_count: Option<usize>) -> Self {
        Self {
            volume,
            face_count,
            skipped: true,
            ..Self::empty()
        }
    }

    /// Project a sketch op's proof. Carries `dof`/`constrainedness`/`conflict`
    /// only — never Euler/volume/face_count/checks (those do not exist for a
    /// sketch).
    pub fn for_sketch(
        dof: u32,
        constrainedness: Constrainedness,
        conflict: Option<ConflictWitness>,
    ) -> Self {
        Self {
            dof: Some(dof),
            constrainedness: Some(constrainedness),
            conflict,
            ..Self::empty()
        }
    }

    /// Project an assembly op's proof: `(satisfied, total)` mates plus residual
    /// `dof` and an optional conflict witness. No solid fields.
    pub fn for_assembly(
        mates_satisfied: (u32, u32),
        dof: Option<u32>,
        conflict: Option<ConflictWitness>,
    ) -> Self {
        Self {
            mates_satisfied: Some(mates_satisfied),
            dof,
            conflict,
            ..Self::empty()
        }
    }

    /// Store this certificate on an event's metadata under
    /// [`EVENT_CERTIFICATE_KEY`]. Because `EventMetadata` is part of the
    /// serialized `TimelineEvent`, the cert is then durable and replay-stable.
    ///
    /// Returns the serialization error rather than panicking — the caller
    /// decides how to react (the kernel contract never lets a recording failure
    /// break the geometry op).
    pub fn store_in(&self, metadata: &mut EventMetadata) -> Result<(), serde_json::Error> {
        let value = serde_json::to_value(self)?;
        metadata
            .properties
            .insert(EVENT_CERTIFICATE_KEY.to_string(), value);
        Ok(())
    }

    /// Read the certificate stored on an event's metadata, if any. An absent or
    /// malformed entry reads back as `None` — never a fabricated certificate,
    /// so an uncertified event can never masquerade as a certified one.
    pub fn from_metadata(metadata: &EventMetadata) -> Option<Self> {
        let value = metadata.properties.get(EVENT_CERTIFICATE_KEY)?;
        serde_json::from_value(value.clone()).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::EventMetadata;
    use geometry_engine::primitives::provenance::{
        ConstructionConsistency, EyesConsistency, LabelsConsistency, MeshQuality,
        TessellationQuality, ValidityCertificate,
    };
    use geometry_engine::primitives::topology_builder::{GeometryId, TopologyBuilder};

    /// A hand-built fully-sound certificate. The kernel's own
    /// `fully_sound_for_test` is `#[cfg(test)]`-gated to geometry-engine, so we
    /// reconstruct the same all-sound shape here to exercise the projection
    /// against a cert whose `brep_valid` and `is_sound()` can be made to diverge.
    fn sound_cert() -> ValidityCertificate {
        ValidityCertificate {
            brep_valid: true,
            watertight: true,
            manifold: true,
            euler_characteristic: 2,
            boundary_edges: 0,
            nonmanifold_edges: 0,
            oriented: true,
            inconsistent_directed_edges: 0,
            self_intersection_free: true,
            construction_consistent: ConstructionConsistency::NotApplicable,
            labels_consistent: LabelsConsistency::NotApplicable,
            eyes_consistent: EyesConsistency::Consistent,
            tessellation: TessellationQuality::empty(),
            mesh_quality: MeshQuality::empty(),
            errors: vec![],
            model_debris_orphan_faces: 0,
        }
    }

    fn box_solid_certificate() -> (ValidityCertificate, Option<f64>, Option<usize>) {
        let mut model = geometry_engine::primitives::topology_builder::BRepModel::new();
        let gid = TopologyBuilder::new(&mut model)
            .create_box_3d(10.0, 10.0, 10.0)
            .expect("create_box_3d");
        let solid_id = match gid {
            GeometryId::Solid(id) => id,
            other => panic!("expected a solid, got {other:?}"),
        };
        let cert = model.certify_solid(solid_id);
        let volume = model.calculate_solid_volume(solid_id);
        let face_count = model.solid_outer_face_count(solid_id);
        (cert, volume, face_count)
    }

    #[test]
    fn solid_projection_matches_kernel_certificate() {
        let (cert, volume, face_count) = box_solid_certificate();
        let ec = EventCertificate::from_solid_certificate(&cert, volume, face_count);

        assert_eq!(ec.is_sound, Some(cert.is_sound()));
        assert_eq!(ec.euler_characteristic, Some(cert.euler_characteristic));
        assert_eq!(ec.volume, volume);
        assert_eq!(ec.face_count, face_count);
        let checks = ec.checks.expect("solid cert carries checks");
        assert_eq!(checks.brep_valid, cert.brep_valid);
        assert_eq!(checks.watertight, cert.watertight);
        assert_eq!(checks.manifold, cert.manifold);
        assert_eq!(checks.oriented, cert.oriented);
        assert_eq!(checks.self_intersection_free, cert.self_intersection_free);
        assert!(!ec.skipped);
    }

    #[test]
    fn is_sound_is_the_full_and_not_a_cheap_subset() {
        // Honesty contract rule 2: `is_sound` is the full `ValidityCertificate::
        // is_sound()`, never a subset of cheap checks. A cert that is `brep_valid`
        // yet NOT watertight is UNSOUND; a projection that read `brep_valid` into
        // `is_sound` would report `Some(true)` here and fail.
        let mut cert = sound_cert();
        cert.watertight = false;
        assert!(cert.brep_valid, "guard: brep_valid stays true");
        assert!(!cert.is_sound(), "guard: the cert is unsound");

        let ec = EventCertificate::from_solid_certificate(&cert, Some(1.0), Some(6));
        assert_eq!(
            ec.is_sound,
            Some(false),
            "is_sound must be the full AND, not brep_valid"
        );
        assert_eq!(ec.checks.map(|c| c.watertight), Some(false));
    }

    #[test]
    fn solid_cert_never_carries_sketch_or_assembly_fields() {
        // Honesty contract rule 1: DOF/constrainedness/conflict/mates do not
        // exist for a solid op and must be `None`.
        let (cert, volume, face_count) = box_solid_certificate();
        let ec = EventCertificate::from_solid_certificate(&cert, volume, face_count);
        assert_eq!(ec.dof, None);
        assert_eq!(ec.constrainedness, None);
        assert_eq!(ec.conflict, None);
        assert_eq!(ec.mates_satisfied, None);
    }

    #[test]
    fn sketch_cert_carries_dof_and_never_solid_fields() {
        // A sketch op has DOF/constrainedness but NO Euler/volume/face_count/
        // checks — those do not exist for a sketch (rule 1).
        let ec = EventCertificate::for_sketch(
            3,
            Constrainedness::UnderConstrained { free_dofs: 3 },
            None,
        );
        assert_eq!(ec.dof, Some(3));
        assert_eq!(
            ec.constrainedness,
            Some(Constrainedness::UnderConstrained { free_dofs: 3 })
        );
        assert_eq!(ec.euler_characteristic, None);
        assert_eq!(ec.volume, None);
        assert_eq!(ec.face_count, None);
        assert!(ec.checks.is_none());
        assert_eq!(ec.is_sound, None);
        assert!(!ec.skipped);
    }

    #[test]
    fn skipped_solid_is_honest_no_fabricated_verdict() {
        // Honesty contract rule 3: a `fast: true` op computed no certificate.
        // `skipped == true`, `is_sound == None`, no checks — never a fabricated
        // green. Volume/face_count are cheap + true, so they are kept (§8).
        let ec = EventCertificate::skipped_solid(Some(1000.0), Some(6));
        assert!(ec.skipped);
        assert_eq!(ec.is_sound, None);
        assert!(ec.checks.is_none());
        assert_eq!(ec.euler_characteristic, None);
        assert_eq!(ec.volume, Some(1000.0));
        assert_eq!(ec.face_count, Some(6));
        assert_eq!(ec.dof, None);
    }

    #[test]
    fn assembly_cert_carries_mates_and_dof_no_solid_fields() {
        let ec = EventCertificate::for_assembly((4, 5), Some(2), None);
        assert_eq!(ec.mates_satisfied, Some((4, 5)));
        assert_eq!(ec.dof, Some(2));
        assert_eq!(ec.euler_characteristic, None);
        assert_eq!(ec.volume, None);
        assert!(ec.checks.is_none());
        assert_eq!(ec.is_sound, None);
    }

    #[test]
    fn round_trips_through_event_metadata() {
        // The carrier is `EventMetadata.properties["certificate"]`, which is part
        // of the serialized event — so a stored cert survives persistence/replay.
        let (cert, volume, face_count) = box_solid_certificate();
        let ec = EventCertificate::from_solid_certificate(&cert, volume, face_count);

        let mut meta = EventMetadata::default();
        assert_eq!(
            EventCertificate::from_metadata(&meta),
            None,
            "absent certificate must read back as None, never a fabricated one"
        );

        ec.store_in(&mut meta).expect("store certificate");
        let read_back = EventCertificate::from_metadata(&meta).expect("stored cert reads back");
        assert_eq!(read_back, ec);
    }

    #[test]
    fn serde_never_emits_a_field_that_does_not_exist_for_the_op_class() {
        // A sketch cert serialized to JSON must not carry `euler_characteristic`,
        // `volume`, `face_count`, or `checks` keys at all — absent, not `null`.
        let ec = EventCertificate::for_sketch(0, Constrainedness::WellConstrained, None);
        let v = serde_json::to_value(&ec).expect("serialize");
        let obj = v.as_object().expect("cert is an object");
        assert!(!obj.contains_key("euler_characteristic"));
        assert!(!obj.contains_key("volume"));
        assert!(!obj.contains_key("face_count"));
        assert!(!obj.contains_key("checks"));
        assert!(!obj.contains_key("mates_satisfied"));
        assert!(obj.contains_key("dof"));
    }
}
