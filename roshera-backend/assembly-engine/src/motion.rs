//! Kinematic drag — driving a joint parameter and re-solving the mates
//! around it (kinematic-assembly campaign, Slice 5; spec §3.4 "Driven vs
//! driving" + §3.6).
//!
//! This is the agent's kinematic hand. `assembly.drag(mate, param, value)`
//! sets a DRIVEN joint parameter and re-solves — and everything about how
//! it does that is chosen so the answer cannot quietly be wrong:
//!
//! * **A drive is a residual, not a mode.** The driven joint contributes
//!   one more row, `param(q) − target = 0` (`jacobian::DriveRow`), to the
//!   SAME Gauss-Newton core every solve runs. So a drag inherits the
//!   solver's honesty contract verbatim: `converged` is RE-MEASURED at the
//!   achieved pose, never asserted, and an unreachable drive reports
//!   itself instead of leaving a half-finished stroke behind.
//! * **Scoped to the affected component.** The re-solve runs over the
//!   driven mate's connected component from the Slice-3 planner's own
//!   partition (`decompose::Decomposition`) — not a parallel notion of
//!   adjacency that could drift out of step with it. Everything outside
//!   that component keeps its poses BYTE-identical, including its
//!   pre-existing violations: a drag repairs nothing it was not asked to
//!   move (the sketch drag-scoping precedent, #45 Slice 3).
//! * **Stepped, not teleported.** A drive walks to its target in bounded
//!   increments ([`MAX_DRIVE_STEP`]). This is what makes multi-turn
//!   winding decidable (below) and what lets the stroke SEE the
//!   configurations it passes through rather than only its endpoints.
//! * **Rank changes are reported.** Numeric DOF is a property of the
//!   CONFIGURATION, not the schematic: a four-bar stretched collinear has
//!   instantaneous mobility 2 where the linkage diagram says 1 (slice-3/4
//!   report, premise #1). A stroke through such a pose must not silently
//!   corrupt — so every step re-measures the scope's mobility and any
//!   change is stamped into [`DragOutcome::rank_transitions`].
//! * **Limits clamp, they don't error.** A beyond-limit target is clamped
//!   and the at-limit fact reported ([`LimitFact`]) — the agent learns the
//!   joint bottomed out, which is information, not a failure.
//!
//! # Multi-turn winding (premise #5)
//!
//! θ is read through `atan2`, so a pose can only ever report it WRAPPED to
//! (−π, π]. The winding is a property of the PATH, and the drag is the one
//! thing that walks the path — so it records the turns on the assembly
//! (`Assembly::windings`) and the coupling residuals read the unwrapped
//! angle (`Assembly::joint_parameters_unwrapped`). Without this a screw
//! driven two full turns snaps its nut home every half-turn instead of
//! advancing it by two leads. Each step is bounded below half a turn, so
//! the turn count is never ambiguous.
//!
//! Citations: screw/twist parameterisation — Murray, Li & Sastry, *A
//! Mathematical Introduction to Robotic Manipulation*, ch. 2; tangent-space
//! Newton — Solà, Deray & Atchuthan, arXiv:1812.01537.

use crate::jacobian::{residual_for_driven, wrap_to_pi, BodyBlock, DriveRow};
use crate::solver::{
    gauss_newton_driven, production_jacobian, residual_norm, SolveReport, SOLVE_TOL,
};
use crate::types::{Assembly, InstanceId, MateKind};
use serde::{Deserialize, Serialize};
use std::f64::consts::{PI, TAU};

/// The largest increment a drive advances its parameter by in one step.
///
/// Bounded strictly below half a turn (π) for the ROTATIONAL case: the
/// drive residual compares angles through `wrap_to_pi`, which is only
/// unambiguous — and only smooth — while the step stays clear of the ±π
/// seam. π/8 leaves generous margin and gives the stroke enough
/// resolution to stamp a rank transition near where it actually happens.
/// Translational drives share the bound; it costs nothing (a step is one
/// small Newton solve from an already-close start) and keeps the stroke's
/// sampling uniform across both parameter kinds.
pub(crate) const MAX_DRIVE_STEP: f64 = PI / 8.0;

