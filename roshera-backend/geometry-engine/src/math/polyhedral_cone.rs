//! Convex polyhedral cones in R³ — the substrate for smooth-B-Rep contact
//! determination (Crozet, Ch. 3).
//!
//! A pointed convex polyhedral cone `C ⊆ R³` at the origin has two dual
//! representations:
//!   * **V-rep** `generators`: extreme unit rays, `C = { Σ λᵢ gᵢ : λᵢ ≥ 0 }`.
//!   * **H-rep** `supports`: outward unit face normals, `C = { x : x·sⱼ ≤ 0 }`.
//!
//! The two are **polar duals**: the polar cone `C° = { y : x·y ≤ 0 ∀ x ∈ C }`
//! has `C`'s supports as its generators and vice-versa, so [`polar`] is just a
//! swap. The V↔H conversion is one routine ([`dual_extreme_rays`]): the extreme
//! rays of `{ y : vᵢ·y ≤ 0 }` are the pairwise plane intersections `vᵢ × vⱼ`
//! kept iff every other constraint holds — which converts generators→supports
//! *and* supports→generators (they're duals of each other).
//!
//! These power the CD critical-point gate (a footpoint pair is a true local
//! minimum-distance only if the separation direction lies in both features'
//! normal cones) and feature-pair culling (two cones with no common ray cannot
//! contact).
//!
//! # Scope
//! Slice 1 — pointed cones (generators spanning ≥ 3D, the vertex-cone case),
//! the full-space and zero cones: construction, polar, membership.
//! Slice 2 — cone **intersection** ([`PolyhedralCone::intersects`],
//! [`PolyhedralCone::intersection`], [`PolyhedralCone::separating_plane`]) for
//! feature-pair culling and the critical-point gate.
//! Slice 3 — conservative angular **dilation** ([`PolyhedralCone::dilate`]):
//! the α-neighborhood over-approximation that makes a point-wise normal cone
//! valid over a curved feature area.
//! Slice 4 — **all ranks**: rank-1 ray cones (smooth-point normal cones) and
//! rank-2 planar wedges (edge cones) join the rank-3 vertex cones, so
//! `from_generators` is correct for every boundary feature.
//! Next — the B-Rep bridge (tangent/normal cones from a vertex/edge of a
//! `BRepModel`) and the critical-point gate itself.
//!
//! # References
//! Crozet, *Efficient contact determination between B-Rep solids*, §3.2–3.4;
//! Rockafellar, *Convex Analysis*, §14 (polar cones).
#![allow(clippy::indexing_slicing)]

use crate::math::Vector3;

/// Angular epsilon for cone membership / dedup (radians-ish, on unit vectors).
const CONE_EPS: f64 = 1e-9;

/// A convex polyhedral cone in R³, stored in both dual representations.
#[derive(Debug, Clone)]
pub struct PolyhedralCone {
    /// Extreme unit rays (V-rep). Empty ⇒ the cone is `{0}` or the full space
    /// (disambiguated by `supports`).
    generators: Vec<Vector3>,
    /// Outward unit face normals (H-rep): `x ∈ C ⇔ x·sⱼ ≤ 0 ∀ j`. Empty with
    /// non-empty `generators` ⇒ the full space R³ (no constraints).
    supports: Vec<Vector3>,
}

impl PolyhedralCone {
    /// The whole space R³ (no supporting constraints). `contains` is always
    /// true. A sentinel generator records non-emptiness.
    pub fn full_space() -> Self {
        Self {
            generators: vec![
                Vector3::X,
                -Vector3::X,
                Vector3::Y,
                -Vector3::Y,
                Vector3::Z,
                -Vector3::Z,
            ],
            supports: Vec::new(),
        }
    }

    /// Build a cone from its generating rays (V-rep). Computes the dual
    /// supports (H-rep) and **canonicalizes the V-rep to its extreme rays** —
    /// redundant input rays (those interior to the conic hull of the others)
    /// are dropped, so `generators` is minimal and `generators ⇄ supports` is a
    /// clean polar round-trip. Rays are normalized and de-duplicated; zero rays
    /// are dropped.
    pub fn from_generators(rays: &[Vector3]) -> Self {
        let raw = normalize_dedup(rays);
        // Rank 0 — the trivial {0} cone.
        if raw.is_empty() {
            return Self {
                generators: Vec::new(),
                supports: Vec::new(),
            };
        }
        // Rank 1 — a single ray (smooth-surface-point normal cone).
        if raw.len() == 1 {
            return ray_cone(raw[0]);
        }
        // Rank 2 — all generators coplanar ⇒ a planar wedge (the edge-cone
        // case). `dual_extreme_rays` cannot find a wedge's in-plane edge
        // supports (coplanar generators' cross products are all the plane
        // normal), so this case is handled explicitly.
        if let Some(normal) = coplanar_normal(&raw) {
            return planar_wedge(&raw, normal);
        }
        // Rank 3 — pointed / space-spanning cone (the vertex-cone case).
        let supports = dual_extreme_rays(&raw);
        // The extreme rays are determined by the faces; recovering them from
        // the supports yields the minimal generator set. (Empty supports here
        // ⇒ the generators positively span R³ ⇒ full space.)
        let generators = if supports.is_empty() {
            raw
        } else {
            dual_extreme_rays(&supports)
        };
        Self {
            generators,
            supports,
        }
    }

    /// The **normal cone** of a vertex: the cone generated by the outward
    /// normals of the faces meeting there. Its [`polar`] is the tangent cone
    /// (the directions that stay inside the solid to first order).
    pub fn normal_cone(face_outward_normals: &[Vector3]) -> Self {
        Self::from_generators(face_outward_normals)
    }

    /// Extreme unit rays (V-rep).
    pub fn generators(&self) -> &[Vector3] {
        &self.generators
    }

    /// Outward unit face normals (H-rep).
    pub fn supports(&self) -> &[Vector3] {
        &self.supports
    }

    /// True iff the cone is all of R³ (no supporting constraints).
    pub fn is_full_space(&self) -> bool {
        self.supports.is_empty() && !self.generators.is_empty()
    }

    /// Polar (dual) cone `C° = { y : x·y ≤ 0 ∀ x ∈ C }`. By the polar-duality
    /// theorem this swaps the two representations.
    pub fn polar(&self) -> Self {
        Self {
            generators: self.supports.clone(),
            supports: self.generators.clone(),
        }
    }

    /// Does direction `d` (need not be unit; the zero vector is the apex, in
    /// every cone) lie inside the cone? Tested against the H-rep.
    pub fn contains(&self, d: &Vector3) -> bool {
        if self.is_full_space() {
            return true;
        }
        // A cone with no generators is the trivial cone {0}: only the apex
        // lies in it. (Without this guard the `all()` over empty supports below
        // would vacuously accept every direction.)
        if self.generators.is_empty() {
            return d.magnitude() < CONE_EPS;
        }
        let scale = d.magnitude().max(1.0);
        self.supports.iter().all(|s| d.dot(s) <= CONE_EPS * scale)
    }

    /// True iff the cone is the trivial `{0}` (apex only).
    pub fn is_zero(&self) -> bool {
        self.generators.is_empty() && self.supports.is_empty()
    }

