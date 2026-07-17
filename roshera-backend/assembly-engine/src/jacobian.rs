//! Analytic constraint Jacobians over se(3) — kinematic-assembly
//! campaign, Slice 3 (spec §3.4).
//!
//! Every residual row in the mate taxonomy (`mate_residual.rs`) gets a
//! closed-form derivative with respect to each involved instance's 6-DOF
//! pose tangent. The derivation is standard screw calculus (Murray, Li &
//! Sastry, *A Mathematical Introduction to Robotic Manipulation*, ch. 2;
//! Solà, Deray & Atchuthan, *A micro Lie theory for state estimation in
//! robotics*, arXiv:1812.01537 §7): under the solver's tangent step —
//! translation `t += v`, rotation `R ← exp(ω̂)·R` about the block PIVOT —
//! a world point `p` carried by the body varies as
//!
//! ```text
//!   δp = v + ω × (p − pivot)
//! ```
//!
//! and a world direction `u` carried by the body varies as `δu = ω × u`.
//! (Directions normalised by `to_world` stay exact: a pose perturbation
//! is a rotation, which preserves the pre-normalisation length, so the
//! normalisation factor is constant to first order.) Every mate row is
//! built from four primitive forms:
//!
//! * point difference   `d = p_b − p_a`             (3 rows)
//! * direction diff/sum `u_b ∓ u_a`                 (3 rows)
//! * projected scalar   `d·u`, `u` rigid on side a  (1 row)
//! * rejection          `d − u(d·u)`                (3 rows)
//!
//! plus the coupling rows, which differentiate the joint parameters
//! `θ = atan2((x_a×x_b)·z_a, x_a·x_b)` and `s = (o_b−o_a)·z_a` of the
//! coupled frame-pair mates via scalar triple products.
//!
//! **Non-smooth row (honest).** `Tangent`'s residual `|d·z_a| − r` is
//! non-differentiable exactly at the plane crossing `d·z_a = 0`. The
//! analytic row takes the CENTRAL subgradient `σ(0) = 0` — the same
//! value central differencing measures there — so the FD gate holds even
//! at the crossing; away from it `σ = sign(d·z_a)` and the row is exact
//! (slice-1/2 report, residual note #4).
//!
//! **Central differences are RETAINED** ([`fd_jacobian`]) as the debug
//! oracle: `tests/jacobian_gate.rs` pins entrywise agreement ≤ 1e-6
//! across the whole taxonomy at generic configurations.
//!
//! # Rigid blocks
//!
//! Columns are per [`BodyBlock`] — a set of instances moving as ONE
//! rigid body (Slice-3 fastened condensation). The block's rotation
//! pivot is its first member's current translation, so a singleton
//! block reproduces `apply_tangent_step` exactly (translation unchanged
//! by ω) and the dense path is the special case of one singleton block
//! per non-ground instance.

// Reason for the module-wide indexing allow: every matrix/vector index
// in this module is bounded by the owning loop (`0..3` on fixed-size
// Matrix3/Vector3, `0..nrows()/ncols()` on DMatrix, `k < 6` on tangent
// steps) — in-bounds by construction. A `.get()` fallback would invent
// behaviour for states the loops cannot produce (workspace convention
// for invariant-guarded escapes).
#![allow(clippy::indexing_slicing)]

use crate::motion::DriveParam;
use crate::types::{Assembly, FeatureRef, Mate, MateKind};
use parry3d_f64::na::{DMatrix, Matrix3, Point3, Quaternion, UnitQuaternion, Vector3};

/// A set of instance indices (into `Assembly::instances`) moving as one
/// rigid body. `members[0]` is the representative: its translation is
/// the block's rotation pivot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BodyBlock {
    pub members: Vec<usize>,
}

impl BodyBlock {
    pub fn singleton(idx: usize) -> Self {
        Self { members: vec![idx] }
    }

    /// The block's rotation pivot: the representative's CURRENT translation.
    pub fn pivot(&self, assembly: &Assembly) -> Vector3<f64> {
        self.members
            .first()
            .and_then(|&idx| assembly.instances.get(idx))
            .map(|inst| {
                Vector3::new(
                    inst.translation[0],
                    inst.translation[1],
                    inst.translation[2],
                )
            })
            .unwrap_or_else(Vector3::zeros)
    }
}

