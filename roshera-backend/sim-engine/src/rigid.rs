//! Can the part still move? The rigid-body check every static study needs
//! before it is solved.
//!
//! A static solve of a part that can still slide or turn has no answer (the
//! stiffness matrix is singular); engines then fail with a solver message or,
//! worse, return a number from a near-singular system. Fusion's pre-check
//! reports a missing constraint; this check says WHICH motion is still free,
//! in words an engineer reads ("it can still turn about the bolt axis"), and
//! is computed from geometry alone, before any mesh or solve.
//!
//! A rigid motion moves a point `p` with velocity `v = t + w x p` (six
//! numbers: a translation `t` and a rotation `w`). A support holds a point
//! against motion along some directions `d`: `d . v = 0`, which is linear in
//! `(t, w)`: the row `[d, p x d]`. Every held point of every support adds its
//! rows; the motions left free are the null space of the stacked rows. The
//! positions are taken about the samples' centroid and scaled by their extent,
//! so the translation and rotation columns are comparable and the rank test
//! does not depend on the part's size or where it sits.

use serde::{Deserialize, Serialize};

/// How a held point resists motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Holds {
    /// Along every direction (a fixed or clamped face).
    Fully,
    /// Only along the surface normal: it may slide over the surface (a roller
    /// or slider; a pin's hole wall, radially).
    Normal,
}

/// One held point of a support: where, the surface normal there, and how it
/// holds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HeldPoint {
    pub point: [f64; 3],
    pub normal: [f64; 3],
    pub holds: Holds,
}

/// A rigid motion the supports leave free.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FreeMotion {
    /// The whole part can slide along `direction` (unit).
    Slide { direction: [f64; 3] },
    /// The part can turn about the axis along `axis` (unit) through
    /// `through`; `pitch` is the slide along the axis per radian of turn
    /// (zero for a plain turn; nonzero is a screw motion).
    Turn {
        axis: [f64; 3],
        through: [f64; 3],
        pitch: f64,
    },
}

impl FreeMotion {
    /// The motion in words, for the study tree and the agent.
    pub fn describe(&self) -> String {
        let v = |a: [f64; 3]| format!("({:.2}, {:.2}, {:.2})", a[0], a[1], a[2]);
        match self {
            FreeMotion::Slide { direction } => format!("can slide along {}", v(*direction)),
            FreeMotion::Turn {
                axis,
                through,
                pitch,
            } if pitch.abs() <= 1e-9 => format!(
                "can turn about an axis along {} through {}",
                v(*axis),
                v(*through)
            ),
            FreeMotion::Turn {
                axis,
                through,
                pitch,
            } => format!(
                "can screw along {} through {} ({:.3} mm per radian)",
                v(*axis),
                v(*through),
                pitch
            ),
        }
    }
}

/// Relative size below which an eigenvalue of the constraint matrix counts
/// as zero: the motion it belongs to is not held.
const NULL_REL: f64 = 1e-9;

/// The rigid motions `held` leaves free, as a basis of the free space (empty:
/// the part cannot move). With no held points every motion is free: three
/// slides and three turns.
pub fn free_motions(held: &[HeldPoint]) -> Vec<FreeMotion> {
    if held.is_empty() {
        let e = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let mut out: Vec<FreeMotion> = e
            .iter()
            .map(|&d| FreeMotion::Slide { direction: d })
            .collect();
        out.extend(e.iter().map(|&a| FreeMotion::Turn {
            axis: a,
            through: [0.0; 3],
            pitch: 0.0,
        }));
        return out;
    }
    // Centre and scale the positions.
    let n = held.len() as f64;
    let mut c = [0.0; 3];
    for h in held {
        for i in 0..3 {
            c[i] += h.point[i] / n;
        }
    }
    let scale = held
        .iter()
        .map(|h| norm(sub(h.point, c)))
        .fold(0.0_f64, f64::max)
        .max(1e-12);
    // Normal matrix A = R^T R of the stacked unit rows [d, p' x d].
    let mut a = [[0.0_f64; 6]; 6];
    let mut add_row = |d: [f64; 3], p: [f64; 3]| {
        let m = cross(p, d);
        let row = [d[0], d[1], d[2], m[0], m[1], m[2]];
        for i in 0..6 {
            for j in 0..6 {
                a[i][j] += row[i] * row[j];
            }
        }
    };
    for h in held {
        let p = scale_v(sub(h.point, c), 1.0 / scale);
        match h.holds {
            Holds::Fully => {
                for d in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
                    add_row(d, p);
                }
            }
            Holds::Normal => {
                let len = norm(h.normal);
                if len > 1e-12 {
                    add_row(scale_v(h.normal, 1.0 / len), p);
                }
            }
        }
    }
    let (values, vectors) = jacobi_eigen(a);
    let largest = values.iter().copied().fold(0.0_f64, f64::max);
    let mut out = Vec::new();
    for k in 0..6 {
        if values[k] > NULL_REL * largest.max(1e-300) {
            continue;
        }
        let v = [
            vectors[0][k],
            vectors[1][k],
            vectors[2][k],
            vectors[3][k],
            vectors[4][k],
            vectors[5][k],
        ];
        let t = [v[0], v[1], v[2]];
        let w = [v[3], v[4], v[5]];
        let wn = norm(w);
        if wn <= 1e-6 * norm(t).max(1e-12) {
            out.push(FreeMotion::Slide {
                direction: scale_v(t, 1.0 / norm(t).max(1e-300)),
            });
        } else {
            // In scaled coordinates v = t + w x p'. The axis is where the
            // velocity is along w: p' = w x t / |w|^2; pitch = t.w / |w|^2.
            let through_scaled = scale_v(cross(w, t), 1.0 / (wn * wn));
            let pitch_scaled = dot(t, w) / (wn * wn);
            out.push(FreeMotion::Turn {
                axis: scale_v(w, 1.0 / wn),
                through: add(c, scale_v(through_scaled, scale)),
                pitch: pitch_scaled * scale,
            });
        }
    }
    out
}

