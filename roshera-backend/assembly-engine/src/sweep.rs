//! Swept clearance through a mechanism's degrees of freedom — the
//! CONTINUOUS gate (kinematic-assembly campaign, Slice 5; spec §3.6).
//!
//! The load-bearing reason for Parry: a static clearance answers "do the
//! parts overlap right now"; a *kinematic* assembly must answer "does the
//! moving part stay clear across its whole range of motion" — the gimbal
//! swing, the actuator stroke.
//!
//! # Why sampling alone is a lie
//!
//! The pre-Slice-5 gate ([`swept_clearance`]) sampled the motion densely
//! and took the minimum Parry distance over the samples. Dense sampling
//! cannot certify anything: a thin blade passes clean THROUGH a wall
//! between two samples and every sample reads clear. That defect is not
//! hypothetical — it is pinned, measured, by
//! `tests/sweep_toi.rs::legacy_dense_sampler_misses_the_tunneling_blade`,
//! which keeps the old sampler's miss on the record.
//!
//! The fix is Parry's **nonlinear time-of-impact**
//! (`query::cast_shapes_nonlinear`) — continuous collision under full
//! rigid motion, the machinery Rapier's motion-clamping CCD is built on.
//! Between consecutive samples each moving instance follows the exact
//! SCREW that carries its pose from one sample to the next (Chasles'
//! theorem — see [`screw_between`]), and the TOI query answers "does
//! anything touch along the way" for the whole continuum, not 73 snapshots
//! of it. For a revolute-driven part the reconstructed screw IS the joint's
//! own motion, so the sweep is exact rather than merely dense.
//!
//! Sampling is RETAINED for the clearance *profile* — the min-clearance
//! curve TOI does not produce — so both facts ride the certificate:
//! `first_contact` (continuous) and `min_certified_clearance` (sampled).
//!
//! # Joints DERIVED from mates (the §2.2 hole)
//!
//! A `Mechanism` used to be AUTHORED: the caller supplied a joint with its
//! own axis coordinates, and nothing ever checked that the declared motion
//! stayed on the mate constraint manifold. A mechanism declared about the
//! wrong axis certified a motion the mates forbid — and no dimension of
//! the certificate caught it.
//!
//! Two changes close it:
//!
//! * **Derived sweeps.** `certify` now reads the joints OUT of the mates
//!   ([`Assembly::derived_sweeps`]): a `Revolute`/`Slider`/`Cylindrical`
//!   mate IS a joint, and its parameter's declared limits ARE its range.
//!   Nothing is authored, so nothing can be authored wrong.
//! * **Manifold re-check.** An authored mechanism is still accepted (the
//!   stateless verify surface takes them), but every sample is now
//!   re-checked against the mates it moves. A motion that leaves the
//!   manifold is REFUSED with a typed, stamped [`ManifoldViolation`] —
//!   never certified clear.
//!
//! # Conservative certificate (unchanged)
//!
//! Parry's mesh distance is tessellation-approximate, so the certified
//! clearance subtracts `epsilon` — the deviation bound the kernel computes
//! — giving a guaranteed lower bound: `certified = min_distance − epsilon`.
//!
//! Citations: Chasles' screw decomposition — Murray, Li & Sastry, *A
//! Mathematical Introduction to Robotic Manipulation*, ch. 2; nonlinear
//! TOI / motion-clamping CCD — Parry `query::cast_shapes_nonlinear`,
//! <https://rapier.rs/docs/user_guides/bevy_plugin/rigid_body_ccd/>.

use crate::interference::{instance_convex_pieces, instance_isometry, instance_trimesh};
use crate::joint::{set_joint, Joint};
use crate::motion::{DriveParam, DriveRefusal};
use crate::types::{Assembly, InstanceId, MateKind};
use parry3d_f64::na::{Isometry3, Point3, Vector3};
use parry3d_f64::query::{self, NonlinearRigidMotion};
use parry3d_f64::shape::{ConvexPolyhedron, TriMesh};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A mate whose residual must keep holding through a motion is judged
/// against this. An on-manifold motion (`set_joint` about the joint's true
/// axis, or a converged driven re-solve) leaves residuals at machine noise
/// — ~1e-15 — so this sits orders of magnitude clear of both sides and
/// only a genuine tear crosses it.
const MANIFOLD_TOL: f64 = 1e-6;

/// Swept-clearance verdict for one moving part across a joint's range
/// (the pre-Slice-5 sampled gate — retained for the clearance profile and
/// for the legacy [`swept_clearance`] entry point).
#[derive(Debug, Clone, PartialEq)]
pub struct SweptClearance {
    /// Certified minimum clearance over the motion: `raw_min_clearance −
    /// epsilon`. Conservative — the true clearance is at least this.
    pub min_clearance: f64,
    /// Raw (un-bounded) minimum Parry distance over the sweep.
    pub raw_min_clearance: f64,
    /// True when the certified clearance drops to ≤ 0 anywhere in the motion.
    pub collides: bool,
    /// Sampling density used.
    pub samples: usize,
}

/// How a swept fact's verdict was reached.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum SweepMethod {
    /// Dense sampling only — cannot see between samples. The pre-Slice-5
    /// method; kept as a named value so a fact can never claim continuous
    /// coverage it did not have.
    SampledDense { samples: usize },
    /// Parry nonlinear time-of-impact between consecutive samples
    /// (continuous — no tunneling), plus the sampled clearance profile.
    NonlinearToi { samples: usize },
    /// NOTHING RAN. The fact carries a [`SweepRefusal`], so no method was
    /// applied at all — and saying so is the point: a refused fact used to
    /// report `NonlinearToi { samples: 0 }`, which names a method that
    /// never executed. Zero samples of a continuous method is not a weak
    /// measurement, it is the absence of one, and this is the value that
    /// says which. Additive on the wire (`{"method":"not_run"}`).
    NotRun,
}

/// Where a swept fact's motion came from.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum SweepSource {
    /// An AUTHORED mechanism (the stateless verify surface). Its motion is
    /// re-checked against the mates it moves — see the module doc.
    Mechanism { moving: InstanceId },
    /// A joint DERIVED from a mate's own free parameter — nothing is
    /// authored, so nothing can be authored wrong.
    DrivenMate { mate_index: u32, param: DriveParam },
    /// A freedom a mate's KIND grants that no (θ, s) drive parameter can
    /// name (`Planar`, `Ball`, `PinSlot` — see
    /// [`MateKind::grants_undriveable_freedom`]). There is no driven
    /// motion to attribute the fact to, so it is attributed to the mate
    /// itself: the freedom is real, and it is why nothing was swept.
    MateFreedom { mate_index: u32 },
    /// A mate this wire cannot NAME: its declaration index does not fit
    /// the `u32` every `mate_index` field on this surface is typed as.
    /// Nothing about its motion can be reported except that the mate
    /// exists and nothing swept it — which is still more than the silent
    /// `continue` this replaces.
    UnnameableMate { mate_index: usize },
}