/// One singleton block per non-ground instance, in instance-vector order —
/// the dense column layout (byte-compatible with the pre-Slice-3 solver's
/// column ordering).
pub(crate) fn singleton_blocks(assembly: &Assembly) -> Vec<BodyBlock> {
    assembly
        .instances
        .iter()
        .enumerate()
        .filter(|(_, inst)| inst.id != assembly.ground)
        .map(|(idx, _)| BodyBlock::singleton(idx))
        .collect()
}

/// Apply a 6-DOF tangent step to a whole block IN PLACE: every member
/// translates by `v` and rotates by `exp(ω̂)` about the block pivot, so
/// the members' RELATIVE poses are exactly preserved. For a singleton
/// this is precisely the solver's historic `apply_tangent_step`.
pub(crate) fn apply_block_step(assembly: &mut Assembly, block: &BodyBlock, step: &[f64; 6]) {
    let pivot = block.pivot(assembly);
    let v = Vector3::new(step[0], step[1], step[2]);
    let delta = UnitQuaternion::from_scaled_axis(Vector3::new(step[3], step[4], step[5]));
    for &idx in &block.members {
        if let Some(instance) = assembly.instances.get_mut(idx) {
            let t = Vector3::new(
                instance.translation[0],
                instance.translation[1],
                instance.translation[2],
            );
            let rotated = pivot + delta * (t - pivot) + v;
            instance.translation = [rotated.x, rotated.y, rotated.z];
            let current = UnitQuaternion::from_quaternion(Quaternion::new(
                instance.rotation[3],
                instance.rotation[0],
                instance.rotation[1],
                instance.rotation[2],
            ));
            let updated = (delta * current).quaternion().to_owned();
            instance.rotation = [updated.i, updated.j, updated.k, updated.w];
        }
    }
}

/// Column layout: for each instance index, the column base of its block
/// and the block pivot (`None` = no columns — ground / frozen).
pub(crate) struct ColumnLayout {
    entry: Vec<Option<(usize, Vector3<f64>)>>,
    pub cols: usize,
}

impl ColumnLayout {
    pub fn build(assembly: &Assembly, blocks: &[BodyBlock]) -> Self {
        let mut entry: Vec<Option<(usize, Vector3<f64>)>> = vec![None; assembly.instances.len()];
        for (block_idx, block) in blocks.iter().enumerate() {
            let pivot = block.pivot(assembly);
            for &member in &block.members {
                if let Some(slot) = entry.get_mut(member) {
                    *slot = Some((6 * block_idx, pivot));
                }
            }
        }
        Self {
            entry,
            cols: 6 * blocks.len(),
        }
    }

    fn of(&self, instance_idx: usize) -> Option<(usize, Vector3<f64>)> {
        self.entry.get(instance_idx).copied().flatten()
    }
}

/// The stacked residual over a mate subset (in subset order) — the
/// row-order contract shared with [`analytic_jacobian`]/[`fd_jacobian`].
pub(crate) fn residual_for(assembly: &Assembly, mate_indices: &[usize]) -> Vec<f64> {
    residual_for_driven(assembly, mate_indices, &[])
}

/// The stacked residual over a mate subset FOLLOWED BY the driven-joint
/// rows (Slice 5, spec §3.4 "Driven vs driving"): a driven parameter is
/// not a special solver mode, it is one more residual row the joint
/// contributes — `param(q) − target = 0`. Row order (mates in subset
/// order, then drives in slice order) is the contract shared with
/// [`analytic_jacobian_driven`].
pub(crate) fn residual_for_driven(
    assembly: &Assembly,
    mate_indices: &[usize],
    drives: &[DriveRow],
) -> Vec<f64> {
    let mut g = Vec::new();
    for &mi in mate_indices {
        if let Some(mate) = assembly.mates.get(mi) {
            g.extend(assembly.mate_residual(mate));
        }
    }
    for drive in drives {
        g.extend(drive_residual(assembly, drive));
    }
    g
}

/// One driven joint parameter: the residual row `param(q) − target = 0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DriveRow {
    /// Index into `Assembly::mates` of the frame-pair mate being driven.
    pub mate_index: u32,
    pub param: DriveParam,
    /// The value the parameter is locked to. For [`DriveParam::Rotation`]
    /// this is an UNWRAPPED angle; the residual compares it against the
    /// wrapped measurement through [`wrap_to_pi`], which is exact as long
    /// as the caller steps by less than half a turn at a time (the
    /// contract [`crate::motion::MAX_DRIVE_STEP`] enforces).
    pub target: f64,
}

