//! Geometric mate residuals over SE(3).
//!
//! Pure functions: given two instances' poses and a mate's features, return a
//! residual vector `g(q)` whose every component is 0 **exactly** when the mate
//! is satisfied. No solve here — S5 drives these to zero with Newton/LM; the
//! norm of `g` is the constraint violation. Each instance's pose is applied to
//! its feature first, so residuals are measured in world space.

use crate::interference::instance_isometry;
use crate::types::{Assembly, FeatureRef, Mate, MateKind};
use parry3d_f64::na::{Isometry3, Point3, Vector3};

/// A feature transformed into world space by an instance pose.
enum WorldFeature {
    Plane {
        point: Point3<f64>,
        normal: Vector3<f64>,
    },
    Axis {
        origin: Point3<f64>,
        direction: Vector3<f64>,
    },
    /// A full connector frame: origin + unit z (primary) + unit x
    /// (secondary); `y = z × x` implied.
    Frame {
        origin: Point3<f64>,
        z: Vector3<f64>,
        x: Vector3<f64>,
    },
}

fn to_world(iso: &Isometry3<f64>, feature: &FeatureRef) -> WorldFeature {
    match feature {
        FeatureRef::Face { point, normal } => WorldFeature::Plane {
            point: iso.transform_point(&Point3::new(point[0], point[1], point[2])),
            normal: iso.transform_vector(&Vector3::new(normal[0], normal[1], normal[2])),
        },
        FeatureRef::Axis { origin, direction } => WorldFeature::Axis {
            origin: iso.transform_point(&Point3::new(origin[0], origin[1], origin[2])),
            direction: iso.transform_vector(&Vector3::new(
                direction[0],
                direction[1],
                direction[2],
            )),
        },
        FeatureRef::Frame {
            origin,
            z_axis,
            x_axis,
        } => WorldFeature::Frame {
            origin: iso.transform_point(&Point3::new(origin[0], origin[1], origin[2])),
            z: unit(iso.transform_vector(&Vector3::new(z_axis[0], z_axis[1], z_axis[2]))),
            x: unit(iso.transform_vector(&Vector3::new(x_axis[0], x_axis[1], x_axis[2]))),
        },
    }
}

/// Unit-normalize, or the zero vector if degenerate (a zero feature direction
/// can never satisfy a mate, so a zero residual contribution is correct — the
/// mate stays unsatisfiable elsewhere).
fn unit(v: Vector3<f64>) -> Vector3<f64> {
    v.try_normalize(1e-12).unwrap_or_else(Vector3::zeros)
}

/// Whether one mate is numerically enforced by the solver, with the reason
/// when it is not. A mate that is NOT enforced contributes zero residual
/// rows — it must never be presented as a constraint (the silent-DOF-lie
/// class the sketch #19 fix killed in 2D).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MateEnforcement {
    pub mate_index: usize,
    pub enforced: bool,
    pub reason: Option<String>,
}

/// Per-mate enforcement verdicts for a whole assembly.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MateEnforcementReport {
    pub mates: Vec<MateEnforcement>,
}

impl MateEnforcementReport {
    pub fn all_enforced(&self) -> bool {
        self.mates.iter().all(|m| m.enforced)
    }

    /// Indices of the refused mates.
    pub fn refused(&self) -> Vec<usize> {
        self.mates
            .iter()
            .filter(|m| !m.enforced)
            .map(|m| m.mate_index)
            .collect()
    }
}

/// Does this feature pairing match what the kind's residual consumes?
fn features_match_kind(kind: &MateKind, fa: &FeatureRef, fb: &FeatureRef) -> bool {
    match kind {
        MateKind::Concentric => {
            matches!(fa, FeatureRef::Axis { .. }) && matches!(fb, FeatureRef::Axis { .. })
        }
        MateKind::Coincident | MateKind::Fixed => {
            matches!(fa, FeatureRef::Face { .. }) && matches!(fb, FeatureRef::Face { .. })
        }
        // Every frame-pair kind + the overlays consume Frame features. The
        // coupling kinds read their COUPLED mates' frames; their own
        // features are descriptive and unchecked here.
        MateKind::GearRatio { .. } | MateKind::RackPinion { .. } | MateKind::Screw { .. } => true,
        _ => matches!(fa, FeatureRef::Frame { .. }) && matches!(fb, FeatureRef::Frame { .. }),
    }
}