/// Relative singular-value floor below which a stroke calls a direction
/// FREE (see [`Assembly::scoped_dof`] for the derivation). Deliberately
/// looser than `dof_analysis`'s exact-rank tolerance: it is the accuracy
/// the solved pose actually carries near a fold, not the accuracy an
/// authored pose carries.
const SINGULAR_REL_TOL: f64 = 1e-4;

/// Which scalar joint parameter of a frame-pair mate is being driven.
///
/// The frame-pair convention (`types::MateKind`) gives every joint mate
/// exactly two scalar parameters: θ = the angle from `x_a` to `x_b` about
/// `ẑ_a`, and s = `(o_b − o_a)·ẑ_a`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriveParam {
    /// θ — rotation about the connector frame's z axis.
    Rotation,
    /// s — translation along the connector frame's z axis.
    Translation,
}

impl DriveParam {
    /// Human name used in refusal reasons.
    fn name(self) -> &'static str {
        match self {
            DriveParam::Rotation => "rotation (θ)",
            DriveParam::Translation => "translation (s)",
        }
    }
}

/// Why a drive request was REFUSED — typed, never a silent no-op.
///
/// A refusal means the request is not expressible: the mate does not
/// exist, is not enforced, or has no such free parameter. It does NOT
/// mean "the motion turned out to be impossible" — an unreachable drive
/// is a converged-false [`DragOutcome`], because that is a measurement,
/// not a declaration error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "refusal", rename_all = "snake_case")]
pub enum DriveRefusal {
    /// No mate at that index.
    UnknownMate { mate_index: u32 },
    /// The mate is carried but not numerically enforced (the honest-refuse
    /// set, a feature/kind mismatch, a broken coupling reference) — it
    /// constrains nothing, so it exposes nothing to drive.
    NotEnforced { mate_index: u32, reason: String },
    /// The mate is enforced but exposes no such free scalar parameter.
    NotDriveable {
        mate_index: u32,
        param: DriveParam,
        reason: String,
    },
}

/// The joint bottomed out: the requested value lay outside the declared
/// limits and was clamped to the band.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LimitFact {
    /// What the caller asked for, recorded verbatim.
    pub requested: f64,
    pub min: f64,
    pub max: f64,
}

/// What the re-solve was allowed to touch — the instrumented proof that a
/// drag stayed scoped.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DragScope {
    /// The NON-ground instances the re-solve could move (ascending).
    pub instances: Vec<InstanceId>,
    /// The mate indices whose residuals entered the re-solve (ascending).
    pub mates: Vec<u32>,
}

/// A coupling whose coupled joint carried a non-zero winding through the
/// stroke — the state the drag leaves behind, made visible.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindingFact {
    /// The COUPLING mate (gear / rack-pinion / screw) this concerns.
    pub mate_index: u32,
    /// Full turns its coupled joint parameter has accumulated.
    pub turns: i32,
}

/// The scope's instantaneous mobility CHANGED at this point of the stroke
/// — the tell that the mechanism passed through a special configuration
/// (premise #1: numeric DOF is configuration-sensitive).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RankTransition {
    /// The driven parameter value where the change was measured.
    pub param: f64,
    pub dof_before: usize,
    pub dof_after: usize,
}

/// The outcome of a kinematic drag.
#[derive(Debug, Clone, PartialEq)]
pub struct DragOutcome {
    /// The re-solve's report at the FINAL step — `converged` re-measured,
    /// never asserted. False ⇒ the drive was unreachable and every pose
    /// has been restored.
    pub report: SolveReport,
    /// The value actually driven to (the request, clamped to limits).
    pub applied: f64,
    /// Present iff the request was clamped.
    pub limit: Option<LimitFact>,
    /// What the re-solve was allowed to touch.
    pub scope: DragScope,
    /// Couplings carrying a winding after the stroke.
    pub windings: Vec<WindingFact>,
    /// Mobility changes observed along the stroke (empty on a generic one).
    pub rank_transitions: Vec<RankTransition>,
}