/// A point on the driven motion — what makes an interference fact
/// actionable: "they hit at θ = 42°", not merely "they hit".
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MotionStamp {
    /// The driven parameter's value at this point of the motion.
    pub param: f64,
}

/// The motion left the constraint manifold — the mates forbid it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ManifoldViolation {
    /// Where on the motion the tear is reported.
    ///
    /// For an AUTHORED mechanism this stamps the WORST tear over the
    /// declared range: `set_joint` places the part at any sample
    /// regardless of the mates, so the whole range is measurable and the
    /// largest violation is the strongest evidence the motion is wrong.
    /// For a DRIVEN sweep it stamps where the sweep STOPPED — a mechanism
    /// that cannot reach a configuration cannot be evaluated past it.
    pub param: f64,
    /// The mate residual norm at that point — the size of the tear.
    pub violation: f64,
}

/// Two instances interpenetrate at a point of the motion.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct InterferenceFact {
    pub a: InstanceId,
    pub b: InstanceId,
    /// Penetration depth (positive = how far they overlap).
    pub depth: f64,
    pub at: MotionStamp,
}

/// Why a motion could not be certified: it says the range was never swept,
/// and why.
///
/// A refusal is not a COLLISION — nothing was found, because nothing was
/// looked at. It is equally not a pass: an unswept range has not been
/// proven clear, so a refused fact carries `clear: false` and the
/// certificate lists it under `unverified_sweeps`, exactly as an unrun
/// pairwise check lands in `interference_unverified` rather than riding
/// `no_static_interference`. The two facts are told apart by `refusal`
/// itself, never by folding one into the other.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "refusal", rename_all = "snake_case")]
pub enum SweepRefusal {
    /// A translational joint with no declared limits has unbounded travel:
    /// there is no finite range to certify. Declare limits, or accept that
    /// the motion is uncertified — never an invented range, never a silent
    /// skip. (Rotation needs no limits: a full turn is compact.)
    UnboundedTravel { mate_index: u32, param: DriveParam },
    /// The mate GRANTS a relative freedom the derived sweep cannot drive:
    /// the kind's free motion is not spanned by the frame's (θ, s)
    /// parameters, so there is no parameter to walk and no range to sweep
    /// (`Planar`, `Ball`, `PinSlot` — the table is on
    /// [`MateKind::grants_undriveable_freedom`]).
    ///
    /// This is the audit-2026-09-03 task-41 refusal. The freedom used to
    /// be `continue`d over: never swept AND never named, so
    /// `swept_clearance_ok` read `true` about motions no check had run
    /// on. An undriveable freedom is not an absent one.
    ///
    /// Not to be confused with [`DriveRefusal::NotDriveable`], which
    /// shares the name at a DIFFERENT level. That one answers a drive
    /// REQUEST: "this enforced mate exposes no such scalar parameter",
    /// per (mate, param). This one is a property of the KIND, before any
    /// request: "no drive parameter of any kind names this freedom". A
    /// mate can produce this refusal without anyone ever asking to drive
    /// it — that is precisely why it exists.
    NotDriveable { mate_index: u32, kind: MateKind },
    /// A dimensional overlay (`Distance`, `Angle`, `Parallel`, `Tangent`)
    /// holding a pair that NO enforced joint mate holds — so the pair's
    /// remaining freedom is the whole picture and nothing sweeps it.
    ///
    /// The overlay itself grants no motion; it removes a scalar. The
    /// refusal is about what is missing around it, which is why it is a
    /// separate variant from [`Self::NotDriveable`]: the remedy is to
    /// declare the joint that actually holds the pair, not to find a
    /// driver for a freedom the taxonomy cannot express. An overlay
    /// riding a real joint produces nothing — see
    /// [`crate::types::Assembly::pair_freedom_is_accounted_for`].
    OverlayWithoutJoint { mate_index: u32, kind: MateKind },
    /// The mate's declaration index does not fit the `u32` this wire
    /// types every `mate_index` as, so its freedom cannot even be
    /// addressed — let alone swept. Structurally unreachable below
    /// `u32::MAX` mates; recorded rather than skipped because a `continue`
    /// here would be the same unrun-check-as-a-pass this task closed
    /// everywhere else.
    MateIndexUnrepresentable { mate_index: usize },
    /// The sweep asked the drag surface to drive this parameter and the
    /// drive REFUSED (unknown mate, an unenforced one, or joint
    /// parameters that cannot be read at all) — so the range was never
    /// walked.
    ///
    /// The refusal is carried verbatim rather than re-worded: the sweep
    /// and the drag tell one story about why a joint could not be moved.
    DriveRefused {
        mate_index: u32,
        param: DriveParam,
        drive_refusal: DriveRefusal,
    },
}

/// One certified statement about one motion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SweptFact {
    pub source: SweepSource,
    /// The parameter range swept (meaningless when `refusal` is set).
    pub range: (f64, f64),
    pub method: SweepMethod,
    /// The range was swept and nothing was found in it. A fact carrying a
    /// `refusal` reports `clear: false`: nothing was swept, so nothing was
    /// proven, and an unswept range must never read as a clear one. The
    /// `refusal` field is what tells "never checked" apart from "checked
    /// and it collides".
    pub clear: bool,
    /// `min sampled distance − epsilon` over the motion; `None` when no
    /// pair had a measurable distance (a meshless instance cannot collide).
    pub min_certified_clearance: Option<f64>,
    /// The ε this fact's bound was computed at — recorded, never implied.
    pub epsilon: f64,
    /// First continuous contact found by TOI, motion-stamped.
    pub first_contact: Option<MotionStamp>,
    /// The motion leaves the mates' constraint manifold.
    pub manifold_violation: Option<ManifoldViolation>,
    /// Motion-stamped interpenetrations found at samples.
    pub interference: Vec<InterferenceFact>,
    /// Present ⇒ the range was not swept; this says why.
    pub refusal: Option<SweepRefusal>,
}

/// A motion whose range was never swept — the check did not run, and this
/// names which motion and why.
///
/// The honest sibling of [`crate::interference::UnverifiedInterferencePair`]
/// and [`crate::mate_contact::UnverifiedMate`]: an unrun check is never
/// folded into a pass, so a non-empty list of these is why the certificate's
/// `swept_clearance_ok` reads `false`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnverifiedSweep {
    /// The motion that was not swept.
    pub source: SweepSource,
    /// Why it was not swept.
    pub refusal: SweepRefusal,
}