/// Wrap an angle into (−π, π]. The drive residual compares angles through
/// this so that "θ is at the target" means θ ≡ target (mod 2π) — the only
/// statement a pose can support — while staying smooth in the pose (wrap
/// is locally the identity away from the ±π seam, and a drive step never
/// reaches the seam because it is bounded below half a turn).
pub(crate) fn wrap_to_pi(angle: f64) -> f64 {
    use std::f64::consts::{PI, TAU};
    let wrapped = angle % TAU;
    if wrapped > PI {
        wrapped - TAU
    } else if wrapped <= -PI {
        wrapped + TAU
    } else {
        wrapped
    }
}

/// The drive's residual rows — one row, or NONE when the target mate has
/// no readable frame pair (kept exactly in step with
/// [`drive_row_grads`], so residual and Jacobian rows never desynchronise).
fn drive_residual(assembly: &Assembly, drive: &DriveRow) -> Vec<f64> {
    let Some((theta, s)) = assembly.joint_parameters_of(drive.mate_index) else {
        return Vec::new();
    };
    match drive.param {
        DriveParam::Rotation => vec![wrap_to_pi(theta - drive.target)],
        DriveParam::Translation => vec![s - drive.target],
    }
}

/// The drive's row gradients. `∂/∂q wrap(θ − target) = ∂θ/∂q` (wrap is
/// locally the identity) and `∂/∂q (s − target) = ∂s/∂q` — exactly the
/// joint-parameter gradients the coupling rows already differentiate.
fn drive_row_grads(assembly: &Assembly, drive: &DriveRow) -> Vec<RowGrad> {
    let Some((theta, slide)) = joint_parameter_grads(assembly, drive.mate_index) else {
        return Vec::new();
    };
    if assembly.joint_parameters_of(drive.mate_index).is_none() {
        return Vec::new();
    }
    match drive.param {
        DriveParam::Rotation => vec![theta],
        DriveParam::Translation => vec![slide],
    }
}

// ── Gradient accumulation ───────────────────────────────────────────────

/// One residual row's gradient: contributions per instance index, each a
/// 6-vector `(∂/∂v, ∂/∂ω)`. Contributions to the same instance are SUMMED
/// (a coupling may touch one instance through both coupled mates).
type RowGrad = Vec<(usize, [f64; 6])>;

fn skew(u: &Vector3<f64>) -> Matrix3<f64> {
    Matrix3::new(0.0, -u.z, u.y, u.z, 0.0, -u.x, -u.y, u.x, 0.0)
}

fn grad6(v: Vector3<f64>, w: Vector3<f64>) -> [f64; 6] {
    [v.x, v.y, v.z, w.x, w.y, w.z]
}

/// Row `r` (0..3) of a 3x3 matrix as a vector.
fn row3(m: &Matrix3<f64>, r: usize) -> Vector3<f64> {
    Vector3::new(m[(r, 0)], m[(r, 1)], m[(r, 2)])
}

/// World features + owning instance data for one side of a mate.
struct Side {
    /// Instance index into `Assembly::instances`.
    idx: usize,
    /// World origin/point of the feature.
    p: Point3<f64>,
    /// World primary direction (frame z / axis dir / face normal), unit.
    z: Vector3<f64>,
    /// World secondary direction (frame x), unit; zero for non-frames.
    x: Vector3<f64>,
    /// Instance translation (the dense pivot; see row derivations —
    /// every lever arm is taken about the INSTANCE translation and the
    /// column layout re-bases it onto the block pivot).
    t: Vector3<f64>,
}

/// Re-base a gradient taken about the instance's own translation onto
/// the block pivot. A world point's variation about pivot `q` is
/// `v + ω×(p − q)`; about the instance translation `t` it is
/// `v + ω×(p − t)`. The two differ by `ω×(t − q)`, which is exactly what
/// a lever-arm re-basing of the TRANSLATION gradient absorbs:
/// `∂/∂ω |_q = ∂/∂ω |_t + (t − q) × ∂/∂v` (since `δp` gains
/// `ω×(t − q)`, every row linear in `δp` gains `∂row/∂p · (ω×(t−q))
/// = (∂/∂v)·(ω×(t−q)) = ω·((t−q)×∂/∂v)`).
fn rebase(grad: [f64; 6], t: &Vector3<f64>, pivot: &Vector3<f64>) -> [f64; 6] {
    let dv = Vector3::new(grad[0], grad[1], grad[2]);
    let dw = Vector3::new(grad[3], grad[4], grad[5]) + (t - pivot).cross(&dv);
    grad6(dv, dw)
}