impl MateKind {
    /// The limits of this kind's `param` freedom, or `None` when the kind
    /// exposes no such driveable parameter.
    ///
    /// # The rule
    ///
    /// A parameter is driveable exactly when the kind's free motion is
    /// SPANNED by the frame's z-screw parameters (θ about `ẑ_a`, s along
    /// `ẑ_a`) — the two quantities `joint_parameters_of` can read. That is
    /// the honest boundary:
    ///
    /// * `Revolute` {θ}, `Slider` {s}, `Cylindrical` {θ, s} — driveable;
    ///   setting the parameter DETERMINES the joint's configuration.
    /// * `Planar` {x, y, θ}, `Ball` {rot x,y,z}, `PinSlot` {θ, slot} —
    ///   refused. Their freedom is not spanned by (θ, s): driving θ on a
    ///   Planar would leave the two in-plane translations free, so the
    ///   "drive" would not determine anything and the re-solve's answer
    ///   would be an artefact of the least-norm step rather than the
    ///   kinematics. PinSlot's travel runs along the SLOT direction on
    ///   frame A, which is deliberately not `ẑ_a`, so `s` is not its
    ///   parameter; reading it would drive the wrong thing.
    /// * `Fastened` (no freedom), the overlays (not joints), the couplings
    ///   (they relate OTHER mates' parameters — drive the base joint), and
    ///   the legacy Face/Axis kinds (no connector frame, so θ has no
    ///   reference direction) — all refused.
    ///
    /// Refusing is the point: a kind whose freedom (θ, s) cannot express
    /// must say so, never silently drive an approximation of it.
    fn drive_limits(&self, param: DriveParam) -> Option<Option<(f64, f64)>> {
        match (self, param) {
            (MateKind::Revolute { limits }, DriveParam::Rotation) => Some(*limits),
            (MateKind::Slider { limits }, DriveParam::Translation) => Some(*limits),
            (MateKind::Cylindrical { rot_limits, .. }, DriveParam::Rotation) => Some(*rot_limits),
            (MateKind::Cylindrical { trans_limits, .. }, DriveParam::Translation) => {
                Some(*trans_limits)
            }
            _ => None,
        }
    }

    /// Why this kind cannot expose `param` (the refusal's reason text).
    fn undriveable_reason(&self, param: DriveParam) -> String {
        let detail = match self {
            MateKind::Fastened | MateKind::Fixed => {
                "a fastened mate has no freedom at all (0 DOF)".to_string()
            }
            MateKind::Revolute { .. } => {
                "a revolute's slide is LOCKED — its only freedom is θ".to_string()
            }
            MateKind::Slider { .. } => {
                "a slider's spin is LOCKED — its only freedom is s".to_string()
            }
            MateKind::Planar => "a planar mate's freedom is 2 in-plane translations + spin, \
                 which the frame's (θ, s) parameters do not span: driving θ alone \
                 would leave the assembly under-determined"
                .to_string(),
            MateKind::Ball => "a ball mate's freedom is 3 rotations, which no single frame \
                 parameter spans"
                .to_string(),
            MateKind::PinSlot { .. } => {
                "a pin-slot's travel runs along the SLOT direction on frame A, not the \
                 frame z axis the (θ, s) joint parameters read — driving s would drive \
                 the wrong direction"
                    .to_string()
            }
            MateKind::GearRatio { .. } | MateKind::RackPinion { .. } | MateKind::Screw { .. } => {
                "a coupling relates the joint parameters of OTHER mates rather than \
                 owning one — drive the base joint it couples instead"
                    .to_string()
            }
            MateKind::Coincident | MateKind::Concentric => {
                "the legacy Face/Axis kinds carry no connector FRAME, so θ has no \
                 reference direction to be measured from — re-declare as a frame-pair \
                 joint kind (Revolute / Slider / Cylindrical)"
                    .to_string()
            }
            _ => "this kind is a dimensional overlay, not a joint — it owns no joint \
                 parameter"
                .to_string(),
        };
        format!(
            "{:?} exposes no driveable {} — {detail}",
            self,
            param.name()
        )
    }
}

