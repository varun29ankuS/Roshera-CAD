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
    }
}

/// Unit-normalize, or the zero vector if degenerate (a zero feature direction
/// can never satisfy a mate, so a zero residual contribution is correct — the
/// mate stays unsatisfiable elsewhere).
fn unit(v: Vector3<f64>) -> Vector3<f64> {
    v.try_normalize(1e-12).unwrap_or_else(Vector3::zeros)
}

impl Assembly {
    /// The constraint-violation residual `g(q)` for one mate: every component is
    /// 0 exactly when the mate is satisfied at the current poses. Returns empty
    /// when the mate's features don't match its kind (defensive — the caller
    /// pairs an axis mate with axis features and a face mate with face features).
    pub fn mate_residual(&self, mate: &Mate) -> Vec<f64> {
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
            _ => Vec::new(),
        }
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