/// Unit-normalize, or zero when degenerate — the same contract as
/// `mate_residual::unit` (a zero feature direction contributes constant
/// zero residual components, hence zero gradient).
fn unit(v: Vector3<f64>) -> Vector3<f64> {
    v.try_normalize(1e-12).unwrap_or_else(Vector3::zeros)
}

fn side_of(
    assembly: &Assembly,
    instance_id: crate::types::InstanceId,
    f: &FeatureRef,
) -> Option<Side> {
    let idx = assembly
        .instances
        .iter()
        .position(|i| i.id == instance_id)?;
    let inst = assembly.instances.get(idx)?;
    let iso = crate::interference::instance_isometry(inst);
    let t = Vector3::new(
        inst.translation[0],
        inst.translation[1],
        inst.translation[2],
    );
    let (p, z, x) = match f {
        FeatureRef::Face { point, normal } => (
            iso.transform_point(&Point3::new(point[0], point[1], point[2])),
            unit(iso.transform_vector(&Vector3::new(normal[0], normal[1], normal[2]))),
            Vector3::zeros(),
        ),
        FeatureRef::Axis { origin, direction } => (
            iso.transform_point(&Point3::new(origin[0], origin[1], origin[2])),
            unit(iso.transform_vector(&Vector3::new(direction[0], direction[1], direction[2]))),
            Vector3::zeros(),
        ),
        FeatureRef::Frame {
            origin,
            z_axis,
            x_axis,
        } => (
            iso.transform_point(&Point3::new(origin[0], origin[1], origin[2])),
            unit(iso.transform_vector(&Vector3::new(z_axis[0], z_axis[1], z_axis[2]))),
            unit(iso.transform_vector(&Vector3::new(x_axis[0], x_axis[1], x_axis[2]))),
        ),
    };
    Some(Side { idx, p, z, x, t })
}

/// Push the 3 rows of a point-difference block `d = p_b − p_a`.
fn rows_point_diff(rows: &mut Vec<RowGrad>, a: &Side, b: &Side) {
    let ra = a.p.coords - a.t;
    let rb = b.p.coords - b.t;
    let ska = skew(&ra);
    let skb = skew(&rb);
    for r in 0..3 {
        let mut ev = Vector3::zeros();
        ev[r] = 1.0;
        // δd_r = (v_b + ω_b×r_b − v_a − ω_a×r_a)·e_r
        //      = e_r·v_b − e_r·v_a + (r_b×e_r)·ω_b − (r_a×e_r)·ω_a
        // (row extraction of ±skew(r): (skew(r)ω)_r = (r×ω)·e_r = ω·(e_r×r)
        //  — implemented via the transposed skew rows below.)
        let wa = row3(&ska, r);
        let wb = row3(&skb, r);
        rows.push(vec![(a.idx, grad6(-ev, wa)), (b.idx, grad6(ev, -wb))]);
    }
}

/// Push the 3 rows of a direction combination `sign_b·u_b + sign_a·u_a`
/// (`align = z_b − z_a` uses (+1, −1); `flush = n_a + n_b` uses (+1, +1)).
/// δu = ω×u = −skew(u)·ω per side.
fn rows_dir_combo(
    rows: &mut Vec<RowGrad>,
    a: &Side,
    ua: &Vector3<f64>,
    sign_a: f64,
    b: &Side,
    ub: &Vector3<f64>,
    sign_b: f64,
) {
    let ska = skew(ua) * (-sign_a);
    let skb = skew(ub) * (-sign_b);
    for r in 0..3 {
        rows.push(vec![
            (a.idx, grad6(Vector3::zeros(), row3(&ska, r))),
            (b.idx, grad6(Vector3::zeros(), row3(&skb, r))),
        ]);
    }
}

/// Gradient of the projected scalar `s = (p_b − p_a)·u`, `u` rigid on a,
/// scaled by `scale`. Returns the two contributions (a-side, b-side).
fn scalar_projection_grads(a: &Side, b: &Side, u: &Vector3<f64>, scale: f64) -> RowGrad {
    let d = b.p - a.p;
    let ra = a.p.coords - a.t;
    let rb = b.p.coords - b.t;
    // δs = u·δd + d·δu ;  δd|a = −(v_a + ω_a×r_a), δd|b = v_b + ω_b×r_b,
    // δu = ω_a×u.  ⇒ ∂/∂ω_a = u×d − r_a×u,  ∂/∂ω_b = r_b×u.
    let ga = grad6((-u) * scale, (u.cross(&d) - ra.cross(u)) * scale);
    let gb = grad6(u * scale, rb.cross(u) * scale);
    vec![(a.idx, ga), (b.idx, gb)]
}