    /// The intersection cone `A ∩ B`.
    ///
    /// `A ∩ B = { x : sⱼ·x ≤ 0 for every support of either cone }`, so its
    /// supports are the union of the two support sets and its extreme rays are
    /// `dual_extreme_rays(union)`. The result is canonicalized (minimal rep).
    /// An empty result is the trivial `{0}` cone (the two cones share only the
    /// apex).
    pub fn intersection(&self, other: &Self) -> PolyhedralCone {
        if self.is_full_space() {
            return other.clone();
        }
        if other.is_full_space() {
            return self.clone();
        }
        if self.is_zero() || other.is_zero() {
            return PolyhedralCone::from_generators(&[]);
        }
        let mut union = self.supports.clone();
        union.extend(other.supports.iter().copied());
        let union = dedup_unit(union);
        let rays = dual_extreme_rays(&union);
        // Canonicalize: rebuild from the extreme rays (empty ⇒ {0} cone).
        PolyhedralCone::from_generators(&rays)
    }

    /// A unit normal `p` of a plane through the origin that separates the two
    /// cones (`A ⊆ {x·p ≤ 0}`, `B ⊆ {x·p ≥ 0}`), or `None` if they share a
    /// non-zero ray.
    ///
    /// The separating normals are exactly `A° ∩ (−B°)`, whose extreme rays are
    /// `dual_extreme_rays(A.generators ∪ −B.generators)` — the slice-1 workhorse
    /// applied to the dual problem. Any such ray is a valid separator.
    pub fn separating_plane(&self, other: &Self) -> Option<Vector3> {
        let mut combined = self.generators.clone();
        combined.extend(other.generators.iter().map(|g| -*g));
        let combined = dedup_unit(combined);
        dual_extreme_rays(&combined).into_iter().next()
    }

    /// Do the two cones share a non-zero ray? On disjoint, carries a separating
    /// plane normal. (Sharing only the apex counts as `Disjoint` — there is no
    /// common contact *direction*.)
    pub fn intersects(&self, other: &Self) -> ConeIntersectionResult {
        if self.intersection(other).is_zero() {
            let p = self.separating_plane(other).unwrap_or(Vector3::Z); // unreachable for genuinely disjoint cones
            ConeIntersectionResult::Disjoint {
                separating_plane: p,
            }
        } else {
            ConeIntersectionResult::Overlapping
        }
    }

    /// Angularly dilate the cone by `half_angle` (radians) — the conservative
    /// over-approximation of the α-neighborhood `{ d : angle(d, C) ≤ α }`.
    ///
    /// This is how a point-wise normal cone is made valid over a curved feature
    /// *area*: `half_angle` bounds how far the surface normal can turn across
    /// the patch, and the dilated cone contains every such normal.
    ///
    /// The α-neighborhood of a convex cone is the conic hull of the α-caps
    /// around its generators, so each generator is ringed with `K` rays at
    /// half-angle `α / cos(π/K)` — a polygon whose *inscribed* circle is exactly
    /// `α`, guaranteeing the result **contains** the true α-neighborhood (it
    /// never under-covers; over-covering is safe for culling). A wide enough
    /// dilation becomes the full space.
    pub fn dilate(&self, half_angle: f64) -> PolyhedralCone {
        if half_angle <= CONE_EPS || self.is_full_space() || self.is_zero() {
            return self.clone();
        }
        const K: usize = 8;
        let alpha = half_angle.min(std::f64::consts::FRAC_PI_2 - CONE_EPS);
        let circum = alpha / (std::f64::consts::PI / K as f64).cos();
        let (cos_a, sin_a) = (circum.cos(), circum.sin());

        let mut rim: Vec<Vector3> = Vec::with_capacity(self.generators.len() * (K + 1));
        for g in &self.generators {
            let e1 = unit_perp(*g);
            let e2 = g.cross(&e1).normalize().unwrap_or(e1);
            rim.push(*g);
            for k in 0..K {
                let phi = std::f64::consts::TAU * (k as f64) / (K as f64);
                let radial = e1 * phi.cos() + e2 * phi.sin();
                rim.push(*g * cos_a + radial * sin_a);
            }
        }
        PolyhedralCone::from_generators(&rim)
    }
}

/// Outcome of [`PolyhedralCone::intersects`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConeIntersectionResult {
    /// The cones share a non-zero ray (a common contact direction exists).
    Overlapping,
    /// The cones meet only at the apex; `separating_plane` is a unit normal `p`
    /// with `A ⊆ {x·p ≤ 0}` and `B ⊆ {x·p ≥ 0}`.
    Disjoint { separating_plane: Vector3 },
}

/// Dedup a set of (already unit-length) vectors by angular proximity.
fn dedup_unit(mut vecs: Vec<Vector3>) -> Vec<Vector3> {
    let mut out: Vec<Vector3> = Vec::new();
    for v in vecs.drain(..) {
        if !out.iter().any(|e| (*e - v).magnitude() < CONE_EPS) {
            out.push(v);
        }
    }
    out
}

/// Extreme rays of the dual cone `{ y : vᵢ·y ≤ 0 ∀ i }`.
///
/// Each extreme ray lies on the intersection of two bounding planes, so it is
/// `± (vᵢ × vⱼ)` for some pair, kept iff it satisfies every remaining
/// constraint. Because generators and supports are polar duals, this single
/// routine performs both V→H and H→V conversion. O(n³) — fine for the small
/// face-counts of vertex cones.
fn dual_extreme_rays(vecs: &[Vector3]) -> Vec<Vector3> {
    let n = vecs.len();
    let mut rays: Vec<Vector3> = Vec::new();
    for i in 0..n {
        for j in (i + 1)..n {
            let cross = vecs[i].cross(&vecs[j]);
            let unit = match cross.normalize() {
                Ok(v) => v,
                Err(_) => continue, // parallel generators — no shared edge
            };
            for &sign in &[1.0_f64, -1.0] {
                let r = unit * sign;
                if vecs.iter().all(|v| v.dot(&r) <= CONE_EPS) {
                    if !rays.iter().any(|e| (*e - r).magnitude() < CONE_EPS) {
                        rays.push(r);
                    }
                }
            }
        }
    }
    rays
}

/// The cone of a single ray `g` (rank 1). Its supports pin a direction to the
/// ray: two orthonormal in-plane normals (forcing `x ⟂ g`) plus `−g` (forcing
/// `x·g ≥ 0`). `contains(λg)` is true for `λ ≥ 0` only.
fn ray_cone(g: Vector3) -> PolyhedralCone {
    let a = unit_perp(g);
    let b = g.cross(&a).normalize().unwrap_or(a);
    PolyhedralCone {
        generators: vec![g],
        supports: vec![a, -a, b, -b, -g],
    }
}

/// If every ray lies in a common plane through the origin, return that plane's
/// unit normal; otherwise `None` (the rays span 3D, or are all parallel).
fn coplanar_normal(gens: &[Vector3]) -> Option<Vector3> {
    // The first non-parallel pair defines a candidate plane. If any generator
    // lies off it, the set spans 3D (rank 3); otherwise they are coplanar.
    for i in 0..gens.len() {
        for j in (i + 1)..gens.len() {
            if let Ok(n) = gens[i].cross(&gens[j]).normalize() {
                if gens.iter().all(|g| g.dot(&n).abs() < CONE_EPS) {
                    return Some(n);
                }
                return None; // an off-plane generator ⇒ rank 3
            }
        }
    }
    None // all parallel ⇒ rank ≤ 1 (handled elsewhere)
}

