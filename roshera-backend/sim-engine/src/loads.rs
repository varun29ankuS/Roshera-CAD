//! Turning a load the engineer states -- "2 kN down on this hole" -- into
//! forces on mesh nodes that add up to exactly that.
//!
//! A load is stated on FACES. The mesh carries them as 6-node (quadratic)
//! triangles. A uniform traction on such a triangle is NOT shared equally by
//! its six nodes: the consistent nodal loads put nothing on the three corners
//! and one third of the triangle's share on each mid-side node (the integral
//! of the quadratic shape functions over the triangle: zero for a corner,
//! A/3 for a mid-side node). Spreading a force equally over every node --
//! the obvious shortcut -- bends the result near the loaded face.
//!
//! Two spreads are offered:
//! - [`Spread::Even`]: the same traction everywhere on the faces (a pressure
//!   pad, a glued-on bracket);
//! - [`Spread::Bearing`]: a pin pulling the part along the load presses on
//!   the half of the hole facing the pull, most at its middle -- traction in
//!   proportion to `max(0, -n . d)`, `n` the face normal out of the material
//!   (into the hole), `d` the load direction.
//!
//! Whatever the spread, the nodal forces sum to the stated force exactly
//! (checked: [`NodalLoads::total`]).

use serde::{Deserialize, Serialize};

/// One 6-node boundary triangle of a loaded face: node ids (three corners,
/// then the mid-sides of edges 0-1, 1-2, 2-0), corner positions, and the
/// outward normal (out of the material).
#[derive(Debug, Clone, PartialEq)]
pub struct Tri6 {
    pub nodes: [u64; 6],
    pub corners: [[f64; 3]; 3],
    pub normal: [f64; 3],
}

impl Tri6 {
    /// The triangle's area (its corners; the mid-sides of a straight-sided
    /// 6-node triangle add nothing to it).
    pub fn area(&self) -> f64 {
        let [a, b, c] = self.corners;
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let x = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        0.5 * (x[0] * x[0] + x[1] * x[1] + x[2] * x[2]).sqrt()
    }
}

/// How a force spreads over its faces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Spread {
    /// The same traction everywhere on the faces.
    Even,
    /// A pin pulling along the force: presses on the half of the hole facing
    /// the pull, in proportion to `max(0, -n . d)`.
    Bearing,
}

/// Forces on nodes (N), node id to force vector.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NodalLoads(pub std::collections::BTreeMap<u64, [f64; 3]>);

impl NodalLoads {
    /// The sum of every nodal force.
    pub fn total(&self) -> [f64; 3] {
        self.0
            .values()
            .fold([0.0; 3], |t, f| [t[0] + f[0], t[1] + f[1], t[2] + f[2]])
    }
}

/// Why a stated load cannot be put on the mesh.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LoadError {
    /// The faces it names have no area in the mesh.
    #[error("the loaded faces have no area in the mesh")]
    NoArea,
    /// A bearing load whose direction no part of the hole faces: the pin
    /// would not press on this face.
    #[error("no part of the loaded faces faces the pull: a pin pulling this way would not press on them")]
    NothingFacesThePull,
}