/// Push the 3 rows of the rejection `d − u(d·u)`, `u` rigid on a.
fn rows_rejection(rows: &mut Vec<RowGrad>, a: &Side, b: &Side, u: &Vector3<f64>) {
    let d = b.p - a.p;
    let ra = a.p.coords - a.t;
    let rb = b.p.coords - b.t;
    let proj = Matrix3::identity() - u * u.transpose();
    // δperp = P·δd − [(d·u)I + u dᵀ]·δu, δu = −skew(u)·ω_a.
    let m_va = -proj; // wrt v_a
    let m_wa = proj * skew(&ra) + (Matrix3::identity() * d.dot(u) + u * d.transpose()) * skew(u);
    let m_vb = proj;
    let m_wb = -(proj * skew(&rb));
    for r in 0..3 {
        rows.push(vec![
            (a.idx, grad6(row3(&m_va, r), row3(&m_wa, r))),
            (b.idx, grad6(row3(&m_vb, r), row3(&m_wb, r))),
        ]);
    }
}

/// Push the 3 rows of the cross product `c = u_a × u_b` (directions
/// rigid on their sides): ∂c/∂ω_a = skew(u_b)·skew(u_a),
/// ∂c/∂ω_b = −skew(u_a)·skew(u_b).
fn rows_cross(rows: &mut Vec<RowGrad>, a: &Side, ua: &Vector3<f64>, b: &Side, ub: &Vector3<f64>) {
    let m_wa = skew(ub) * skew(ua);
    let m_wb = -(skew(ua) * skew(ub));
    for r in 0..3 {
        rows.push(vec![
            (a.idx, grad6(Vector3::zeros(), row3(&m_wa, r))),
            (b.idx, grad6(Vector3::zeros(), row3(&m_wb, r))),
        ]);
    }
}

/// Gradients of the joint parameters (θ, s) of frame-pair mate `index`
/// (the quantities the coupling kinds relate). Returns `None` when the
/// target has no frame pair (matching `joint_parameters_of`).
fn joint_parameter_grads(assembly: &Assembly, index: u32) -> Option<(RowGrad, RowGrad)> {
    let mate = assembly.mates.get(index as usize)?;
    let (FeatureRef::Frame { .. }, FeatureRef::Frame { .. }) = (&mate.feature_a, &mate.feature_b)
    else {
        return None;
    };
    let a = side_of(assembly, mate.a, &mate.feature_a)?;
    let b = side_of(assembly, mate.b, &mate.feature_b)?;
    let (xa, xb, za) = (a.x, b.x, a.z);
    // θ = atan2(S, C), C = x_a·x_b, S = (x_a×x_b)·z_a.
    let c = xa.dot(&xb);
    let s = xa.cross(&xb).dot(&za);
    let denom = c * c + s * s;
    let theta = if denom < 1e-12 {
        // Degenerate (x_b parallel to z_a): θ undefined — zero gradient,
        // matching the vanishing sensitivity of atan2 at a singular pair.
        vec![(a.idx, [0.0; 6]), (b.idx, [0.0; 6])]
    } else {
        // δC = ω_a·(x_a×x_b) + ω_b·(x_b×x_a)
        // δS = ω_a·(x_a×(x_b×z_a) + z_a×(x_a×x_b)) + ω_b·(x_b×(z_a×x_a))
        let dc_a = xa.cross(&xb);
        let dc_b = xb.cross(&xa);
        let ds_a = xa.cross(&xb.cross(&za)) + za.cross(&xa.cross(&xb));
        let ds_b = xb.cross(&za.cross(&xa));
        let gtheta_a = (ds_a * c - dc_a * s) / denom;
        let gtheta_b = (ds_b * c - dc_b * s) / denom;
        vec![
            (a.idx, grad6(Vector3::zeros(), gtheta_a)),
            (b.idx, grad6(Vector3::zeros(), gtheta_b)),
        ]
    };
    // s = (o_b − o_a)·z_a — the projected scalar form.
    let slide = scalar_projection_grads(&a, &b, &za, 1.0);
    Some((theta, slide))
}

/// Scale every contribution of a row gradient.
fn scaled(row: &RowGrad, k: f64) -> RowGrad {
    row.iter()
        .map(|(idx, g)| {
            (
                *idx,
                [g[0] * k, g[1] * k, g[2] * k, g[3] * k, g[4] * k, g[5] * k],
            )
        })
        .collect()
}

