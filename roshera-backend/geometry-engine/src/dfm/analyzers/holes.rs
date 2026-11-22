//! `hole_census` — every hole of a solid, as the drilling sequence that
//! makes it, for the hole rules of every process pack.
//!
//! ## Reuse, not a second recogniser
//!
//! Recognised holes are [`crate::drawing::hole_features::recognize_hole_stacks`]
//! verbatim — the recogniser the certified drawing hole callouts use: THRU
//! decided by TOPOLOGY (the drilled wall opens onto an exit plane), a flat
//! floor vs a drill-point cone, counterbore / countersink entries. A DFM
//! hole rule and the drawing's `Ø6.6 THRU ⌴ Ø11 ↧ 6.8` therefore can never
//! disagree about what the hole is.
//!
//! The one number the stack does not carry for a THRU hole is its drilled
//! LENGTH; it is the union of the drilled wall faces' trim-edge station
//! ranges in the same adjacency graph (`AagNode::station_range`), exact.
//!
//! ## What is NOT a recognised hole is still a hole
//!
//! Every full-turn cavity cylinder ([`super::features::CavityCylinder::is_full_turn`])
//! not claimed by a stack — a cross-drilled bore, a hole breaking out into a
//! pocket, a chamfered exit — is an [`UnrecognisedBore`]: its diameter is
//! exact, but its wall may be SHORTER than the hole (a cross hole splits it)
//! and its entry side and floor are unknown. Rules use it only where that
//! is enough to decide (a diameter rule; a depth ratio whose LOWER bound
//! already exceeds the maximum) and refuse otherwise. A cavity cylinder
//! with an unknown sweep is reported here too — it may be a hole. A
//! PARTIAL-turn cavity cylinder (a pocket corner radius, a slot end) is never
//! a hole: that is the gate that keeps a corner fillet out of the hole rules.

use std::collections::BTreeSet;

use crate::dfm::analyzers::features::ConcaveFeatures;
use crate::drawing::hole_features::{recognize_hole_stacks, HoleStack};
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;

/// One recognised hole.
#[derive(Debug, Clone, PartialEq)]
pub struct HoleRecord {
    pub stack: HoleStack,
    /// Drilled (full-diameter) length: the blind depth from the entry, or
    /// the drilled wall's length for a THRU hole. Exact.
    pub drilled_length: f64,
    /// Every face of the stack, ascending.
    pub faces: Vec<FaceId>,
}

impl HoleRecord {
    /// Directions a tool can approach from (unit, pointing from the part
    /// toward the spindle): the entry side, and the exit side of a THRU
    /// hole without an entry feature.
    pub fn approach_directions(&self) -> Vec<[f64; 3]> {
        let a = self.stack.axis;
        let entry = [-a[0], -a[1], -a[2]];
        if self.stack.through
            && self.stack.counterbore.is_none()
            && self.stack.countersink.is_none()
        {
            vec![entry, a]
        } else {
            vec![entry]
        }
    }
}

/// A full-turn (or unknown-sweep) bore no recognised stack claims.
#[derive(Debug, Clone, PartialEq)]
pub struct UnrecognisedBore {
    pub faces: Vec<FaceId>,
    pub diameter: f64,
    pub origin: [f64; 3],
    pub axis: [f64; 3],
    /// Length of the bore's own wall — a LOWER bound on the hole's length.
    pub wall_length: f64,
    /// The sweep was proven full (false: no rim to read it from).
    pub full_turn: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct HoleCensus {
    pub holes: Vec<HoleRecord>,
    pub unrecognised: Vec<UnrecognisedBore>,
}

/// Census of `solid_id`'s holes, reading the same adjacency graph
/// `features` was built from.
pub fn hole_census(model: &BRepModel, solid_id: SolidId, features: &ConcaveFeatures) -> HoleCensus {
    let stacks = recognize_hole_stacks(model, solid_id);
    let mut claimed: BTreeSet<FaceId> = BTreeSet::new();
    let mut holes = Vec::with_capacity(stacks.len());
    for stack in stacks {
        let faces = stack.faces();
        claimed.extend(faces.iter().copied());
        let drilled_length = match stack.depth {
            Some(d) => d,
            None => {
                let mut lo = f64::INFINITY;
                let mut hi = f64::NEG_INFINITY;
                for &f in &stack.drill_faces {
                    if let Some(r) = features.graph.node(f).and_then(|n| n.station_range) {
                        lo = lo.min(r[0]);
                        hi = hi.max(r[1]);
                    }
                }
                if hi >= lo {
                    hi - lo
                } else {
                    // The recogniser only claims walls with station ranges;
                    // an absent one leaves the length unknown, never zero.
                    f64::NAN
                }
            }
        };
        holes.push(HoleRecord {
            stack,
            drilled_length,
            faces,
        });
    }

    let mut unrecognised = Vec::new();
    for c in &features.cavity_cylinders {
        if c.is_partial() || c.faces.iter().any(|f| claimed.contains(f)) {
            continue;
        }
        unrecognised.push(UnrecognisedBore {
            faces: c.faces.clone(),
            diameter: 2.0 * c.radius,
            origin: c.origin,
            axis: c.axis,
            wall_length: c.length(),
            full_turn: c.is_full_turn(),
        });
    }
    HoleCensus {
        holes,
        unrecognised,
    }
}
