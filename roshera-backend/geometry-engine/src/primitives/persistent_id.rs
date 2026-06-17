//! Persistent topological identifiers (#11, slice 40-A — types only).
//!
//! The transient `VertexId / EdgeId / FaceId` are dense `u32` indices into the
//! stores: stable within one `BRepModel` instance, but reallocated on every
//! operation that synthesizes topology and re-minted from scratch on timeline
//! replay. An agent that said "fillet edge 17" cannot find edge 17 again after a
//! parameter edit. A [`PersistentId`] is the durable name that DOES survive:
//! derived deterministically from operation lineage (Kripac's persistent-naming
//! scheme), so replaying the same timeline — even with edited parameters —
//! re-derives the same PID for topology whose ROLE is unchanged.
//!
//! This slice adds the types + a deterministic derivation. Call-site wiring
//! (primitive roots, extrude/boolean/fillet lineage, recorder + replay) lands in
//! the following slices (40-B…40-H), each independently shippable and tested.
//!
//! Determinism: PIDs are computed with UUIDv5 (name-based SHA-1) under a fixed
//! Roshera namespace. UUIDv5 is endian-stable and reproducible across runs and
//! machines — the property replay-determinism requires — and `uuid` is already a
//! workspace dependency, so no new hash crate is pulled in.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Fixed namespace for all Roshera persistent-id derivation. Generated once
/// (UUIDv4) and frozen — changing it would invalidate every PID ever minted.
const PID_NAMESPACE: Uuid = Uuid::from_u128(0x9d6e_3b1c_4a27_4f88_b0a5_1e7c_2d34_56f9);

/// A 128-bit persistent identifier for a topological entity, derived from
/// operation lineage and stable across regeneration + parameter edits.
#[derive(Copy, Clone, Eq, Hash, PartialEq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PersistentId(pub u128);

impl PersistentId {
    /// Mint a ROOT PID with no parent topology — primitive creation, where the
    /// only identity is the operation's own seed (e.g. timeline event id +
    /// parameters). Same seed → same PID; different seed → different PID.
    pub fn root(seed: &[u8]) -> Self {
        PersistentId(Uuid::new_v5(&PID_NAMESPACE, seed).as_u128())
    }

    /// Derive a PID from parent PIDs + an operation tag + a [`Role`]. The role
    /// canonically encodes "which output of this operation is this", so two runs
    /// of the same operation on the same parents derive identical PIDs. Parent
    /// ORDER is significant (it is part of the role semantics), so callers pass
    /// parents in a fixed, role-defined order.
    pub fn derive(parents: &[PersistentId], op_tag: &str, role: &Role) -> Self {
        let mut buf: Vec<u8> = Vec::with_capacity(parents.len() * 16 + op_tag.len() + 32);
        // Length-prefixed sections so distinct (parents, tag, role) splits can
        // never alias to the same byte string.
        buf.extend_from_slice(&(parents.len() as u32).to_le_bytes());
        for p in parents {
            buf.extend_from_slice(&p.0.to_le_bytes());
        }
        buf.extend_from_slice(&(op_tag.len() as u32).to_le_bytes());
        buf.extend_from_slice(op_tag.as_bytes());
        let role_bytes = role.canonical_bytes();
        buf.extend_from_slice(&(role_bytes.len() as u32).to_le_bytes());
        buf.extend_from_slice(&role_bytes);
        PersistentId(Uuid::new_v5(&PID_NAMESPACE, &buf).as_u128())
    }

    /// The raw 128-bit value (for wire formats / inverse maps).
    #[inline]
    pub fn as_u128(self) -> u128 {
        self.0
    }
}

impl std::fmt::Debug for PersistentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Short hex prefix — enough to disambiguate in logs without flooding.
        write!(f, "PID({:08x})", (self.0 >> 96) as u32)
    }
}

/// Which primitive a [`Role::Root`] names.
#[derive(Clone, Copy, Eq, Hash, PartialEq, Debug, Serialize, Deserialize)]
pub enum PrimitiveKind {
    Box,
    Sphere,
    Cylinder,
    Cone,
    Torus,
    Plane,
    Other,
}

/// Stable, serializable description of "which output is this" for every
/// operation that synthesizes topology. Hashed into a [`PersistentId`] via
/// [`PersistentId::derive`]. Variants follow the Kripac scheme: each carries the
/// parent PID(s) it derives from plus a discriminator for its position in the
/// operation's output set. New operations add variants here; the `Generic`
/// escape hatch lets a not-yet-specialized op participate without blocking.
#[derive(Clone, Eq, Hash, PartialEq, Debug, Serialize, Deserialize)]
pub enum Role {
    // --- Primitive root (no parents) ---
    /// `key` distinguishes primitives with identical parameters in one timeline
    /// (e.g. the event id), so two same-size boxes get different roots.
    Root {
        kind: PrimitiveKind,
        key: String,
    },

    // --- Extrude ---
    ExtrudeSide {
        base_edge_pid: PersistentId,
    },
    ExtrudeCapStart,
    ExtrudeCapEnd,
    ExtrudeSideEdgeStart {
        base_vertex_pid: PersistentId,
    },
    ExtrudeSideEdgeEnd {
        base_vertex_pid: PersistentId,
    },

    // --- Revolve (analytic bands, #19) ---
    RevolveBand {
        base_edge_pid: PersistentId,
    },
    RevolveSeam {
        base_vertex_pid: PersistentId,
    },
    RevolveRing {
        base_vertex_pid: PersistentId,
    },