/// Clamp `target` into `limits`, reporting the hit.
fn clamp_to_limits(target: f64, limits: Option<(f64, f64)>) -> (f64, Option<LimitFact>) {
    let Some((min, max)) = limits else {
        return (target, None);
    };
    // A declared band with min > max is not orderable; treat it as its own
    // closure rather than inventing a side to clamp to.
    let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
    if target < lo || target > hi {
        let applied = target.clamp(lo, hi);
        (
            applied,
            Some(LimitFact {
                requested: target,
                min,
                max,
            }),
        )
    } else {
        (target, None)
    }
}

/// The winding an UNWRAPPED angle carries: how many full turns separate it
/// from its wrapped image.
pub(crate) fn turns_of_angle(unwrapped: f64) -> i32 {
    let turns = (unwrapped - wrap_to_pi(unwrapped)) / TAU;
    // Rounding is exact here: the numerator is an integer multiple of TAU
    // by construction of `wrap_to_pi`.
    turns.round() as i32
}

impl Assembly {
    /// The limits of mate `index`'s `param` freedom, or `None` when the
    /// mate exposes no such driveable parameter (unknown, unenforced,
    /// frameless, or a kind whose freedom (θ, s) cannot span — see
    /// [`MateKind::drive_limits`]).
    ///
    /// This is the single question "is this a joint parameter, and what
    /// bounds it" — the derived-sweep builder reads joints out of the
    /// mates through it, so the sweep surface and the drag surface can
    /// never disagree about what is driveable.
    pub(crate) fn driveable_limits(
        &self,
        index: u32,
        param: DriveParam,
    ) -> Option<Option<(f64, f64)>> {
        let mate = self.mates.get(index as usize)?;
        if !mate.kind.is_numerically_enforced() {
            return None;
        }
        let limits = mate.kind.drive_limits(param)?;
        // A joint whose frames cannot be resolved has no readable
        // parameter, whatever its kind claims.
        self.joint_parameters_of(index)?;
        Some(limits)
    }

    /// Read the driven parameter's CURRENT value — unwrapped for rotation,
    /// so a stroke starting from a wound joint continues from where the
    /// last one left it rather than from its wrapped shadow.
    fn drive_value(&self, mate_index: u32, param: DriveParam) -> Option<f64> {
        let (theta, s) = self.joint_parameters_unwrapped(mate_index)?;
        Some(match param {
            DriveParam::Rotation => theta,
            DriveParam::Translation => s,
        })
    }