/// Merge two row gradients (contributions to the same instance summed by
/// the matrix-assembly accumulation, so plain concatenation suffices).
fn merged(mut a: RowGrad, b: RowGrad) -> RowGrad {
    a.extend(b);
    a
}

/// The analytic row gradients of ONE mate, in exactly the row order of
/// `Assembly::mate_residual`. An empty vec ⇔ an empty residual (refused
/// kind, feature mismatch, unknown instance, broken coupling).
fn mate_row_grads(assembly: &Assembly, mate: &Mate) -> Vec<RowGrad> {
    if !mate.kind.is_numerically_enforced() {
        return Vec::new();
    }
    // Coupling kinds relate the COUPLED mates' joint parameters.
    match mate.kind {
        MateKind::GearRatio { ratio, couples, .. } => {
            let (Some((t1, _)), Some((t2, _))) = (
                joint_parameter_grads(assembly, couples[0]),
                joint_parameter_grads(assembly, couples[1]),
            ) else {
                return Vec::new();
            };
            // Residual guards on joint_parameters_of, which additionally
            // requires resolvable instances; grads use the same lookups.
            if assembly.joint_parameters_of(couples[0]).is_none()
                || assembly.joint_parameters_of(couples[1]).is_none()
            {
                return Vec::new();
            }
            return vec![merged(scaled(&t1, ratio), t2)];
        }
        MateKind::RackPinion {
            pinion_radius,
            couples,
            ..
        } => {
            let (Some((t1, _)), Some((_, s2))) = (
                joint_parameter_grads(assembly, couples[0]),
                joint_parameter_grads(assembly, couples[1]),
            ) else {
                return Vec::new();
            };
            if assembly.joint_parameters_of(couples[0]).is_none()
                || assembly.joint_parameters_of(couples[1]).is_none()
            {
                return Vec::new();
            }
            return vec![merged(scaled(&t1, pinion_radius), scaled(&s2, -1.0))];
        }
        MateKind::Screw { lead, couples, .. } => {
            let Some((theta, s)) = joint_parameter_grads(assembly, couples) else {
                return Vec::new();
            };
            if assembly.joint_parameters_of(couples).is_none() {
                return Vec::new();
            }
            return vec![merged(s, scaled(&theta, -lead / std::f64::consts::TAU))];
        }
        _ => {}
    }

    let (Some(a), Some(b)) = (
        side_of(assembly, mate.a, &mate.feature_a),
        side_of(assembly, mate.b, &mate.feature_b),
    ) else {
        return Vec::new();
    };

    let mut rows: Vec<RowGrad> = Vec::new();
    match (&mate.kind, &mate.feature_a, &mate.feature_b) {
        (MateKind::Concentric, FeatureRef::Axis { .. }, FeatureRef::Axis { .. }) => {
            // cross = d_a × d_b (3) ; perp = δ − d_a(δ·d_a) (3)
            rows_cross(&mut rows, &a, &a.z, &b, &b.z);
            rows_rejection(&mut rows, &a, &b, &a.z);
        }
        (MateKind::Coincident, FeatureRef::Face { .. }, FeatureRef::Face { .. }) => {
            // dist = (p_b − p_a)·n_a (1) ; cross = n_a × n_b (3)
            rows.push(scalar_projection_grads(&a, &b, &a.z, 1.0));
            rows_cross(&mut rows, &a, &a.z, &b, &b.z);
        }
        (
            MateKind::Fixed,
            FeatureRef::Face {
                point: pa,
                normal: na,
            },
            FeatureRef::Face {
                point: pb,
                normal: nb,
            },
        ) => {
            let _ = (pa, pb);
            // d (3) ; flush = n_a + n_b (3) ; spin = t_b − t_a (3), where
            // the tangents are the rigidly-carried local_tangent images —
            // rigid a-/b-directions, so the direction-combo form applies.
            rows_point_diff(&mut rows, &a, &b);
            rows_dir_combo(&mut rows, &a, &a.z, 1.0, &b, &b.z, 1.0);
            let ta = fixed_world_tangent(assembly, mate.a, na);
            let tb = fixed_world_tangent(assembly, mate.b, nb);
            rows_dir_combo(&mut rows, &a, &ta, -1.0, &b, &tb, 1.0);
        }
        // Frame-pair joint kinds + overlays.
        (kind, FeatureRef::Frame { .. }, FeatureRef::Frame { .. }) => {
            let d = b.p - a.p;
            match kind {
                MateKind::Fastened => {
                    rows_point_diff(&mut rows, &a, &b);
                    rows_dir_combo(&mut rows, &a, &a.z, -1.0, &b, &b.z, 1.0);
                    rows_dir_combo(&mut rows, &a, &a.x, -1.0, &b, &b.x, 1.0);
                }
                MateKind::Revolute { .. } => {
                    rows_point_diff(&mut rows, &a, &b);
                    rows_dir_combo(&mut rows, &a, &a.z, -1.0, &b, &b.z, 1.0);
                }
                MateKind::Slider { .. } => {
                    rows_dir_combo(&mut rows, &a, &a.z, -1.0, &b, &b.z, 1.0);
                    rows_dir_combo(&mut rows, &a, &a.x, -1.0, &b, &b.x, 1.0);
                    rows_rejection(&mut rows, &a, &b, &a.z);
                }
                MateKind::Cylindrical { .. } => {
                    rows_dir_combo(&mut rows, &a, &a.z, -1.0, &b, &b.z, 1.0);
                    rows_rejection(&mut rows, &a, &b, &a.z);
                }
                MateKind::Planar => {
                    rows_dir_combo(&mut rows, &a, &a.z, -1.0, &b, &b.z, 1.0);
                    rows.push(scalar_projection_grads(&a, &b, &a.z, 1.0));
                }
                MateKind::Ball => {
                    rows_point_diff(&mut rows, &a, &b);
                }
                MateKind::PinSlot { slot_dir_x, .. } => {
                    rows_dir_combo(&mut rows, &a, &a.z, -1.0, &b, &b.z, 1.0);
                    let s = if *slot_dir_x { a.x } else { a.z.cross(&a.x) };
                    rows_rejection(&mut rows, &a, &b, &s);
                }
                MateKind::Distance { .. } => {
                    rows.push(scalar_projection_grads(&a, &b, &a.z, 1.0));
                }
                MateKind::Angle { .. } => {
                    // δ(z_a·z_b): ∂/∂ω_a = z_a×z_b, ∂/∂ω_b = z_b×z_a.
                    rows.push(vec![
                        (a.idx, grad6(Vector3::zeros(), a.z.cross(&b.z))),
                        (b.idx, grad6(Vector3::zeros(), b.z.cross(&a.z))),
                    ]);
                }
                MateKind::Parallel => {
                    rows_cross(&mut rows, &a, &a.z, &b, &b.z);
                }
                MateKind::Tangent { .. } => {
                    // |d·z_a| − r : central subgradient σ(0) = 0 (module doc).
                    let sd = d.dot(&a.z);
                    let sigma = if sd == 0.0 { 0.0 } else { sd.signum() };
                    rows.push(scalar_projection_grads(&a, &b, &a.z, sigma));
                }
                _ => {}
            }
        }
        _ => {}
    }
    rows
}