/// The consistent nodal forces of `force` (N) spread over `tris` as
/// `spread`: they sum to `force` exactly; corners of each triangle get none,
/// each mid-side node one third of the triangle's share.
pub fn nodal_forces(
    tris: &[Tri6],
    force: [f64; 3],
    spread: Spread,
) -> Result<NodalLoads, LoadError> {
    let fmag = (force[0] * force[0] + force[1] * force[1] + force[2] * force[2]).sqrt();
    let d = if fmag > 0.0 {
        [force[0] / fmag, force[1] / fmag, force[2] / fmag]
    } else {
        [0.0; 3]
    };
    // Weight of each triangle: area x the traction's relative intensity.
    let weights: Vec<f64> = tris
        .iter()
        .map(|t| {
            let a = t.area();
            match spread {
                Spread::Even => a,
                Spread::Bearing => {
                    let n = t.normal;
                    let nn = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-300);
                    let facing = -(n[0] * d[0] + n[1] * d[1] + n[2] * d[2]) / nn;
                    a * facing.max(0.0)
                }
            }
        })
        .collect();
    let sum: f64 = weights.iter().sum();
    if !(sum > 0.0) {
        return Err(match spread {
            Spread::Bearing if tris.iter().any(|t| t.area() > 0.0) => {
                LoadError::NothingFacesThePull
            }
            _ => LoadError::NoArea,
        });
    }
    let mut out = NodalLoads::default();
    for (t, w) in tris.iter().zip(&weights) {
        // A triangle that carries nothing names no loaded node: a zero entry
        // would list the far side of a pinned hole as loaded.
        if *w <= 0.0 {
            continue;
        }
        let share = w / sum;
        for &node in &t.nodes[3..6] {
            let f = out.0.entry(node).or_insert([0.0; 3]);
            for i in 0..3 {
                f[i] += force[i] * share / 3.0;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// Two triangles making a 10 x 10 square in z = 0, normal +z.
    fn square() -> Vec<Tri6> {
        vec![
            Tri6 {
                nodes: [1, 2, 3, 10, 11, 12],
                corners: [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0]],
                normal: [0.0, 0.0, 1.0],
            },
            Tri6 {
                nodes: [1, 3, 4, 12, 13, 14],
                corners: [[0.0, 0.0, 0.0], [10.0, 10.0, 0.0], [0.0, 10.0, 0.0]],
                normal: [0.0, 0.0, 1.0],
            },
        ]
    }

    /// An even load sums to the stated force, puts nothing on the corners
    /// and a sixth of it on each mid-side node of two equal triangles (the
    /// shared mid-side node, 12, gets two sixths).
    ///
    /// Mutation: share the force over all six nodes -> corners loaded ->
    /// RED.
    #[test]
    fn an_even_load_goes_to_the_mid_side_nodes_and_sums_exactly() {
        let f = [0.0, 0.0, -2000.0];
        let loads = nodal_forces(&square(), f, Spread::Even).unwrap();
        let t = loads.total();
        assert!((t[2] + 2000.0).abs() < 1e-9 && t[0].abs() < 1e-12, "{t:?}");
        for corner in [1, 2, 3, 4] {
            assert!(
                !loads.0.contains_key(&corner),
                "corner {corner} carries load"
            );
        }
        assert!((loads.0[&10][2] + 2000.0 / 6.0).abs() < 1e-9);
        assert!(
            (loads.0[&12][2] + 2000.0 / 3.0).abs() < 1e-9,
            "shared mid-side"
        );
    }

    /// A pin pulling down presses only on the half of the hole facing the
    /// pull, and the total is still the stated force.
    ///
    /// Mutation: spread a bearing load evenly -> the upper half carries
    /// load -> RED.
    #[test]
    fn a_bearing_load_presses_on_the_half_facing_the_pull() {
        // A ring of wall triangles round a hole along x; normals point into
        // the hole (out of the material).
        let mut tris = Vec::new();
        let n = 24;
        for k in 0..n {
            let a0 = k as f64 * std::f64::consts::TAU / n as f64;
            let a1 = (k + 1) as f64 * std::f64::consts::TAU / n as f64;
            let am = 0.5 * (a0 + a1);
            let p = |a: f64, x: f64| [x, 6.0 * a.cos(), 6.0 * a.sin()];
            tris.push(Tri6 {
                nodes: [
                    k * 10,
                    k * 10 + 1,
                    k * 10 + 2,
                    k * 10 + 3,
                    k * 10 + 4,
                    k * 10 + 5,
                ],
                corners: [p(a0, 0.0), p(a1, 0.0), p(am, 8.0)],
                normal: [0.0, -am.cos(), -am.sin()],
            });
        }
        let f = [0.0, 0.0, -2000.0];
        let loads = nodal_forces(&tris, f, Spread::Bearing).unwrap();
        let t = loads.total();
        assert!((t[2] + 2000.0).abs() < 1e-6, "{t:?}");
        for (k, tri) in tris.iter().enumerate() {
            // The wall faces the downward pull where its into-the-hole
            // normal points up: the lower half of the hole (sin < 0).
            let upper = tri.normal[2] < -1e-9;
            let loaded = tri.nodes[3..6].iter().any(|n| loads.0.contains_key(n));
            if upper {
                assert!(!loaded, "triangle {k} on the upper half is loaded");
            }
        }
        assert!(
            nodal_forces(&tris[..1], [0.0, 0.0, 0.0], Spread::Bearing).is_err(),
            "no pull, nothing to press"
        );
    }
}