impl Assembly {
    /// Per-mate enforcement report: names every mate the solver does NOT
    /// numerically enforce (typed refuse set, kind/feature mismatch,
    /// invalid coupling reference, unknown instance) with the reason.
    /// `certify` folds `all_enforced` into the certificate so an
    /// unenforced mate can never ride a `sound` verdict.
    pub fn mate_enforcement_report(&self) -> MateEnforcementReport {
        let mates = self
            .mates
            .iter()
            .enumerate()
            .map(|(idx, mate)| {
                let reason = self.refusal_reason(mate);
                MateEnforcement {
                    mate_index: idx,
                    enforced: reason.is_none(),
                    reason,
                }
            })
            .collect();
        MateEnforcementReport { mates }
    }

    /// `None` = enforced; `Some(reason)` = refused.
    fn refusal_reason(&self, mate: &Mate) -> Option<String> {
        if !mate.kind.is_numerically_enforced() {
            return Some(format!(
                "{:?} is typed but not numerically enforced yet — declared honestly, \
                 never a silent zero-DOF lie",
                mate.kind
            ));
        }
        if self.instance(mate.a).is_none() || self.instance(mate.b).is_none() {
            return Some("mate references an unknown instance".to_string());
        }
        if !features_match_kind(&mate.kind, &mate.feature_a, &mate.feature_b) {
            return Some(format!(
                "feature kinds do not match the mate kind {:?} (legacy kinds take \
                 Face/Axis, frame kinds take Frame connectors)",
                mate.kind
            ));
        }
        // Coupling kinds must reference in-range, non-coupling, frame-pair
        // mates (a coupling of a coupling has no joint parameter to read).
        let couple_refs: &[u32] = match &mate.kind {
            MateKind::GearRatio { couples, .. } | MateKind::RackPinion { couples, .. } => couples,
            MateKind::Screw { couples, .. } => std::slice::from_ref(couples),
            _ => &[],
        };
        for &target in couple_refs {
            let Some(target_mate) = self.mates.get(target as usize) else {
                return Some(format!(
                    "coupling references mate index {target}, which does not exist"
                ));
            };
            if target_mate.kind.is_coupling() {
                return Some(format!(
                    "coupling references mate index {target}, which is itself a coupling"
                ));
            }
            if !matches!(
                (&target_mate.feature_a, &target_mate.feature_b),
                (FeatureRef::Frame { .. }, FeatureRef::Frame { .. })
            ) {
                return Some(format!(
                    "coupling references mate index {target}, which has no frame pair \
                     to read joint parameters from"
                ));
            }
        }
        None
    }