impl SweptFact {
    /// A refused sweep: nothing swept, so nothing proven — `clear: false`,
    /// with the reason carried.
    ///
    /// The method is [`SweepMethod::NotRun`], not `NonlinearToi { samples:
    /// 0 }`. The old value named a method that never executed; zero
    /// samples of a continuous method is the ABSENCE of a measurement, not
    /// a weak one, and a fact that says so cannot be misread as a thin
    /// sweep.
    fn refused(source: SweepSource, epsilon: f64, refusal: SweepRefusal) -> Self {
        Self {
            source,
            range: (0.0, 0.0),
            method: SweepMethod::NotRun,
            clear: false,
            min_certified_clearance: None,
            epsilon,
            first_contact: None,
            manifold_violation: None,
            interference: Vec::new(),
            refusal: Some(refusal),
        }
    }
}

/// The mate's declaration index as this wire types it, or the FACT that
/// says it cannot be named at all.
///
/// Split out of [`Assembly::derived_sweeps`]'s loop so the branch is
/// reachable from a test: an index above `u32::MAX` needs four billion
/// mates to reach through the assembly, and the certificate must still be
/// honest about a mate it cannot address.
///
/// The `Err` is boxed because a `SweptFact` is a large value on an arm
/// that is effectively never taken (`clippy::result_large_err`); the
/// caller unboxes it straight into the fact list.
fn nameable_index(mate_index: usize, epsilon: f64) -> Result<u32, Box<SweptFact>> {
    u32::try_from(mate_index).map_err(|_| {
        Box::new(SweptFact::refused(
            SweepSource::UnnameableMate { mate_index },
            epsilon,
            SweepRefusal::MateIndexUnrepresentable { mate_index },
        ))
    })
}

/// The exact rigid SCREW carrying `from` to `to`, as a Parry nonlinear
/// motion (Chasles' theorem: every rigid displacement is a rotation about
/// an axis plus a translation along it).
///
/// Parry's `NonlinearRigidMotion` applies its angular velocity about
/// `start * local_center`, so putting that point ON the screw axis makes
/// `position_at_time` trace the true helical path. The naive choice —
/// rotating about the body's own origin with a linear velocity — would
/// trace the CHORD of a revolute's arc instead of the arc, quietly
/// under-sweeping exactly the region a swept gate exists to check
/// (pinned by `screw_of_a_revolute_traces_the_ARC_not_the_chord`).
///
/// Derivation. With `Δ = to · from⁻¹ = (R, d)`, axis `û`, angle `φ`: the
/// axial pitch is `h = d·û`, the radial part `d⊥ = d − hû`, and the screw
/// axis passes through `c = ½(d⊥ + cot(φ/2)·(û × d⊥))` — the unique point
/// perpendicular to `û` with `(I − R)c = d⊥`. Parry's motion then needs
/// `linvel = d − (I − R)c = hû` and `angvel = ûφ`, which reproduces `from`
/// at `t = 0` and `to` at `t = 1` exactly.
fn screw_between(from: &Isometry3<f64>, to: &Isometry3<f64>) -> NonlinearRigidMotion {
    let delta = to * from.inverse();
    let d = delta.translation.vector;
    match delta.rotation.axis_angle() {
        Some((axis, angle)) if angle.abs() > 1e-12 => {
            let u = axis.into_inner();
            let h = d.dot(&u);
            let d_perp = d - u * h;
            let half = angle / 2.0;
            // sin(half) is bounded away from zero: `axis_angle` yields
            // φ ∈ (0, π], and the tiny-φ case took the branch below.
            let cot_half = half.cos() / half.sin();
            let c = (d_perp + u.cross(&d_perp) * cot_half) * 0.5;
            NonlinearRigidMotion::new(
                *from,
                from.inverse_transform_point(&Point3::from(c)),
                u * h,
                u * angle,
            )
        }
        // A pure translation has no axis to find: rotate about nothing,
        // slide along d.
        _ => NonlinearRigidMotion::new(*from, Point3::origin(), d, Vector3::zeros()),
    }
}

/// One instance's collision view: the EXACT mesh plus its convex
/// decomposition.
///
/// Both are needed, and for different questions. Parry's nonlinear TOI is
/// built for SUPPORT MAPS — hand it two `TriMesh`es and it takes the
/// composite×composite path, casting every triangle of one against the
/// whole of the other: MEASURED at ~1.5 s per pair per sub-step on a
/// 128-triangle ring, which is not a gate, it is a hang. On the convex
/// pieces the same query is the GJK path Parry intends, and it is
/// microseconds.
///
/// The pieces are also SOUND for the verdict that matters: a convex hull
/// CONTAINS the geometry it wraps, so "no piece ever touched" proves "the
/// parts never touched". The error is one-directional — pieces can only
/// over-report — and the exact mesh is kept precisely to adjudicate those
/// candidates (see [`SweepAccumulator::cast`]).
struct Body {
    id: InstanceId,
    mesh: TriMesh,
    pieces: Vec<ConvexPolyhedron>,
}

/// Every instance that carries a usable collision mesh. A meshless
/// instance cannot collide with anything, so it is simply absent — never
/// a silent zero-distance.
fn collision_bodies(assembly: &Assembly) -> Vec<Body> {
    assembly
        .instances
        .iter()
        .filter_map(|instance| {
            let mesh = instance_trimesh(instance)?;
            let pieces = instance_convex_pieces(instance);
            if pieces.is_empty() {
                return None;
            }
            Some(Body {
                id: instance.id,
                mesh,
                pieces,
            })
        })
        .collect()
}

/// World isometries of the collision bodies at the assembly's current poses.
fn isometries_of(assembly: &Assembly, bodies: &[Body]) -> Vec<Isometry3<f64>> {
    bodies
        .iter()
        .map(|b| {
            assembly
                .instance(b.id)
                .map(instance_isometry)
                .unwrap_or_else(Isometry3::identity)
        })
        .collect()
}

/// The `s`-th of `n` samples across `range`.
fn sample_at(range: (f64, f64), s: usize, n: usize) -> f64 {
    if n <= 1 {
        range.0
    } else {
        range.0 + (range.1 - range.0) * (s as f64) / ((n - 1) as f64)
    }
}