/// World image of `mate_residual::local_tangent(normal)` under the
/// instance's CURRENT pose — the rigidly-carried spin-lock tangent of a
/// `Fixed` mate side.
fn fixed_world_tangent(
    assembly: &Assembly,
    id: crate::types::InstanceId,
    local_normal: &[f64; 3],
) -> Vector3<f64> {
    let n = unit(Vector3::new(
        local_normal[0],
        local_normal[1],
        local_normal[2],
    ));
    let axes = [Vector3::x(), Vector3::y(), Vector3::z()];
    let mut best = axes[0];
    let mut best_dot = f64::INFINITY;
    for e in axes {
        let dcomp = n.dot(&e).abs();
        if dcomp < best_dot {
            best_dot = dcomp;
            best = e;
        }
    }
    let local = unit(best - n * best.dot(&n));
    assembly
        .instance(id)
        .map(|inst| crate::interference::instance_isometry(inst).transform_vector(&local))
        .unwrap_or_else(Vector3::zeros)
}

/// The analytic constraint Jacobian over `mate_indices` (rows, in subset
/// order) and `layout` columns. Row count matches
/// [`residual_for`] exactly; instances without columns (ground / frozen)
/// contribute nothing.
pub(crate) fn analytic_jacobian(
    assembly: &Assembly,
    layout: &ColumnLayout,
    mate_indices: &[usize],
) -> DMatrix<f64> {
    analytic_jacobian_driven(assembly, layout, mate_indices, &[])
}