/// Eigen-decomposition of a symmetric 6x6 matrix by cyclic Jacobi rotations:
/// the eigenvalues and the eigenvectors as COLUMNS. Converges for any
/// symmetric input; the sweep cap only bounds work on pathological floats.
fn jacobi_eigen(mut a: [[f64; 6]; 6]) -> ([f64; 6], [[f64; 6]; 6]) {
    let mut v = [[0.0_f64; 6]; 6];
    for i in 0..6 {
        v[i][i] = 1.0;
    }
    for _sweep in 0..100 {
        let off: f64 = (0..6)
            .flat_map(|i| (0..6).filter(move |&j| j != i).map(move |j| (i, j)))
            .map(|(i, j)| a[i][j] * a[i][j])
            .sum();
        let diag: f64 = (0..6).map(|i| a[i][i] * a[i][i]).sum();
        if off <= 1e-30 * diag.max(1e-300) {
            break;
        }
        for p in 0..6 {
            for q in (p + 1)..6 {
                if a[p][q].abs() <= 1e-300 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let cs = 1.0 / (t * t + 1.0).sqrt();
                let sn = t * cs;
                for k in 0..6 {
                    let akp = a[k][p];
                    let akq = a[k][q];
                    a[k][p] = cs * akp - sn * akq;
                    a[k][q] = sn * akp + cs * akq;
                }
                for k in 0..6 {
                    let apk = a[p][k];
                    let aqk = a[q][k];
                    a[p][k] = cs * apk - sn * aqk;
                    a[q][k] = sn * apk + cs * aqk;
                }
                for k in 0..6 {
                    let vkp = v[k][p];
                    let vkq = v[k][q];
                    v[k][p] = cs * vkp - sn * vkq;
                    v[k][q] = sn * vkp + cs * vkq;
                }
            }
        }
    }
    let mut values = [0.0; 6];
    for i in 0..6 {
        values[i] = a[i][i];
    }
    (values, v)
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale_v(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn square(z: f64, holds: Holds, normal: [f64; 3]) -> Vec<HeldPoint> {
        [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]
            .iter()
            .map(|&[x, y]| HeldPoint {
                point: [x + 50.0, y - 30.0, z],
                normal,
                holds,
            })
            .collect()
    }

    /// A face held fully at four points leaves nothing free.
    #[test]
    fn a_fully_held_face_leaves_nothing_free() {
        let free = free_motions(&square(0.0, Holds::Fully, [0.0, 0.0, -1.0]));
        assert!(free.is_empty(), "{free:?}");
    }

    /// A face that may only not move along its normal (a roller) leaves the
    /// part free to slide in its plane and to turn about its normal: three
    /// free motions, none of them a lift or a tip.
    #[test]
    fn a_roller_face_leaves_two_slides_and_a_turn() {
        let free = free_motions(&square(0.0, Holds::Normal, [0.0, 0.0, -1.0]));
        assert_eq!(free.len(), 3, "{free:?}");
        for m in &free {
            match m {
                FreeMotion::Slide { direction } => {
                    assert!(direction[2].abs() < 1e-6, "no lift: {m:?}")
                }
                FreeMotion::Turn { axis, pitch, .. } => {
                    assert!(axis[2].abs() > 1.0 - 1e-6, "turn about the normal: {m:?}");
                    assert!(pitch.abs() < 1e-6, "{m:?}");
                }
            }
        }
    }

    /// A pin in a hole (the hole wall held radially only) leaves the part
    /// free to slide along the hole and turn about the hole's own axis -- an
    /// axis through the hole's centre, not the origin.
    #[test]
    fn a_pinned_hole_turns_about_its_own_axis() {
        let (cx, cy, r) = (40.0, -12.0, 6.0);
        let held: Vec<HeldPoint> = (0..12)
            .flat_map(|k| {
                let a = k as f64 * std::f64::consts::TAU / 12.0;
                [0.0, 8.0].map(|z| HeldPoint {
                    point: [cx + r * a.cos(), cy + r * a.sin(), z],
                    normal: [a.cos(), a.sin(), 0.0],
                    holds: Holds::Normal,
                })
            })
            .collect();
        let free = free_motions(&held);
        assert_eq!(free.len(), 2, "{free:?}");
        let turn = free
            .iter()
            .find_map(|m| match m {
                FreeMotion::Turn { axis, through, .. } => Some((*axis, *through)),
                _ => None,
            })
            .expect("a free turn");
        assert!(
            turn.0[2].abs() > 1.0 - 1e-6,
            "about the hole's axis: {turn:?}"
        );
        assert!(
            (turn.1[0] - cx).abs() < 1e-6 && (turn.1[1] - cy).abs() < 1e-6,
            "through the hole's centre: {turn:?}"
        );
        let words = free
            .iter()
            .map(FreeMotion::describe)
            .collect::<Vec<_>>()
            .join("; ");
        assert!(words.contains("turn about an axis along"), "{words}");
    }

    /// With nothing held, all six motions are free.
    #[test]
    fn nothing_held_frees_all_six() {
        assert_eq!(free_motions(&[]).len(), 6);
    }
}