/// The accumulating verdict of one sweep.
struct SweepAccumulator {
    raw_min: f64,
    first_contact: Option<MotionStamp>,
    interference: Vec<InterferenceFact>,
    /// Pairs already in contact at the START of the motion — mating pairs
    /// (see [`SweepAccumulator::seed`]).
    seated: BTreeSet<(usize, usize)>,
    /// Exact pair distances at the most recent sample, so a sub-step's
    /// motion bound can be tested without re-measuring them
    /// (see [`SweepAccumulator::cast`]).
    last_distance: BTreeMap<(usize, usize), f64>,
}

/// A SOUND upper bound on how far any point of the body moves over the
/// sub-step `t ∈ [0, 1]` — the classic conservative-advancement bound.
///
/// A point at distance `r` from the screw axis rotating through `φ` travels
/// `2r·sin(φ/2) ≤ r·φ` along its arc, and the axial slide adds `|linvel|`.
/// Bounding `r` by the distance from the axis POINT (never less than the
/// distance from the axis LINE) keeps the estimate on the safe side.
fn motion_bound(motion: &NonlinearRigidMotion, mesh: &TriMesh) -> f64 {
    let centre = motion.start * motion.local_center;
    let r_max = mesh
        .vertices()
        .iter()
        .map(|v| (motion.start * v - centre).norm())
        .fold(0.0_f64, f64::max);
    motion.linvel.norm() + motion.angvel.norm() * r_max
}

/// Each unordered (moving, other) pair exactly once.
fn pairs(bodies: &[Body], moving: &[usize]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for &m in moving {
        for other in 0..bodies.len() {
            if other == m {
                continue;
            }
            // When BOTH are moving the pair would be visited twice; keep
            // the first ordering only.
            if moving.contains(&other) && other < m {
                continue;
            }
            out.push((m, other));
        }
    }
    out
}

impl SweepAccumulator {
    fn new() -> Self {
        Self {
            raw_min: f64::INFINITY,
            first_contact: None,
            interference: Vec::new(),
            seated: BTreeSet::new(),
            last_distance: BTreeMap::new(),
        }
    }

    /// Record which pairs are ALREADY in contact before the motion starts.
    ///
    /// These are mating pairs — a lid seated flush on a base, a shaft
    /// bottomed in a bore. They are in contact BY DESIGN, and whether that
    /// is acceptable is the static dimension's verdict
    /// (`no_static_interference`), reached before any motion. The swept
    /// gate asks a different question: does the MOTION bring things
    /// together that were apart? Asking TOI "when do these first touch"
    /// about two surfaces that already touch has no answer — it reports
    /// the coplanar sliding contact itself, which is precisely Parry's
    /// documented TriMesh internal-edge ghost-collision behaviour, and
    /// would fail every correctly-mated assembly in existence.
    ///
    /// Seated pairs stay under the SAMPLE-wise interference check, which
    /// reads penetration DEPTH and so still catches a mating pair the
    /// motion drives into each other.
    ///
    /// Honest residual: a seated pair that penetrates ONLY between two
    /// samples (never at one) is not caught. Closing that needs a
    /// penetration-depth-aware continuous query rather than a first-touch
    /// one; the sampled depth check bounds it in the meantime.
    fn seed(&mut self, bodies: &[Body], isos: &[Isometry3<f64>], moving: &[usize]) {
        for (m, other) in pairs(bodies, moving) {
            let (Some(ba), Some(bb), Some(ia), Some(ib)) = (
                bodies.get(m),
                bodies.get(other),
                isos.get(m),
                isos.get(other),
            ) else {
                continue;
            };
            if matches!(query::distance(ia, &ba.mesh, ib, &bb.mesh), Ok(d) if d <= CONTACT_TOL) {
                self.seated.insert((m, other));
            }
        }
    }

    /// Record the sampled clearance + any interpenetration at one sample.
    fn sample(&mut self, bodies: &[Body], isos: &[Isometry3<f64>], moving: &[usize], param: f64) {
        for (m, other) in pairs(bodies, moving) {
            let (Some(ba), Some(bb), Some(ia), Some(ib)) = (
                bodies.get(m),
                bodies.get(other),
                isos.get(m),
                isos.get(other),
            ) else {
                continue;
            };
            let Ok(distance) = query::distance(ia, &ba.mesh, ib, &bb.mesh) else {
                self.last_distance.remove(&(m, other));
                continue;
            };
            self.last_distance.insert((m, other), distance);
            if distance > CONTACT_TOL {
                // SEPARATED: this pair carries a real clearance, so it is
                // what the ε-conservative margin is about.
                self.raw_min = self.raw_min.min(distance);
                continue;
            }
            // TOUCHING or overlapping. A mating pair's clearance is zero by
            // design, so feeding it to the margin would fail every
            // correctly-assembled mechanism (`0 − ε < 0`). Contact is
            // judged by penetration DEPTH instead — exactly the static
            // dimension's contract, so "interfering" means one thing across
            // the engine.
            if let Ok(Some(contact)) = query::contact(ia, &ba.mesh, ib, &bb.mesh, 0.0) {
                if contact.dist < -CONTACT_TOL {
                    self.interference.push(InterferenceFact {
                        a: ba.id,
                        b: bb.id,
                        depth: -contact.dist,
                        at: MotionStamp { param },
                    });
                }
            }
        }
    }