    // --- Boolean ---
    BooleanFromA {
        source_face_pid: PersistentId,
    },
    BooleanFromB {
        source_face_pid: PersistentId,
    },
    BooleanCutEdge {
        face_a_pid: PersistentId,
        face_b_pid: PersistentId,
    },

    // --- Fillet / chamfer ---
    FilletRoll {
        source_edge_pid: PersistentId,
    },
    FilletSeamA {
        source_edge_pid: PersistentId,
        on_face: PersistentId,
    },
    FilletSeamB {
        source_edge_pid: PersistentId,
        on_face: PersistentId,
    },
    ChamferBevel {
        source_edge_pid: PersistentId,
    },

    // --- Pattern / mirror ---
    PatternInstance {
        source_pid: PersistentId,
        index: u32,
    },
    Mirrored {
        source_pid: PersistentId,
        axis_key: String,
    },

    /// Escape hatch for an operation output not yet given a specialized role.
    /// `label` must be stable for the same logical output across runs.
    Generic {
        source_pid: PersistentId,
        label: String,
    },
}

impl Role {
    /// Deterministic byte encoding for hashing. JSON of a fixed-shape enum (no
    /// maps, fields in declaration order) is reproducible run-to-run, so it is a
    /// sound canonical form without pulling in a binary-serialization crate.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        // `to_vec` on a value of this type is infallible in practice (no
        // non-string map keys, no floats-as-keys); fall back to a tagged empty
        // encoding rather than panic, preserving the deny-panic policy.
        serde_json::to_vec(self).unwrap_or_else(|_| b"role:unencodable".to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_deterministic_and_distinct() {
        assert_eq!(PersistentId::root(b"box#1"), PersistentId::root(b"box#1"));
        assert_ne!(PersistentId::root(b"box#1"), PersistentId::root(b"box#2"));
        // Non-zero (a real hash, not a default).
        assert_ne!(PersistentId::root(b"box#1").as_u128(), 0);
    }

    #[test]
    fn derive_is_deterministic() {
        let p = PersistentId::root(b"parent");
        let role = Role::ExtrudeSide { base_edge_pid: p };
        let a = PersistentId::derive(&[p], "extrude_face", &role);
        let b = PersistentId::derive(&[p], "extrude_face", &role);
        assert_eq!(a, b, "same lineage → same PID");
    }

    #[test]
    fn derive_varies_with_each_input() {
        let p = PersistentId::root(b"parent");
        let q = PersistentId::root(b"other");
        let role = Role::ExtrudeCapStart;
        let base = PersistentId::derive(&[p], "extrude_face", &role);
        // Different parents.
        assert_ne!(base, PersistentId::derive(&[q], "extrude_face", &role));
        // Different op tag.
        assert_ne!(base, PersistentId::derive(&[p], "revolve_face", &role));
        // Different role.
        assert_ne!(
            base,
            PersistentId::derive(&[p], "extrude_face", &Role::ExtrudeCapEnd)
        );
        // Parent order matters.
        assert_ne!(
            PersistentId::derive(&[p, q], "boolean", &Role::ExtrudeCapStart),
            PersistentId::derive(&[q, p], "boolean", &Role::ExtrudeCapStart)
        );
    }

    #[test]
    fn length_prefixing_prevents_aliasing() {
        // Two different (tag, role-label) splits that would concatenate to the
        // same bytes without length prefixes must produce different PIDs.
        let p = PersistentId::root(b"x");
        let a = PersistentId::derive(
            &[p],
            "ab",
            &Role::Generic {
                source_pid: p,
                label: "c".into(),
            },
        );
        let b = PersistentId::derive(
            &[p],
            "a",
            &Role::Generic {
                source_pid: p,
                label: "bc".into(),
            },
        );
        assert_ne!(a, b);
    }

    #[test]
    fn role_serialization_round_trips() {
        let p = PersistentId::root(b"seed");
        let roles = vec![
            Role::Root {
                kind: PrimitiveKind::Cylinder,
                key: "evt-7".into(),
            },
            Role::BooleanFromA { source_face_pid: p },
            Role::FilletSeamA {
                source_edge_pid: p,
                on_face: p,
            },
            Role::PatternInstance {
                source_pid: p,
                index: 3,
            },
        ];
        for r in roles {
            let json = serde_json::to_string(&r).expect("serialize");
            let back: Role = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(r, back);
            // canonical_bytes stable across calls.
            assert_eq!(r.canonical_bytes(), back.canonical_bytes());
        }
    }

    #[test]
    fn composability_derives_stack() {
        // "top edge of fillet of cap of extrude" resolves to one stable PID.
        let root = PersistentId::root(b"box");
        let cap = PersistentId::derive(&[root], "extrude_face", &Role::ExtrudeCapEnd);
        let roll = PersistentId::derive(
            &[cap],
            "fillet_edges",
            &Role::FilletRoll {
                source_edge_pid: cap,
            },
        );
        let again = {
            let cap2 = PersistentId::derive(&[root], "extrude_face", &Role::ExtrudeCapEnd);
            PersistentId::derive(
                &[cap2],
                "fillet_edges",
                &Role::FilletRoll {
                    source_edge_pid: cap2,
                },
            )
        };
        assert_eq!(roll, again, "derived-from-derived is stable");
    }
}