    /// The constraint-violation residual `g(q)` for one mate: every component is
    /// 0 exactly when the mate is satisfied at the current poses. Returns empty
    /// when the mate's features don't match its kind (defensive — the caller
    /// pairs an axis mate with axis features and a face mate with face features).
    pub fn mate_residual(&self, mate: &Mate) -> Vec<f64> {
        // Refused kinds contribute NO rows — they are surfaced by the
        // enforcement report + certificate, never silently counted.
        if !mate.kind.is_numerically_enforced() {
            return Vec::new();
        }
        // Coupling kinds read their COUPLED mates' joint parameters; their
        // own features are descriptive only.
        match mate.kind {
            MateKind::GearRatio { ratio, at, couples } => {
                let (Some((theta1, _)), Some((theta2, _))) = (
                    self.joint_parameters_unwrapped(couples[0]),
                    self.joint_parameters_unwrapped(couples[1]),
                ) else {
                    return Vec::new();
                };
                return vec![ratio * (theta1 - at[0]) + (theta2 - at[1])];
            }
            MateKind::RackPinion {
                pinion_radius,
                at,
                couples,
            } => {
                let (Some((theta, _)), Some((_, s))) = (
                    self.joint_parameters_unwrapped(couples[0]),
                    self.joint_parameters_unwrapped(couples[1]),
                ) else {
                    return Vec::new();
                };
                return vec![pinion_radius * (theta - at[0]) - (s - at[1])];
            }
            MateKind::Screw { lead, at, couples } => {
                let Some((theta, s)) = self.joint_parameters_unwrapped(couples) else {
                    return Vec::new();
                };
                return vec![(s - at[1]) - lead * (theta - at[0]) / std::f64::consts::TAU];
            }
            _ => {}
        }

        let (Some(ia), Some(ib)) = (self.instance(mate.a), self.instance(mate.b)) else {
            return Vec::new();
        };
        let fa = to_world(&instance_isometry(ia), &mate.feature_a);
        let fb = to_world(&instance_isometry(ib), &mate.feature_b);

        match (mate.kind, fa, fb) {
            (
                MateKind::Concentric,
                WorldFeature::Axis {
                    origin: oa,
                    direction: da,
                },
                WorldFeature::Axis {
                    origin: ob,
                    direction: db,
                },
            ) => concentric_residual(oa, unit(da), ob, unit(db)),
            (
                MateKind::Coincident,
                WorldFeature::Plane {
                    point: pa,
                    normal: na,
                },
                WorldFeature::Plane {
                    point: pb,
                    normal: nb,
                },
            ) => coincident_residual(pa, unit(na), pb, unit(nb)),
            // Fixed = the TRUE rigid lock (a bolt pattern): the two declared
            // face frames are welded — points coincide, normals antiparallel
            // (flush), and the deterministically derived in-plane tangents
            // align. Rank 6: no in-plane slide, no spin. This replaces the
            // Phase-1 face-flush reduction that silently left 3 DOF free
            // (the §2.2 "Fixed is not rigid" lie, killed Slice 1).
            (MateKind::Fixed, _, _) => {
                if let (
                    FeatureRef::Face {
                        point: pa,
                        normal: na,
                    },
                    FeatureRef::Face {
                        point: pb,
                        normal: nb,
                    },
                ) = (&mate.feature_a, &mate.feature_b)
                {
                    fixed_residual(
                        &instance_isometry(ia),
                        pa,
                        na,
                        &instance_isometry(ib),
                        pb,
                        nb,
                    )
                } else {
                    Vec::new()
                }
            }
            // ── Frame-pair joint kinds + overlays (Slice 2) ────────────
            (
                kind,
                WorldFeature::Frame {
                    origin: oa,
                    z: za,
                    x: xa,
                },
                WorldFeature::Frame {
                    origin: ob,
                    z: zb,
                    x: xb,
                },
            ) => frame_pair_residual(kind, oa, za, xa, ob, zb, xb),
            _ => Vec::new(),
        }
    }

    /// Joint parameters (θ, s) of mate `index`, read from its world frame
    /// pair: θ = angle from `x_a` to `x_b` about `ẑ_a`, s = `(o_b − o_a)·ẑ_a`.
    /// `None` when the index is out of range or the mate has no frame pair.
    /// Public: the caller captures these AT DECLARATION as a coupling's
    /// reference configuration (`at`).
    pub fn joint_parameters_of(&self, index: u32) -> Option<(f64, f64)> {
        let mate = self.mates.get(index as usize)?;
        let ia = self.instance(mate.a)?;
        let ib = self.instance(mate.b)?;
        let fa = to_world(&instance_isometry(ia), &mate.feature_a);
        let fb = to_world(&instance_isometry(ib), &mate.feature_b);
        let (
            WorldFeature::Frame {
                origin: oa,
                z: za,
                x: xa,
            },
            WorldFeature::Frame {
                origin: ob, x: xb, ..
            },
        ) = (fa, fb)
        else {
            return None;
        };
        let theta = (xa.cross(&xb).dot(&za)).atan2(xa.dot(&xb));
        let s = (ob - oa).dot(&za);
        Some((theta, s))
    }