    /// The scope's INSTANTANEOUS mobility along a stroke: `6·blocks −
    /// rank(J)` over the scope's own mates. Measured WITHOUT the drive row
    /// — this is the mechanism's freedom, not the driven system's.
    ///
    /// # Why this does not reuse `dof_analysis`'s rank tolerance
    ///
    /// `dof_analysis` answers "what is the rank AT this authored pose" and
    /// uses an exact-rank tolerance (1e-6 relative). A stroke asks a
    /// different question, at a pose it did not author but SOLVED for —
    /// and near a singular configuration the residual is QUADRATIC in the
    /// pose error, so `SOLVE_TOL` (1e-9) still admits a pose ~1e-4 away
    /// from the exact fold (`err ≈ √(SOLVE_TOL·L)` for a mechanism of
    /// scale `L`). A singular value below that floor is not
    /// distinguishable from zero at the pose we actually have; counting it
    /// as an independent constraint would report a rigidity the mechanism
    /// does not possess.
    ///
    /// [`SINGULAR_REL_TOL`] is that floor. It is not a knife-edge: on the
    /// parallelogram four-bar the collapsing singular value falls from
    /// ~1e-1 (generic) to ~1e-5 (stretched collinear) against a σ_max of
    /// ~10 — four decades of separation, with the threshold ~100× clear of
    /// both sides. Mechanisms are singular by a wide margin or not at all;
    /// it is only the *exact-rank* cliff that lands in the gap.
    fn scoped_dof(&self, blocks: &[BodyBlock], mate_indices: &[usize]) -> usize {
        let config_dim = 6 * blocks.len();
        let jac = production_jacobian(self, blocks, mate_indices);
        if jac.nrows() == 0 || jac.ncols() == 0 {
            return config_dim;
        }
        let svals = jac.singular_values();
        let max_sv = svals.iter().cloned().fold(0.0_f64, f64::max);
        let tol = (max_sv * SINGULAR_REL_TOL).max(1e-9);
        let rank = svals.iter().filter(|&&s| s > tol).count();
        config_dim.saturating_sub(rank)
    }

    /// Couplings in `mate_indices` whose coupled joints carry a winding.
    fn winding_facts(&self, mate_indices: &[usize]) -> Vec<WindingFact> {
        mate_indices
            .iter()
            .filter_map(|&mi| {
                let mate = self.mates.get(mi)?;
                let couples: Vec<u32> = match mate.kind {
                    MateKind::GearRatio { couples, .. } | MateKind::RackPinion { couples, .. } => {
                        couples.to_vec()
                    }
                    MateKind::Screw { couples, .. } => vec![couples],
                    _ => return None,
                };
                let turns = couples
                    .iter()
                    .map(|&c| self.turns_of(c))
                    .find(|&t| t != 0)?;
                Some(WindingFact {
                    mate_index: u32::try_from(mi).ok()?,
                    turns,
                })
            })
            .collect()
    }

    /// Validate a drive request against the taxonomy, returning the
    /// clamped value + the at-limit fact.
    fn prepare_drive(
        &self,
        mate_index: u32,
        param: DriveParam,
        target: f64,
    ) -> Result<(f64, Option<LimitFact>), DriveRefusal> {
        let Some(mate) = self.mates.get(mate_index as usize) else {
            return Err(DriveRefusal::UnknownMate { mate_index });
        };
        // Enforcement first: an unenforced mate constrains nothing, so it
        // exposes nothing to drive. Reuse the enforcement report's reason
        // so the drag and the certificate tell the SAME story.
        if let Some(reason) = self
            .mate_enforcement_report()
            .mates
            .get(mate_index as usize)
            .and_then(|verdict| verdict.reason.clone())
        {
            return Err(DriveRefusal::NotEnforced { mate_index, reason });
        }
        let Some(limits) = mate.kind.drive_limits(param) else {
            return Err(DriveRefusal::NotDriveable {
                mate_index,
                param,
                reason: mate.kind.undriveable_reason(param),
            });
        };
        // The (θ, s) reader needs a resolvable frame pair on both sides.
        if self.joint_parameters_of(mate_index).is_none() {
            return Err(DriveRefusal::NotDriveable {
                mate_index,
                param,
                reason: "the mate's connector frames could not be resolved, so its joint \
                         parameters cannot be read"
                    .to_string(),
            });
        }
        Ok(clamp_to_limits(target, limits))
    }