    /// Run continuous TOI across one sub-step of the motion.
    fn cast(
        &mut self,
        bodies: &[Body],
        from: &[Isometry3<f64>],
        to: &[Isometry3<f64>],
        moving: &[usize],
        param_from: f64,
        param_to: f64,
    ) {
        for (m, other) in pairs(bodies, moving) {
            if self.seated.contains(&(m, other)) {
                continue; // mating pair — see `seed`
            }
            let (Some(ba), Some(bb)) = (bodies.get(m), bodies.get(other)) else {
                continue;
            };
            let (Some(fa), Some(ta), Some(fb), Some(tb)) =
                (from.get(m), to.get(m), from.get(other), to.get(other))
            else {
                continue;
            };
            let motion_a = screw_between(fa, ta);
            let motion_b = screw_between(fb, tb);
            // CONSERVATIVE ADVANCEMENT. The exact distance at the sub-step's
            // START, minus the furthest either body can possibly travel over
            // it, is a sound lower bound on the distance throughout it
            // (`dist(t) ≥ dist(0) − reach`). When that stays positive the
            // pair provably cannot touch and the whole piece×piece cast is
            // skipped — which is most pairs, most of the time, in any design
            // with real clearances. `sample` already measured dist(0), so the
            // filter costs nothing beyond a vertex sweep. This is what keeps a
            // certificate that now sweeps EVERY derived joint affordable
            // enough to run on every certify (the standing autocert-perf rule).
            let reach = motion_bound(&motion_a, &ba.mesh) + motion_bound(&motion_b, &bb.mesh);
            if matches!(self.last_distance.get(&(m, other)), Some(&d) if d > reach) {
                continue;
            }
            // Cast every convex piece of one against every piece of the
            // other. `stop_at_penetration: false` = DIRECTIONAL TOI: a hit
            // is reported only where the shapes actually close on each
            // other, so tangential sliding is not a collision.
            let mut earliest: Option<f64> = None;
            for pa in &ba.pieces {
                for pb in &bb.pieces {
                    let hit =
                        query::cast_shapes_nonlinear(&motion_a, pa, &motion_b, pb, 0.0, 1.0, false);
                    if let Ok(Some(hit)) = hit {
                        earliest = Some(
                            earliest.map_or(hit.time_of_impact, |e: f64| e.min(hit.time_of_impact)),
                        );
                    }
                }
            }
            let Some(toi) = earliest else {
                continue; // no piece touched ⇒ the parts provably did not
            };
            // ADJUDICATE the candidate against the EXACT meshes at the hit
            // configuration. A convex piece can only over-report, and it
            // reliably does so on through-holes: the hull of a bored part
            // fills its own bore (an annulus's hull is a solid disc), so a
            // peg travelling down a bore with real clearance "hits" a piece
            // that is not there. VHACD does not save it — no decomposition
            // can empty a closed through-hole. The static dimension solves
            // exactly this with an exact-mesh distance test (the F6 fix);
            // the swept gate uses the same adjudicator at the candidate
            // time, so "touching" means one thing across the engine.
            //
            // Honest residual: only the candidate INSTANT is adjudicated.
            // A pair whose pieces touch spuriously at `toi` but whose real
            // surfaces meet slightly later in the SAME sub-step is not
            // caught. The sampled depth check bounds it, and shrinking the
            // sub-step shrinks it; closing it outright needs a continuous
            // query over the exact meshes, which is the cost this whole
            // structure exists to avoid.
            let at_hit_a = motion_a.position_at_time(toi);
            let at_hit_b = motion_b.position_at_time(toi);
            if matches!(
                query::distance(&at_hit_a, &ba.mesh, &at_hit_b, &bb.mesh),
                Ok(d) if d > CONTACT_TOL
            ) {
                continue; // the pieces met; the PARTS did not
            }
            let param = param_from + (param_to - param_from) * toi;
            if self
                .first_contact
                .is_none_or(|current| param < current.param)
            {
                self.first_contact = Some(MotionStamp { param });
            }
        }
    }
}

/// Overlap beyond this is interference; touching (mating faces seat flush)
/// is not — the same threshold the static dimension uses, so "interfering"
/// means one thing across the engine.
const CONTACT_TOL: f64 = 1.0e-3;

/// Sweep `moving` through `joint`'s free DOF (its first parameter) across
/// `param_range` in `samples` steps, taking the minimum Parry clearance of
/// the moving part against every other instance over the motion.
///
/// **This is the pre-Slice-5 SAMPLED gate.** It cannot see between samples
/// and therefore cannot certify a motion — a thin blade tunnels straight
/// through it (`tests/sweep_toi.rs::legacy_dense_sampler_misses_the_
/// tunneling_blade` keeps that measured). It is retained as a clearance
/// probe and for callers pinning the historic behaviour;
/// [`Assembly::sweep_driven`] and [`Assembly::sweep_mechanism_checked`]
/// are the certifying gates, and `certify` routes through them.
#[allow(clippy::too_many_arguments)]
pub fn swept_clearance(
    assembly: &Assembly,
    moving: InstanceId,
    joint: &Joint,
    base_translation: &[f64; 3],
    base_rotation: &[f64; 4],
    param_range: (f64, f64),
    samples: usize,
    epsilon: f64,
) -> SweptClearance {
    let n = samples.max(1);
    let others: Vec<InstanceId> = assembly
        .instances
        .iter()
        .map(|instance| instance.id)
        .filter(|&id| id != moving)
        .collect();

    // One working clone: each sample re-sets the moving pose from `base`, so the
    // motion never accumulates and the meshes are cloned only once.
    let mut work = assembly.clone();
    let mut raw_min = f64::INFINITY;
    for s in 0..n {
        let t = sample_at(param_range, s, n);
        if let Some(instance) = work.instances.iter_mut().find(|i| i.id == moving) {
            set_joint(instance, joint, &[t], base_translation, base_rotation);
        }
        for &other in &others {
            if let Some(distance) = work.clearance(moving, other) {
                raw_min = raw_min.min(distance);
            }
        }
    }

    let certified = if raw_min.is_finite() {
        raw_min - epsilon
    } else {
        f64::INFINITY
    };
    SweptClearance {
        min_clearance: certified,
        raw_min_clearance: raw_min,
        collides: certified <= 0.0,
        samples: n,
    }
}

/// Fuse an accumulator into the final fact.
fn finish_fact(
    source: SweepSource,
    range: (f64, f64),
    samples: usize,
    epsilon: f64,
    acc: SweepAccumulator,
    manifold_violation: Option<ManifoldViolation>,
) -> SweptFact {
    let min_certified = if acc.raw_min.is_finite() {
        Some(acc.raw_min - epsilon)
    } else {
        None
    };
    // Clear = the motion stayed on the manifold, nothing closed on
    // anything continuously, no sample interpenetrated, AND the
    // ε-conservative margin survived. The margin is a SEPARATE failure
    // from a contact: a sweep can fail its bound with a real gap and no
    // touch at all — that is what ε being load-bearing means.
    let clear = manifold_violation.is_none()
        && acc.first_contact.is_none()
        && acc.interference.is_empty()
        && min_certified.is_none_or(|m| m > 0.0);
    SweptFact {
        source,
        range,
        method: SweepMethod::NonlinearToi { samples },
        clear,
        min_certified_clearance: min_certified,
        epsilon,
        first_contact: acc.first_contact,
        manifold_violation,
        interference: acc.interference,
        refusal: None,
    }
}