    /// Joint parameters of mate `index` with θ UNWRAPPED by the mate's
    /// recorded winding: `θ_unwrapped = θ_wrapped + turns·2π`
    /// (`s` is already unbounded and needs no unwinding).
    ///
    /// This is the form every COUPLING residual reads. `atan2` can only
    /// ever report the wrapped angle, so a coupling driven through
    /// multiple turns would otherwise see θ snap from +π to −π every
    /// half-turn and haul its coupled parameter back with it — the
    /// premise-#5 corruption. The winding is written by
    /// [`Assembly::drag`], which walks the path and therefore knows it;
    /// with no winding recorded this is exactly `joint_parameters_of`.
    pub fn joint_parameters_unwrapped(&self, index: u32) -> Option<(f64, f64)> {
        let (theta, s) = self.joint_parameters_of(index)?;
        Some((
            theta + f64::from(self.turns_of(index)) * std::f64::consts::TAU,
            s,
        ))
    }

    /// L2 norm of a mate's residual — 0 iff the mate holds.
    pub fn mate_violation(&self, mate: &Mate) -> f64 {
        self.mate_residual(mate)
            .iter()
            .map(|x| x * x)
            .sum::<f64>()
            .sqrt()
    }
}

/// The frame-pair residuals (§3.2 taxonomy). Mated convention: the two
/// connector frames ALIGN — `z_b = z_a` and, where spin is locked,
/// `x_b = x_a` (Onshape's primary-axis alignment; a flipped mate is
/// authored by flipping the connector frame itself). Component blocks:
///   * `d`      = `o_b − o_a`                    (3 comps; rank 3)
///   * `align`  = `z_b − z_a`                    (3 comps; rank 2 — zero iff aligned)
///   * `spin`   = `x_b − x_a`                    (3 comps; +rank 1 given align)
///   * `perp`   = `d − (d·ẑ_a)ẑ_a`              (3 comps; rank 2 — origin on the z line)
///   * `online` = `d − (d·ŝ)ŝ`, `s` = slot dir  (3 comps; rank 2 — origin on the slot line)
/// Every residual is smooth in the poses; the numeric Jacobian rank at a
/// satisfied generic configuration is the row noted per kind (pinned by
/// `tests/dof_table.rs`).
///
/// KNOWN LIMIT (honest): every orientation residual has zero gradient at
/// the exact ANTIPODE (`z_b = −z_a`) — a mate declared 180° away from its
/// aligned state can stall the Newton solve there; the solve reports
/// `converged: false` rather than pretending. Author the connector frame
/// the way the joint actually points.
#[allow(clippy::too_many_arguments)]
fn frame_pair_residual(
    kind: MateKind,
    oa: Point3<f64>,
    za: Vector3<f64>,
    xa: Vector3<f64>,
    ob: Point3<f64>,
    zb: Vector3<f64>,
    xb: Vector3<f64>,
) -> Vec<f64> {
    let d = ob - oa;
    let align = zb - za;
    let spin = xb - xa;
    let perp = d - za * d.dot(&za);
    match kind {
        // Rank 6: total lock.
        MateKind::Fastened => vec![
            d.x, d.y, d.z, align.x, align.y, align.z, spin.x, spin.y, spin.z,
        ],
        // Rank 5: spin about z stays free.
        MateKind::Revolute { .. } => vec![d.x, d.y, d.z, align.x, align.y, align.z],
        // Rank 5: slide along z stays free (orientation fully locked, origin
        // held on the z line).
        MateKind::Slider { .. } => vec![
            align.x, align.y, align.z, spin.x, spin.y, spin.z, perp.x, perp.y, perp.z,
        ],
        // Rank 4: spin + slide stay free.
        MateKind::Cylindrical { .. } => vec![align.x, align.y, align.z, perp.x, perp.y, perp.z],
        // Rank 3: in-plane slide x/y + spin stay free.
        MateKind::Planar => vec![align.x, align.y, align.z, d.dot(&za)],
        // Rank 3: all rotations stay free.
        MateKind::Ball => vec![d.x, d.y, d.z],
        // Rank 4: pin spin + slot slide stay free. The slot direction lives
        // on frame A (the slotted part): its x, or y = z × x.
        MateKind::PinSlot { slot_dir_x, .. } => {
            let s = if slot_dir_x { xa } else { za.cross(&xa) };
            let online = d - s * d.dot(&s);
            vec![align.x, align.y, align.z, online.x, online.y, online.z]
        }
        // Overlays — one/two equations each.
        MateKind::Distance { value } => vec![d.dot(&za) - value],
        MateKind::Angle { value } => vec![za.dot(&zb) - value.cos()],
        MateKind::Parallel => {
            let cross = za.cross(&zb);
            vec![cross.x, cross.y, cross.z]
        }
        MateKind::Tangent { radius } => vec![d.dot(&za).abs() - radius],
        // Legacy kinds never reach here (matched on Face/Axis features);
        // couplings and the refuse set are handled before feature matching.
        _ => Vec::new(),
    }
}