/// A planar wedge: the convex sector of coplanar rays (in the plane with unit
/// normal `n`). Generators are the two angular extremes; supports are the two
/// plane normals `±n` (pinning `x` into the plane) plus the two in-plane edge
/// normals (pinning `x` to the sector).
fn planar_wedge(gens: &[Vector3], n: Vector3) -> PolyhedralCone {
    use std::f64::consts::{PI, TAU};
    // Angle of each ray in an in-plane basis (u, v).
    let u = gens[0];
    let v = match n.cross(&u).normalize() {
        Ok(v) => v,
        Err(_) => return PolyhedralCone::from_generators(&[gens[0]]),
    };
    let mut ang: Vec<(f64, Vector3)> = gens
        .iter()
        .map(|g| (g.dot(&v).atan2(g.dot(&u)), *g))
        .collect();
    ang.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // The convex sector is the complement of the LARGEST circular gap; its
    // bounding rays are the extremes.
    let m = ang.len();
    let (mut max_gap, mut gap_at) = (-1.0_f64, 0usize);
    for i in 0..m {
        let next = (i + 1) % m;
        let mut gap = ang[next].0 - ang[i].0;
        if next == 0 {
            gap += TAU;
        }
        if gap > max_gap {
            max_gap = gap;
            gap_at = i;
        }
    }
    let e_lo = ang[(gap_at + 1) % m].1; // ray just after the gap
    let e_hi = ang[gap_at].1; // ray just before the gap

    // If the rays span ≥ π the convex cone is a half-plane (or larger): fall
    // back to just the plane constraints (a conservative over-approximation).
    if max_gap <= PI + CONE_EPS {
        return PolyhedralCone {
            generators: gens.to_vec(),
            supports: vec![n, -n],
        };
    }

    // In-plane outward edge normals (pointing out of the sector).
    let edge_support = |edge: Vector3, other: Vector3| -> Vector3 {
        let c = n.cross(&edge).normalize().unwrap_or(edge);
        if other.dot(&c) <= 0.0 {
            c
        } else {
            -c
        }
    };
    let s_lo = edge_support(e_lo, e_hi);
    let s_hi = edge_support(e_hi, e_lo);
    PolyhedralCone {
        generators: vec![e_lo, e_hi],
        supports: vec![n, -n, s_lo, s_hi],
    }
}

/// A unit vector perpendicular to `v` (`v` assumed nonzero/unit).
fn unit_perp(v: Vector3) -> Vector3 {
    let seed = if v.x.abs() < 0.9 {
        Vector3::X
    } else {
        Vector3::Y
    };
    (seed - v * seed.dot(&v)).normalize().unwrap_or(Vector3::X)
}