impl Assembly {
    /// Continuous swept gate over a joint DERIVED from mate `mate_index`'s
    /// own free parameter (module doc).
    ///
    /// The mechanism is not authored: each sample is reached by actually
    /// DRIVING the mate ([`Assembly::drag`]), so the motion rides the
    /// constraint manifold by construction — and when the mechanism cannot
    /// reach a configuration, the sweep stops there and says so with a
    /// [`ManifoldViolation`] instead of certifying a motion it could not
    /// make.
    pub fn sweep_driven(
        &self,
        mate_index: u32,
        param: DriveParam,
        range: (f64, f64),
        samples: usize,
        epsilon: f64,
    ) -> Result<SweptFact, DriveRefusal> {
        let source = SweepSource::DrivenMate { mate_index, param };
        let n = samples.max(2);
        let bodies = collision_bodies(self);
        let mut work = self.clone();

        // Driving to the range start both validates the request (an
        // inexpressible drive refuses here) and establishes the scope.
        let start = work.drag(mate_index, param, range.0)?;
        let moving: Vec<usize> = start
            .scope
            .instances
            .iter()
            .filter_map(|id| bodies.iter().position(|b| b.id == *id))
            .collect();

        let mut acc = SweepAccumulator::new();
        let mut manifold_violation = (!start.report.converged).then_some(ManifoldViolation {
            param: range.0,
            violation: start.report.final_residual_norm,
        });

        if manifold_violation.is_none() {
            let mut prev = isometries_of(&work, &bodies);
            acc.seed(&bodies, &prev, &moving);
            acc.sample(&bodies, &prev, &moving, range.0);
            for s in 1..n {
                let t = sample_at(range, s, n);
                let step = work.drag(mate_index, param, t)?;
                if !step.report.converged {
                    // The mechanism cannot follow the driven path — it is
                    // stuck at the previous sample. Report where the sweep
                    // stopped; never certify past a configuration the mates
                    // forbid.
                    manifold_violation = Some(ManifoldViolation {
                        param: t,
                        violation: step.report.final_residual_norm,
                    });
                    break;
                }
                let next = isometries_of(&work, &bodies);
                acc.cast(
                    &bodies,
                    &prev,
                    &next,
                    &moving,
                    sample_at(range, s - 1, n),
                    t,
                );
                acc.sample(&bodies, &next, &moving, t);
                prev = next;
            }
        }

        Ok(finish_fact(
            source,
            range,
            n,
            epsilon,
            acc,
            manifold_violation,
        ))
    }

    /// Continuous swept gate over an AUTHORED mechanism, with the
    /// constraint-manifold re-check that closes the §2.2 wrong-axis hole
    /// (module doc).
    ///
    /// The declared motion is placed by `set_joint` — which will happily
    /// move a part about any axis at all — and then every sample is judged
    /// against the mates that part carries. A motion the mates forbid is
    /// refused with a stamped violation, so an off-axis mechanism can no
    /// longer certify a swing its own assembly makes impossible.
    pub fn sweep_mechanism_checked(
        &self,
        mechanism: &crate::certificate::Mechanism,
        epsilon: f64,
    ) -> SweptFact {
        let source = SweepSource::Mechanism {
            moving: mechanism.moving,
        };
        let n = mechanism.samples.max(2);
        let bodies = collision_bodies(self);
        let moving: Vec<usize> = bodies
            .iter()
            .position(|b| b.id == mechanism.moving)
            .into_iter()
            .collect();
        // Only mates touching the moved instance can change; a violation
        // anywhere else is the static picture's business, not this motion's.
        let watched: Vec<usize> = (0..self.mates.len())
            .filter(|&mi| {
                self.mates
                    .get(mi)
                    .is_some_and(|m| m.a == mechanism.moving || m.b == mechanism.moving)
            })
            .collect();

        let mut work = self.clone();
        let mut acc = SweepAccumulator::new();
        let mut worst: Option<ManifoldViolation> = None;
        let mut prev: Option<Vec<Isometry3<f64>>> = None;

        for s in 0..n {
            let t = sample_at(mechanism.range, s, n);
            if let Some(instance) = work.instances.iter_mut().find(|i| i.id == mechanism.moving) {
                set_joint(
                    instance,
                    &mechanism.joint,
                    &[t],
                    &mechanism.base_translation,
                    &mechanism.base_rotation,
                );
            }
            // The manifold re-check: does the DECLARED motion keep the
            // mates it moves satisfied?
            let violation = watched
                .iter()
                .filter_map(|&mi| work.mates.get(mi))
                .map(|m| work.mate_violation(m))
                .fold(0.0_f64, f64::max);
            if violation > MANIFOLD_TOL && worst.is_none_or(|w| violation > w.violation) {
                worst = Some(ManifoldViolation {
                    param: t,
                    violation,
                });
            }
            let isos = isometries_of(&work, &bodies);
            if prev.is_none() {
                acc.seed(&bodies, &isos, &moving);
            }
            if let Some(previous) = &prev {
                acc.cast(
                    &bodies,
                    previous,
                    &isos,
                    &moving,
                    sample_at(mechanism.range, s.saturating_sub(1), n),
                    t,
                );
            }
            acc.sample(&bodies, &isos, &moving, t);
            prev = Some(isos);
        }

        finish_fact(source, mechanism.range, n, epsilon, acc, worst)
    }