/// Two axes collinear: directions parallel (`dir_a × dir_b → 0`) AND axis_b's
/// origin on axis_a's line (perpendicular offset → 0). Six components; the norm
/// is 0 iff the axes coincide.
fn concentric_residual(
    origin_a: Point3<f64>,
    dir_a: Vector3<f64>,
    origin_b: Point3<f64>,
    dir_b: Vector3<f64>,
) -> Vec<f64> {
    let cross = dir_a.cross(&dir_b); // rotational: 0 when parallel
    let delta = origin_b - origin_a;
    let perp = delta - dir_a * delta.dot(&dir_a); // translational: offset off the line
    vec![cross.x, cross.y, cross.z, perp.x, perp.y, perp.z]
}

/// Deterministic in-plane tangent for a face feature, derived from the LOCAL
/// normal. The local feature never changes during a solve (only the pose
/// does), so the derived tangent is a rigid part of the feature frame and the
/// residual stays smooth in the pose. Construction: take the global axis
/// LEAST aligned with the normal (ties break x → y → z) and project it into
/// the plane. Both sides of a Fixed mate derive their tangent the same way,
/// so "tangents aligned" is a well-defined, repeatable spin lock.
fn local_tangent(normal: &[f64; 3]) -> Vector3<f64> {
    let n = unit(Vector3::new(normal[0], normal[1], normal[2]));
    let axes = [Vector3::x(), Vector3::y(), Vector3::z()];
    let mut best = axes[0];
    let mut best_dot = f64::INFINITY;
    for e in axes {
        let d = n.dot(&e).abs();
        if d < best_dot {
            best_dot = d;
            best = e;
        }
    }
    unit(best - n * best.dot(&n))
}

/// The full rigid lock between two face frames (Fixed / bolt pattern). Nine
/// components, rank 6 — zero exactly when:
///   * the declared points coincide            (3: no translation freedom)
///   * the normals are ANTIPARALLEL (flush)    (n_a + n_b → 0; rank 2)
///   * the derived in-plane tangents align     (t_b − t_a → 0; rank 1 = spin)
/// The tangents are derived from the LOCAL normals (see [`local_tangent`]) and
/// carried to world by each instance's rotation, so the lock is rigid in the
/// features, not in any world coordinate.
fn fixed_residual(
    iso_a: &Isometry3<f64>,
    point_a: &[f64; 3],
    normal_a: &[f64; 3],
    iso_b: &Isometry3<f64>,
    point_b: &[f64; 3],
    normal_b: &[f64; 3],
) -> Vec<f64> {
    let pa = iso_a.transform_point(&Point3::new(point_a[0], point_a[1], point_a[2]));
    let pb = iso_b.transform_point(&Point3::new(point_b[0], point_b[1], point_b[2]));
    let na = unit(iso_a.transform_vector(&Vector3::new(normal_a[0], normal_a[1], normal_a[2])));
    let nb = unit(iso_b.transform_vector(&Vector3::new(normal_b[0], normal_b[1], normal_b[2])));
    let ta = iso_a.transform_vector(&local_tangent(normal_a));
    let tb = iso_b.transform_vector(&local_tangent(normal_b));
    let d = pb - pa;
    let flush = na + nb;
    let spin = tb - ta;
    vec![
        d.x, d.y, d.z, flush.x, flush.y, flush.z, spin.x, spin.y, spin.z,
    ]
}