    /// Drive `mate_index`'s `param` to `target` and re-solve the affected
    /// component (module doc).
    ///
    /// Returns `Err` only when the REQUEST is inexpressible (unknown /
    /// unenforced / undriveable mate). A drive that is expressible but
    /// unreachable — the joint is welded shut, or the target fights
    /// another mate — returns `Ok` with `report.converged == false` and
    /// every pose restored exactly as it found them: a failed drag never
    /// leaves a half-stroke behind.
    pub fn drag(
        &mut self,
        mate_index: u32,
        param: DriveParam,
        target: f64,
    ) -> Result<DragOutcome, DriveRefusal> {
        let (applied, limit) = self.prepare_drive(mate_index, param, target)?;

        // Scope: the driven mate's component, from the planner's own
        // partition. Blocks are the component's CONDENSED bodies — the
        // same rigid grouping `solve_decomposed` would hand Newton.
        let decomposition = self.decomposition();
        let (blocks, scope, comp_mates) = decomposition.drag_scope(self, mate_index as usize);

        // Restore point: a failed drag must be invisible.
        let poses: Vec<([f64; 3], [f64; 4])> = self
            .instances
            .iter()
            .map(|i| (i.translation, i.rotation))
            .collect();
        let windings_before = self.windings.clone();

        let Some(start) = self.drive_value(mate_index, param) else {
            return Err(DriveRefusal::NotDriveable {
                mate_index,
                param,
                reason: "the mate's joint parameters became unreadable".to_string(),
            });
        };

        // Walk to the target in bounded increments (module doc).
        let span = applied - start;
        let steps = (span.abs() / MAX_DRIVE_STEP).ceil().max(1.0);
        let step_count = steps as usize;

        let mut rank_transitions: Vec<RankTransition> = Vec::new();
        let mut dof_prev = self.scoped_dof(&blocks, &comp_mates);
        let mut report = SolveReport {
            converged: true,
            iterations: 0,
            final_residual_norm: 0.0,
        };

        for k in 1..=step_count {
            let t_k = if k == step_count {
                applied
            } else {
                start + span * (k as f64) / (step_count as f64)
            };
            // The winding the step's target carries. Fixing it BEFORE the
            // solve keeps the coupling residuals reading the intended
            // unwrapped angle throughout the step, so a coupled parameter
            // tracks the turn instead of being hauled back by the wrap.
            if matches!(param, DriveParam::Rotation) {
                let turns = turns_of_angle(t_k);
                if turns == 0 {
                    self.windings.remove(&mate_index);
                } else {
                    self.windings.insert(mate_index, turns);
                }
            }
            let drives = [DriveRow {
                mate_index,
                param,
                target: t_k,
            }];
            report = if blocks.is_empty() {
                // The driven joint is welded shut: condensation found both
                // sides of the mate rigid with each other (a seated
                // Fastened elsewhere in the stack), so there are no columns
                // to move. That is not a declaration error — it is a
                // measurement, so the drive residual is reported as-is.
                let norm = residual_norm(&residual_for_driven(self, &comp_mates, &drives));
                SolveReport {
                    converged: norm <= SOLVE_TOL,
                    iterations: 0,
                    final_residual_norm: norm,
                }
            } else {
                gauss_newton_driven(self, &blocks, &comp_mates, &drives)
            };
            if !report.converged {
                break;
            }
            // Premise #1: the mechanism's mobility is a property of the
            // configuration. Re-measure it every step and stamp any change.
            let dof_now = self.scoped_dof(&blocks, &comp_mates);
            if dof_now != dof_prev {
                rank_transitions.push(RankTransition {
                    param: t_k,
                    dof_before: dof_prev,
                    dof_after: dof_now,
                });
                dof_prev = dof_now;
            }
        }

        if !report.converged {
            for (instance, (translation, rotation)) in self.instances.iter_mut().zip(&poses) {
                instance.translation = *translation;
                instance.rotation = *rotation;
            }
            self.windings = windings_before;
            return Ok(DragOutcome {
                report,
                applied,
                limit,
                scope,
                windings: Vec::new(),
                rank_transitions: Vec::new(),
            });
        }

        let windings = self.winding_facts(&comp_mates);
        Ok(DragOutcome {
            report,
            applied,
            limit,
            scope,
            windings,
            rank_transitions,
        })
    }
}