    /// The swept facts for every joint DERIVED from this assembly's own
    /// mates (module doc). Nothing is authored, so nothing can be authored
    /// wrong — this is what makes a mechanism's certificate honest by
    /// construction rather than by the caller's care.
    ///
    /// Ranges come from the joint itself:
    /// * rotation with limits ⇒ the limit band; without ⇒ the FULL TURN
    ///   (rotation is compact — a free revolute genuinely reaches every
    ///   angle, so certifying the turn certifies everything it can do);
    /// * translation with limits ⇒ the limit band; without ⇒ REFUSED,
    ///   because unbounded travel has no finite range to certify and
    ///   inventing one would be a lie.
    ///
    /// # Every freedom leaves a fact (audit 2026-09-03, task 41)
    ///
    /// A mate's freedom leaves this function in one of these shapes, and
    /// no path silently returns nothing:
    ///
    /// * SWEPT — the (θ, s) drive walks the range and the fact carries the
    ///   verdict;
    /// * REFUSED because the range is not finite
    ///   ([`SweepRefusal::UnboundedTravel`]) or the drive itself said no
    ///   ([`SweepRefusal::DriveRefused`]);
    /// * REFUSED because no drive parameter names the freedom at all
    ///   ([`SweepRefusal::NotDriveable`] — `Planar`, `Ball`, `PinSlot`);
    /// * REFUSED because a dimensional overlay is holding a pair no joint
    ///   holds ([`SweepRefusal::OverlayWithoutJoint`]);
    /// * REFUSED because the mate's index cannot be named on this wire
    ///   ([`SweepRefusal::MateIndexUnrepresentable`]).
    ///
    /// Producing NO fact means, and only means, that the mate has no
    /// freedom of its own for this function to speak about: a rigid kind
    /// (`Fastened`, `Fixed`), a coupling (it relates OTHER mates'
    /// parameters, and the motion it shapes is swept through the base
    /// joint), an unenforced kind (`Cam`, `Path`, `Symmetric` — refused by
    /// name in `mates_enforced`), or an overlay riding a joint that has
    /// already accounted for the pair.
    ///
    /// # The one stated exception
    ///
    /// The legacy Face/Axis kinds — `Coincident` (3 DOF free) and
    /// `Concentric` (2 DOF free) — DO carry freedom that this function
    /// neither sweeps nor names. That is a **decision, stated**, not an
    /// oversight of the contract above: the reasoning, and what it would
    /// cost to reverse, are recorded on
    /// [`MateKind::grants_undriveable_freedom`]. Until it is reversed,
    /// `swept_clearance_ok` is silent about the motion of a pair held only
    /// by a legacy mate.
    ///
    /// The full table is on [`MateKind::grants_undriveable_freedom`].
    pub(crate) fn derived_sweeps(&self, epsilon: f64) -> Vec<SweptFact> {
        use std::f64::consts::TAU;
        /// Samples per derived sweep. TOI covers the continuum BETWEEN
        /// samples, so this sets the resolution of the clearance profile
        /// and of the re-solve grid — not whether a hit is found.
        const DERIVED_SAMPLES: usize = 25;

        // One pass, read by the overlay condition below; recomputing it per
        // mate would make this quadratic for no gain.
        let enforcement = self.mate_enforcement_report();

        let mut facts = Vec::new();
        for (mate_index, mate) in self.mates.iter().enumerate() {
            let index = match nameable_index(mate_index, epsilon) {
                Ok(index) => index,
                Err(fact) => {
                    facts.push(*fact);
                    continue;
                }
            };
            // The freedom no drive parameter can name. Skipping it was the
            // task-41 defect: unswept AND unnamed, so `swept_clearance_ok`
            // spoke for a motion nothing had looked at.
            if mate.kind.grants_undriveable_freedom() {
                facts.push(SweptFact::refused(
                    SweepSource::MateFreedom { mate_index: index },
                    epsilon,
                    SweepRefusal::NotDriveable {
                        mate_index: index,
                        kind: mate.kind,
                    },
                ));
            } else if mate.kind.is_dimensional_overlay()
                && !self.pair_freedom_is_accounted_for(mate_index, &enforcement)
            {
                // An overlay REMOVES a scalar; it never grants motion. That
                // is only harmless while a joint holds the pair — and
                // nothing requires one to. A lone enforced `Distance`
                // leaves 5 DOF that nothing sweeps.
                facts.push(SweptFact::refused(
                    SweepSource::MateFreedom { mate_index: index },
                    epsilon,
                    SweepRefusal::OverlayWithoutJoint {
                        mate_index: index,
                        kind: mate.kind,
                    },
                ));
            }
            for param in [DriveParam::Rotation, DriveParam::Translation] {
                // The KIND decides whether this parameter is one of its
                // freedoms; whether this MATE can actually be driven is
                // the drag surface's answer, below, so the two can never
                // disagree about a joint they both see.
                let Some(limits) = mate.kind.drive_limits(param) else {
                    // Not one of this kind's (θ, s) freedoms. Every kind
                    // this fires for is accounted for above, or has no
                    // freedom to account for, or is the stated legacy
                    // exception — see this function's doc.
                    continue;
                };
                let source = SweepSource::DrivenMate {
                    mate_index: index,
                    param,
                };
                let range = match (limits, param) {
                    (Some((min, max)), _) => (min, max),
                    (None, DriveParam::Rotation) => (0.0, TAU),
                    (None, DriveParam::Translation) => {
                        facts.push(SweptFact::refused(
                            source,
                            epsilon,
                            SweepRefusal::UnboundedTravel {
                                mate_index: index,
                                param,
                            },
                        ));
                        continue;
                    }
                };
                match self.sweep_driven(index, param, range, DERIVED_SAMPLES, epsilon) {
                    Ok(fact) => facts.push(fact),
                    // The drive refused, so the range was never walked.
                    // This used to be dropped, defended by a COINCIDENCE:
                    // every mate whose drive refuses also fails
                    // `is_numerically_enforced`, so `mates_enforced` was
                    // already false and the certificate already unsound.
                    // A verdict must not rest on two predicates happening
                    // to agree — the refusal now rides its own dimension.
                    Err(drive_refusal) => facts.push(SweptFact::refused(
                        source,
                        epsilon,
                        SweepRefusal::DriveRefused {
                            mate_index: index,
                            param,
                            drive_refusal,
                        },
                    )),
                }
            }
        }
        facts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Instance, Mesh};
    use parry3d_f64::na::{Translation3, UnitQuaternion};
    use std::f64::consts::PI;

    fn cube(h: f64) -> Mesh {
        Mesh {
            vertices: vec![
                [-h, -h, -h],
                [h, -h, -h],
                [h, h, -h],
                [-h, h, -h],
                [-h, -h, h],
                [h, -h, h],
                [h, h, h],
                [-h, h, h],
            ],
            triangles: vec![
                [0, 2, 1],
                [0, 3, 2],
                [4, 5, 6],
                [4, 6, 7],
                [0, 1, 5],
                [0, 5, 4],
                [2, 3, 7],
                [2, 7, 6],
                [1, 2, 6],
                [1, 6, 5],
                [3, 0, 4],
                [3, 4, 7],
            ],
        }
    }

    fn cube_at(id: u32, h: f64, pos: [f64; 3]) -> Instance {
        let mut instance = Instance::new(InstanceId(id), format!("cube_{id}"), cube(h));
        instance.translation = pos;
        instance
    }

    fn revolute_z() -> Joint {
        Joint::Revolute {
            axis_origin: [0.0, 0.0, 0.0],
            axis_dir: [0.0, 0.0, 1.0],
        }
    }

    // Part 1 swings on a radius-10 circle about z (base at [10,0,0]).
    fn swinging_assembly(neighbor_pos: [f64; 3]) -> Assembly {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(cube_at(0, 1.0, [0.0, 0.0, 0.0])); // hub at the centre
        assembly.add_instance(cube_at(1, 1.0, [10.0, 0.0, 0.0])); // the swinging arm
        assembly.add_instance(cube_at(2, 1.0, neighbor_pos)); // the neighbour
        assembly
    }