/// The analytic Jacobian over `mate_indices` FOLLOWED BY the driven-joint
/// rows — the row-order contract of [`residual_for_driven`]. With an empty
/// `drives` this is bit-for-bit [`analytic_jacobian`], so every pre-Slice-5
/// path is unchanged by construction.
pub(crate) fn analytic_jacobian_driven(
    assembly: &Assembly,
    layout: &ColumnLayout,
    mate_indices: &[usize],
    drives: &[DriveRow],
) -> DMatrix<f64> {
    let mut all_rows: Vec<RowGrad> = Vec::new();
    for &mi in mate_indices {
        if let Some(mate) = assembly.mates.get(mi) {
            all_rows.extend(mate_row_grads(assembly, mate));
        }
    }
    for drive in drives {
        all_rows.extend(drive_row_grads(assembly, drive));
    }
    let mut jac = DMatrix::<f64>::zeros(all_rows.len(), layout.cols);
    for (r, row) in all_rows.iter().enumerate() {
        for (instance_idx, grad) in row {
            let Some((col_base, pivot)) = layout.of(*instance_idx) else {
                continue;
            };
            let t = assembly
                .instances
                .get(*instance_idx)
                .map(|i| Vector3::new(i.translation[0], i.translation[1], i.translation[2]))
                .unwrap_or_else(Vector3::zeros);
            let g = rebase(*grad, &t, &pivot);
            for (k, gk) in g.iter().enumerate() {
                jac[(r, col_base + k)] += gk;
            }
        }
    }
    jac
}

/// Central-difference Jacobian over block steps — the DEBUG ORACLE the
/// analytic rows are gated against (and the pre-Slice-3 production
/// Jacobian, generalised from singleton steps to block steps).
pub(crate) fn fd_jacobian(
    assembly: &Assembly,
    blocks: &[BodyBlock],
    mate_indices: &[usize],
) -> DMatrix<f64> {
    const EPS: f64 = 1e-6;
    let rows = residual_for(assembly, mate_indices).len();
    let cols = 6 * blocks.len();
    let mut jac = DMatrix::<f64>::zeros(rows, cols);
    for (block_idx, block) in blocks.iter().enumerate() {
        for k in 0..6 {
            let mut step = [0.0_f64; 6];
            step[k] = EPS;
            let mut plus_asm = assembly.clone();
            apply_block_step(&mut plus_asm, block, &step);
            let plus = residual_for(&plus_asm, mate_indices);
            step[k] = -EPS;
            let mut minus_asm = assembly.clone();
            apply_block_step(&mut minus_asm, block, &step);
            let minus = residual_for(&minus_asm, mate_indices);
            for r in 0..rows.min(plus.len()).min(minus.len()) {
                jac[(r, 6 * block_idx + k)] = (plus[r] - minus[r]) / (2.0 * EPS);
            }
        }
    }
    jac
}

/// Agreement probe between the analytic Jacobian, the FD oracle, and the
/// PRODUCTION Jacobian the solve/DOF path consumes (the Slice-3 gate).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JacobianProbe {
    pub rows: usize,
    pub cols: usize,
    /// Entrywise `max |J_analytic − J_fd|` — the gate bound is 1e-6.
    pub max_abs_disagreement: f64,
    /// Whether `solve`/`dof_analysis` consume the ANALYTIC Jacobian:
    /// the production matrix is bitwise-identical to the analytic one
    /// and (where FD noise separates them) not the FD matrix.
    pub solver_uses_analytic: bool,
}

impl Assembly {
    /// Compare the analytic Jacobian against the central-difference
    /// oracle on the dense (singleton-block) layout, and report which
    /// path production consumes. See `tests/jacobian_gate.rs`.
    pub fn jacobian_probe(&self) -> JacobianProbe {
        let blocks = singleton_blocks(self);
        let layout = ColumnLayout::build(self, &blocks);
        let all: Vec<usize> = (0..self.mates.len()).collect();
        let analytic = analytic_jacobian(self, &layout, &all);
        let fd = fd_jacobian(self, &blocks, &all);
        let production = crate::solver::production_jacobian(self, &blocks, &all);
        let mut max_abs = 0.0_f64;
        if analytic.nrows() == fd.nrows() && analytic.ncols() == fd.ncols() {
            for r in 0..analytic.nrows() {
                for c in 0..analytic.ncols() {
                    max_abs = max_abs.max((analytic[(r, c)] - fd[(r, c)]).abs());
                }
            }
        } else {
            max_abs = f64::INFINITY;
        }
        let uses_analytic = production == analytic && (production != fd || fd == analytic);
        JacobianProbe {
            rows: analytic.nrows(),
            cols: analytic.ncols(),
            max_abs_disagreement: max_abs,
            solver_uses_analytic: uses_analytic,
        }
    }
}