/// Two planar faces flush: coplanar (signed distance of `point_b` to plane_a → 0)
/// AND normals collinear (`normal_a × normal_b → 0`, satisfied parallel OR
/// antiparallel). Four components; the norm is 0 iff the planes coincide.
fn coincident_residual(
    point_a: Point3<f64>,
    normal_a: Vector3<f64>,
    point_b: Point3<f64>,
    normal_b: Vector3<f64>,
) -> Vec<f64> {
    let dist = (point_b - point_a).dot(&normal_a);
    let cross = normal_a.cross(&normal_b);
    vec![dist, cross.x, cross.y, cross.z]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Instance, InstanceId, Mesh};

    fn at(id: u32, t: [f64; 3]) -> Instance {
        let mut instance = Instance::new(InstanceId(id), format!("part_{id}"), Mesh::default());
        instance.translation = t;
        instance
    }

    fn axis_feature(origin: [f64; 3], direction: [f64; 3]) -> FeatureRef {
        FeatureRef::Axis { origin, direction }
    }
    fn face_feature(point: [f64; 3], normal: [f64; 3]) -> FeatureRef {
        FeatureRef::Face { point, normal }
    }

    fn mate(kind: MateKind, fa: FeatureRef, fb: FeatureRef) -> Mate {
        Mate {
            kind,
            a: InstanceId(0),
            feature_a: fa,
            b: InstanceId(1),
            feature_b: fb,
        }
    }

    #[test]
    fn collinear_axes_satisfy_concentric() {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(at(0, [0.0, 0.0, 0.0]));
        assembly.add_instance(at(1, [0.0, 0.0, 0.0]));
        let m = mate(
            MateKind::Concentric,
            axis_feature([0.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            axis_feature([0.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        );
        assert!(assembly.mate_violation(&m) < 1e-9);
    }

    #[test]
    fn offset_axes_violate_concentric() {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(at(0, [0.0, 0.0, 0.0]));
        assembly.add_instance(at(1, [5.0, 0.0, 0.0])); // axis_b shifted 5 in x
        let m = mate(
            MateKind::Concentric,
            axis_feature([0.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            axis_feature([0.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        );
        let v = assembly.mate_violation(&m);
        assert!(
            (v - 5.0).abs() < 1e-9,
            "perpendicular offset should be 5, got {v}"
        );
    }

    #[test]
    fn flush_faces_satisfy_coincident() {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(at(0, [0.0, 0.0, 0.0]));
        assembly.add_instance(at(1, [0.0, 0.0, 0.0]));
        let m = mate(
            MateKind::Coincident,
            face_feature([0.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            face_feature([0.0, 0.0, 0.0], [0.0, 0.0, -1.0]), // antiparallel = flush
        );
        assert!(assembly.mate_violation(&m) < 1e-9);
    }

    #[test]
    fn separated_faces_violate_coincident() {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(at(0, [0.0, 0.0, 0.0]));
        assembly.add_instance(at(1, [0.0, 0.0, 3.0])); // face_b 3 above plane_a
        let m = mate(
            MateKind::Coincident,
            face_feature([0.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            face_feature([0.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        );
        let v = assembly.mate_violation(&m);
        assert!(
            (v - 3.0).abs() < 1e-9,
            "out-of-plane distance should be 3, got {v}"
        );
    }

    #[test]
    fn residual_is_zero_iff_mate_holds() {
        // VERIFY/HARNESS invariant: the norm is 0 exactly when every component is.
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(at(0, [0.0, 0.0, 0.0]));
        assembly.add_instance(at(1, [0.0, 0.0, 0.0]));
        let held = mate(
            MateKind::Concentric,
            axis_feature([0.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            axis_feature([0.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        );
        assert!(assembly.mate_violation(&held) < 1e-9);
        assert!(assembly
            .mate_residual(&held)
            .iter()
            .all(|&x| x.abs() < 1e-9));
    }
}