    #[test]
    fn sweep_clear_of_a_distant_neighbor() {
        // Neighbour parked at radius 30 — the radius-10 swing never reaches it.
        let assembly = swinging_assembly([30.0, 0.0, 0.0]);
        let sc = swept_clearance(
            &assembly,
            InstanceId(1),
            &revolute_z(),
            &[10.0, 0.0, 0.0],
            &[0.0, 0.0, 0.0, 1.0],
            (0.0, 2.0 * PI),
            73,
            0.01,
        );
        assert!(sc.raw_min_clearance > 0.0);
        assert!(
            !sc.collides,
            "swing radius 10 cannot reach a part at radius 30"
        );
    }

    #[test]
    fn sweep_through_a_neighbor_collides() {
        // Neighbour sits ON the swing circle at [0,10,0]; the arm passes through
        // it near θ = 90°.
        let assembly = swinging_assembly([0.0, 10.0, 0.0]);
        let sc = swept_clearance(
            &assembly,
            InstanceId(1),
            &revolute_z(),
            &[10.0, 0.0, 0.0],
            &[0.0, 0.0, 0.0, 1.0],
            (0.0, 2.0 * PI),
            73,
            0.0,
        );
        assert!(sc.collides, "the swing arc passes through the neighbour");
        assert!(sc.raw_min_clearance <= 1e-9, "overlap reads ~0 distance");
    }

    #[test]
    fn epsilon_bound_is_conservative() {
        // Same clear sweep, but a 2.0 tessellation bound must shrink the certified
        // clearance below the raw distance.
        let assembly = swinging_assembly([30.0, 0.0, 0.0]);
        let sc = swept_clearance(
            &assembly,
            InstanceId(1),
            &revolute_z(),
            &[10.0, 0.0, 0.0],
            &[0.0, 0.0, 0.0, 1.0],
            (0.0, 2.0 * PI),
            73,
            2.0,
        );
        assert!(
            sc.min_clearance < sc.raw_min_clearance,
            "epsilon must make the certificate conservative"
        );
        assert!((sc.raw_min_clearance - sc.min_clearance - 2.0).abs() < 1e-9);
        assert!(!sc.collides, "still clear after the 2.0 bound");
    }

    #[test]
    fn screw_between_reproduces_both_endpoints() {
        // The property the whole TOI gate rests on: the reconstructed screw
        // must hit `from` at t=0 and `to` at t=1 EXACTLY.
        let cases = [
            (
                Isometry3::from_parts(
                    Translation3::new(10.0, 0.0, 0.0),
                    UnitQuaternion::identity(),
                ),
                Isometry3::from_parts(
                    Translation3::new(0.0, 10.0, 0.0),
                    UnitQuaternion::from_scaled_axis(Vector3::z() * (PI / 2.0)),
                ),
            ),
            (
                Isometry3::from_parts(
                    Translation3::new(1.0, 2.0, 3.0),
                    UnitQuaternion::from_scaled_axis(Vector3::new(0.3, -0.2, 0.1)),
                ),
                Isometry3::from_parts(
                    Translation3::new(-4.0, 0.5, 9.0),
                    UnitQuaternion::from_scaled_axis(Vector3::new(-0.1, 0.7, 0.25)),
                ),
            ),
            // Pure translation — the no-axis branch.
            (
                Isometry3::from_parts(Translation3::new(0.0, 0.0, 0.0), UnitQuaternion::identity()),
                Isometry3::from_parts(Translation3::new(2.0, 3.0, 4.0), UnitQuaternion::identity()),
            ),
        ];
        for (from, to) in cases {
            let motion = screw_between(&from, &to);
            let at0 = motion.position_at_time(0.0);
            let at1 = motion.position_at_time(1.0);
            assert!(
                (at0.translation.vector - from.translation.vector).norm() < 1e-9
                    && at0.rotation.angle_to(&from.rotation) < 1e-9,
                "t=0 must reproduce `from`: {at0:?} vs {from:?}"
            );
            assert!(
                (at1.translation.vector - to.translation.vector).norm() < 1e-9
                    && at1.rotation.angle_to(&to.rotation) < 1e-9,
                "t=1 must reproduce `to`: {at1:?} vs {to:?}"
            );
        }
    }

    #[test]
    fn screw_of_a_revolute_traces_the_arc_not_the_chord() {
        // The load-bearing property: a part swinging on a radius-10 circle
        // stays ON the circle at every intermediate time. A naive
        // rotate-about-own-origin + linear-translation motion would cut the
        // chord and under-sweep the very region the gate exists to check.
        let angle = 0.6_f64;
        let from = Isometry3::from_parts(
            Translation3::new(10.0, 0.0, 0.0),
            UnitQuaternion::identity(),
        );
        let rot = UnitQuaternion::from_scaled_axis(Vector3::z() * angle);
        let to = Isometry3::from_parts(Translation3::from(rot * Vector3::new(10.0, 0.0, 0.0)), rot);
        let motion = screw_between(&from, &to);
        for k in 0..=10 {
            let t = f64::from(k) / 10.0;
            let radius = motion.position_at_time(t).translation.vector.norm();
            assert!(
                (radius - 10.0).abs() < 1e-9,
                "t={t}: the screw left the arc (radius {radius})"
            );
        }
    }

    /// The `u32::try_from` branch in `derived_sweeps` is not reachable
    /// through the assembly (it needs four billion mates), so the honest
    /// gate is on the helper the loop actually calls. `usize` is wider
    /// than `u32` only on 64-bit targets, which is where this can arise.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn a_mate_this_wire_cannot_name_still_leaves_a_fact() {
        let too_wide = u32::MAX as usize + 1;
        let Err(fact) = nameable_index(too_wide, 0.01) else {
            assert!(false, "an index above u32::MAX cannot be named");
            return;
        };
        let fact = *fact;
        assert_eq!(
            fact.source,
            SweepSource::UnnameableMate {
                mate_index: too_wide
            }
        );
        assert_eq!(
            fact.refusal,
            Some(SweepRefusal::MateIndexUnrepresentable {
                mate_index: too_wide
            })
        );
        assert!(!fact.clear, "an unnamed mate was not swept: {fact:?}");
        assert_eq!(
            fact.method,
            SweepMethod::NotRun,
            "and no method ran on it: {fact:?}"
        );
        // Both new fields are `usize`, which is the one width on this wire
        // that is not a `u32` — measure it rather than assume it.
        let json = serde_json::to_string(&fact);
        let Ok(json) = json else {
            assert!(false, "the fact serialises: {json:?}");
            return;
        };
        let parsed: Result<SweptFact, _> = serde_json::from_str(&json);
        assert_eq!(
            parsed.ok(),
            Some(fact),
            "and an unnameable mate's fact crosses the wire intact"
        );
        // The representable case is untouched.
        assert_eq!(nameable_index(7, 0.01).ok(), Some(7));
    }
}