/// Normalize each ray and drop near-duplicates and zeros.
fn normalize_dedup(rays: &[Vector3]) -> Vec<Vector3> {
    let mut out: Vec<Vector3> = Vec::new();
    for r in rays {
        if let Ok(u) = r.normalize() {
            if !out.iter().any(|e| (*e - u).magnitude() < CONE_EPS) {
                out.push(u);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // ---- known closed-form: cube vertex ---------------------------------

    #[test]
    fn cube_vertex_normal_cone_supports() {
        // Vertex of [0,L]³ at the +++ corner: incident faces' outward normals
        // are +X,+Y,+Z; the normal cone is the +++ octant, supports the −−−.
        let c = PolyhedralCone::normal_cone(&[Vector3::X, Vector3::Y, Vector3::Z]);
        assert_eq!(c.supports().len(), 3);
        for expected in [-Vector3::X, -Vector3::Y, -Vector3::Z] {
            assert!(
                c.supports()
                    .iter()
                    .any(|s| (*s - expected).magnitude() < 1e-9),
                "missing support {expected:?} in {:?}",
                c.supports()
            );
        }
    }

    #[test]
    fn cube_vertex_membership() {
        let c = PolyhedralCone::normal_cone(&[Vector3::X, Vector3::Y, Vector3::Z]);
        assert!(c.contains(&Vector3::new(1.0, 1.0, 1.0)), "interior");
        assert!(c.contains(&Vector3::X), "boundary ray");
        assert!(!c.contains(&Vector3::new(-1.0, -1.0, -1.0)), "antipode");
        assert!(!c.contains(&(-Vector3::X)), "opposite face");
    }

    #[test]
    fn cube_vertex_tangent_cone_is_opposite_octant() {
        // Polar of the +++ normal cone is the −−− tangent cone (inward dirs).
        let tangent = PolyhedralCone::normal_cone(&[Vector3::X, Vector3::Y, Vector3::Z]).polar();
        assert!(tangent.contains(&Vector3::new(-1.0, -1.0, -1.0)), "inward");
        assert!(!tangent.contains(&Vector3::new(1.0, 1.0, 1.0)), "outward");
    }

    // ---- property tests over random pointed cones -----------------------

    /// A pointed cone: 3–5 generators all within the hemisphere around +Z
    /// (z-component ≥ 0.3), so the cone is strictly contained in z > 0 and its
    /// antipodes are guaranteed outside.
    fn pointed_cone() -> impl Strategy<Value = (PolyhedralCone, Vec<Vector3>)> {
        prop::collection::vec((-1.0f64..1.0, -1.0f64..1.0, 0.3f64..1.0), 3..6).prop_filter_map(
            "non-degenerate pointed cone",
            |raw| {
                let gens: Vec<Vector3> = raw
                    .into_iter()
                    .filter_map(|(x, y, z)| Vector3::new(x, y, z).normalize().ok())
                    .collect();
                if gens.len() < 3 {
                    return None;
                }
                let cone = PolyhedralCone::from_generators(&gens);
                // Need a genuinely bounded (pointed, supported) cone for the
                // antipode oracle to hold.
                if cone.supports().is_empty() {
                    return None;
                }
                Some((cone, gens))
            },
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// Every generator lies in its own cone.
        #[test]
        fn generators_are_contained((cone, gens) in pointed_cone()) {
            for g in &gens {
                prop_assert!(cone.contains(g), "generator {g:?} not in cone {cone:?}");
            }
        }

        /// Any non-negative combination of generators is inside; its antipode
        /// (for a pointed cone) is outside. Labels membership without solving it.
        #[test]
        fn conic_combos_in_antipodes_out(
            (cone, gens) in pointed_cone(),
            weights in prop::collection::vec(0.0f64..1.0, 3..6),
        ) {
            let mut d = Vector3::ZERO;
            for (g, &w) in gens.iter().zip(weights.iter()) {
                d = d + *g * w;
            }
            if d.magnitude() < 1e-6 {
                return Ok(()); // all-zero weights → apex, skip
            }
            prop_assert!(cone.contains(&d), "conic combo {d:?} not in cone");
            prop_assert!(!cone.contains(&(-d)), "antipode {:?} in pointed cone", -d);
        }

        /// Polar duality round-trips: recomputing the extreme rays of the dual
        /// of the supports recovers the generators (as a set). This validates
        /// the V↔H conversion against itself.
        #[test]
        fn duality_round_trips((cone, _gens) in pointed_cone()) {
            let recovered = super::dual_extreme_rays(cone.supports());
            // recovered should equal cone.generators() as a set.
            for g in cone.generators() {
                prop_assert!(
                    recovered.iter().any(|r| (*r - *g).magnitude() < 1e-7),
                    "generator {g:?} not recovered from supports; got {recovered:?}"
                );
            }
            prop_assert_eq!(recovered.len(), cone.generators().len());
        }

        /// `polar` is an involution: `(C°)° == C`.
        #[test]
        fn polar_is_involution((cone, _gens) in pointed_cone()) {
            let back = cone.polar().polar();
            prop_assert_eq!(back.generators().len(), cone.generators().len());
            for g in cone.generators() {
                prop_assert!(back.generators().iter().any(|h| (*h - *g).magnitude() < 1e-9));
            }
        }
    }

    // ---- helpers --------------------------------------------------------

    fn nrm(x: f64, y: f64, z: f64) -> Vector3 {
        Vector3::new(x, y, z).normalize().expect("nonzero")
    }
    fn has_vec(set: &[Vector3], v: Vector3) -> bool {
        set.iter().any(|s| (*s - v).magnitude() < 1e-9)
    }

    // ---- more known closed-form cases -----------------------------------

    #[test]
    fn cube_vertex_neg_corner_supports() {
        let c = PolyhedralCone::normal_cone(&[-Vector3::X, -Vector3::Y, -Vector3::Z]);
        assert_eq!(c.supports().len(), 3);
        for s in [Vector3::X, Vector3::Y, Vector3::Z] {
            assert!(has_vec(c.supports(), s), "missing {s:?}");
        }
    }

    #[test]
    fn square_pyramid_apex_has_four_supports() {
        let g = [
            nrm(1.0, 1.0, 1.0),
            nrm(1.0, -1.0, 1.0),
            nrm(-1.0, -1.0, 1.0),
            nrm(-1.0, 1.0, 1.0),
        ];
        let c = PolyhedralCone::from_generators(&g);
        assert_eq!(c.generators().len(), 4, "four extreme rays");
        assert_eq!(c.supports().len(), 4, "four faces");
        assert!(c.contains(&Vector3::Z), "axis is interior");
        assert!(!c.contains(&(-Vector3::Z)), "anti-axis outside");
    }

    #[test]
    fn pentagonal_pyramid_apex_has_five_supports() {
        let g: Vec<Vector3> = (0..5)
            .map(|k| {
                let a = std::f64::consts::TAU * (k as f64) / 5.0;
                nrm(a.cos(), a.sin(), 1.0)
            })
            .collect();
        let c = PolyhedralCone::from_generators(&g);
        assert_eq!(c.generators().len(), 5);
        assert_eq!(c.supports().len(), 5);
        assert!(c.contains(&Vector3::Z));
    }

    #[test]
    fn tetra_apex_three_supports() {
        let g = [
            nrm(1.0, 1.0, 1.0),
            nrm(1.0, -1.0, -1.0),
            nrm(-1.0, 1.0, -1.0),
        ];
        let c = PolyhedralCone::from_generators(&g);
        assert_eq!(c.supports().len(), 3);
    }

    // ---- canonicalization / cleaning ------------------------------------

    #[test]
    fn redundant_interior_generator_dropped() {
        // (1,1,1) is interior to cone(X,Y,Z) — not extreme.
        let c = PolyhedralCone::from_generators(&[
            Vector3::X,
            Vector3::Y,
            Vector3::Z,
            Vector3::new(1.0, 1.0, 1.0),
        ]);
        assert_eq!(c.generators().len(), 3, "redundant ray removed");
        assert_eq!(c.supports().len(), 3);
    }

    #[test]
    fn duplicate_generators_deduped() {
        let c = PolyhedralCone::from_generators(&[Vector3::X, Vector3::Y, Vector3::Z, Vector3::X]);
        assert_eq!(c.generators().len(), 3);
    }

    #[test]
    fn zero_rays_dropped() {
        let c =
            PolyhedralCone::from_generators(&[Vector3::X, Vector3::Y, Vector3::Z, Vector3::ZERO]);
        assert_eq!(c.generators().len(), 3);
    }

    #[test]
    fn non_unit_rays_are_normalized() {
        let c = PolyhedralCone::from_generators(&[
            Vector3::X * 5.0,
            Vector3::Y * 3.0,
            Vector3::Z * 0.1,
        ]);
        for g in c.generators() {
            assert!(
                (g.magnitude() - 1.0).abs() < 1e-9,
                "generator not unit: {g:?}"
            );
        }
        for s in c.supports() {
            assert!(
                (s.magnitude() - 1.0).abs() < 1e-9,
                "support not unit: {s:?}"
            );
        }
    }

    #[test]
    fn permutation_invariant_supports() {
        let a = PolyhedralCone::normal_cone(&[Vector3::X, Vector3::Y, Vector3::Z]);
        let b = PolyhedralCone::normal_cone(&[Vector3::Z, Vector3::X, Vector3::Y]);
        assert_eq!(a.supports().len(), b.supports().len());
        for s in a.supports() {
            assert!(
                has_vec(b.supports(), *s),
                "support {s:?} not permutation-stable"
            );
        }
    }

    // ---- membership edge cases ------------------------------------------

    #[test]
    fn apex_is_in_every_cone() {
        let c = PolyhedralCone::normal_cone(&[Vector3::X, Vector3::Y, Vector3::Z]);
        assert!(c.contains(&Vector3::ZERO));
    }

    #[test]
    fn membership_is_scale_invariant() {
        let c = PolyhedralCone::normal_cone(&[Vector3::X, Vector3::Y, Vector3::Z]);
        let d = Vector3::new(1.0, 2.0, 3.0);
        assert_eq!(c.contains(&d), c.contains(&(d * 10.0)));
        assert_eq!(c.contains(&d), c.contains(&(d * 0.01)));
    }

    #[test]
    fn just_inside_and_outside_a_face() {
        let c = PolyhedralCone::normal_cone(&[Vector3::X, Vector3::Y, Vector3::Z]);
        assert!(
            c.contains(&Vector3::new(0.01, 1.0, 1.0)),
            "just inside +x face"
        );
        assert!(
            !c.contains(&Vector3::new(-0.01, 1.0, 1.0)),
            "just outside +x face"
        );
    }

    #[test]
    fn midpoint_of_two_generators_is_contained() {
        let c = PolyhedralCone::normal_cone(&[Vector3::X, Vector3::Y, Vector3::Z]);
        assert!(c.contains(&(Vector3::X + Vector3::Y)));
        assert!(c.contains(&(Vector3::X + Vector3::Z)));
    }

    #[test]
    fn negated_support_is_inside_the_octant() {
        // For the octant, supports are −X,−Y,−Z; negating one gives a boundary
        // generator (X / Y / Z), which is contained.
        let c = PolyhedralCone::normal_cone(&[Vector3::X, Vector3::Y, Vector3::Z]);
        for s in c.supports() {
            assert!(
                c.contains(&(-*s)),
                "negated support {:?} should be inside",
                -*s
            );
        }
    }

    // ---- full space / zero cone -----------------------------------------

    #[test]
    fn full_space_contains_everything() {
        let c = PolyhedralCone::full_space();
        assert!(c.is_full_space());
        assert!(c.contains(&Vector3::new(7.0, -3.0, 2.0)));
        assert!(c.contains(&(-Vector3::Z)));
    }

    #[test]
    fn generators_spanning_r3_is_full_space() {
        // Six axis directions positively span R³.
        let c = PolyhedralCone::from_generators(&[
            Vector3::X,
            -Vector3::X,
            Vector3::Y,
            -Vector3::Y,
            Vector3::Z,
            -Vector3::Z,
        ]);
        assert!(c.supports().is_empty(), "spanning cone has no supports");
        assert!(c.contains(&Vector3::new(1.0, -1.0, 1.0)));
    }

    #[test]
    fn empty_cone_contains_only_apex() {
        let c = PolyhedralCone::from_generators(&[]);
        assert!(c.is_zero());
        assert!(c.contains(&Vector3::ZERO));
        assert!(!c.contains(&Vector3::X));
    }

    // ---- polar / tangent relationships ----------------------------------

    #[test]
    fn polar_swaps_generators_and_supports() {
        let c = PolyhedralCone::normal_cone(&[Vector3::X, Vector3::Y, Vector3::Z]);
        let p = c.polar();
        assert_eq!(p.generators().len(), c.supports().len());
        for s in c.supports() {
            assert!(has_vec(p.generators(), *s));
        }
        for g in c.generators() {
            assert!(has_vec(p.supports(), *g));
        }
    }

    #[test]
    fn tangent_cone_contains_inward_axis() {
        let tangent = PolyhedralCone::normal_cone(&[Vector3::X, Vector3::Y, Vector3::Z]).polar();
        assert!(tangent.contains(&Vector3::new(-1.0, -1.0, -1.0)));
        assert!(!tangent.contains(&Vector3::new(1.0, 1.0, 1.0)));
    }

    #[test]
    fn narrow_cone_excludes_side_directions() {
        // Three rays bunched near +Z.
        let c = PolyhedralCone::from_generators(&[
            nrm(0.1, 0.0, 1.0),
            nrm(-0.05, 0.087, 1.0),
            nrm(-0.05, -0.087, 1.0),
        ]);
        assert!(c.contains(&Vector3::Z), "axis inside narrow cone");
        assert!(!c.contains(&Vector3::X), "side direction outside");
        assert!(!c.contains(&(-Vector3::Z)), "anti-axis outside");
    }

    #[test]
    fn supports_are_outward_on_octant() {
        let c = PolyhedralCone::normal_cone(&[Vector3::X, Vector3::Y, Vector3::Z]);
        for g in c.generators() {
            for s in c.supports() {
                assert!(
                    g.dot(s) <= 1e-9,
                    "generator {g:?} not on inner side of {s:?}"
                );
            }
        }
    }

    #[test]
    fn centroid_direction_is_strictly_interior() {
        let c = PolyhedralCone::normal_cone(&[Vector3::X, Vector3::Y, Vector3::Z]);
        let centroid = Vector3::X + Vector3::Y + Vector3::Z;
        assert!(c.contains(&centroid));
        for s in c.supports() {
            assert!(
                centroid.dot(s) < -1e-6,
                "centroid not strictly inside {s:?}"
            );
        }
    }

    // ---- more property tests --------------------------------------------

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// Membership is invariant to positive scaling of the query direction.
        #[test]
        fn prop_scale_invariance(
            (cone, _gens) in pointed_cone(),
            x in -3.0f64..3.0, y in -3.0f64..3.0, z in -3.0f64..3.0,
            k in 0.01f64..100.0,
        ) {
            let d = Vector3::new(x, y, z);
            if d.magnitude() < 1e-6 { return Ok(()); }
            prop_assert_eq!(cone.contains(&d), cone.contains(&(d * k)));
        }

        /// Every support is unit length and outward (non-positive on all generators).
        #[test]
        fn prop_supports_unit_and_outward((cone, _gens) in pointed_cone()) {
            for s in cone.supports() {
                prop_assert!((s.magnitude() - 1.0).abs() < 1e-9, "support not unit");
                for g in cone.generators() {
                    prop_assert!(g.dot(s) <= 1e-7, "generator on wrong side of support");
                }
            }
        }

        /// Every generator is unit length.
        #[test]
        fn prop_generators_unit((cone, _gens) in pointed_cone()) {
            for g in cone.generators() {
                prop_assert!((g.magnitude() - 1.0).abs() < 1e-9);
            }
        }

        /// The apex (origin) is in every cone.
        #[test]
        fn prop_apex_contained((cone, _gens) in pointed_cone()) {
            prop_assert!(cone.contains(&Vector3::ZERO));
        }

        /// Building the cone from a permutation of the input rays yields the
        /// same support set.
        #[test]
        fn prop_permutation_invariance((_cone, gens) in pointed_cone()) {
            let mut rev = gens.clone();
            rev.reverse();
            let a = PolyhedralCone::from_generators(&gens);
            let b = PolyhedralCone::from_generators(&rev);
            prop_assert_eq!(a.supports().len(), b.supports().len());
            for s in a.supports() {
                prop_assert!(b.supports().iter().any(|t| (*t - *s).magnitude() < 1e-7));
            }
        }

        /// Convexity: the sum (a conic combination) of any two generators is
        /// contained.
        #[test]
        fn prop_sum_of_two_generators_contained((cone, _gens) in pointed_cone()) {
            let gens = cone.generators();
            if gens.len() >= 2 {
                let d = gens[0] + gens[1];
                prop_assert!(cone.contains(&d), "sum of two generators not contained");
            }
        }

        /// The centroid (sum) of all generators is contained.
        #[test]
        fn prop_centroid_contained((cone, _gens) in pointed_cone()) {
            let mut d = Vector3::ZERO;
            for g in cone.generators() {
                d = d + *g;
            }
            if d.magnitude() > 1e-6 {
                prop_assert!(cone.contains(&d));
            }
        }

        /// Canonicalization is idempotent: rebuilding from the canonical
        /// generators reproduces the same cone.
        #[test]
        fn prop_canonicalization_idempotent((cone, _gens) in pointed_cone()) {
            let rebuilt = PolyhedralCone::from_generators(cone.generators());
            prop_assert_eq!(rebuilt.generators().len(), cone.generators().len());
            prop_assert_eq!(rebuilt.supports().len(), cone.supports().len());
            for g in cone.generators() {
                prop_assert!(rebuilt.generators().iter().any(|h| (*h - *g).magnitude() < 1e-7));
            }
        }

        /// The polar's supports are exactly the original generators.
        #[test]
        fn prop_polar_supports_are_generators((cone, _gens) in pointed_cone()) {
            let p = cone.polar();
            prop_assert_eq!(p.supports().len(), cone.generators().len());
            for g in cone.generators() {
                prop_assert!(p.supports().iter().any(|s| (*s - *g).magnitude() < 1e-9));
            }
        }

        /// A direction strictly inside (the generator centroid, nudged) stays
        /// contained; its antipode does not.
        #[test]
        fn prop_interior_and_antipode((cone, _gens) in pointed_cone()) {
            let mut c = Vector3::ZERO;
            for g in cone.generators() { c = c + *g; }
            if c.magnitude() < 1e-6 { return Ok(()); }
            prop_assert!(cone.contains(&c));
            prop_assert!(!cone.contains(&(-c)), "antipode of interior in pointed cone");
        }
    }

    // ---- batch 3: more configurations + structural invariants -----------

    #[test]
    fn obtuse_three_face_corner() {
        // Three faces tilted up, 120° apart in the xy plane — a wide corner.
        let c = PolyhedralCone::from_generators(&[
            nrm(1.0, 0.0, 0.3),
            nrm(-0.5, 0.866, 0.3),
            nrm(-0.5, -0.866, 0.3),
        ]);
        assert_eq!(c.supports().len(), 3);
        assert!(c.contains(&Vector3::Z));
        assert!(!c.contains(&(-Vector3::Z)));
    }

    #[test]
    fn acute_three_face_corner() {
        let c = PolyhedralCone::from_generators(&[
            nrm(0.2, 0.0, 1.0),
            nrm(-0.1, 0.17, 1.0),
            nrm(-0.1, -0.17, 1.0),
        ]);
        assert_eq!(c.supports().len(), 3);
        assert!(c.contains(&Vector3::Z));
    }

    #[test]
    fn hexagonal_fan_has_six_supports() {
        let g: Vec<Vector3> = (0..6)
            .map(|k| {
                let a = std::f64::consts::TAU * (k as f64) / 6.0;
                nrm(a.cos(), a.sin(), 1.0)
            })
            .collect();
        let c = PolyhedralCone::from_generators(&g);
        assert_eq!(c.generators().len(), 6);
        assert_eq!(c.supports().len(), 6);
    }

    #[test]
    fn octant_supports_are_distinct() {
        let c = PolyhedralCone::normal_cone(&[Vector3::X, Vector3::Y, Vector3::Z]);
        let s = c.supports();
        for i in 0..s.len() {
            for j in (i + 1)..s.len() {
                assert!((s[i] - s[j]).magnitude() > 1e-6, "duplicate support");
            }
        }
    }

    #[test]
    fn polar_of_full_space_is_zero_cone() {
        let z = PolyhedralCone::full_space().polar();
        assert!(z.contains(&Vector3::ZERO));
        assert!(!z.contains(&Vector3::X), "polar of R³ is {{0}}");
    }

    #[test]
    fn different_corners_have_different_supports() {
        let a = PolyhedralCone::normal_cone(&[Vector3::X, Vector3::Y, Vector3::Z]);
        let b = PolyhedralCone::normal_cone(&[Vector3::X, Vector3::Y, -Vector3::Z]);
        // The −Z-flipped corner must differ in at least one support.
        assert!(
            a.supports().iter().any(|s| !has_vec(b.supports(), *s)),
            "distinct corners share all supports"
        );
    }

    #[test]
    fn cone_edge_direction_is_on_boundary() {
        // X+Y is the edge of the octant between the −X and −Y faces (and on the
        // −Z face plane) — contained, on the boundary.
        let c = PolyhedralCone::normal_cone(&[Vector3::X, Vector3::Y, Vector3::Z]);
        assert!(c.contains(&(Vector3::X + Vector3::Y)));
    }

    #[test]
    fn mixed_sign_corner_three_supports() {
        let c = PolyhedralCone::normal_cone(&[Vector3::X, Vector3::Y, -Vector3::Z]);
        assert_eq!(c.supports().len(), 3);
        assert!(
            c.contains(&Vector3::new(1.0, 1.0, -1.0)),
            "interior of mixed corner"
        );
    }

    #[test]
    fn each_octant_generator_lies_on_two_faces() {
        // Every extreme ray is the intersection of ≥2 supporting planes.
        let c = PolyhedralCone::normal_cone(&[Vector3::X, Vector3::Y, Vector3::Z]);
        for g in c.generators() {
            let on = c
                .supports()
                .iter()
                .filter(|s| g.dot(s).abs() < 1e-9)
                .count();
            assert!(on >= 2, "generator {g:?} on only {on} faces");
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// No two supports are near-duplicates.
        #[test]
        fn prop_supports_distinct((cone, _gens) in pointed_cone()) {
            let s = cone.supports();
            for i in 0..s.len() {
                for j in (i + 1)..s.len() {
                    prop_assert!((s[i] - s[j]).magnitude() > 1e-7, "duplicate support");
                }
            }
        }

        /// No two generators are near-duplicates.
        #[test]
        fn prop_generators_distinct((cone, _gens) in pointed_cone()) {
            let g = cone.generators();
            for i in 0..g.len() {
                for j in (i + 1)..g.len() {
                    prop_assert!((g[i] - g[j]).magnitude() > 1e-7, "duplicate generator");
                }
            }
        }

        /// Structural: every extreme ray (generator) is the intersection of at
        /// least two supporting planes.
        #[test]
        fn prop_generator_on_two_faces((cone, _gens) in pointed_cone()) {
            for g in cone.generators() {
                let on = cone.supports().iter().filter(|s| g.dot(s).abs() < 1e-6).count();
                prop_assert!(on >= 2, "generator on fewer than 2 faces: {on}");
            }
        }
    }

    // ===== slice 2: cone intersection ====================================

    fn octant() -> PolyhedralCone {
        PolyhedralCone::normal_cone(&[Vector3::X, Vector3::Y, Vector3::Z])
    }

    #[test]
    fn cone_intersects_itself() {
        assert_eq!(
            octant().intersects(&octant()),
            ConeIntersectionResult::Overlapping
        );
    }

    #[test]
    fn opposite_octants_are_disjoint() {
        let pos = octant();
        let neg = PolyhedralCone::normal_cone(&[-Vector3::X, -Vector3::Y, -Vector3::Z]);
        match pos.intersects(&neg) {
            ConeIntersectionResult::Disjoint {
                separating_plane: p,
            } => {
                for g in pos.generators() {
                    assert!(g.dot(&p) <= 1e-7, "A-gen {g:?} not on ≤0 side");
                }
                for h in neg.generators() {
                    assert!(h.dot(&p) >= -1e-7, "B-gen {h:?} not on ≥0 side");
                }
            }
            ConeIntersectionResult::Overlapping => panic!("opposite octants overlap?"),
        }
    }

    #[test]
    fn octant_intersects_full_space() {
        assert_eq!(
            octant().intersects(&PolyhedralCone::full_space()),
            ConeIntersectionResult::Overlapping
        );
    }

    #[test]
    fn zero_cone_is_disjoint_from_octant() {
        let zero = PolyhedralCone::from_generators(&[]);
        assert!(matches!(
            zero.intersects(&octant()),
            ConeIntersectionResult::Disjoint { .. }
        ));
    }

    #[test]
    fn normal_cone_disjoint_from_its_tangent_cone() {
        let normal = octant();
        let tangent = normal.polar(); // −−− octant; shares only the apex
        assert!(matches!(
            normal.intersects(&tangent),
            ConeIntersectionResult::Disjoint { .. }
        ));
    }

    #[test]
    fn corners_sharing_a_quarter_plane_overlap() {
        // cone(X,Y,Z) and cone(X,Y,−Z) share the z=0, x,y≥0 quarter.
        let a = octant();
        let b = PolyhedralCone::normal_cone(&[Vector3::X, Vector3::Y, -Vector3::Z]);
        assert_eq!(a.intersects(&b), ConeIntersectionResult::Overlapping);
        // the witness: X+Y is in both.
        let inter = a.intersection(&b);
        assert!(!inter.is_zero());
        for g in inter.generators() {
            assert!(a.contains(g), "intersection gen {g:?} not in A");
            assert!(b.contains(g), "intersection gen {g:?} not in B");
        }
    }

    #[test]
    fn disjoint_intersection_is_the_zero_cone() {
        let pos = octant();
        let neg = PolyhedralCone::normal_cone(&[-Vector3::X, -Vector3::Y, -Vector3::Z]);
        assert!(pos.intersection(&neg).is_zero());
    }

    #[test]
    fn intersection_is_symmetric_on_known_cases() {
        let a = octant();
        let b = PolyhedralCone::normal_cone(&[Vector3::X, Vector3::Y, -Vector3::Z]);
        assert_eq!(a.intersects(&b), b.intersects(&a));
    }

    // ---- property tests over random cones around arbitrary axes ----------

    fn unit_vec() -> impl Strategy<Value = Vector3> {
        (-1.0f64..1.0, -1.0f64..1.0, -1.0f64..1.0).prop_filter_map("nonzero", |(x, y, z)| {
            Vector3::new(x, y, z).normalize().ok()
        })
    }

    fn perp(a: Vector3) -> Vector3 {
        let t = if a.x.abs() < 0.9 {
            Vector3::X
        } else {
            Vector3::Y
        };
        (t - a * t.dot(&a)).normalize().unwrap_or(Vector3::X)
    }

    /// A pointed cone around a random axis (so two of them may or may not meet).
    fn any_cone() -> impl Strategy<Value = PolyhedralCone> {
        (
            unit_vec(),
            prop::collection::vec((-0.6f64..0.6, -0.6f64..0.6), 3..6),
        )
            .prop_filter_map("pointed cone", |(axis, offs)| {
                let e1 = perp(axis);
                let e2 = axis.cross(&e1).normalize().ok()?;
                let gens: Vec<Vector3> = offs
                    .iter()
                    .filter_map(|&(a, b)| (axis + e1 * a + e2 * b).normalize().ok())
                    .collect();
                if gens.len() < 3 {
                    return None;
                }
                let c = PolyhedralCone::from_generators(&gens);
                if c.supports().is_empty() {
                    None
                } else {
                    Some(c)
                }
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// A returned separating plane genuinely separates the two cones.
        #[test]
        fn prop_separating_plane_separates(a in any_cone(), b in any_cone()) {
            if let ConeIntersectionResult::Disjoint { separating_plane: p } = a.intersects(&b) {
                for g in a.generators() {
                    prop_assert!(g.dot(&p) <= 1e-6, "A-gen {g:?} on wrong side of {p:?}");
                }
                for h in b.generators() {
                    prop_assert!(h.dot(&p) >= -1e-6, "B-gen {h:?} on wrong side of {p:?}");
                }
            }
        }

        /// Overlapping carries a witness: the intersection cone is non-trivial
        /// and every one of its generators lies in BOTH cones.
        #[test]
        fn prop_overlap_has_witness(a in any_cone(), b in any_cone()) {
            if a.intersects(&b) == ConeIntersectionResult::Overlapping {
                let inter = a.intersection(&b);
                prop_assert!(!inter.is_zero(), "overlap but empty intersection");
                for g in inter.generators() {
                    prop_assert!(a.contains(g), "intersection gen not in A");
                    prop_assert!(b.contains(g), "intersection gen not in B");
                }
            }
        }

        /// The intersection verdict is symmetric.
        #[test]
        fn prop_intersection_symmetric(a in any_cone(), b in any_cone()) {
            let ab = matches!(a.intersects(&b), ConeIntersectionResult::Overlapping);
            let ba = matches!(b.intersects(&a), ConeIntersectionResult::Overlapping);
            prop_assert_eq!(ab, ba);
        }

        /// Every cone overlaps itself.
        #[test]
        fn prop_self_overlap(a in any_cone()) {
            prop_assert_eq!(a.intersects(&a), ConeIntersectionResult::Overlapping);
        }

        /// Brute-force agreement: if any sampled direction lies in both cones,
        /// the verdict must be Overlapping.
        #[test]
        fn prop_bruteforce_overlap(
            a in any_cone(),
            b in any_cone(),
            samples in prop::collection::vec(unit_vec(), 64),
        ) {
            let common = samples.iter().any(|d| a.contains(d) && b.contains(d));
            if common {
                prop_assert_eq!(
                    a.intersects(&b),
                    ConeIntersectionResult::Overlapping,
                    "a sampled direction is in both but verdict is Disjoint"
                );
            }
        }
    }

    // ===== slice 3: angular dilation =====================================

    /// Rotate unit `v` by `theta` toward the perpendicular direction at angle
    /// `phi` (in the plane perpendicular to `v`).
    fn rotate(v: Vector3, phi: f64, theta: f64) -> Vector3 {
        let e1 = super::unit_perp(v);
        let e2 = v.cross(&e1).normalize().unwrap_or(e1);
        let radial = e1 * phi.cos() + e2 * phi.sin();
        v * theta.cos() + radial * theta.sin()
    }

    #[test]
    fn dilate_by_zero_is_identity() {
        let c = octant();
        let d = c.dilate(0.0);
        assert_eq!(d.supports().len(), c.supports().len());
        assert!(d.contains(&Vector3::X) && !d.contains(&(-Vector3::X)));
    }

    #[test]
    fn dilate_contains_original_generators() {
        let c = octant();
        let d = c.dilate(0.2);
        for g in c.generators() {
            assert!(d.contains(g), "dilated cone lost generator {g:?}");
        }
    }

    #[test]
    fn dilate_full_space_stays_full_space() {
        assert!(PolyhedralCone::full_space().dilate(0.3).is_full_space());
    }

    #[test]
    fn dilate_zero_cone_stays_zero() {
        assert!(PolyhedralCone::from_generators(&[]).dilate(0.3).is_zero());
    }

    #[test]
    fn dilate_widens_a_narrow_cone() {
        // A narrow cone around +Z; a direction 0.1 rad off a generator is
        // outside the original but inside the cone dilated by 0.3.
        let c = PolyhedralCone::from_generators(&[
            nrm(0.05, 0.0, 1.0),
            nrm(-0.025, 0.043, 1.0),
            nrm(-0.025, -0.043, 1.0),
        ]);
        let g = c.generators()[0];
        let just_outside = rotate(g, 0.0, 0.1);
        assert!(
            !c.contains(&just_outside),
            "should start outside the narrow cone"
        );
        assert!(
            c.dilate(0.3).contains(&just_outside),
            "dilation should admit it"
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// Dilation never drops an original generator.
        #[test]
        fn prop_dilate_contains_original(c in any_cone(), alpha in 0.0f64..0.5) {
            let d = c.dilate(alpha);
            for g in c.generators() {
                prop_assert!(d.contains(g));
            }
        }

        /// CONSERVATIVENESS: any generator rotated by θ ≤ α lands inside the
        /// cone dilated by α. (The safety property — dilation never misses a
        /// direction within α of the cone.)
        #[test]
        fn prop_dilate_is_conservative(
            c in any_cone(),
            alpha in 0.05f64..0.5,
            gi in 0usize..8,
            phi in 0.0f64..std::f64::consts::TAU,
            frac in 0.0f64..1.0,
        ) {
            let g = c.generators()[gi % c.generators().len()];
            let theta = alpha * frac; // θ ≤ α
            let d = c.dilate(alpha);
            prop_assert!(
                d.contains(&rotate(g, phi, theta)),
                "direction within α of a generator not in dilate(α)"
            );
        }

        /// Conservativeness on an INTERIOR direction (a conic combination),
        /// not just a generator.
        #[test]
        fn prop_dilate_conservative_interior(
            c in any_cone(),
            alpha in 0.05f64..0.4,
            phi in 0.0f64..std::f64::consts::TAU,
            frac in 0.0f64..1.0,
        ) {
            let g = c.generators();
            let mut interior = Vector3::ZERO;
            for r in g { interior = interior + *r; }
            if interior.magnitude() < 1e-6 { return Ok(()); }
            let interior = interior.normalize().unwrap();
            let d = c.dilate(alpha);
            prop_assert!(d.contains(&rotate(interior, phi, alpha * frac)));
        }

        /// Monotonicity: a smaller dilation is contained in a larger one.
        #[test]
        fn prop_dilate_monotone(c in any_cone(), a in 0.05f64..0.25, extra in 0.05f64..0.25) {
            let small = c.dilate(a);
            let large = c.dilate(a + extra);
            for g in small.generators() {
                prop_assert!(large.contains(g), "dilate(α) ⊄ dilate(β) for α<β");
            }
        }
    }

    // ===== slice 4: planar (edge) cones + ray cones ======================

    #[test]
    fn box_edge_normal_cone_is_a_quarter() {
        // A convex box edge where the x=const and y=const faces meet: outward
        // normals X and Y. The normal cone is the +X+Y quarter in the z=0 plane.
        let c = PolyhedralCone::from_generators(&[Vector3::X, Vector3::Y]);
        assert_eq!(c.generators().len(), 2);
        assert!(
            c.contains(&(Vector3::X + Vector3::Y)),
            "interior of quarter"
        );
        assert!(c.contains(&Vector3::X), "boundary ray X");
        assert!(!c.contains(&(Vector3::X - Vector3::Y)), "below the quarter");
        assert!(!c.contains(&Vector3::Z), "out of plane");
        assert!(!c.contains(&(-Vector3::X - Vector3::Y)), "opposite quarter");
    }

    #[test]
    fn box_edge_tangent_cone_is_a_dihedral_wedge() {
        // Polar of the edge normal cone = the inward dihedral wedge
        // {x ≤ 0, y ≤ 0, z free}.
        let tangent = PolyhedralCone::from_generators(&[Vector3::X, Vector3::Y]).polar();
        assert!(
            tangent.contains(&Vector3::new(-1.0, -1.0, 5.0)),
            "inward, any z"
        );
        assert!(
            tangent.contains(&Vector3::new(-1.0, -1.0, -5.0)),
            "inward, any z"
        );
        assert!(!tangent.contains(&Vector3::new(1.0, 1.0, 0.0)), "outward");
    }

    #[test]
    fn edge_wedge_three_coplanar_rays_keeps_extremes() {
        // Three coplanar rays (in the z=0 plane); the wedge is the sector
        // between the two angular extremes.
        let c = PolyhedralCone::from_generators(&[
            nrm(1.0, 0.0, 0.0),
            nrm(1.0, 1.0, 0.0),
            nrm(0.0, 1.0, 0.0),
        ]);
        assert_eq!(c.generators().len(), 2, "extreme rays only");
        assert!(c.contains(&nrm(1.0, 1.0, 0.0)), "middle ray inside");
        assert!(!c.contains(&nrm(1.0, -0.2, 0.0)), "just outside one edge");
        assert!(!c.contains(&Vector3::Z), "out of plane");
    }

    #[test]
    fn ray_cone_contains_only_its_own_direction() {
        let c = PolyhedralCone::from_generators(&[Vector3::Z]);
        assert!(c.contains(&(Vector3::Z * 3.0)), "forward along the ray");
        assert!(c.contains(&Vector3::ZERO), "apex");
        assert!(!c.contains(&(-Vector3::Z)), "backward");
        assert!(!c.contains(&Vector3::X), "perpendicular");
        assert!(!c.contains(&Vector3::new(0.1, 0.0, 1.0)), "off the ray");
    }

    /// A random planar wedge: two rays < π apart in a random plane.
    fn planar_wedge_strategy() -> impl Strategy<Value = (PolyhedralCone, Vector3, Vector3, Vector3)>
    {
        (
            unit_vec(),
            0.0f64..std::f64::consts::TAU,
            0.2f64..2.8, // sweep < π
        )
            .prop_filter_map("wedge", |(n, a0, sweep)| {
                let u = super::unit_perp(n);
                let v = n.cross(&u).normalize().ok()?;
                let g1 = (u * a0.cos() + v * a0.sin()).normalize().ok()?;
                let a1 = a0 + sweep;
                let g2 = (u * a1.cos() + v * a1.sin()).normalize().ok()?;
                let c = PolyhedralCone::from_generators(&[g1, g2]);
                if c.generators().len() != 2 {
                    return None;
                }
                Some((c, n, g1, g2))
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// Wedge membership: conic combinations of the two edge rays are
        /// inside; pushing out of the plane leaves it; the antipode is out.
        #[test]
        fn prop_planar_wedge_membership(
            (c, n, g1, g2) in planar_wedge_strategy(),
            l in 0.0f64..1.0, m in 0.0f64..1.0,
        ) {
            let inside = g1 * l + g2 * m;
            if inside.magnitude() > 1e-6 {
                prop_assert!(c.contains(&inside), "conic combo not in wedge");
                prop_assert!(!c.contains(&(inside + n * 0.3)), "out-of-plane accepted");
                prop_assert!(!c.contains(&(inside - n * 0.3)), "out-of-plane accepted");
                prop_assert!(!c.contains(&(-inside)), "antipode in wedge");
            }
        }

        /// Ray-cone membership: forward multiples in, backward/perpendicular out.
        #[test]
        fn prop_ray_cone(g in unit_vec(), k in 0.1f64..10.0) {
            let c = PolyhedralCone::from_generators(&[g]);
            prop_assert!(c.contains(&(g * k)));
            prop_assert!(!c.contains(&(g * -k)));
            let perp = super::unit_perp(g);
            prop_assert!(!c.contains(&perp));
        }
    }
}
